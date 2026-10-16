use std::collections::HashMap;

use rusqlite::params;

use crate::model::{ScopeKind, Symbol, SymbolKind};

use super::schema;
use super::store::DbStore;

pub struct QueryResult {
    pub symbol: Symbol,
    pub file_path: String,
}

/// Column list selected by every symbol query (order matters for row_to_symbol).
const SYM_COLS: &str = "s.id, sn.text, sq.text, s.kind, s.flags, s.file_id, \
     s.line_start, s.line_end, s.col_start, s.col_end, s.sel_line, s.sel_col, \
     s.scope_kind, s.scope_depth, s.parent_id, sc.text, f.path";

const SYM_JOINS: &str = "FROM symbols s \
     JOIN strings sn ON s.name_id = sn.id \
     JOIN strings sq ON s.qual_id = sq.id \
     LEFT JOIN strings sc ON s.container_id = sc.id \
     JOIN files f ON s.file_id = f.id";

/// Lazily-read source files, cached for the duration of a query so display text
/// (signature / documentation / context line) can be reconstructed on demand
/// instead of being stored in the DB.
struct LineCache {
    files: HashMap<String, Vec<String>>,
}

impl LineCache {
    fn new() -> Self {
        Self { files: HashMap::new() }
    }
    fn lines(&mut self, path: &str) -> &[String] {
        if !self.files.contains_key(path) {
            let content = std::fs::read_to_string(path).unwrap_or_default();
            let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            self.files.insert(path.to_string(), lines);
        }
        self.files.get(path).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Reconstruct the declaration/signature text starting at `line_start`.
fn build_signature(lines: &[String], line_start: usize) -> Option<String> {
    let mut out = String::new();
    let end = (line_start + 8).min(lines.len());
    for i in line_start..end {
        let l = lines[i].trim();
        if l.is_empty() {
            continue;
        }
        if let Some(brace) = l.find('{') {
            out.push_str(l[..brace].trim());
            break;
        }
        if let Some(semi) = l.find(';') {
            out.push_str(l[..=semi].trim());
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(l);
    }
    let t = out.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Reconstruct a preceding comment block as documentation.
fn build_docs(lines: &[String], line_start: usize) -> Option<String> {
    if line_start == 0 {
        return None;
    }
    let mut block: Vec<&str> = Vec::new();
    let mut i = line_start;
    while i > 0 {
        let l = lines[i - 1].trim();
        if l.starts_with("//") {
            block.push(l.trim_start_matches('/').trim());
        } else if l.starts_with('*') || l.starts_with("/*") || l.ends_with("*/") {
            let cleaned = l
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim_end_matches("*/")
                .trim();
            if !cleaned.is_empty() {
                block.push(cleaned);
            }
        } else {
            break;
        }
        i -= 1;
    }
    if block.is_empty() {
        return None;
    }
    block.reverse();
    Some(block.join("\n"))
}

impl DbStore {
    // ---- Definition queries ----

    pub fn find_definitions(&self, name: &str) -> anyhow::Result<Vec<QueryResult>> {
        let sql = format!(
            "SELECT {} {} WHERE sn.text = ? ORDER BY (s.flags & 1) DESC, f.path",
            SYM_COLS, SYM_JOINS
        );
        self.query_symbols(&sql, params![name])
    }

    pub fn find_definition_at(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> anyhow::Result<Option<QueryResult>> {
        let sql = format!(
            "SELECT {} {} WHERE f.path = ? AND s.line_start <= ? AND s.line_end >= ? \
             AND s.col_start <= ? AND s.col_end >= ? \
             ORDER BY (s.line_end - s.line_start) ASC LIMIT 1",
            SYM_COLS, SYM_JOINS
        );
        let l = line as i64;
        let c = column as i64;
        let mut results = self.query_symbols(&sql, params![file_path, l, l, c, c])?;
        Ok(results.into_iter().next())
    }

    pub fn find_struct_members(&self, struct_name: &str) -> anyhow::Result<Vec<QueryResult>> {
        // Members are symbols whose container name matches the aggregate name.
        let sql = format!(
            "SELECT {} {} WHERE sc.text = ? ORDER BY f.path, s.line_start",
            SYM_COLS, SYM_JOINS
        );
        self.query_symbols(&sql, params![struct_name])
    }

    // ---- Reference queries ----

    pub fn find_references(
        &self,
        name: &str,
    ) -> anyhow::Result<Vec<(i64, String, u32, u32, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, r.line, r.col, r.kind FROM refs r \
             JOIN strings sn ON r.name_id = sn.id \
             JOIN files f ON r.file_id = f.id \
             WHERE sn.text = ? ORDER BY f.path, r.line",
        )?;
        let mut rows = stmt.query(params![name])?;
        let mut raw: Vec<(String, u32, u32, i64)> = Vec::new();
        while let Some(row) = rows.next()? {
            raw.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
        }
        drop(rows);

        let mut cache = LineCache::new();
        let mut out = Vec::with_capacity(raw.len());
        for (path, line, col, kind) in raw {
            let ctx = cache
                .lines(&path)
                .get(line as usize)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let kind_str = schema::refkind_from_int(kind).as_str().to_string();
            out.push((0i64, path, line, col, kind_str, ctx));
        }
        Ok(out)
    }

    pub fn find_member_references(
        &self,
        _struct_name: &str,
        member_name: &str,
    ) -> anyhow::Result<Vec<(String, u32, u32, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, r.line, r.col FROM refs r \
             JOIN strings sn ON r.name_id = sn.id \
             JOIN files f ON r.file_id = f.id \
             WHERE sn.text = ? ORDER BY f.path, r.line",
        )?;
        let mut rows = stmt.query(params![member_name])?;
        let mut raw: Vec<(String, u32, u32)> = Vec::new();
        while let Some(row) = rows.next()? {
            raw.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        drop(rows);
        let mut cache = LineCache::new();
        let mut out = Vec::with_capacity(raw.len());
        for (path, line, col) in raw {
            let ctx = cache
                .lines(&path)
                .get(line as usize)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            out.push((path, line, col, ctx));
        }
        Ok(out)
    }

    // ---- Symbol listing ----

    pub fn list_symbols(
        &self,
        kind_filter: Option<&str>,
        file_filter: Option<&str>,
        scope_filter: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let mut sql = format!("SELECT {} {} WHERE 1=1", SYM_COLS, SYM_JOINS);
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(kind) = kind_filter {
            sql.push_str(" AND s.kind = ?");
            args.push(Box::new(schema::kind_to_int(kind)));
        }
        if let Some(file) = file_filter {
            sql.push_str(" AND f.path = ?");
            args.push(Box::new(file.to_string()));
        }
        if let Some(scope) = scope_filter {
            sql.push_str(" AND sc.text = ?");
            args.push(Box::new(scope.to_string()));
        }
        sql.push_str(" ORDER BY f.path, s.line_start LIMIT ?");
        args.push(Box::new(limit as i64));
        self.query_symbols(&sql, rusqlite::params_from_iter(args))
    }

    pub fn workspace_symbols(&self, query: &str, limit: usize) -> anyhow::Result<Vec<QueryResult>> {
        let sql = format!(
            "SELECT {} FROM symbols_fts \
             JOIN symbols s ON s.id = symbols_fts.rowid \
             JOIN strings sn ON s.name_id = sn.id \
             JOIN strings sq ON s.qual_id = sq.id \
             LEFT JOIN strings sc ON s.container_id = sc.id \
             JOIN files f ON s.file_id = f.id \
             WHERE symbols_fts MATCH ? ORDER BY rank LIMIT ?",
            SYM_COLS
        );
        self.query_symbols(&sql, params![query, limit as i64])
    }

    pub fn document_symbols(&self, file_path: &str) -> anyhow::Result<Vec<QueryResult>> {
        let sql = format!(
            "SELECT {} {} WHERE f.path = ? ORDER BY s.line_start",
            SYM_COLS, SYM_JOINS
        );
        self.query_symbols(&sql, params![file_path])
    }

    // ---- Call graph ----

    pub fn find_callers(&self, func_name: &str, _max_depth: u32) -> anyhow::Result<Vec<(String, String, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, sc.text, ce.line FROM call_edges ce \
             JOIN strings sc ON ce.caller_id = sc.id \
             JOIN strings se ON ce.callee_id = se.id \
             JOIN files f ON ce.file_id = f.id \
             WHERE se.text = ? ORDER BY f.path, ce.line",
        )?;
        let mut rows = stmt.query(params![func_name])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    pub fn find_callees(&self, func_name: &str) -> anyhow::Result<Vec<(String, u32, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, ce.line, se.text FROM call_edges ce \
             JOIN strings sc ON ce.caller_id = sc.id \
             JOIN strings se ON ce.callee_id = se.id \
             JOIN files f ON ce.file_id = f.id \
             WHERE sc.text = ? ORDER BY f.path, ce.line",
        )?;
        let mut rows = stmt.query(params![func_name])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    // ---- Include queries ----

    pub fn find_includes(&self, file_path: &str) -> anyhow::Result<Vec<(String, u32)>> {
        let Some(file_id) = self.get_file_id(file_path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT f.path, i.line FROM includes i JOIN files f ON i.to_file_id = f.id \
             WHERE i.from_file_id = ? ORDER BY i.line",
        )?;
        let mut rows = stmt.query(params![file_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    pub fn find_included_by(&self, file_path: &str) -> anyhow::Result<Vec<(String, u32)>> {
        let Some(file_id) = self.get_file_id(file_path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT f.path, i.line FROM includes i JOIN files f ON i.from_file_id = f.id \
             WHERE i.to_file_id = ? ORDER BY f.path, i.line",
        )?;
        let mut rows = stmt.query(params![file_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    // ---- Symbol info ----

    pub fn symbol_info(&self, name: &str) -> anyhow::Result<Vec<QueryResult>> {
        let sql = format!(
            "SELECT {} {} WHERE sn.text = ? ORDER BY s.kind, f.path",
            SYM_COLS, SYM_JOINS
        );
        self.query_symbols(&sql, params![name])
    }

    // ---- Internal helpers ----

    fn query_symbols<P>(&self, sql: &str, params: P) -> anyhow::Result<Vec<QueryResult>>
    where
        P: rusqlite::Params,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(Self::row_to_symbol(row)?);
        }
        drop(rows);
        self.hydrate(&mut results);
        Ok(results)
    }

    /// Fill in display-only text (signature, documentation) from source files.
    fn hydrate(&self, results: &mut [QueryResult]) {
        let mut cache = LineCache::new();
        for r in results.iter_mut() {
            let path = r.file_path.clone();
            let ls = r.symbol.line_start as usize;
            let lines = cache.lines(&path);
            if !lines.is_empty() {
                if ls < lines.len() {
                    r.symbol.signature = build_signature(lines, ls);
                    r.symbol.documentation = build_docs(lines, ls);
                }
            }
        }
    }

    fn row_to_symbol(row: &rusqlite::Row) -> anyhow::Result<QueryResult> {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let qualified_name: String = row.get(2)?;
        let kind_int: i64 = row.get(3)?;
        let flags: i64 = row.get(4)?;
        let _file_id: i64 = row.get(5)?;
        let line_start: i64 = row.get(6)?;
        let line_end: i64 = row.get(7)?;
        let column_start: i64 = row.get(8)?;
        let column_end: i64 = row.get(9)?;
        let sel_line: Option<i64> = row.get(10)?;
        let sel_col: Option<i64> = row.get(11)?;
        let scope_kind_int: i64 = row.get(12)?;
        let scope_depth: i64 = row.get(13)?;
        let parent_symbol_id: Option<i64> = row.get(14)?;
        let container_name: Option<String> = row.get(15)?;
        let file_path: String = row.get(16)?;

        let kind: SymbolKind = schema::kind_from_int(kind_int);
        let scope_kind: ScopeKind = schema::scope_from_int(scope_kind_int);

        let sym = Symbol {
            id: Some(id),
            name,
            qualified_name,
            kind,
            is_definition: flags & schema::F_SYMBOL_IS_DEF != 0,
            file_path: file_path.clone(),
            line_start: line_start as u32,
            line_end: line_end as u32,
            column_start: column_start as u32,
            column_end: column_end as u32,
            selection_line: sel_line.map(|v| v as u32),
            selection_column: sel_col.map(|v| v as u32),
            scope_kind,
            scope_depth: scope_depth as u32,
            parent_symbol_id,
            container_name,
            declared_type: None,
            resolved_type: None,
            return_type: None,
            signature: None,
            access_modifier: None,
            storage_class: None,
            linkage: None,
            is_virtual: flags & schema::F_SYMBOL_VIRTUAL != 0,
            is_override: flags & schema::F_SYMBOL_OVERRIDE != 0,
            is_final: flags & schema::F_SYMBOL_FINAL != 0,
            is_const_method: flags & schema::F_SYMBOL_CONST != 0,
            base_classes: None,
            template_params: None,
            declaration_text: None,
            definition_text: None,
            documentation: None,
        };

        Ok(QueryResult { symbol: sym, file_path })
    }
}
