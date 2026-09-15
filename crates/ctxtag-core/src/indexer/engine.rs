use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use walkdir::WalkDir;

use crate::db::DbStore;
use crate::model::{Include, Macro, Reference, Symbol};
use crate::parser::{
    CallGraph, IncludeTracker, LanguageKind, MacroTracker, ScopeBuilder,
    SourceTree, SymbolExtractor,
};

use super::include_resolver::IncludeResolver;
use super::incremental::IncrementalIndexer;

/// Configuration for indexing.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub root_dir: String,
    pub db_path: String,
    pub extensions: Vec<String>,
    pub exclude_patterns: Vec<String>,
    /// If true, uses deferred index creation (indexes built after all inserts)
    /// and aggressive SQLite pragmas for ~1.7x faster bulk indexing.
    /// Default: false (incremental/LSP mode with indexes maintained live).
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

/// Main indexing engine.
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
        Ok(Self {
            config,
            store,
            include_resolver,
        })
    }

    /// Index an entire project directory using a two-pass streaming approach.
    ///
    /// Pass 1: Parse each file, extract and write symbols/macros/includes to DB immediately.
    /// Pass 2: Parse each file again, resolve references against in-memory symbol map,
    ///         batch-write to DB.
    ///
    /// Only one file's data is held in memory at a time.
    /// Symbol ID map and file ID map are cached in memory to avoid per-query DB lookups.
    pub fn index(&mut self) -> anyhow::Result<IndexStats> {
        let files = self.discover_files();
        let total = files.len();
        let mut stats = IndexStats::default();

        let start = Instant::now();

        // Pass 1: Extract symbols, macros, includes - batch write to DB
        // Accumulates data across files and flushes in batches to reduce
        // transaction overhead (from ~100k transactions to ~1k).
        tracing::info!("Pass 1: symbol extraction ({} files)", total);
        let pass1_start = Instant::now();

        const P1_BATCH: usize = 50;
        let mut file_batch: Vec<(String, String, &'static str)> = Vec::with_capacity(P1_BATCH);
        let mut sym_batch: Vec<Symbol> = Vec::with_capacity(2048);
        let mut macro_batch: Vec<Macro> = Vec::with_capacity(256);
        let mut all_includes: Vec<Include> = Vec::new();
        let mut file_map: HashMap<String, i64> = HashMap::new();

        for (i, file_path) in files.iter().enumerate() {
            if i > 0 && i % (total.saturating_sub(1) / 100).max(1) == 0 {
                tracing::info!("Pass 1: {}/{} files ({}%)", i, total, i * 100 / total);
            }

            match self.index_file_pass1(file_path) {
                Ok((content, language, symbols, macros, includes)) => {
                    let lang_str: &'static str = if language == LanguageKind::Cpp { "cpp" } else { "c" };
                    file_batch.push((file_path.clone(), content, lang_str));
                    sym_batch.extend(symbols);
                    macro_batch.extend(macros);
                    all_includes.extend(includes);
                    stats.files_indexed += 1;

                    // Flush batch when large enough or at the end
                    if file_batch.len() >= P1_BATCH || i == total - 1 {
                        let fb_refs: Vec<_> = file_batch.iter().map(|(p, c, l)| (p.as_str(), c.as_str(), *l)).collect();
                        let fmap = self.store.batch_upsert_files(&fb_refs)?;
                        file_map.extend(fmap);
                        file_batch.clear();

                        if !sym_batch.is_empty() {
                            let count = sym_batch.len();
                            self.store.batch_insert_symbols(&sym_batch, &file_map)?;
                            stats.symbols_extracted += count;
                            sym_batch.clear();
                        }

                        if !macro_batch.is_empty() {
                            let count = macro_batch.len();
                            self.store.batch_insert_macros(&macro_batch, &file_map)?;
                            stats.macros_indexed += count;
                            macro_batch.clear();
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to index {}: {}", file_path, e);
                    stats.files_failed += 1;
                }
            }
        }

        tracing::info!(
            "Pass 1 complete in {:.1}s: {} symbols, {} macros",
            pass1_start.elapsed().as_secs_f32(),
            stats.symbols_extracted,
            stats.macros_indexed,
        );

        // Resolve include paths
        let resolved_includes = self.include_resolver.resolve_all(&all_includes, &files);
        stats.includes_resolved = resolved_includes.len();
        drop(all_includes);

        // Load symbol map into memory for Pass 2
        // (file_map is already cached from Pass 1 batch inserts)
        tracing::info!("Loading symbol map from DB...");
        let load_start = Instant::now();
        let symbol_map = self.store.load_symbol_map()?;
        tracing::info!(
            "Loaded {} symbols in {:.1}s",
            symbol_map.len(),
            load_start.elapsed().as_secs_f32(),
        );

        // Pass 2: Resolve references and call edges using in-memory maps
        // Accumulates references across a batch of files before writing to DB
        // to reduce transaction overhead.
        tracing::info!("Pass 2: reference resolution ({} files)", total);
        let pass2_start = Instant::now();

        const BATCH_SIZE: usize = 50;
        let mut ref_batch: Vec<Reference> = Vec::with_capacity(4096);
        let mut edge_batch: Vec<(i64, i64, String, u32, u32, String)> = Vec::with_capacity(1024);

        for (i, file_path) in files.iter().enumerate() {
            if i > 0 && i % (total.saturating_sub(1) / 100).max(1) == 0 {
                tracing::info!("Pass 2: {}/{} files ({}%)", i, total, i * 100 / total);
            }

            match self.index_file_pass2(file_path, &symbol_map, &mut stats) {
                Ok((refs, edges)) => {
                    ref_batch.extend(refs);
                    edge_batch.extend(edges);

                    // Flush batch when large enough or at the end
                    if ref_batch.len() >= BATCH_SIZE * 200 || edge_batch.len() >= BATCH_SIZE * 50
                        || i == total - 1
                    {
                        if !ref_batch.is_empty() {
                            let count = ref_batch.len();
                            self.store.batch_insert_references(&ref_batch, &file_map)?;
                            stats.references_resolved += count;
                            ref_batch.clear();
                        }
                        if !edge_batch.is_empty() {
                            let count = edge_batch.len();
                            self.store.batch_insert_call_edges(&edge_batch, &file_map)?;
                            stats.call_edges += count;
                            edge_batch.clear();
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to resolve refs for {}: {}", file_path, e);
                    stats.files_failed += 1;
                }
            }
        }

        // Flush any remaining items
        if !ref_batch.is_empty() {
            let count = ref_batch.len();
            self.store.batch_insert_references(&ref_batch, &file_map)?;
            stats.references_resolved += count;
        }
        if !edge_batch.is_empty() {
            let count = edge_batch.len();
            self.store.batch_insert_call_edges(&edge_batch, &file_map)?;
            stats.call_edges += count;
        }

        tracing::info!(
            "Pass 2 complete in {:.1}s: {} references, {} call edges",
            pass2_start.elapsed().as_secs_f32(),
            stats.references_resolved,
            stats.call_edges,
        );

        // Build indexes after all data has been inserted (bulk mode optimization).
        // This avoids index maintenance overhead during the ~1.7M row inserts,
        // giving a 1.7x speedup over live index maintenance.
        if self.config.bulk_mode {
            let idx_start = Instant::now();
            tracing::info!("Building indexes...");
            self.store.finalize_indexes()?;
            tracing::info!(
                "Indexes built in {:.1}s",
                idx_start.elapsed().as_secs_f32()
            );
        }

        let elapsed = start.elapsed();
        tracing::info!("Indexing complete in {:.1}s: {:?}", elapsed.as_secs_f32(), stats);
        Ok(stats)
    }

    /// Pass 1: Parse a single file, extract symbols/macros/includes.
    /// Returns (content, language, symbols, macros, includes) for batch insertion.
    fn index_file_pass1(
        &self,
        file_path: &str,
    ) -> anyhow::Result<(String, LanguageKind, Vec<Symbol>, Vec<Macro>, Vec<Include>)> {
        let content = fs::read_to_string(file_path)?;
        let content_bytes = content.as_bytes();

        let language = Self::detect_language(file_path);
        let mut tree = SourceTree::parse(language, content_bytes)?;
        tree.set_file_path(file_path.to_string());

        // Extract symbols
        let mut extractor = SymbolExtractor::new(file_path);
        let symbols = extractor.extract(&tree);

        // Extract macros
        let mut macro_tracker = MacroTracker::new();
        let macros = macro_tracker.track(&tree);

        // Extract includes
        let mut include_tracker = IncludeTracker::new();
        let includes = include_tracker.track(&tree);

        drop(tree);

        Ok((content, language, symbols, macros, includes))
    }

    /// Pass 2: Parse a single file, resolve references and call edges.
    /// Uses in-memory symbol_map to resolve real DB symbol IDs.
    /// Returns (references, resolved_call_edges) for batch insertion.
    fn index_file_pass2(
        &self,
        file_path: &str,
        symbol_map: &HashMap<String, i64>,
        _stats: &mut IndexStats,
    ) -> anyhow::Result<(Vec<Reference>, Vec<(i64, i64, String, u32, u32, String)>)> {
        let content = fs::read_to_string(file_path)?;
        let content_bytes = content.as_bytes();

        let language = Self::detect_language(file_path);
        let mut tree = SourceTree::parse(language, content_bytes)?;
        tree.set_file_path(file_path.to_string());

        // Build scope chain from tree
        let mut scope_builder = ScopeBuilder::new();
        let global_scope = scope_builder.build(&tree);
        let scopes = vec![global_scope];

        // Resolve references using the in-memory symbol map for real DB IDs
        let references = resolve_references_with_map(&tree, &scopes, symbol_map);

        // Extract call graph
        let mut call_graph = CallGraph::new();
        let edges = call_graph.extract(&tree);

        // Resolve call edges using in-memory symbol map
        let mut resolved_edges: Vec<(i64, i64, String, u32, u32, String)> = Vec::new();
        for edge in edges {
            if let Some(caller_name) = &edge.caller_name {
                if let (Some(&caller_id), Some(&callee_id)) = (
                    symbol_map.get(caller_name),
                    symbol_map.get(&edge.callee_name),
                ) {
                    resolved_edges.push((
                        caller_id,
                        callee_id,
                        file_path.to_string(),
                        edge.line,
                        edge.column,
                        edge.call_text,
                    ));
                }
            }
        }

        Ok((references, resolved_edges))
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

    /// Discover C/C++ source files in the project root.
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

            // Check extension
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if !self.config.extensions.contains(&ext.to_string()) {
                continue;
            }

            // Check exclude patterns
            let path_str = path.to_string_lossy().to_string();
            let mut excluded = false;
            for pattern in &self.config.exclude_patterns {
                if path_str.contains(pattern) {
                    excluded = true;
                    break;
                }
            }
            if excluded {
                continue;
            }

            // Make path absolute
            if let Ok(abs) = fs::canonicalize(path) {
                files.push(abs.to_string_lossy().to_string());
            }
        }

        files.sort();
        files
    }

    /// Incremental update: only re-index changed files.
    pub fn update(&self) -> anyhow::Result<IndexStats> {
        let incremental = IncrementalIndexer::new(&self.config.root_dir, &self.store);
        incremental.update()
    }
}

/// Statistics from an indexing run.
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


// ─── Reference Resolution with DB Symbol Map ──────────────────────────

/// Resolves references in a source tree using the DB symbol map.
/// Returns references with real DB symbol IDs instead of placeholders.
fn resolve_references_with_map(
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

    // Also resolve operand if it's an identifier
    if let Some(operand) = node.child_by_field_name("operand") {
        if operand.kind() == "identifier" {
            resolve_ref_id(operand, tree, scopes, symbol_map, refs);
        }
    }
}

fn lookup_id(
    name: &str,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
) -> i64 {
    // Fast path: DB symbol map
    if let Some(&id) = symbol_map.get(name) {
        return id;
    }
    // Scope chain fallback
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

fn classify_ref(parent_kind: &str) -> (crate::model::ReferenceKind, Option<String>) {
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

fn is_keyword(name: &str) -> bool {
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

fn is_type_ctx(kind: &str) -> bool {
    matches!(
        kind,
        "type_identifier" | "struct_specifier" | "enum_specifier"
        | "class_specifier" | "enum_identifier" | "primitive_type"
        | "sized_type_specifier"
    )
}

fn is_def_ctx(kind: &str) -> bool {
    matches!(
        kind,
        "declaration" | "type_definition" | "function_definition"
        | "field_declaration" | "enum_specifier" | "struct_specifier"
        | "class_specifier" | "namespace_definition" | "parameter_declaration"
        | "init_declarator" | "field_declarator" | "enumerator"
    )
}
