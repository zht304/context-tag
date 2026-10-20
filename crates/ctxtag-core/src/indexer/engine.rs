use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

use rayon::prelude::*;
use tree_sitter::Parser;
use walkdir::WalkDir;

use crate::db::DbStore;
use crate::model::{Include, Macro, Reference, Symbol, SymbolKind};
use crate::parser::{
    CallGraph, IncludeTracker, LanguageKind, MacroTracker, SourceTree, SymbolExtractor, TypeResolver,
};

use super::include_resolver::IncludeResolver;
use super::incremental::IncrementalIndexer;

/// Files are parsed in parallel in chunks, then written sequentially. Chunking
/// bounds memory regardless of project size while keeping a single parse per
/// file (symbols, references, macros, includes and call edges are all extracted
/// from one tree walk).
const CHUNK: usize = 128;

/// Everything extracted from one file in a single parse (pass 1). References
/// are resolved separately in pass 2 once the global type tables are complete.
struct FileData {
    path: String,
    content: String,
    lang: LanguageKind,
    symbols: Vec<Symbol>,
    macros: Vec<Macro>,
    includes: Vec<Include>,
    calls: Vec<(String, String, u32, u32)>, // caller, callee, line, col
}

/// Configuration for indexing.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// One or more project roots to walk. All are merged into a single index.
    pub root_dirs: Vec<String>,
    /// Optional path to a file listing source paths (one per line, `#` comments
    /// and blanks ignored) — e.g. the output of `find`. When set, this list is
    /// authoritative and the roots are not walked; ext/exclude filters are not
    /// applied to explicitly-listed files.
    pub file_list: Option<String>,
    pub db_path: String,
    pub extensions: Vec<String>,
    pub exclude_patterns: Vec<String>,
    /// Bulk mode defers index creation and uses aggressive SQLite pragmas.
    pub bulk_mode: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            root_dirs: vec![".".into()],
            file_list: None,
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
        let include_resolver = IncludeResolver::new(config.root_dirs.clone());
        Ok(Self { config, store, include_resolver })
    }

    pub fn index(&mut self) -> anyhow::Result<IndexStats> {
        let files = self.discover_files();
        let total = files.len();
        let mut stats = IndexStats::default();
        let start = Instant::now();

        // Record the roots this DB covers so a later `update` can rediscover
        // the full set (and add new roots) instead of purging the others.
        // Skipped in file-list mode, where the list — not roots — is authoritative.
        if self.config.file_list.is_none() {
            let canon: Vec<String> = self
                .config
                .root_dirs
                .iter()
                .filter_map(|r| fs::canonicalize(r).ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            self.store.register_roots(&canon)?;
        }

        if self.config.bulk_mode {
            // Fresh DB in bulk mode: start clean and defer indexes.
            let _ = self.store.drop_indexes();
        }

        let mut global_map: HashMap<String, i64> = HashMap::new();
        let mut all_includes: Vec<Include> = Vec::new();
        let mut type_builder = GlobalTypesBuilder::default();
        let threads = rayon::current_num_threads();
        tracing::info!("Indexing {} files ({} threads, two passes)", total, threads);

        // ---- Pass 1: symbols, macros, includes, call edges. Accumulate the
        // cross-file type tables that pass 2 needs to qualify member refs. ----
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
            let mut calls: Vec<(String, String, String, u32, u32)> = Vec::new();
            for d in data {
                stats.files_indexed += 1;
                for (caller, callee, line, col) in d.calls {
                    calls.push((caller, callee, d.path.clone(), line, col));
                }
                all_includes.extend(d.includes);
                syms.extend(d.symbols);
                macs.extend(d.macros);
            }

            for s in &syms {
                type_builder.feed(s);
            }
            for m in &macs {
                type_builder.feed_macro(&m.name);
            }

            stats.symbols_extracted += self.store.batch_insert_symbols(&syms, &fmap)?;
            stats.macros_indexed += self.store.batch_insert_macros(&macs, &fmap)?;
            stats.call_edges += self.store.batch_insert_call_edges(&calls, &fmap)?;

            if total > 2000 && ci % 20 == 0 {
                tracing::info!(
                    "  pass1 {}/{} files ({}%)",
                    stats.files_indexed,
                    total,
                    stats.files_indexed * 100 / total.max(1)
                );
            }
        }

        let global = type_builder.finish();
        tracing::info!(
            "Global type tables: {} typedef->tag, {} field->type",
            global.typedef_to_tag.len(),
            global.field_type.len()
        );

        // Resolve and persist include edges against the full file map.
        let resolved = self.include_resolver.resolve_all(&all_includes, &files);
        stats.includes_resolved = self.store.batch_insert_includes(&resolved, &global_map)?;

        // ---- Pass 2: references, resolved against the complete type tables. ----
        for (ci, chunk) in files.chunks(CHUNK).enumerate() {
            let refs: Vec<Reference> = chunk
                .par_iter()
                .filter_map(|p| Self::resolve_refs_for_file(p, &global))
                .flatten()
                .collect();
            stats.references_resolved += self.store.batch_insert_references(&refs, &global_map)?;

            if total > 2000 && ci % 20 == 0 {
                tracing::info!(
                    "  pass2 {}/{} files ({}%)",
                    (ci + 1) * CHUNK.min(total),
                    total,
                    ((ci + 1) * CHUNK).min(total) * 100 / total.max(1)
                );
            }
        }

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
            calls,
        })
    }

    /// Pass 2: re-parse one file and resolve references against the completed
    /// global type tables. Kept separate from `parse_file` because reference
    /// qualification needs cross-file type info that isn't known until every
    /// file's symbols have been collected.
    fn resolve_refs_for_file(path: &str, global: &GlobalTypes) -> Option<Vec<Reference>> {
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

        // References are resolved by name at query time; pass an empty symbol
        // map so we emit name-based references.
        let empty: HashMap<String, i64> = HashMap::new();
        let no_scopes: &[crate::model::Scope] = &[];
        Some(resolve_references_with_map(&tree, no_scopes, &empty, global))
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
        discover_source_files(
            &self.config.root_dirs,
            self.config.file_list.as_deref(),
            &self.config.extensions,
            &self.config.exclude_patterns,
        )
    }

    pub fn update(&mut self) -> anyhow::Result<IndexStats> {
        let mut incremental = IncrementalIndexer::new(
            self.config.root_dirs.clone(),
            self.config.file_list.clone(),
            &mut self.store,
            self.config.extensions.clone(),
            self.config.exclude_patterns.clone(),
        );
        incremental.update()
    }
}

/// Discover the source files to index.
///
/// When `file_list` is `Some(path)`, that file is authoritative: each non-empty
/// line not starting with `#` is a path (relative paths resolve against the
/// cwd), canonicalized and kept only if it is an existing file. Extension and
/// exclude filters are intentionally NOT applied to an explicit list — the
/// caller already curated it (e.g. via `find`).
///
/// Otherwise every root in `root_dirs` is walked, keeping files whose extension
/// is in `extensions` and whose path contains none of `exclude_patterns`.
///
/// The result is deduplicated (roots may overlap) and sorted for determinism.
pub fn discover_source_files(
    root_dirs: &[String],
    file_list: Option<&str>,
    extensions: &[String],
    exclude_patterns: &[String],
) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();

    if let Some(list_path) = file_list {
        if let Ok(content) = fs::read_to_string(list_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Ok(abs) = fs::canonicalize(line) {
                    if abs.is_file() {
                        files.push(abs.to_string_lossy().to_string());
                    }
                }
            }
        } else {
            tracing::error!("Cannot read file list: {}", list_path);
        }
    } else {
        for root in root_dirs {
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
                if !extensions.contains(&ext.to_string()) {
                    continue;
                }
                let path_str = path.to_string_lossy().to_string();
                if exclude_patterns.iter().any(|p| path_str.contains(p)) {
                    continue;
                }
                if let Ok(abs) = fs::canonicalize(path) {
                    files.push(abs.to_string_lossy().to_string());
                }
            }
        }
    }

    files.sort();
    files.dedup();
    files
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

/// Cross-file type tables used to qualify member references beyond what a
/// single file's declarations reveal. Built during pass 1 (symbols) and
/// consumed during pass 2 (references):
/// - `typedef_to_tag`: alias -> underlying struct/union/class/enum tag. Lets a
///   receiver declared via a header typedef (e.g. `drm_i915_private *dev`) be
///   resolved even though the typedef isn't in the same file.
/// - `field_type`: (struct tag, field name) -> the field's own struct tag, for
///   fields whose type is a struct/typedef thereof. Enables chained dereference
///   (`a->b->c`: resolve `a`, look up `b`'s type, then qualify `c`).
#[derive(Default, Clone)]
pub struct GlobalTypes {
    pub typedef_to_tag: HashMap<String, String>,
    pub field_type: HashMap<(String, String), String>,
    /// Every name bound to a file-scope symbol or macro anywhere in the project.
    /// Used to decide whether a plain-identifier reference could denote a
    /// global; a name absent from this set that is also a function local is a
    /// pure local and its references are not stored.
    pub global_names: HashSet<String>,
}

/// Mutable accumulator fed with pass-1 symbols. Values are stored as raw core
/// type names and resolved to real struct tags by `finish`.
#[derive(Default)]
struct GlobalTypesBuilder {
    struct_tags: HashSet<String>,
    // alias -> core type name (unresolved; may itself be another alias)
    typedefs: HashMap<String, String>,
    // (container, field) -> core type name (unresolved)
    fields: HashMap<(String, String), String>,
    global_names: HashSet<String>,
}

impl GlobalTypesBuilder {
    fn feed(&mut self, sym: &Symbol) {
        if !sym.name.is_empty() {
            self.global_names.insert(sym.name.clone());
        }
        match sym.kind {
            SymbolKind::Struct | SymbolKind::Union | SymbolKind::Class | SymbolKind::Enum => {
                if !sym.name.is_empty() {
                    self.struct_tags.insert(sym.name.clone());
                }
            }
            SymbolKind::Typedef => {
                if let Some(dt) = &sym.declared_type {
                    let core = TypeResolver::extract_simple_type_name(dt);
                    if !core.is_empty() && !sym.name.is_empty() {
                        self.typedefs.entry(sym.name.clone()).or_insert(core);
                    }
                }
            }
            SymbolKind::Member | SymbolKind::StaticMember => {
                if let (Some(container), Some(dt)) = (&sym.container_name, &sym.declared_type) {
                    if container.is_empty() {
                        return;
                    }
                    let core = TypeResolver::extract_simple_type_name(dt);
                    if !core.is_empty() {
                        self.fields
                            .entry((container.clone(), sym.name.clone()))
                            .or_insert(core);
                    }
                }
            }
            _ => {}
        }
    }

    fn feed_macro(&mut self, name: &str) {
        if !name.is_empty() {
            self.global_names.insert(name.to_string());
        }
    }

    /// Resolve accumulated core names to concrete struct tags. Typedefs are
    /// followed transitively (alias -> alias -> struct); entries that bottom out
    /// in a primitive or unknown name are dropped, since they can't qualify a
    /// member access.
    fn finish(self) -> GlobalTypes {
        let mut typedef_to_tag: HashMap<String, String> = HashMap::new();
        for alias in self.typedefs.keys() {
            if let Some(tag) = self.resolve_alias(alias, 0) {
                typedef_to_tag.insert(alias.clone(), tag);
            }
        }

        let mut field_type: HashMap<(String, String), String> = HashMap::new();
        for (key, core) in &self.fields {
            let resolved = if self.struct_tags.contains(core) {
                Some(core.clone())
            } else {
                typedef_to_tag.get(core).cloned()
            };
            if let Some(tag) = resolved {
                field_type.insert(key.clone(), tag);
            }
        }

        GlobalTypes { typedef_to_tag, field_type, global_names: self.global_names }
    }

    fn resolve_alias(&self, alias: &str, depth: usize) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let core = self.typedefs.get(alias)?;
        if self.struct_tags.contains(core) {
            return Some(core.clone());
        }
        if self.typedefs.contains_key(core) && core != alias {
            return self.resolve_alias(core, depth + 1);
        }
        None
    }
}

pub(super) fn resolve_references_with_map(
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    global: &GlobalTypes,
) -> Vec<Reference> {
    let mut references: Vec<Reference> = Vec::new();
    let root = tree.root_node();
    // Build a per-file typedef map (global typedefs fill in aliases defined in
    // other files) and a file-scope variable-type map for globals. Function
    // locals are collected per function during the walk so a name reused with a
    // different type in another function can't poison this one.
    let mut typedefs = collect_typedefs(tree);
    for (alias, tag) in &global.typedef_to_tag {
        typedefs.entry(alias.clone()).or_insert_with(|| tag.clone());
    }
    let file_vars = collect_var_types_subtree(root, tree, &typedefs, true);
    let empty_locals: HashMap<String, Option<String>> = HashMap::new();
    let vars = VarTypes { file: &file_vars, local: &empty_locals };
    walk_refs_node(
        root, tree, scopes, symbol_map, &vars, &typedefs, global, None, &mut references,
    );
    references
}

/// Two-level variable-type view: file-scope globals plus the current function's
/// locals/parameters. A function local shadows a file-scope name of the same
/// identifier. Values are the resolved struct tag, or `None` when the name's
/// type isn't a struct or is ambiguous within its scope.
struct VarTypes<'a> {
    file: &'a HashMap<String, Option<String>>,
    local: &'a HashMap<String, Option<String>>,
}

impl<'a> VarTypes<'a> {
    fn tag_of(&self, name: &str) -> Option<String> {
        match self.local.get(name) {
            Some(v) => v.clone(),
            None => self.file.get(name).and_then(|v| v.clone()),
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.local.contains_key(name)
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_refs_node(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    vars: &VarTypes,
    typedefs: &HashMap<String, String>,
    global: &GlobalTypes,
    // Struct tag of the aggregate currently being brace-initialized, so
    // designated initializers (`.field = value`) can be qualified.
    init_tag: Option<&str>,
    refs: &mut Vec<Reference>,
) {
    match node.kind() {
        "function_definition" => {
            // C/C++ functions don't nest, so the whole subtree shares one local
            // type/scope map built from this function's parameters and body
            // declarations, layered over the file-scope globals.
            let fn_locals = collect_var_types_subtree(node, tree, typedefs, false);
            let fvars = VarTypes { file: vars.file, local: &fn_locals };
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk_refs_node(
                    child, tree, scopes, symbol_map, &fvars, typedefs, global, init_tag, refs,
                );
            }
            return;
        }
        "identifier" => {
            resolve_ref_id(node, tree, scopes, symbol_map, vars, &global.global_names, refs);
            return;
        }
        "call_expression" => {
            // A plain-identifier callee (`foo()`) is recorded by name. A member
            // callee (`ops->request()`) is a field_expression, so let the normal
            // recursion handle it and record the qualified member access instead
            // of the useless `ops->request` text.
            let func = node.child_by_field_name("function");
            let plain_callee = func.map(|f| f.kind() == "identifier").unwrap_or(false);
            if plain_callee {
                resolve_ref_call(node, tree, scopes, symbol_map, vars, &global.global_names, refs);
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if plain_callee && func.map(|f| f == child).unwrap_or(false) {
                    continue;
                }
                walk_refs_node(
                    child, tree, scopes, symbol_map, vars, typedefs, global, None, refs,
                );
            }
            return;
        }
        "qualified_identifier" => {
            resolve_ref_qualified(node, tree, symbol_map, refs);
        }
        "field_expression" => {
            resolve_ref_field(
                node, tree, scopes, symbol_map, vars, &global.field_type, &global.global_names, refs,
            );
        }
        "initializer_list" => {
            // The aggregate's type comes from the enclosing declaration; a nested
            // initializer_list has no cheaply-known member type, so it yields None.
            let tag = aggregate_tag_for_list(node, tree, typedefs);
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk_refs_node(
                    child, tree, scopes, symbol_map, vars, typedefs, global, tag.as_deref(), refs,
                );
            }
            return;
        }
        "initializer_pair" => {
            resolve_ref_init_pair(node, tree, init_tag, refs);
            if let Some(value) = node.child_by_field_name("value") {
                walk_refs_node(
                    value, tree, scopes, symbol_map, vars, typedefs, global, None, refs,
                );
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_refs_node(
            child, tree, scopes, symbol_map, vars, typedefs, global, init_tag, refs,
        );
    }
}

/// Resolve the struct tag of the aggregate an `initializer_list` initializes by
/// walking up to the enclosing declaration. Returns `None` for nested lists
/// (a member's own aggregate type isn't tracked per-file).
fn aggregate_tag_for_list(
    node: tree_sitter::Node,
    tree: &SourceTree,
    typedefs: &HashMap<String, String>,
) -> Option<String> {
    let mut cur = node.parent();
    while let Some(p) = cur {
        match p.kind() {
            "initializer_pair" | "initializer_list" => return None,
            "declaration" | "field_declaration" | "parameter_declaration" => {
                return p
                    .child_by_field_name("type")
                    .and_then(|t| base_type_tag(t, tree, typedefs));
            }
            _ => cur = p.parent(),
        }
    }
    None
}

/// Record a designated initializer's field (`.request = ...`) as a member
/// reference, qualified by the initialized struct's tag when known.
fn resolve_ref_init_pair(
    node: tree_sitter::Node,
    tree: &SourceTree,
    init_tag: Option<&str>,
    refs: &mut Vec<Reference>,
) {
    let designator = match node.child_by_field_name("designator") {
        Some(d) => d,
        None => return,
    };
    // field_designator wraps a field_identifier; array/subscript designators
    // ([0] = ...) have no field name and are skipped.
    let field_ident = match designator.named_child(0) {
        Some(c) if c.kind() == "field_identifier" => c,
        _ => return,
    };
    let field_name = tree.node_text(&field_ident);
    let symbol_name = match init_tag {
        Some(tag) => format!("{}::{}", tag, field_name),
        None => field_name.to_string(),
    };
    let pos = field_ident.start_position();
    let end_pos = field_ident.end_position();
    refs.push(Reference {
        id: None,
        symbol_id: 0,
        symbol_name,
        file_path: tree.file_path().to_string(),
        line: pos.row as u32,
        column: pos.column as u32,
        end_line: Some(end_pos.row as u32),
        end_column: Some(end_pos.column as u32),
        ref_kind: crate::model::ReferenceKind::Access,
        context: Some("member_access".to_string()),
        context_detail: None,
        context_line: None,
        is_definition: false,
    });
}

fn resolve_ref_id(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    vars: &VarTypes,
    global_names: &HashSet<String>,
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
    // A pure local (bound in the enclosing function, no global of the same name)
    // is not stored: it can't be queried project-wide and only bloats the DB.
    if vars.is_local(name) && !global_names.contains(name) {
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
    vars: &VarTypes,
    global_names: &HashSet<String>,
    refs: &mut Vec<Reference>,
) {
    let function = match node.child_by_field_name("function") {
        Some(f) => f,
        None => return,
    };
    let name = tree.node_text(&function);
    if vars.is_local(name) && !global_names.contains(name) {
        return;
    }
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

#[allow(clippy::too_many_arguments)]
fn resolve_ref_field(
    node: tree_sitter::Node,
    tree: &SourceTree,
    scopes: &[crate::model::Scope],
    symbol_map: &HashMap<String, i64>,
    vars: &VarTypes,
    field_type: &HashMap<(String, String), String>,
    global_names: &HashSet<String>,
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

    // Qualify the member by the receiver's struct tag when it can be inferred,
    // so `Struct.member` queries don't match same-named members of unrelated
    // structs. The receiver type follows dereference chains (a->b->c) via the
    // global field-type table. Falls back to the bare member name.
    let recv_tag = node
        .child_by_field_name("argument")
        .and_then(|op| receiver_type_tag(op, tree, vars, field_type, 0));
    let symbol_name = match recv_tag {
        Some(tag) => format!("{}::{}", tag, field_name),
        None => field_name.to_string(),
    };

    refs.push(Reference {
        id: None,
        symbol_id,
        symbol_name,
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
    if let Some(operand) = node.child_by_field_name("argument") {
        if operand.kind() == "identifier" {
            resolve_ref_id(operand, tree, scopes, symbol_map, vars, global_names, refs);
        }
    }
}

/// Peel a declarator down to the declared variable's identifier name.
/// Handles pointers, arrays, parentheses and initializers; returns `None` for
/// function declarators (a function name is not a variable).
fn declarator_var_name(node: tree_sitter::Node, tree: &SourceTree) -> Option<String> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => {
            Some(tree.node_text(&node).to_string())
        }
        // `(*name)(args)` — inner declarator is an unnamed child, not a field.
        "parenthesized_declarator" => node
            .named_child(0)
            .and_then(|c| declarator_var_name(c, tree)),
        "pointer_declarator" | "array_declarator" | "attributed_declarator" | "init_declarator" => {
            node.child_by_field_name("declarator")
                .and_then(|c| declarator_var_name(c, tree))
        }
        _ => None,
    }
}

/// Resolve a type-specifier node to a struct/union/class/enum tag name.
/// Typedef aliases are resolved through the per-file typedef map. Returns
/// `None` for primitive/sized types and unresolved aliases.
fn base_type_tag(
    node: tree_sitter::Node,
    tree: &SourceTree,
    typedefs: &HashMap<String, String>,
) -> Option<String> {
    match node.kind() {
        "struct_specifier" | "union_specifier" | "class_specifier" | "enum_specifier" => node
            .child_by_field_name("name")
            .map(|n| tree.node_text(&n).to_string()),
        "type_identifier" => typedefs.get(tree.node_text(&node)).cloned(),
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(tag) = base_type_tag(child, tree, typedefs) {
                    return Some(tag);
                }
            }
            None
        }
    }
}

/// Resolve a `field_expression` operand to its struct tag, following
/// dereference chains. Plain identifiers use the per-file variable-type map;
/// nested `a->b` resolves `a`'s tag then looks up `b`'s type in the global
/// field-type table. `*p` and `(expr)` wrappers are peeled. Returns `None` for
/// calls and other expressions whose type isn't tracked.
fn receiver_type_tag(
    operand: tree_sitter::Node,
    tree: &SourceTree,
    vars: &VarTypes,
    field_type: &HashMap<(String, String), String>,
    depth: usize,
) -> Option<String> {
    if depth > 8 {
        return None;
    }
    match operand.kind() {
        "identifier" => {
            let name = tree.node_text(&operand);
            vars.tag_of(name)
        }
        "pointer_expression" | "parenthesized_expression" => operand
            .named_child(0)
            .and_then(|c| receiver_type_tag(c, tree, vars, field_type, depth + 1)),
        "field_expression" => {
            let arg = operand.child_by_field_name("argument")?;
            let field = operand.child_by_field_name("field")?;
            let base = receiver_type_tag(arg, tree, vars, field_type, depth + 1)?;
            let field_name = tree.node_text(&field);
            field_type.get(&(base, field_name.to_string())).cloned()
        }
        _ => None,
    }
}

/// Build a per-file typedef alias -> struct-tag map for `typedef struct X Y;`.
fn collect_typedefs(tree: &SourceTree) -> HashMap<String, String> {
    let mut map = HashMap::new();
    fn walk(
        node: tree_sitter::Node,
        tree: &SourceTree,
        map: &mut HashMap<String, String>,
    ) {
        if node.kind() == "type_definition" {
            if let Some(ty) = node.child_by_field_name("type") {
                if let Some(tag) = base_type_tag(ty, tree, map) {
                    let mut cursor = node.walk();
                    for d in node.children_by_field_name("declarator", &mut cursor) {
                        if let Some(alias) = declarator_var_name(d, tree) {
                            map.insert(alias, tag.clone());
                        }
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            walk(child, tree, map);
        }
    }
    walk(tree.root_node(), tree, &mut map);
    map
}

/// Build a variable-name -> resolved-struct-tag map by walking from `start`.
/// A `None` value means the name is declared with a non-struct type or with
/// conflicting types within the walked region, so member accesses on it must not
/// be qualified. Only variable declarations (`declaration`,
/// `parameter_declaration`) are considered — struct field declarations name
/// members, not in-scope variables. When `skip_functions` is set, function
/// bodies are not descended, so the result holds only file-scope globals;
/// per-function locals are collected separately with `skip_functions = false`.
fn collect_var_types_subtree(
    start: tree_sitter::Node,
    tree: &SourceTree,
    typedefs: &HashMap<String, String>,
    skip_functions: bool,
) -> HashMap<String, Option<String>> {
    let mut map: HashMap<String, Option<String>> = HashMap::new();
    fn walk(
        node: tree_sitter::Node,
        tree: &SourceTree,
        typedefs: &HashMap<String, String>,
        skip_functions: bool,
        map: &mut HashMap<String, Option<String>>,
    ) {
        if skip_functions && node.kind() == "function_definition" {
            return;
        }
        match node.kind() {
            "declaration" | "parameter_declaration" => {
                if let Some(ty) = node.child_by_field_name("type") {
                    let tag = base_type_tag(ty, tree, typedefs);
                    let mut cursor = node.walk();
                    for d in node.children_by_field_name("declarator", &mut cursor) {
                        if let Some(name) = declarator_var_name(d, tree) {
                            map.entry(name)
                                .and_modify(|existing| {
                                    if *existing != tag {
                                        *existing = None;
                                    }
                                })
                                .or_insert_with(|| tag.clone());
                        }
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            walk(child, tree, typedefs, skip_functions, map);
        }
    }
    walk(start, tree, typedefs, skip_functions, &mut map);
    map
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
