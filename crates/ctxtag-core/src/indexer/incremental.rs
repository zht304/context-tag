use std::collections::HashMap;
use std::fs;
use sha2::{Digest, Sha256};

use crate::db::DbStore;
use crate::parser::{
    CallGraph, IncludeTracker, LanguageKind, MacroTracker, ScopeBuilder,
    SourceTree, SymbolExtractor,
};
use super::engine::{GlobalTypes, IndexStats, discover_source_files, resolve_references_with_map};
use super::include_resolver::IncludeResolver;

/// Incremental indexer: detects changed/new/deleted files and re-indexes only those.
pub struct IncrementalIndexer<'a> {
    root_dirs: Vec<String>,
    file_list: Option<String>,
    store: &'a mut DbStore,
    extensions: Vec<String>,
    exclude_patterns: Vec<String>,
}

impl<'a> IncrementalIndexer<'a> {
    pub fn new(
        root_dirs: Vec<String>,
        file_list: Option<String>,
        store: &'a mut DbStore,
        extensions: Vec<String>,
        exclude_patterns: Vec<String>,
    ) -> Self {
        Self {
            root_dirs,
            file_list,
            store,
            extensions,
            exclude_patterns,
        }
    }

    pub fn update(&mut self) -> anyhow::Result<IndexStats> {
        let mut stats = IndexStats::default();

        // 1. Discover current source files on disk
        let disk_files = self.discover_files();
        let disk_paths: std::collections::HashSet<_> = disk_files.iter().cloned().collect();

        // 2. Load DB file map
        let db_file_map = self.store.load_file_map()?;

        // 3. Detect deleted files (in DB but not on disk)
        for (path, file_id) in &db_file_map {
            if !disk_paths.contains(path.as_str()) {
                tracing::info!("Deleted: {}", path);
                self.store.delete_file_data(*file_id)?;
                self.store.delete_file(*file_id)?;
                stats.files_indexed += 1; // count cleanup
            }
        }

        // 4. Classify disk files into new, changed, and unchanged
        let mut new_files: Vec<String> = Vec::new();
        let mut changed_files: Vec<String> = Vec::new();

        for path in &disk_files {
            match self.store.file_hash(path) {
                Ok(Some(old_hash)) => {
                    // File exists in DB, check if content changed
                    match fs::read_to_string(path) {
                        Ok(content) => {
                            let new_hash = self.compute_hash(&content);
                            if old_hash != new_hash {
                                changed_files.push(path.clone());
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Cannot read {}: {}", path, e);
                            stats.files_failed += 1;
                        }
                    }
                }
                Ok(None) => {
                    // New file not in DB
                    new_files.push(path.clone());
                }
                Err(e) => {
                    tracing::warn!("DB error for {}: {}", path, e);
                    stats.files_failed += 1;
                }
            }
        }

        if new_files.is_empty() && changed_files.is_empty() {
            tracing::info!("No changes detected, skipping update");
            return Ok(stats);
        }

        let dirty_count = new_files.len() + changed_files.len();
        tracing::info!(
            "Incremental update: {} new, {} changed files",
            new_files.len(),
            changed_files.len()
        );

        // 5. For changed files: delete old data first
        for path in &changed_files {
            if let Ok(Some(file_id)) = self.store.get_file_id(path) {
                tracing::info!("Cleaning old data for: {}", path);
                let _ = self.store.delete_file_data(file_id);
            }
        }

        // 6. Re-index dirty files: Pass 1 (symbols, macros, includes)
        let mut dirty_files: Vec<String> = changed_files.clone();
        dirty_files.extend(new_files.clone());

        let mut file_map: HashMap<String, i64> = HashMap::new();
        let mut file_batch: Vec<(String, String, &'static str)> = Vec::new();
        let mut sym_batch: Vec<crate::model::Symbol> = Vec::new();
        let mut macro_batch: Vec<crate::model::Macro> = Vec::new();
        let mut inc_batch: Vec<crate::model::Include> = Vec::new();

        for file_path in &dirty_files {
            match self.index_file_pass1(file_path) {
                Ok((content, language, symbols, macros, includes)) => {
                    let lang_str: &'static str = if language == LanguageKind::Cpp { "cpp" } else { "c" };
                    file_batch.push((file_path.clone(), content, lang_str));
                    sym_batch.extend(symbols);
                    macro_batch.extend(macros);
                    inc_batch.extend(includes);
                    stats.files_indexed += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to index {}: {}", file_path, e);
                    stats.files_failed += 1;
                }
            }
        }

        // Flush Pass 1 data
        if !file_batch.is_empty() {
            let fb_refs: Vec<_> = file_batch.iter().map(|(p, c, l)| (p.as_str(), c.as_str(), *l)).collect();
            let fmap = self.store.batch_upsert_files(&fb_refs)?;
            file_map.extend(fmap);
        }

        if !sym_batch.is_empty() {
            let count = sym_batch.len();
            self.store.batch_insert_symbols(&sym_batch, &file_map)?;
            stats.symbols_extracted += count;
        }

        if !macro_batch.is_empty() {
            let count = macro_batch.len();
            self.store.batch_insert_macros(&macro_batch, &file_map)?;
            stats.macros_indexed += count;
        }

        // Re-resolve and re-insert include edges for the dirty files (their old
        // edges were removed by delete_file_data above). Resolve against the
        // full on-disk file set and the full DB file map so cross-file targets
        // that were not re-indexed still link up.
        if !inc_batch.is_empty() {
            let mut resolver = IncludeResolver::new(self.root_dirs.clone());
            let resolved = resolver.resolve_all(&inc_batch, &disk_files);
            let full_map = self.store.load_file_map()?;
            stats.includes_resolved = self.store.batch_insert_includes(&resolved, &full_map)?;
        }

        // 7. Pass 2: resolve references and call edges for dirty files.
        //    References and edges are stored name-based (resolved at query time),
        //    so no global symbol map is required.
        let mut ref_batch: Vec<crate::model::Reference> = Vec::new();
        let mut edge_batch: Vec<(String, String, String, u32, u32)> = Vec::new();

        for file_path in &dirty_files {
            match self.index_file_pass2(file_path) {
                Ok((refs, edges)) => {
                    ref_batch.extend(refs);
                    edge_batch.extend(edges);
                }
                Err(e) => {
                    tracing::warn!("Failed to resolve refs for {}: {}", file_path, e);
                    stats.files_failed += 1;
                }
            }
        }

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

        tracing::info!("Incremental update complete: {:?}", stats);
        Ok(stats)
    }

    /// Discover source files on disk (shared logic with IndexEngine).
    fn discover_files(&self) -> Vec<String> {
        discover_source_files(
            &self.root_dirs,
            self.file_list.as_deref(),
            &self.extensions,
            &self.exclude_patterns,
        )
    }

    fn compute_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Parse a single file and extract symbols/macros (Pass 1).
    fn index_file_pass1(
        &self,
        file_path: &str,
    ) -> anyhow::Result<(String, LanguageKind, Vec<crate::model::Symbol>, Vec<crate::model::Macro>, Vec<crate::model::Include>)> {
        let content = fs::read_to_string(file_path)?;
        let content_bytes = content.as_bytes();

        let language = Self::detect_language(file_path);
        let mut tree = SourceTree::parse(language, content_bytes)?;
        tree.set_file_path(file_path.to_string());

        let mut extractor = SymbolExtractor::new(file_path);
        let symbols = extractor.extract(&tree);

        let mut macro_tracker = MacroTracker::new();
        let macros = macro_tracker.track(&tree);

        let mut include_tracker = IncludeTracker::new();
        let includes = include_tracker.track(&tree);

        drop(tree);

        Ok((content, language, symbols, macros, includes))
    }

    /// Parse a single file and resolve references / call edges (name-based).
    fn index_file_pass2(
        &self,
        file_path: &str,
    ) -> anyhow::Result<(Vec<crate::model::Reference>, Vec<(String, String, String, u32, u32)>)> {
        let content = fs::read_to_string(file_path)?;
        let content_bytes = content.as_bytes();

        let language = Self::detect_language(file_path);
        let mut tree = SourceTree::parse(language, content_bytes)?;
        tree.set_file_path(file_path.to_string());

        let mut scope_builder = ScopeBuilder::new();
        let global_scope = scope_builder.build(&tree);
        let scopes = vec![global_scope];

        let empty: HashMap<String, i64> = HashMap::new();
        // Single-file reindex lacks the cross-file type tables, so member refs
        // fall back to same-file inference (as before global tables existed).
        let references =
            resolve_references_with_map(&tree, &scopes, &empty, &GlobalTypes::default());

        let mut call_graph = CallGraph::new();
        let edges = call_graph.extract(&tree);

        let mut name_edges: Vec<(String, String, String, u32, u32)> = Vec::new();
        for edge in edges {
            if let Some(caller_name) = edge.caller_name {
                if !caller_name.is_empty() && !edge.callee_name.is_empty() {
                    name_edges.push((
                        caller_name,
                        edge.callee_name,
                        file_path.to_string(),
                        edge.line,
                        edge.column,
                    ));
                }
            }
        }

        Ok((references, name_edges))
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
}
