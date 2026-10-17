//! Compact, Source-Insight-style schema.
//!
//! Design goals (vs. the previous bloated schema):
//!   * Small DB: no per-row TEXT duplication. All identifiers/strings are
//!     interned once into `strings` and referenced by integer id. Display text
//!     (signature, documentation, reference context line) is NOT stored — it is
//!     reconstructed on demand from the source file at query time.
//!   * Fast indexing: fixed-width integer rows, deferred index build, and
//!     prepared-statement batch inserts.
//!
//! Enum values (kind / ref_kind / scope_kind) are stored as small integers.

use crate::model::{ReferenceKind, ScopeKind, SymbolKind};

pub const TABLES: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS strings (
        id INTEGER PRIMARY KEY,
        text TEXT NOT NULL UNIQUE
    );"#,
    r#"CREATE TABLE IF NOT EXISTS files (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        hash TEXT NOT NULL,
        lang INTEGER NOT NULL,
        mtime INTEGER NOT NULL DEFAULT 0
    );"#,
    r#"CREATE TABLE IF NOT EXISTS symbols (
        id INTEGER PRIMARY KEY,
        name_id INTEGER NOT NULL,
        qual_id INTEGER NOT NULL,
        kind INTEGER NOT NULL,
        flags INTEGER NOT NULL DEFAULT 0,
        file_id INTEGER NOT NULL,
        line_start INTEGER NOT NULL,
        line_end INTEGER NOT NULL,
        col_start INTEGER NOT NULL,
        col_end INTEGER NOT NULL,
        sel_line INTEGER,
        sel_col INTEGER,
        scope_kind INTEGER NOT NULL DEFAULT 0,
        scope_depth INTEGER NOT NULL DEFAULT 0,
        parent_id INTEGER,
        container_id INTEGER
    );"#,
    // WITHOUT ROWID: the table is a single B-tree clustered by the primary key
    // (name_id first), so "all references to symbol X" is one contiguous range
    // scan and no separate name index is needed. end_line/end_col are dropped —
    // they are derivable from col + the interned name length.
    r#"CREATE TABLE IF NOT EXISTS refs (
        name_id INTEGER NOT NULL,
        file_id INTEGER NOT NULL,
        line INTEGER NOT NULL,
        col INTEGER NOT NULL,
        kind INTEGER NOT NULL DEFAULT 0,
        flags INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (name_id, file_id, line, col)
    ) WITHOUT ROWID;"#,
    r#"CREATE TABLE IF NOT EXISTS call_edges (
        id INTEGER PRIMARY KEY,
        caller_id INTEGER NOT NULL,
        callee_id INTEGER NOT NULL,
        file_id INTEGER NOT NULL,
        line INTEGER NOT NULL,
        col INTEGER NOT NULL
    );"#,
    r#"CREATE TABLE IF NOT EXISTS macros (
        id INTEGER PRIMARY KEY,
        name_id INTEGER NOT NULL,
        flags INTEGER NOT NULL DEFAULT 0,
        params_id INTEGER,
        body_id INTEGER,
        file_id INTEGER NOT NULL,
        line INTEGER NOT NULL
    );"#,
    r#"CREATE TABLE IF NOT EXISTS includes (
        id INTEGER PRIMARY KEY,
        from_file_id INTEGER NOT NULL,
        to_file_id INTEGER NOT NULL,
        line INTEGER NOT NULL,
        flags INTEGER NOT NULL DEFAULT 0
    );"#,
    r#"CREATE TABLE IF NOT EXISTS struct_members (
        struct_id INTEGER NOT NULL,
        member_id INTEGER NOT NULL,
        idx INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (struct_id, member_id)
    );"#,
    r#"CREATE TABLE IF NOT EXISTS inheritance (
        derived_id INTEGER NOT NULL,
        base_id INTEGER NOT NULL,
        flags INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (derived_id, base_id)
    );"#,
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
        name
    );"#,
];

pub const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name_id);",
    "CREATE INDEX IF NOT EXISTS idx_symbols_qual ON symbols(qual_id);",
    "CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);",
    "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id, line_start);",
    "CREATE INDEX IF NOT EXISTS idx_symbols_container ON symbols(container_id);",
    "CREATE INDEX IF NOT EXISTS idx_refs_file ON refs(file_id);",
    "CREATE INDEX IF NOT EXISTS idx_call_caller ON call_edges(caller_id);",
    "CREATE INDEX IF NOT EXISTS idx_call_callee ON call_edges(callee_id);",
    "CREATE INDEX IF NOT EXISTS idx_macros_name ON macros(name_id);",
    "CREATE INDEX IF NOT EXISTS idx_includes_from ON includes(from_file_id);",
    "CREATE INDEX IF NOT EXISTS idx_includes_to ON includes(to_file_id);",
];

pub const INDEX_NAMES: &[&str] = &[
    "idx_symbols_name",
    "idx_symbols_qual",
    "idx_symbols_kind",
    "idx_symbols_file",
    "idx_symbols_container",
    "idx_refs_file",
    "idx_call_caller",
    "idx_call_callee",
    "idx_macros_name",
    "idx_includes_from",
    "idx_includes_to",
];

// ─── symbol.flags bit layout ───────────────────────────────────────────
pub const F_SYMBOL_IS_DEF: i64 = 1 << 0;
pub const F_SYMBOL_VIRTUAL: i64 = 1 << 1;
pub const F_SYMBOL_OVERRIDE: i64 = 1 << 2;
pub const F_SYMBOL_FINAL: i64 = 1 << 3;
pub const F_SYMBOL_CONST: i64 = 1 << 4;

// ─── refs.flags / includes.flags / macros.flags ────────────────────────
pub const F_REF_IS_DEF: i64 = 1 << 0;
pub const F_REF_MEMBER: i64 = 1 << 1;
pub const F_INCLUDE_SYSTEM: i64 = 1 << 0;
pub const F_MACRO_FN_LIKE: i64 = 1 << 0;

// ─── language ids ──────────────────────────────────────────────────────
pub const LANG_C: i64 = 0;
pub const LANG_CPP: i64 = 1;

pub fn kind_to_int(k: &str) -> i64 {
    match k {
        "function" => 1,
        "method" => 2,
        "constructor" => 3,
        "destructor" => 4,
        "operator" => 5,
        "lambda" => 6,
        "parameter" => 7,
        "variable" => 8,
        "local_variable" => 9,
        "member" => 10,
        "static_member" => 11,
        "struct" => 12,
        "class" => 13,
        "union" => 14,
        "enum" => 15,
        "enum_value" => 16,
        "typedef" => 17,
        "using_alias" => 18,
        "namespace" => 19,
        "macro" => 20,
        "label" => 21,
        "template" => 22,
        _ => 8,
    }
}

pub fn kind_from_int(v: i64) -> SymbolKind {
    match v {
        1 => SymbolKind::Function,
        2 => SymbolKind::Method,
        3 => SymbolKind::Constructor,
        4 => SymbolKind::Destructor,
        5 => SymbolKind::Operator,
        6 => SymbolKind::Lambda,
        7 => SymbolKind::Parameter,
        9 => SymbolKind::LocalVariable,
        10 => SymbolKind::Member,
        11 => SymbolKind::StaticMember,
        12 => SymbolKind::Struct,
        13 => SymbolKind::Class,
        14 => SymbolKind::Union,
        15 => SymbolKind::Enum,
        16 => SymbolKind::EnumValue,
        17 => SymbolKind::Typedef,
        18 => SymbolKind::UsingAlias,
        19 => SymbolKind::Namespace,
        20 => SymbolKind::Macro,
        21 => SymbolKind::Label,
        22 => SymbolKind::Template,
        _ => SymbolKind::Variable,
    }
}

/// The set of kind ids that represent an aggregate type (struct/class/union).
pub const AGGREGATE_KINDS: [i64; 3] = [12, 13, 14];
/// The set of kind ids that represent a callable.
pub const CALLABLE_KINDS: [i64; 3] = [1, 2, 3];

pub fn refkind_to_int(k: &str) -> i64 {
    match k {
        "definition" => 1,
        "declaration" => 2,
        "call" => 3,
        "access" => 4,
        "type_use" => 5,
        "macro_use" => 6,
        "address_of" => 7,
        "dereference" => 8,
        "scope_resolution" => 9,
        "assignment" => 10,
        "return" => 11,
        _ => 4,
    }
}

pub fn refkind_from_int(v: i64) -> ReferenceKind {
    match v {
        1 => ReferenceKind::Definition,
        2 => ReferenceKind::Declaration,
        3 => ReferenceKind::Call,
        5 => ReferenceKind::TypeUse,
        6 => ReferenceKind::MacroUse,
        7 => ReferenceKind::AddressOf,
        8 => ReferenceKind::Dereference,
        9 => ReferenceKind::ScopeResolution,
        10 => ReferenceKind::Assignment,
        11 => ReferenceKind::Return,
        _ => ReferenceKind::Access,
    }
}

pub fn scope_to_int(k: &str) -> i64 {
    match k {
        "global" => 0,
        "namespace" => 1,
        "class" => 2,
        "struct" => 3,
        "union" => 4,
        "enum" => 5,
        "function" => 6,
        "block" => 7,
        _ => 0,
    }
}

pub fn scope_from_int(v: i64) -> ScopeKind {
    match v {
        1 => ScopeKind::Namespace,
        2 => ScopeKind::Class,
        3 => ScopeKind::Struct,
        4 => ScopeKind::Union,
        5 => ScopeKind::Enum,
        6 => ScopeKind::Function,
        7 => ScopeKind::Block,
        _ => ScopeKind::Global,
    }
}

pub fn create_schema(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in TABLES {
        conn.execute_batch(stmt)?;
    }
    for stmt in INDEXES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

pub fn create_schema_no_index(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in TABLES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

pub fn create_indexes(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in INDEXES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

pub fn drop_indexes(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for name in INDEX_NAMES {
        let _ = conn.execute_batch(&format!("DROP INDEX IF EXISTS `{}`;", name));
    }
    Ok(())
}
