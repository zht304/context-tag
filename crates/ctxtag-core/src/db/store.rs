use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::model::{Include, Macro, Reference, Symbol};

use super::schema::{
    self, create_indexes, create_schema, create_schema_no_index, drop_indexes,
};

/// Compact, interned write/read store.
///
/// All identifier strings live once in the `strings` table; every other table
/// references them by integer id. `str_cache` mirrors that mapping in memory so
/// interning during a bulk index is a hash lookup, not a DB round-trip.
pub struct DbStore {
    pub(crate) conn: rusqlite::Connection,
    str_cache: HashMap<String, i64>,
}

/// Intern a string, returning its id. Uses an in-memory cache first, then a
/// single UPSERT ... RETURNING statement so a miss costs one round-trip.
fn intern(tx: &Transaction, cache: &mut HashMap<String, i64>, s: &str) -> anyhow::Result<i64> {
    if let Some(&id) = cache.get(s) {
        return Ok(id);
    }
    let id: i64 = tx.query_row(
        "INSERT INTO strings(text) VALUES(?1) ON CONFLICT(text) DO UPDATE SET text=text RETURNING id",
        params![s],
        |r| r.get(0),
    )?;
    cache.insert(s.to_string(), id);
    Ok(id)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn lang_from_path(path: &str) -> i64 {
    match path.rsplit('.').next() {
        Some("cpp") | Some("cxx") | Some("cc") | Some("hpp") | Some("hxx") | Some("hh") => {
            schema::LANG_CPP
        }
        _ => schema::LANG_C,
    }
}

fn lang_from_str(s: &str) -> i64 {
    if s == "cpp" {
        schema::LANG_CPP
    } else {
        schema::LANG_C
    }
}

fn symbol_flags(sym: &Symbol) -> i64 {
    let mut f = 0i64;
    if sym.is_definition {
        f |= schema::F_SYMBOL_IS_DEF;
    }
    if sym.is_virtual {
        f |= schema::F_SYMBOL_VIRTUAL;
    }
    if sym.is_override {
        f |= schema::F_SYMBOL_OVERRIDE;
    }
    if sym.is_final {
        f |= schema::F_SYMBOL_FINAL;
    }
    if sym.is_const_method {
        f |= schema::F_SYMBOL_CONST;
    }
    f
}

const INS_SYMBOL: &str = "INSERT INTO symbols (
    name_id, qual_id, kind, flags, file_id,
    line_start, line_end, col_start, col_end, sel_line, sel_col,
    scope_kind, scope_depth, parent_id, container_id
) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";

const INS_REF: &str = "INSERT INTO refs (
    name_id, file_id, line, col, end_line, end_col, kind, flags
) VALUES (?,?,?,?,?,?,?,?)";

const INS_CALL: &str =
    "INSERT INTO call_edges (caller_id, callee_id, file_id, line, col) VALUES (?,?,?,?,?)";

const INS_MACRO: &str =
    "INSERT INTO macros (name_id, flags, params_id, body_id, file_id, line) VALUES (?,?,?,?,?,?)";

const INS_INCLUDE: &str =
    "INSERT INTO includes (from_file_id, to_file_id, line, flags) VALUES (?,?,?,?)";

const INS_FTS: &str = "INSERT INTO symbols_fts(rowid, name) VALUES (?,?)";

impl DbStore {
    pub fn new(db_path: &str) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        create_schema(&conn)?;
        Ok(Self {
            conn,
            str_cache: HashMap::new(),
        })
    }

    /// Bulk-import mode: aggressive pragmas, indexes deferred until
    /// `finalize_indexes()`.
    pub fn new_bulk_mode(db_path: &str) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        conn.pragma_update(None, "cache_size", "-1000000")?;
        conn.pragma_update(None, "temp_store", "memory")?;
        conn.pragma_update(None, "mmap_size", "2147483648")?;
        create_schema_no_index(&conn)?;
        Ok(Self {
            conn,
            str_cache: HashMap::new(),
        })
    }

    pub fn finalize_indexes(&self) -> anyhow::Result<()> {
        create_indexes(&self.conn)?;
        self.conn.execute_batch("ANALYZE;")?;
        Ok(())
    }

    pub fn drop_indexes(&self) -> anyhow::Result<()> {
        drop_indexes(&self.conn)
    }

    // ---- Files ----

    pub fn upsert_file(&mut self, path: &str, content: &str, language: &str) -> anyhow::Result<i64> {
        let hash = hash_content(content);
        let lang = lang_from_str(language);
        let now = now_secs();
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM files WHERE path = ?", params![path], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            self.conn
                .execute("UPDATE files SET hash=?, mtime=? WHERE id=?", params![hash, now, id])?;
            Ok(id)
        } else {
            self.conn.execute(
                "INSERT INTO files (path, hash, lang, mtime) VALUES (?,?,?,?)",
                params![path, hash, lang, now],
            )?;
            Ok(self.conn.last_insert_rowid())
        }
    }

    pub fn batch_upsert_files(
        &mut self,
        files: &[(&str, &str, &str)],
    ) -> anyhow::Result<HashMap<String, i64>> {
        if files.is_empty() {
            return Ok(HashMap::new());
        }
        let now = now_secs();
        let hashes: Vec<String> = files.iter().map(|(_, c, _)| hash_content(c)).collect();
        let tx = self.conn.transaction()?;
        let mut map = HashMap::with_capacity(files.len());
        {
            let mut sel = tx.prepare_cached("SELECT id FROM files WHERE path=?")?;
            let mut ins =
                tx.prepare_cached("INSERT INTO files (path, hash, lang, mtime) VALUES (?,?,?,?)")?;
            let mut upd = tx.prepare_cached("UPDATE files SET hash=?, mtime=? WHERE id=?")?;
            for ((path, _content, language), hash) in files.iter().zip(hashes.into_iter()) {
                let lang = lang_from_str(language);
                let existing: Option<i64> = sel
                    .query_row(params![path], |r| r.get(0))
                    .optional()?;
                let file_id = match existing {
                    Some(id) => {
                        upd.execute(params![hash, now, id])?;
                        id
                    }
                    None => {
                        ins.execute(params![path, hash, lang, now])?;
                        tx.last_insert_rowid()
                    }
                };
                map.insert(path.to_string(), file_id);
            }
        }
        tx.commit()?;
        Ok(map)
    }

    pub fn get_file_id(&self, path: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM files WHERE path = ?", params![path], |r| r.get(0))
            .optional()?;
        Ok(id)
    }

    pub fn file_hash(&self, path: &str) -> anyhow::Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row("SELECT hash FROM files WHERE path = ?", params![path], |r| r.get(0))
            .optional()?;
        Ok(hash)
    }

    pub fn load_file_map(&self) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare("SELECT id, path FROM files")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, id) = row?;
            map.insert(path, id);
        }
        Ok(map)
    }

    pub fn load_symbol_map(&self) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT s.id, sn.text FROM symbols s JOIN strings sn ON s.name_id = sn.id")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.entry(name).or_insert(id);
        }
        Ok(map)
    }

    // ---- Symbols ----

    pub fn insert_symbol(&mut self, sym: &Symbol) -> anyhow::Result<i64> {
        let file_id = self.upsert_file(&sym.file_path, "", &lang_str_for(&sym.file_path))?;
        let map = HashMap::from([(sym.file_path.clone(), file_id)]);
        self.batch_insert_symbols(std::slice::from_ref(sym), &map)?;
        let id: i64 = self.conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))?;
        Ok(id)
    }

    pub fn batch_insert_symbols(
        &mut self,
        symbols: &[Symbol],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if symbols.is_empty() {
            return Ok(0);
        }
        let Self { conn, str_cache } = self;
        let tx = conn.transaction()?;
        let mut count = 0;
        {
            let mut ins = tx.prepare_cached(INS_SYMBOL)?;
            let mut fts = tx.prepare_cached(INS_FTS)?;
            for sym in symbols {
                let file_id = *file_map.get(&sym.file_path).unwrap_or(&0);
                let name_id = intern(&tx, str_cache, &sym.name)?;
                let qual_id = if sym.qualified_name == sym.name {
                    name_id
                } else {
                    intern(&tx, str_cache, &sym.qualified_name)?
                };
                let container_id = match &sym.container_name {
                    Some(c) if !c.is_empty() => Some(intern(&tx, str_cache, c)?),
                    _ => None,
                };
                let kind = schema::kind_to_int(sym.kind.as_str());
                let flags = symbol_flags(sym);
                let scope_kind = schema::scope_to_int(sym.scope_kind.as_str());
                ins.execute(params![
                    name_id,
                    qual_id,
                    kind,
                    flags,
                    file_id,
                    sym.line_start as i64,
                    sym.line_end as i64,
                    sym.column_start as i64,
                    sym.column_end as i64,
                    sym.selection_line.map(|v| v as i64),
                    sym.selection_column.map(|v| v as i64),
                    scope_kind,
                    sym.scope_depth as i64,
                    sym.parent_symbol_id,
                    container_id,
                ])?;
                let id = tx.last_insert_rowid();
                fts.execute(params![id, sym.name])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    // ---- References ----

    pub fn insert_reference(&mut self, ref_: &Reference) -> anyhow::Result<i64> {
        let file_id = self.get_file_id(&ref_.file_path)?.unwrap_or(0);
        let map = HashMap::from([(ref_.file_path.clone(), file_id)]);
        self.batch_insert_references(std::slice::from_ref(ref_), &map)?;
        let id: i64 = self.conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))?;
        Ok(id)
    }

    pub fn batch_insert_references(
        &mut self,
        references: &[Reference],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if references.is_empty() {
            return Ok(0);
        }
        let Self { conn, str_cache } = self;
        let tx = conn.transaction()?;
        let mut count = 0;
        {
            let mut ins = tx.prepare_cached(INS_REF)?;
            for r in references {
                let file_id = *file_map.get(&r.file_path).unwrap_or(&0);
                let name_id = intern(&tx, str_cache, &r.symbol_name)?;
                let kind = schema::refkind_to_int(r.ref_kind.as_str());
                let flags = if r.is_definition { schema::F_REF_IS_DEF } else { 0 };
                ins.execute(params![
                    name_id,
                    file_id,
                    r.line as i64,
                    r.column as i64,
                    r.end_line.map(|v| v as i64),
                    r.end_column.map(|v| v as i64),
                    kind,
                    flags,
                ])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    // ---- Call edges (name-based) ----

    pub fn batch_insert_call_edges(
        &mut self,
        edges: &[(String, String, String, u32, u32)],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if edges.is_empty() {
            return Ok(0);
        }
        let Self { conn, str_cache } = self;
        let tx = conn.transaction()?;
        let mut count = 0;
        {
            let mut ins = tx.prepare_cached(INS_CALL)?;
            for (caller, callee, file_path, line, col) in edges {
                let file_id = *file_map.get(file_path).unwrap_or(&0);
                let caller_id = intern(&tx, str_cache, caller)?;
                let callee_id = intern(&tx, str_cache, callee)?;
                ins.execute(params![caller_id, callee_id, file_id, *line as i64, *col as i64])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn insert_call_edge(
        &mut self,
        caller_name: &str,
        callee_name: &str,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> anyhow::Result<()> {
        let file_id = self.get_file_id(file_path)?.unwrap_or(0);
        let edges = vec![(
            caller_name.to_string(),
            callee_name.to_string(),
            file_path.to_string(),
            line,
            column,
        )];
        let map = HashMap::from([(file_path.to_string(), file_id)]);
        self.batch_insert_call_edges(&edges, &map)?;
        Ok(())
    }

    // ---- Macros ----

    pub fn insert_macro(&mut self, m: &Macro) -> anyhow::Result<i64> {
        let file_id = self.upsert_file(&m.file_path, "", &lang_str_for(&m.file_path))?;
        let map = HashMap::from([(m.file_path.clone(), file_id)]);
        self.batch_insert_macros(std::slice::from_ref(m), &map)?;
        let id: i64 = self.conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))?;
        Ok(id)
    }

    pub fn batch_insert_macros(
        &mut self,
        macros: &[Macro],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if macros.is_empty() {
            return Ok(0);
        }
        let Self { conn, str_cache } = self;
        let tx = conn.transaction()?;
        let mut count = 0;
        {
            let mut ins = tx.prepare_cached(INS_MACRO)?;
            for m in macros {
                let file_id = *file_map.get(&m.file_path).unwrap_or(&0);
                let name_id = intern(&tx, str_cache, &m.name)?;
                let params_id = if m.params.is_empty() {
                    None
                } else {
                    Some(intern(&tx, str_cache, &serde_json::to_string(&m.params).unwrap_or_default())?)
                };
                let body_id = if m.body.is_empty() {
                    None
                } else {
                    Some(intern(&tx, str_cache, &m.body)?)
                };
                let flags = if m.is_function_like { schema::F_MACRO_FN_LIKE } else { 0 };
                ins.execute(params![name_id, flags, params_id, body_id, file_id, m.line as i64])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    // ---- Includes ----

    pub fn batch_insert_includes(
        &mut self,
        includes: &[Include],
        file_map: &HashMap<String, i64>,
    ) -> anyhow::Result<usize> {
        if includes.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut count = 0;
        {
            let mut ins = tx.prepare_cached(INS_INCLUDE)?;
            for inc in includes {
                let from_id = match file_map.get(&inc.from_file) {
                    Some(&id) => id,
                    None => continue,
                };
                let to_id = match file_map.get(&inc.to_file) {
                    Some(&id) => id,
                    None => continue,
                };
                let flags = if inc.is_system { schema::F_INCLUDE_SYSTEM } else { 0 };
                ins.execute(params![from_id, to_id, inc.line as i64, flags])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn insert_include(&mut self, inc: &Include) -> anyhow::Result<()> {
        let map = self.load_file_map()?;
        self.batch_insert_includes(std::slice::from_ref(inc), &map)?;
        Ok(())
    }

    // ---- Struct members / inheritance ----

    pub fn insert_struct_member(
        &mut self,
        struct_symbol_id: i64,
        member_symbol_id: i64,
        member_index: u32,
        _access_modifier: Option<&str>,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO struct_members (struct_id, member_id, idx) VALUES (?,?,?)",
            params![struct_symbol_id, member_symbol_id, member_index as i64],
        )?;
        Ok(())
    }

    pub fn insert_inheritance(
        &mut self,
        derived_id: i64,
        base_id: i64,
        _access_modifier: Option<&str>,
        is_virtual: bool,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO inheritance (derived_id, base_id, flags) VALUES (?,?,?)",
            params![derived_id, base_id, if is_virtual { 1 } else { 0 }],
        )?;
        Ok(())
    }

    // ---- Deletion (incremental) ----

    pub fn delete_file_data(&mut self, file_id: i64) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id=?)",
            params![file_id],
        )?;
        tx.execute("DELETE FROM refs WHERE file_id=?", params![file_id])?;
        tx.execute("DELETE FROM call_edges WHERE file_id=?", params![file_id])?;
        tx.execute("DELETE FROM macros WHERE file_id=?", params![file_id])?;
        tx.execute(
            "DELETE FROM includes WHERE from_file_id=? OR to_file_id=?",
            params![file_id, file_id],
        )?;
        tx.execute(
            "DELETE FROM struct_members WHERE struct_id IN (SELECT id FROM symbols WHERE file_id=?)
             OR member_id IN (SELECT id FROM symbols WHERE file_id=?)",
            params![file_id, file_id],
        )?;
        tx.execute(
            "DELETE FROM inheritance WHERE derived_id IN (SELECT id FROM symbols WHERE file_id=?)
             OR base_id IN (SELECT id FROM symbols WHERE file_id=?)",
            params![file_id, file_id],
        )?;
        tx.execute("DELETE FROM symbols WHERE file_id=?", params![file_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_file(&mut self, file_id: i64) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE id = ?", params![file_id])?;
        Ok(())
    }

    // ---- Misc ----

    pub fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn begin_transaction(&mut self) -> anyhow::Result<rusqlite::Transaction> {
        Ok(self.conn.transaction()?)
    }

    pub fn commit_transaction(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<serde_json::Value> {
        let c = |sql: &str| -> i64 {
            self.conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
        };
        Ok(serde_json::json!({
            "files": c("SELECT COUNT(*) FROM files"),
            "symbols": c("SELECT COUNT(*) FROM symbols"),
            "references": c("SELECT COUNT(*) FROM refs"),
            "macros": c("SELECT COUNT(*) FROM macros"),
            "includes": c("SELECT COUNT(*) FROM includes"),
            "call_edges": c("SELECT COUNT(*) FROM call_edges"),
            "strings": c("SELECT COUNT(*) FROM strings"),
            "db_bytes": c("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()"),
        }))
    }
}

fn lang_str_for(path: &str) -> String {
    if lang_from_path(path) == schema::LANG_CPP {
        "cpp".to_string()
    } else {
        "c".to_string()
    }
}
