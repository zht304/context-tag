use rusqlite::{params, OptionalExtension};

use crate::model::Symbol;

use super::store::DbStore;

pub struct QueryResult {
    pub symbol: Symbol,
    pub file_path: String,
}

impl DbStore {
    // ---- Definition queries ----

    /// Find symbol definitions by name.
    pub fn find_definitions(&self, name: &str) -> anyhow::Result<Vec<QueryResult>> {
        self.query_symbols("SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.name = ?", params![name])
    }

    /// Find symbol definition at a specific file position.
    pub fn find_definition_at(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> anyhow::Result<Option<QueryResult>> {
        let results = self.query_symbols(
            "SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE f.path = ? AND s.line_start <= ? AND s.line_end >= ? AND s.column_start <= ? AND s.column_end >= ?",
            params![file_path, line as i32, line as i32, column as i32, column as i32],
        )?;
        Ok(results.into_iter().next())
    }

    /// Find struct/class members.
    pub fn find_struct_members(&self, struct_name: &str) -> anyhow::Result<Vec<QueryResult>> {
        // Find the struct symbol
        let struct_symbols = self.query_symbols(
            "SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE s.name = ? AND s.kind IN ('struct', 'class', 'union')",
            params![struct_name],
        )?;

        let mut all_members = Vec::new();
        for ss in struct_symbols {
            let struct_id = ss.symbol.id.unwrap_or(0);
            let members = self.query_symbols(
                "SELECT s.*, f.path FROM symbols s
                 JOIN struct_members sm ON s.id = sm.member_symbol_id
                 JOIN files f ON s.file_id = f.id
                 WHERE sm.struct_symbol_id = ?
                 ORDER BY sm.member_index",
                params![struct_id],
            )?;
            all_members.extend(members);
        }
        Ok(all_members)
    }

    // ---- Reference queries ----

    /// Find all references to a symbol by name.
    pub fn find_references(&self, name: &str) -> anyhow::Result<Vec<(i64, String, u32, u32, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT r.symbol_id, f.path, r.line, r.column, r.ref_kind, COALESCE(r.context_line, '')
             FROM "references" r
             JOIN files f ON r.file_id = f.id
             WHERE r.symbol_name = ?
             ORDER BY f.path, r.line"#
        )?;

        let mut results = Vec::new();
        let mut rows = stmt.query(params![name])?;
        while let Some(row) = rows.next()? {
            let symbol_id: i64 = row.get(0)?;
            let file_path: String = row.get(1)?;
            let line: u32 = row.get(2)?;
            let column: u32 = row.get(3)?;
            let ref_kind: String = row.get(4)?;
            let context_line: String = row.get(5)?;
            results.push((symbol_id, file_path, line, column, ref_kind, context_line));
        }
        Ok(results)
    }

    /// Find references to a specific struct member.
    pub fn find_member_references(
        &self,
        struct_name: &str,
        member_name: &str,
    ) -> anyhow::Result<Vec<(String, u32, u32, String)>> {
        // Find struct ID
        let struct_id: Option<i64> = self.conn.query_row(
            "SELECT s.id FROM symbols s WHERE s.name = ? AND s.kind IN ('struct', 'class', 'union') LIMIT 1",
            params![struct_name],
            |r| r.get(0),
        ).optional()?;

        let struct_id = match struct_id {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        // Find member symbol ID
        let member_id: Option<i64> = self.conn.query_row(
            "SELECT sm.member_symbol_id FROM struct_members sm WHERE sm.struct_symbol_id = ?
             JOIN symbols s ON sm.member_symbol_id = s.id WHERE s.name = ? LIMIT 1",
            params![struct_id, member_name],
            |r| r.get(0),
        ).optional()?;

        let member_id = match member_id {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        let mut stmt = self.conn.prepare(
            r#"SELECT f.path, r.line, r.column, COALESCE(r.context_line, '')
             FROM "references" r
             JOIN files f ON r.file_id = f.id
             WHERE r.symbol_id = ?
             ORDER BY f.path, r.line"#
        )?;

        let mut results = Vec::new();
        let mut rows = stmt.query(params![member_id])?;
        while let Some(row) = rows.next()? {
            let file_path: String = row.get(0)?;
            let line: u32 = row.get(1)?;
            let column: u32 = row.get(2)?;
            let context_line: String = row.get(3)?;
            results.push((file_path, line, column, context_line));
        }
        Ok(results)
    }

    // ---- Symbol listing ----

    /// List all symbols with optional filters.
    pub fn list_symbols(
        &self,
        kind_filter: Option<&str>,
        file_filter: Option<&str>,
        scope_filter: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let mut query = String::from(
            "SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE 1=1"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(kind) = kind_filter {
            query.push_str(" AND s.kind = ?");
            params_vec.push(Box::new(kind.to_string()));
        }
        if let Some(file) = file_filter {
            query.push_str(" AND f.path = ?");
            params_vec.push(Box::new(file.to_string()));
        }
        if let Some(scope) = scope_filter {
            query.push_str(" AND s.container_name = ?");
            params_vec.push(Box::new(scope.to_string()));
        }
        query.push_str(" ORDER BY f.path, s.line_start LIMIT ?");
        params_vec.push(Box::new(limit as i32));

        self.query_symbols(&query, rusqlite::params_from_iter(params_vec))
    }

    /// Workspace symbol search (FTS).
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> anyhow::Result<Vec<QueryResult>> {
        self.query_symbols(
            "SELECT s.*, f.path FROM symbols_fts
             JOIN symbols s ON s.id = symbols_fts.rowid
             JOIN files f ON s.file_id = f.id
             WHERE symbols_fts MATCH ?
             ORDER BY rank
             LIMIT ?",
            params![query, limit as i32],
        )
    }

    /// Document symbols (outline for a file).
    pub fn document_symbols(&self, file_path: &str) -> anyhow::Result<Vec<QueryResult>> {
        self.query_symbols(
            "SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE f.path = ? ORDER BY s.line_start",
            params![file_path],
        )
    }

    // ---- Call graph ----

    pub fn find_callers(&self, func_name: &str, max_depth: u32) -> anyhow::Result<Vec<(String, String, u32)>> {
        let func_ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM symbols WHERE name = ? AND kind IN ('function', 'method', 'constructor')"
            )?;
            let mut rows = stmt.query(params![func_name])?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                ids.push(row.get::<_, i64>(0)?);
            }
            ids
        };

        if func_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut callers = Vec::new();
        for callee_id in &func_ids {
            let placeholders: String = func_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let mut stmt = self.conn.prepare(&format!(
                "SELECT f.path, s.name, ce.line FROM call_edges ce
                 JOIN files f ON ce.file_id = f.id
                 JOIN symbols s ON ce.caller_symbol_id = s.id
                 WHERE ce.callee_symbol_id IN ({}) ORDER BY f.path, ce.line",
                placeholders
            ))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(func_ids.clone()))?;
            while let Some(row) = rows.next()? {
                let file: String = row.get(0)?;
                let caller: String = row.get(1)?;
                let line: u32 = row.get(2)?;
                callers.push((file, caller, line));
            }
        }
        Ok(callers)
    }

    pub fn find_callees(&self, func_name: &str) -> anyhow::Result<Vec<(String, u32, String)>> {
        let func_ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM symbols WHERE name = ? AND kind IN ('function', 'method', 'constructor')"
            )?;
            let mut rows = stmt.query(params![func_name])?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                ids.push(row.get::<_, i64>(0)?);
            }
            ids
        };

        if func_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut callees = Vec::new();
        let placeholders: String = func_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT f.path, ce.line, s.name FROM call_edges ce
             JOIN files f ON ce.file_id = f.id
             JOIN symbols s ON ce.callee_symbol_id = s.id
             WHERE ce.caller_symbol_id IN ({}) ORDER BY f.path, ce.line",
            placeholders
        ))?;
        let mut rows = stmt.query(rusqlite::params_from_iter(func_ids))?;
        while let Some(row) = rows.next()? {
            let file: String = row.get(0)?;
            let line: u32 = row.get(1)?;
            let callee: String = row.get(2)?;
            callees.push((file, line, callee));
        }
        Ok(callees)
    }

    // ---- Include queries ----

    pub fn find_includes(&self, file_path: &str) -> anyhow::Result<Vec<(String, u32)>> {
        let file_id: Option<i64> = self.get_file_id(file_path)?;
        let file_id = match file_id {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT f.path, i.line FROM includes i JOIN files f ON i.to_file_id = f.id
             WHERE i.from_file_id = ? ORDER BY i.line"
        )?;
        let mut rows = stmt.query(params![file_id])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let line: u32 = row.get(1)?;
            results.push((path, line));
        }
        Ok(results)
    }

    pub fn find_included_by(&self, file_path: &str) -> anyhow::Result<Vec<(String, u32)>> {
        let file_id: Option<i64> = self.get_file_id(file_path)?;
        let file_id = match file_id {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT f.path, i.line FROM includes i JOIN files f ON i.from_file_id = f.id
             WHERE i.to_file_id = ? ORDER BY f.path, i.line"
        )?;
        let mut rows = stmt.query(params![file_id])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let line: u32 = row.get(1)?;
            results.push((path, line));
        }
        Ok(results)
    }

    // ---- Symbol info ----

    pub fn symbol_info(&self, name: &str) -> anyhow::Result<Vec<QueryResult>> {
        self.query_definitions(name)
    }

    fn query_definitions(&self, name: &str) -> anyhow::Result<Vec<QueryResult>> {
        self.query_symbols(
            "SELECT s.*, f.path FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE s.name = ? ORDER BY s.kind, f.path",
            params![name],
        )
    }

    // ---- Internal helpers ----

    fn query_symbols<P>(
        &self,
        sql: &str,
        params: P,
    ) -> anyhow::Result<Vec<QueryResult>>
    where
        P: rusqlite::Params,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        let mut results = Vec::new();

        while let Some(row) = rows.next()? {
            let sym = Self::row_to_symbol(row)?;
            results.push(sym);
        }
        Ok(results)
    }

    fn row_to_symbol(row: &rusqlite::Row) -> anyhow::Result<QueryResult> {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let qualified_name: String = row.get(2)?;
        let kind_str: String = row.get(3)?;
        let is_definition: i32 = row.get(4)?;
        let file_id: i64 = row.get(5)?;
        let line_start: i32 = row.get(6)?;
        let line_end: i32 = row.get(7)?;
        let column_start: i32 = row.get(8)?;
        let column_end: i32 = row.get(9)?;
        let selection_line: Option<i32> = row.get(10)?;
        let selection_column: Option<i32> = row.get(11)?;
        let scope_kind_str: String = row.get(12)?;
        let scope_depth: i32 = row.get(13)?;
        let parent_symbol_id: Option<i64> = row.get(14)?;
        let container_name: Option<String> = row.get(15)?;
        let declared_type: Option<String> = row.get(16)?;
        let resolved_type: Option<String> = row.get(17)?;
        let return_type: Option<String> = row.get(18)?;
        let signature: Option<String> = row.get(19)?;
        let access_modifier: Option<String> = row.get(20)?;
        let storage_class: Option<String> = row.get(21)?;
        let linkage: Option<String> = row.get(22)?;
        let is_virtual: i32 = row.get(23)?;
        let is_override: i32 = row.get(24)?;
        let is_final: i32 = row.get(25)?;
        let is_const_method: i32 = row.get(26)?;
        let base_classes: Option<String> = row.get(27)?;
        let template_params: Option<String> = row.get(28)?;
        let declaration_text: Option<String> = row.get(29)?;
        let definition_text: Option<String> = row.get(30)?;
        let documentation: Option<String> = row.get(31)?;
        let _lsp_kind: i32 = row.get(32)?;
        let file_path: String = row.get(33)?;

        let kind = match kind_str.as_str() {
            "function" => crate::model::SymbolKind::Function,
            "method" => crate::model::SymbolKind::Method,
            "constructor" => crate::model::SymbolKind::Constructor,
            "destructor" => crate::model::SymbolKind::Destructor,
            "operator" => crate::model::SymbolKind::Operator,
            "lambda" => crate::model::SymbolKind::Lambda,
            "parameter" => crate::model::SymbolKind::Parameter,
            "variable" => crate::model::SymbolKind::Variable,
            "local_variable" => crate::model::SymbolKind::LocalVariable,
            "member" => crate::model::SymbolKind::Member,
            "static_member" => crate::model::SymbolKind::StaticMember,
            "struct" => crate::model::SymbolKind::Struct,
            "class" => crate::model::SymbolKind::Class,
            "union" => crate::model::SymbolKind::Union,
            "enum" => crate::model::SymbolKind::Enum,
            "enum_value" => crate::model::SymbolKind::EnumValue,
            "typedef" => crate::model::SymbolKind::Typedef,
            "using_alias" => crate::model::SymbolKind::UsingAlias,
            "namespace" => crate::model::SymbolKind::Namespace,
            "macro" => crate::model::SymbolKind::Macro,
            "label" => crate::model::SymbolKind::Label,
            "template" => crate::model::SymbolKind::Template,
            _ => crate::model::SymbolKind::Variable,
        };

        let scope_kind = match scope_kind_str.as_str() {
            "global" => crate::model::ScopeKind::Global,
            "namespace" => crate::model::ScopeKind::Namespace,
            "class" => crate::model::ScopeKind::Class,
            "struct" => crate::model::ScopeKind::Struct,
            "union" => crate::model::ScopeKind::Union,
            "enum" => crate::model::ScopeKind::Enum,
            "function" => crate::model::ScopeKind::Function,
            "block" => crate::model::ScopeKind::Block,
            _ => crate::model::ScopeKind::Global,
        };

        let sym = crate::model::Symbol {
            id: Some(id),
            name,
            qualified_name,
            kind,
            is_definition: is_definition != 0,
            file_path: file_path.clone(),
            line_start: line_start as u32,
            line_end: line_end as u32,
            column_start: column_start as u32,
            column_end: column_end as u32,
            selection_line: selection_line.map(|v| v as u32),
            selection_column: selection_column.map(|v| v as u32),
            scope_kind,
            scope_depth: scope_depth as u32,
            parent_symbol_id,
            container_name,
            declared_type,
            resolved_type,
            return_type,
            signature,
            access_modifier,
            storage_class,
            linkage,
            is_virtual: is_virtual != 0,
            is_override: is_override != 0,
            is_final: is_final != 0,
            is_const_method: is_const_method != 0,
            base_classes: base_classes.and_then(|s| serde_json::from_str(&s).ok()),
            template_params: template_params.and_then(|s| serde_json::from_str(&s).ok()),
            declaration_text,
            definition_text,
            documentation,
        };

        Ok(QueryResult {
            symbol: sym,
            file_path,
        })
    }
}
