use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use tree_sitter::Parser;
use walkdir::WalkDir;

use crate::db::DbStore;
use crate::model::{Include, Macro, Reference, Symbol};
use crate::parser::{
    CallGraph, IncludeTracker, LanguageKind, MacroTracker, SourceTree, SymbolExtractor,
};

use super::include_resolver::IncludeResolver;
use super::incremental::IncrementalIndexer;

/// Files are parsed in parallel in chunks, then written sequentially. Chunking
/// bounds memory regardless of project size while keeping a single parse per
/// file (symbols, references, macros, includes and call edges are all extracted
/// from one tree walk).
const CHUNK: usize = 128;

/// Everything extracted from one file in a single parse.
struct FileData {
    path: String,
    content: String,
    lang: LanguageKind,
    symbols: Vec<Symbol>,
    macros: Vec<Macro>,
    includes: Vec<Include>,
    refs: Vec<Reference>,
    calls: Vec<(String, String, u32, u32)>, // caller, callee, line, col
}

/// Configuration for indexing.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub root_dir: String,
    pub db_path: String,
    pub extensions: Vec<String>,
    pub exclude_patterns: Vec<String>,
    /// Bulk mode defers index creation and uses aggressive SQLite pragmas.
    pub bulk_mode: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            root_dir: ".".into(),
            db_path: ".ctxtag.db".into(),
            extensions: vec![
                "c".into(), "h".into(), "cpp".into(), "cxx".into(), "cc".into(),
                "hpp".into(), "hxx".into(), "hh".into(),
            ],
            exclude_patterns: vec![
                "vendor".into(), "build".into(), "target".into(),
                ".git".into(), "node_modules".into(),
            ],
            bulk_mode: false,
        }
    }
}

pub struct IndexEngine {
    config: IndexConfig,
    store: DbStore,
    include_resolver: IncludeResolver,
}

impl IndexEngine {
    pub fn new(config: IndexConfig) -> anyhow::Result<Self> {
        let store = if config.bulk_mode {
            DbStore::new_bulk_mode(&config.db_path)?
        } else {
            DbStore::new(&config.db_path)?
        };
        let include_resolver = IncludeResolver::new(&config.root_dir);
        Ok(Self { config, store, include_resolver })
    }

    pub fn index(&mut self) -> anyhow::Result<IndexStats> {
        let files = self.discover_files();
        let total = files.len();
        let mut stats = IndexStats::default();
        let start = Instant::now();

        if self.config.bulk_mode {
            // Fresh DB in bulk mode: start clean and defer indexes.
            let _ = self.store.drop_indexes();
        }

        let mut global_map: HashMap<String, i64> = HashMap::new();
        let mut all_includes: Vec<Include> = Vec::new();
        let threads = rayon::current_num_threads();
        tracing::info!("Indexing {} files ({} threads, single pass)", total, threads);

        for (ci, chunk) in files.chunks(CHUNK).enumerate() {
            let data: Vec<FileData> = chunk
                .par_iter()
                .filter_map(|p| Self::parse_file(p))
                .collect();

            // Register files (hash computed from in-memory content, no clone).
            let fb_refs: Vec<(&str, &str, &str)> = data
                .iter()
                .map(|d| {
                    (
                        d.path.as_str(),
                        d.content.as_str(),
                        if d.lang == LanguageKind::Cpp { "cpp" } else { "c" },
                    )
                })
                .collect();
            let fmap = self.store.batch_upsert_files(&fb_refs)?;
            global_map.extend(fmap.clone());

            // Move extracted rows out of FileData into per-chunk batches.
            let mut syms: Vec<Symbol> = Vec::new();
            let mut macs: Vec<Macro> = Vec::new();
            let mut refs: Vec<Reference> = Vec::new();
            let mut calls: Vec<(String, String, String, u32, u32)> = Vec::new();
            for d in data {
                stats.files_indexed += 1;
                for (caller, callee, line, col) in d.calls {
                    calls.push((caller, callee, d.path.clone(), line, col));
                }
                all_includes.extend(d.includes);
                syms.extend(d.symbols);
                macs.extend(d.macros);
                refs.extend(d.refs);
            }

            stats.symbols_extracted += self.store.batch_insert_symbols(&syms, &fmap)?;
            stats.macros_indexed += self.store.batch_insert_macros(&macs, &fmap)?;
            stats.references_resolved += self.store.batch_insert_references(&refs, &fmap)?;
            stats.call_edges += self.store.batch_insert_call_edges(&calls, &fmap)?;

            if total > 2000 && ci % 20 == 0 {
                tracing::info!(
                    "  {}/{} files ({}%)",
                    stats.files_indexed,
                    total,
                    stats.files_indexed * 100 / total.max(1)
                );
            }
        }

        // Resolve and persist include edges against the full file map.
        let resolved = self.include_resolver.resolve_all(&all_includes, &files);
        stats.includes_resolved = self.store.batch_insert_includes(&resolved, &global_map)?;

        if self.config.bulk_mode {
            let idx_start = Instant::now();
            tracing::info!("Building indexes...");
            self.store.finalize_indexes()?;
            tracing::info!("Indexes built in {:.1}s", idx_start.elapsed().as_secs_f32());
        }

        tracing::info!(
            "Indexing complete in {:.1}s: {:?}",
            start.elapsed().as_secs_f32(),
            stats
        );
        Ok(stats)
    }

    /// Parse one file exactly once, extracting all entity kinds.
    fn parse_file(path: &str) -> Option<FileData> {
        let mut parser = Parser::new();
        Self::init_parser(&mut parser, path).ok()?;
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read {}: {}", path, e);
                return None;
            }
        };
        let language = Self::detect_language(path);
        let mut tree = match SourceTree::parse_with_parser(&mut parser, language, content.as_bytes())
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", path, e);
                return None;
            }
        };
        tree.set_file_path(path.to_string());

        let mut extractor = SymbolExtractor::new(path);
        let symbols = extractor.extract(&tree);

        let mut macro_tracker = MacroTracker::new();
        let macros = macro_tracker.track(&tree);

        let mut include_tracker = IncludeTracker::new();
        let includes = include_tracker.track(&tree);

        // References are resolved by name at query time; pass an empty symbol
        // map so we emit name-based references without a second parse.
        let empty: HashMap<String, i64> = HashMap::new();
        let no_scopes: &[crate::model::Scope] = &[];
        let refs = resolve_references_with_map(&tree, no_scopes, &empty);

        let mut call_graph = CallGraph::new();
        let edges = call_graph.extract(&tree);
        let mut calls = Vec::new();
        for e in edges {
            if let Some(caller) = e.caller_name {
                if !caller.is_empty() && !e.callee_name.is_empty() {
                    calls.push((caller, e.callee_name, e.line, e.column));
                }
            }
        }

        Some(FileData {
            path: path.to_string(),
            content,
            lang: language,
            symbols,
            macros,
            includes,
            refs,
            calls,
        })
    }

    fn detect_language(file_path: &str) -> LanguageKind {
        if file_path.ends_with(".cpp")
            || file_path.ends_with(".cxx")
            || file_path.ends_with(".cc")
            || file_path.ends_with(".hpp")
            || file_path.ends_with(".hxx")
            || file_path.ends_with(".hh")
        {
            LanguageKind::Cpp
        } else {
            LanguageKind::C
        }
    }

    fn init_parser(parser: &mut Parser, file_path: &str) -> anyhow::Result<()> {
        let language = Self::detect_language(file_path);
        parser.set_language(&language.language())?;
        Ok(())
    }

    pub fn discover_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let root = Path::new(&self.config.root_dir);
        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !self.config.extensions.contains(&ext.to_string()) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if self.config.exclude_patterns.iter().any(|p| path_str.contains(p)) {
                continue;
            }
            if let Ok(abs) = fs::canonicalize(path) {
                files.push(abs.to_string_lossy().to_string());
            }
        }
        files.sort();
        files
    }

    pub fn update(&mut self) -> anyhow::Result<IndexStats> {
        let mut incremental = IncrementalIndexer::new(
            &self.config.root_dir,
            &mut self.store,
            self.config.extensions.clone(),
            self.config.exclude_patterns.clone(),
        );
        incremental.update()
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub files_failed: usize,
    pub symbols_extracted: usize,
    pub references_resolved: usize,
    pub macros_indexed: usize,
    pub includes_resolved: usize,
    pub call_edges: usize,
}

// ─── Reference Resolution (name-based) ─────────────────────────────────

pub(super) fn resolve_references_with_map(
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
) -> Vec<Reference> {
    let mut references: Vec<Reference> = Vec::new();
    let root = tree.root_node();
    walk_refs_node(root, tree, scopes, symbol_map, &mut references);
    references
}

fn walk_refs_node(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    refs: &mut Vec<Reference>,
) {
    match node.kind() {
        "identifier" => {
            resolve_ref_id(node, tree, scopes, symbol_map, refs);
            return;
        }
        "call_expression" => {
            resolve_ref_call(node, tree, scopes, symbol_map, refs);
            return;
        }
        "qualified_identifier" => {
            resolve_ref_qualified(node, tree, symbol_map, refs);
        }
        "field_expression" => {
            resolve_ref_field(node, tree, scopes, symbol_map, refs);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_refs_node(child, tree, scopes, symbol_map, refs);
    }
}

fn resolve_ref_id(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    refs: &mut Vec<Reference>,
) {
    let name = tree.node_text(&node);
    let parent = match node.parent() {
        Some(p) => p,
        None => return,
    };
    if is_keyword(name) || is_type_ctx(parent.kind()) {
        return;
    }
    let (ref_kind, context) = classify_ref(parent.kind());
    let symbol_id = lookup_id(name, scopes, symbol_map);
    let pos = node.start_position();
    let end_pos = node.end_position();
    refs.push(Reference {
        id: None,
        symbol_id,
        symbol_name: name.to_string(),
        file_path: tree.file_path().to_string(),
        line: pos.row as u32,
        column: pos.column as u32,
        end_line: Some(end_pos.row as u32),
        end_column: Some(end_pos.column as u32),
        ref_kind,
        context,
        context_detail: None,
        context_line: None,
        is_definition: is_def_ctx(parent.kind()),
    });
}

fn resolve_ref_call(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    refs: &mut Vec<Reference>,
) {
    let function = match node.child_by_field_name("function") {
        Some(f) => f,
        None => return,
    };
    let name = tree.node_text(&function);
    let symbol_id = lookup_id(name, scopes, symbol_map);
    let pos = function.start_position();
    let end_pos = function.end_position();
    refs.push(Reference {
        id: None,
        symbol_id,
        symbol_name: name.to_string(),
        file_path: tree.file_path().to_string(),
        line: pos.row as u32,
        column: pos.column as u32,
        end_line: Some(end_pos.row as u32),
        end_column: Some(end_pos.column as u32),
        ref_kind: crate::model::ReferenceKind::Call,
        context: Some("function_call".to_string()),
        context_detail: None,
        context_line: None,
        is_definition: false,
    });
}

fn resolve_ref_qualified(
    node: tree_sitter::Node,
    tree: &SourceTree,
    symbol_map: &HashMap<String, i64>,
    refs: &mut Vec<Reference>,
) {
    let full_name = tree.node_text(&node);
    let symbol_id = *symbol_map.get(full_name).unwrap_or(&0);
    let pos = node.start_position();
    let end_pos = node.end_position();
    refs.push(Reference {
        id: None,
        symbol_id,
        symbol_name: full_name.to_string(),
        file_path: tree.file_path().to_string(),
        line: pos.row as u32,
        column: pos.column as u32,
        end_line: Some(end_pos.row as u32),
        end_column: Some(end_pos.column as u32),
        ref_kind: crate::model::ReferenceKind::Access,
        context: Some("qualified".to_string()),
        context_detail: Some(full_name.to_string()),
        context_line: None,
        is_definition: false,
    });
}

fn resolve_ref_field(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    refs: &mut Vec<Reference>,
) {
    let field = match node.child_by_field_name("field") {
        Some(f) => f,
        None => return,
    };
    let field_name = tree.node_text(&field);
    let symbol_id = lookup_id(field_name, scopes, symbol_map);
    let pos = field.start_position();
    let end_pos = field.end_position();
    let is_ptr = tree.node_text(&node).contains("->");
    refs.push(Reference {
        id: None,
        symbol_id,
        symbol_name: field_name.to_string(),
        file_path: tree.file_path().to_string(),
        line: pos.row as u32,
        column: pos.column as u32,
        end_line: Some(end_pos.row as u32),
        end_column: Some(end_pos.column as u32),
        ref_kind: if is_ptr {
            crate::model::ReferenceKind::Dereference
        } else {
            crate::model::ReferenceKind::Access
        },
        context: Some("member_access".to_string()),
        context_detail: None,
        context_line: None,
        is_definition: false,
    });
    if let Some(operand) = node.child_by_field_name("operand") {
        if operand.kind() == "identifier" {
            resolve_ref_id(operand, tree, scopes, symbol_map, refs);
        }
    }
}

pub(super) fn lookup_id(
    name: &str,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
) -> i64 {
    if let Some(&id) = symbol_map.get(name) {
        return id;
    }
    if let Some(sym) = crate::model::Scope::lookup_chain(scopes, name) {
        if let Some(id) = sym.id {
            return id;
        }
        if let Some(&id) = symbol_map.get(&sym.qualified_name) {
            return id;
        }
    }
    0
}

pub(super) fn classify_ref(
    parent_kind: &str,
) -> (crate::model::ReferenceKind, Option<String>) {
    match parent_kind {
        "field_declaration" => (crate::model::ReferenceKind::Declaration, Some("field".into())),
        "declaration" => (crate::model::ReferenceKind::Declaration, Some("declaration".into())),
        "type_definition" => (crate::model::ReferenceKind::Declaration, Some("typedef".into())),
        "parameter_declaration" => (crate::model::ReferenceKind::Declaration, Some("parameter".into())),
        "return_statement" => (crate::model::ReferenceKind::Return, Some("return".into())),
        "assignment_expression" => (crate::model::ReferenceKind::Assignment, Some("assignment".into())),
        _ => (crate::model::ReferenceKind::Access, None),
    }
}

pub(super) fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "auto" | "break" | "case" | "char" | "const" | "continue"
            | "default" | "do" | "double" | "else" | "enum" | "extern"
            | "float" | "for" | "goto" | "if" | "inline" | "int" | "long"
            | "register" | "restrict" | "return" | "short" | "signed"
            | "sizeof" | "static" | "struct" | "switch" | "typedef"
            | "union" | "unsigned" | "void" | "volatile" | "while"
            | "class" | "namespace" | "template" | "typename" | "virtual"
            | "public" | "private" | "protected" | "this" | "new" | "delete"
            | "true" | "false" | "nullptr" | "try" | "catch" | "throw"
    )
}

pub(super) fn is_type_ctx(kind: &str) -> bool {
    matches!(
        kind,
        "type_identifier" | "struct_specifier" | "enum_specifier"
            | "class_specifier" | "enum_identifier" | "primitive_type"
            | "sized_type_specifier"
    )
}

pub(super) fn is_def_ctx(kind: &str) -> bool {
    matches!(
        kind,
        "declaration" | "type_definition" | "function_definition"
            | "field_declaration" | "enum_specifier" | "struct_specifier"
            | "class_specifier" | "namespace_definition" | "parameter_declaration"
            | "init_declarator" | "field_declarator" | "enumerator"
    )
}
