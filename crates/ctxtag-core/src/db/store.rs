use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashMap;


use crate::model::{Include, Macro, Reference, Symbol};

use super::schema::{create_schema, create_schema_no_index, create_indexes, drop_indexes};

pub struct DbStore {
    pub(crate) conn: rusqlite::Connection,
}

impl DbStore {
    pub fn new(db_path: &str) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        create_schema(&conn)?;
        Ok(Self { conn: conn })
    }

    /// Create a DbStore optimized for bulk import.
    /// Uses aggressive pragmas (synchronous=OFF, large cache, 4KB pages)
    /// and defers index creation until finalize_indexes() is called.
    /// This strategy (Config C from profiling) yields ~1.7x speedup
    /// by avoiding index maintenance during bulk inserts.
    pub fn new_bulk_mode(db_path: &str) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        conn.pragma_update(None, "cache_size", "500000")?;
        conn.pragma_update(None, "page_size", "4096")?;
        conn.pragma_update(None, "temp_store", "memory")?;
        conn.pragma_update(None, "mmap_size", "2147483648")?;
        create_schema_no_index(&conn)?;
        Ok(Self { conn: conn })
    }

    /// Finalize the bulk import by building all indexes.
    /// Must be called after all data has been inserted.
    /// Index build is very fast (~0.0s for 1.7M rows).
    pub fn finalize_indexes(&self) -> anyhow::Result<()> {
        create_indexes(&self.conn)
    }

    /// Drop all user-created indexes to prepare for bulk insert.
    /// Use when switching an existing DB to bulk-import mode mid-session.
    pub fn drop_indexes(&self) -> anyhow::Result<()> {
        drop_indexes(&self.conn)
    }

    // ---- Files ----

    pub fn upsert_file(&mut self, path: &str, content: &str, language: &str) -> anyhow::Result<i64> {
        let hash = self.content_hash(content);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let id: Option<i64> = self.conn.query_row(
            "SELECT id FROM files WHERE path = ?",
            params![path],
            |r| r.get(0),
        )
        .optional()?;

        if let Some(id) = id {
            self.conn.execute(
                "UPDATE files SET hash = ?, last_indexed = ? WHERE id = ?",
                params![hash, now, id],
            )?;
            Ok(id)
        } else {
            self.conn.execute(
                "INSERT INTO files (path, hash, language, last_indexed) VALUES (?, ?, ?, ?)",
                params![path, hash, language, now],
            )?;
            Ok(self.conn.last_insert_rowid())
        }
    }

    /// Upsert a batch of files within a single transaction.
    /// Returns HashMap of path -> file_id.
    pub fn batch_upsert_files(
        &mut self,
        files: &[(&str, &str, &str)],
    ) -> anyhow::Result<HashMap<String, i64>> {
        if files.is_empty() {
            return Ok(HashMap::new());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Pre-compute hashes outside the transaction to avoid borrow conflict
        let hashes: Vec<String> = files.iter().map(|(_, c, _)| {
            let mut hasher = sha2::Sha256::new();
            hasher.update(c.as_bytes());
            format!("{:x}", hasher.finalize())
        }).collect();

        let tx = self.conn.transaction()?;
        let mut map = HashMap::new();

        for ((path, _content, language), hash) in files.iter().zip(hashes.into_iter()) {
            let id: Option<i64> = tx.query_row(
                "SELECT id FROM files WHERE path = ?",
                params![path],
                |r| r.get(0),
            )
            .optional()?;

            let file_id = if let Some(id) = id {
                tx.execute(
                    "UPDATE files SET hash = ?, last_indexed = ? WHERE id = ?",
                    params![hash, now, id],
                )?;
                id
            } else {
                tx.execute(
                    "INSERT INTO files (path, hash, language, last_indexed) VALUES (?, ?, ?, ?)",
                    params![path, hash, language, now],
                )?;
                tx.last_insert_rowid()
            };
            map.insert(path.to_string(), file_id);
        }

        tx.commit()?;
        Ok(map)
    }

    pub fn get_file_id(&self, path: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM files WHERE path = ?",
                params![path],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }

    pub fn file_hash(&self, path: &str) -> anyhow::Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT hash FROM files WHERE path = ?",
                params![path],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hash)
    }

    // ---- Symbols ----

    pub fn insert_symbol(&mut self, sym: &Symbol) -> anyhow::Result<i64> {
        let file_id = self.upsert_file(&sym.file_path, "", &self.language_from_path(&sym.file_path))?;
        let tx = self.conn.transaction()?;

        tx.execute(
            "INSERT INTO symbols (
                name, qualified_name, kind, is_definition, file_id,
                line_start, line_end, column_start, column_end,
                selection_line, selection_column,
                scope_kind, scope_depth, parent_symbol_id, container_name,
                declared_type, resolved_type, return_type, signature,
                access_modifier, storage_class, linkage,
                is_virtual, is_override, is_final, is_const_method,
                base_classes, template_params,
                declaration_text, definition_text, documentation,
                lsp_kind
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            )",
            params![
                sym.name,
                sym.qualified_name,
                sym.kind.as_str(),
                sym.is_definition as i32,
                file_id,
                sym.line_start as i32,
                sym.line_end as i32,
                sym.column_start as i32,
                sym.column_end as i32,
                sym.selection_line.map(|v| v as i32),
                sym.selection_column.map(|v| v as i32),
                sym.scope_kind.as_str(),
                sym.scope_depth as i32,
                sym.parent_symbol_id,
                sym.container_name,
                sym.declared_type,
                sym.resolved_type,
                sym.return_type,
                sym.signature,
                sym.access_modifier,
                sym.storage_class,
                sym.linkage,
                sym.is_virtual as i32,
                sym.is_override as i32,
                sym.is_final as i32,
                sym.is_const_method as i32,
                sym.base_classes.as_ref().map(|v| v.to_string()),
                sym.template_params.as_ref().map(|v| v.to_string()),
                sym.declaration_text,
                sym.definition_text,
                sym.documentation,
                sym.kind.lsp_kind() as i32,
            ],
        )?;
        let id: i64 = tx.last_insert_rowid();
        tx.commit()?;

        // Update FTS
        self.conn.execute(
            "INSERT INTO symbols_fts(rowid, name, qualified_name) VALUES (?, ?, ?)",
            params![id, sym.name, sym.qualified_name],
        )?;

        Ok(id)
    }

    /// Batch insert symbols within a single transaction.
    /// `file_map` provides file_path -> file_id to avoid DB lookups.
    pub fn batch_insert_symbols(
        &mut self,
        symbols: &[Symbol],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if symbols.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let mut count = 0;

        for sym in symbols {
            let file_id = *file_map.get(&sym.file_path).unwrap_or(&0);
            tx.execute(
                "INSERT INTO symbols (
                    name, qualified_name, kind, is_definition, file_id,
                    line_start, line_end, column_start, column_end,
                    selection_line, selection_column,
                    scope_kind, scope_depth, parent_symbol_id, container_name,
                    declared_type, resolved_type, return_type, signature,
                    access_modifier, storage_class, linkage,
                    is_virtual, is_override, is_final, is_const_method,
                    base_classes, template_params,
                    declaration_text, definition_text, documentation,
                    lsp_kind
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                )",
                params![
                    sym.name,
                    sym.qualified_name,
                    sym.kind.as_str(),
                    sym.is_definition as i32,
                    file_id,
                    sym.line_start as i32,
                    sym.line_end as i32,
                    sym.column_start as i32,
                    sym.column_end as i32,
                    sym.selection_line.map(|v| v as i32),
                    sym.selection_column.map(|v| v as i32),
                    sym.scope_kind.as_str(),
                    sym.scope_depth as i32,
                    sym.parent_symbol_id,
                    sym.container_name,
                    sym.declared_type,
                    sym.resolved_type,
                    sym.return_type,
                    sym.signature,
                    sym.access_modifier,
                    sym.storage_class,
                    sym.linkage,
                    sym.is_virtual as i32,
                    sym.is_override as i32,
                    sym.is_final as i32,
                    sym.is_const_method as i32,
                    sym.base_classes.as_ref().map(|v| v.to_string()),
                    sym.template_params.as_ref().map(|v| v.to_string()),
                    sym.declaration_text,
                    sym.definition_text,
                    sym.documentation,
                    sym.kind.lsp_kind() as i32,
                ],
            )?;
            let id: i64 = tx.last_insert_rowid();

            // Update FTS
            tx.execute(
                "INSERT INTO symbols_fts(rowid, name, qualified_name) VALUES (?, ?, ?)",
                params![id, sym.name, sym.qualified_name],
            )?;
            count += 1;
        }

        tx.commit()?;
        Ok(count)
    }

    // ---- References ----

    pub fn insert_reference(&mut self, ref_: &Reference) -> anyhow::Result<i64> {
        let file_id = self.get_file_id(&ref_.file_path)?.unwrap_or_else(|| {
            // Fallback: create file entry if missing
            self.upsert_file(&ref_.file_path, "", &self.language_from_path(&ref_.file_path))
                .unwrap_or(0)
        });

        self.conn.execute(
            r#"INSERT INTO "references" (
                symbol_id, symbol_name, file_id, line, column, end_line, end_column,
                ref_kind, context, context_detail, context_line, is_definition
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            params![
                ref_.symbol_id,
                ref_.symbol_name,
                file_id,
                ref_.line as i32,
                ref_.column as i32,
                ref_.end_line.map(|v| v as i32),
                ref_.end_column.map(|v| v as i32),
                ref_.ref_kind.as_str(),
                ref_.context,
                ref_.context_detail,
                ref_.context_line,
                ref_.is_definition as i32,
            ],
        )?;
        let id: i64 = self.conn.last_insert_rowid();
        Ok(id)
    }

    // ---- Macros ----

    pub fn insert_macro(&mut self, macro_: &Macro) -> anyhow::Result<i64> {
        let file_id = self.upsert_file(&macro_.file_path, "", &self.language_from_path(&macro_.file_path))?;
        self.conn.execute(
            "INSERT INTO macros (name, is_function_like, params, body, file_id, line, conditional_context)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                macro_.name,
                macro_.is_function_like as i32,
                serde_json::to_string(&macro_.params).ok(),
                &macro_.body,
                file_id,
                macro_.line as i32,
                macro_.conditional_context.as_ref().map(|v| v.to_string()),
            ],
        )?;
        let id: i64 = self.conn.last_insert_rowid();
        Ok(id)
    }

    /// Batch insert macros within a single transaction.
    /// `file_map` provides file_path -> file_id.
    pub fn batch_insert_macros(
        &mut self,
        macros: &[Macro],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if macros.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let mut count = 0;

        for macro_ in macros {
            let file_id = *file_map.get(&macro_.file_path).unwrap_or(&0);
            tx.execute(
                "INSERT INTO macros (name, is_function_like, params, body, file_id, line, conditional_context)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    macro_.name,
                    macro_.is_function_like as i32,
                    serde_json::to_string(&macro_.params).ok(),
                    &macro_.body,
                    file_id,
                    macro_.line as i32,
                    macro_.conditional_context.as_ref().map(|v| v.to_string()),
                ],
            )?;
            count += 1;
        }

        tx.commit()?;
        Ok(count)
    }

    // ---- Includes ----

    pub fn insert_include(&mut self, inc: &Include) -> anyhow::Result<()> {
        let from_id = self.upsert_file(&inc.from_file, "", &self.language_from_path(&inc.from_file))?;
        let to_id = self.upsert_file(&inc.to_file, "", &self.language_from_path(&inc.to_file))?;
        self.conn.execute(
            "INSERT INTO includes (from_file_id, to_file_id, line, is_system, original_text)
             VALUES (?, ?, ?, ?, ?)",
            params![
                from_id,
                to_id,
                inc.line as i32,
                inc.is_system as i32,
                &inc.original_text,
            ],
        )?;
        Ok(())
    }

    // ---- Struct Members ----

    pub fn insert_struct_member(
        &mut self,
        struct_symbol_id: i64,
        member_symbol_id: i64,
        member_index: u32,
        access_modifier: Option<&str>,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO struct_members (struct_symbol_id, member_symbol_id, member_index, access_modifier)
             VALUES (?, ?, ?, ?)",
            params![struct_symbol_id, member_symbol_id, member_index as i32, access_modifier],
        )?;
        Ok(())
    }

    // ---- Call Edges ----

    pub fn insert_call_edge(
        &mut self,
        caller_id: i64,
        callee_id: i64,
        file_path: &str,
        line: u32,
        column: u32,
        call_type: &str,
        args_text: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_id = self.upsert_file(file_path, "", &self.language_from_path(file_path))?;
        self.conn.execute(
            "INSERT INTO call_edges (caller_symbol_id, callee_symbol_id, file_id, line, column, call_type, args_text)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                caller_id,
                callee_id,
                file_id,
                line as i32,
                column as i32,
                call_type,
                args_text,
            ],
        )?;
        Ok(())
    }

    // ---- Type Aliases ----

    pub fn insert_type_alias(
        &mut self,
        alias_name: &str,
        underlying_type: &str,
        resolved_symbol_id: Option<i64>,
        file_path: &str,
        line: u32,
    ) -> anyhow::Result<()> {
        let file_id = self.upsert_file(file_path, "", &self.language_from_path(file_path))?;
        self.conn.execute(
            "INSERT INTO type_aliases (alias_name, underlying_type, resolved_symbol_id, file_id, line)
             VALUES (?, ?, ?, ?, ?)",
            params![alias_name, underlying_type, resolved_symbol_id, file_id, line as i32],
        )?;
        Ok(())
    }

    // ---- Inheritance ----

    pub fn insert_inheritance(
        &mut self,
        derived_id: i64,
        base_id: i64,
        access_modifier: Option<&str>,
        is_virtual: bool,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO inheritance (derived_symbol_id, base_symbol_id, access_modifier, is_virtual)
             VALUES (?, ?, ?, ?)",
            params![derived_id, base_id, access_modifier, is_virtual as i32],
        )?;
        Ok(())
    }

    // ---- Helpers ----

    fn content_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn language_from_path(&self, path: &str) -> String {
        match path.rsplit('.').next() {
            Some("c") | Some("h") => "c".to_string(),
            Some("cpp") | Some("cxx") | Some("cc") | Some("hpp") | Some("hxx") | Some("hh") => {
                "cpp".to_string()
            }
            _ => "c".to_string(),
        }
    }

    pub fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn begin_transaction(&mut self) -> anyhow::Result<rusqlite::Transaction> {
        Ok(self.conn.transaction()?)
    }

    pub fn commit_transaction(&mut self) -> anyhow::Result<()> {
        // No-op: transactions are auto-committed when dropped.
        // This exists for API symmetry with the engine.
        Ok(())
    }

    /// Lookup symbol ID by name and kind. Returns the first match.
    pub fn get_symbol_id(&self, name: &str, kind: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM symbols WHERE name = ? AND kind = ? LIMIT 1",
                params![name, kind],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Lookup symbol ID by name (any kind). Returns the first match.
    pub fn get_symbol_id_any(&self, name: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM symbols WHERE name = ? LIMIT 1",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Load all symbol names into a HashMap for fast lookup.
    /// Returns (name -> id) mapping, taking the first match per name.
    pub fn load_symbol_map(&self) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name FROM symbols"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.entry(name).or_insert(id);
        }
        Ok(map)
    }

    /// Batch insert references within a transaction.
    /// Also accepts a file_id cache to avoid repeated DB lookups.
    pub fn batch_insert_references(
        &mut self,
        references: &[Reference],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if references.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let mut count = 0;
        for ref_ in references {
            let file_id = *file_map.get(&ref_.file_path).unwrap_or(&0);
            tx.execute(
                r#"INSERT INTO "references" (
                    symbol_id, symbol_name, file_id, line, column, end_line, end_column,
                    ref_kind, context, context_detail, context_line, is_definition
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                params![
                    ref_.symbol_id,
                    ref_.symbol_name,
                    file_id,
                    ref_.line as i32,
                    ref_.column as i32,
                    ref_.end_line.map(|v| v as i32),
                    ref_.end_column.map(|v| v as i32),
                    ref_.ref_kind.as_str(),
                    ref_.context.as_deref(),
                    ref_.context_detail.as_deref(),
                    ref_.context_line.as_deref(),
                    ref_.is_definition as i32,
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// Batch insert call edges within a transaction.
    pub fn batch_insert_call_edges(
        &mut self,
        edges: &[(i64, i64, String, u32, u32, String)],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if edges.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let mut count = 0;
        for (caller_id, callee_id, file_path, line, column, args_text) in edges {
            let file_id = *file_map.get(file_path).unwrap_or(&0);
            tx.execute(
                "INSERT INTO call_edges (caller_symbol_id, callee_symbol_id, file_id, line, column, call_type, args_text)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    caller_id, callee_id, file_id,
                    *line as i32, *column as i32,
                    "direct", args_text.as_str(),
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// Load all file paths into a HashMap for fast lookup.
    pub fn load_file_map(&self) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path FROM files"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, id) = row?;
            map.insert(path, id);
        }
        Ok(map)
    }

    // ---- Stats ----

    pub fn stats(&self) -> anyhow::Result<serde_json::Value> {
        let file_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let symbol_count: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
        let ref_count: i64 =
            self.conn.query_row(r#"SELECT COUNT(*) FROM "references""#, [], |r| r.get(0))?;
        let macro_count: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM macros", [], |r| r.get(0))?;
        let include_count: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM includes", [], |r| r.get(0))?;
        let call_edge_count: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM call_edges", [], |r| r.get(0))?;

        Ok(serde_json::json!({
            "files": file_count,
            "symbols": symbol_count,
            "references": ref_count,
            "macros": macro_count,
            "includes": include_count,
            "call_edges": call_edge_count,
        }))
    }
}
