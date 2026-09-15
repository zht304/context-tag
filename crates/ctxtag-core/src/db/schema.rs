pub const TABLES: &[&str] = &[
    r#"
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    hash TEXT NOT NULL,
    language TEXT NOT NULL CHECK (language IN ('c', 'cpp')),
    last_indexed INTEGER NOT NULL,
    parse_ok INTEGER DEFAULT 1,
    error_message TEXT,
    include_guard_macro TEXT
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS includes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_file_id INTEGER NOT NULL,
    to_file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    is_system INTEGER DEFAULT 0,
    original_text TEXT,
    FOREIGN KEY (from_file_id) REFERENCES files(id),
    FOREIGN KEY (to_file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS symbols (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    is_definition INTEGER DEFAULT 0,
    file_id INTEGER NOT NULL,
    line_start INTEGER NOT NULL,
    line_end INTEGER NOT NULL,
    column_start INTEGER NOT NULL,
    column_end INTEGER NOT NULL,
    selection_line INTEGER,
    selection_column INTEGER,
    scope_kind TEXT NOT NULL,
    scope_depth INTEGER DEFAULT 0,
    parent_symbol_id INTEGER,
    container_name TEXT,
    declared_type TEXT,
    resolved_type TEXT,
    return_type TEXT,
    signature TEXT,
    access_modifier TEXT,
    storage_class TEXT,
    linkage TEXT,
    is_virtual INTEGER DEFAULT 0,
    is_override INTEGER DEFAULT 0,
    is_final INTEGER DEFAULT 0,
    is_const_method INTEGER DEFAULT 0,
    base_classes TEXT,
    template_params TEXT,
    declaration_text TEXT,
    definition_text TEXT,
    documentation TEXT,
    lsp_kind INTEGER DEFAULT 12,
    FOREIGN KEY (file_id) REFERENCES files(id),
    FOREIGN KEY (parent_symbol_id) REFERENCES symbols(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS `references` (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_id INTEGER NOT NULL,
    symbol_name TEXT NOT NULL,
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    column INTEGER NOT NULL,
    end_line INTEGER,
    end_column INTEGER,
    ref_kind TEXT NOT NULL,
    context TEXT,
    context_detail TEXT,
    context_line TEXT,
    is_definition INTEGER DEFAULT 0,
    FOREIGN KEY (symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS struct_members (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    struct_symbol_id INTEGER NOT NULL,
    member_symbol_id INTEGER NOT NULL,
    member_index INTEGER DEFAULT 0,
    access_modifier TEXT,
    FOREIGN KEY (struct_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (member_symbol_id) REFERENCES symbols(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS call_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    caller_symbol_id INTEGER NOT NULL,
    callee_symbol_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    column INTEGER NOT NULL,
    call_type TEXT NOT NULL,
    args_text TEXT,
    FOREIGN KEY (caller_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (callee_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS macros (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    is_function_like INTEGER DEFAULT 0,
    params TEXT,
    body TEXT,
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    conditional_context TEXT,
    FOREIGN KEY (file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS `macro_references` (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    macro_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    column INTEGER NOT NULL,
    context_line TEXT,
    FOREIGN KEY (macro_id) REFERENCES macros(id),
    FOREIGN KEY (file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS type_aliases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alias_name TEXT NOT NULL,
    underlying_type TEXT NOT NULL,
    resolved_symbol_id INTEGER,
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    FOREIGN KEY (resolved_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (file_id) REFERENCES files(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS inheritance (
    derived_symbol_id INTEGER NOT NULL,
    base_symbol_id INTEGER NOT NULL,
    access_modifier TEXT,
    is_virtual INTEGER DEFAULT 0,
    PRIMARY KEY (derived_symbol_id, base_symbol_id),
    FOREIGN KEY (derived_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (base_symbol_id) REFERENCES symbols(id)
);
"#,
    r#"
CREATE TABLE IF NOT EXISTS overrides (
    override_symbol_id INTEGER NOT NULL,
    base_symbol_id INTEGER NOT NULL,
    PRIMARY KEY (override_symbol_id, base_symbol_id),
    FOREIGN KEY (override_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY (base_symbol_id) REFERENCES symbols(id)
);
"#,
    // FTS5
    r#"
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    name, qualified_name, content=symbols, content_rowid=id
);
"#,
];

pub const INDEXES: &[&str] = &[
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_qualified ON symbols(qualified_name);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_range ON symbols(file_id, line_start, line_end);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_symbols_container ON symbols(parent_symbol_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_refs_symbol ON `references`(symbol_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_refs_symbol_name ON `references`(symbol_name);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_refs_file_pos ON `references`(file_id, line, column);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_refs_symbol_file ON `references`(symbol_id, file_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_call_edges_caller ON call_edges(caller_symbol_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_call_edges_callee ON call_edges(callee_symbol_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_struct_members_struct ON struct_members(struct_symbol_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_includes_from ON includes(from_file_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_includes_to ON includes(to_file_id);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_macros_name ON macros(name);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_type_aliases_name ON type_aliases(alias_name);"#,
    r#"CREATE INDEX IF NOT EXISTS idx_inheritance_base ON inheritance(base_symbol_id);"#,
];

/// The list of user-created index names (excluding sqlite internal indexes).
/// Used for dropping and recreating indexes during bulk import.
pub const INDEX_NAMES: &[&str] = &[
    "idx_symbols_name",
    "idx_symbols_qualified",
    "idx_symbols_kind",
    "idx_symbols_file",
    "idx_symbols_range",
    "idx_symbols_container",
    "idx_refs_symbol",
    "idx_refs_symbol_name",
    "idx_refs_file_pos",
    "idx_refs_symbol_file",
    "idx_call_edges_caller",
    "idx_call_edges_callee",
    "idx_struct_members_struct",
    "idx_includes_from",
    "idx_includes_to",
    "idx_macros_name",
    "idx_type_aliases_name",
    "idx_inheritance_base",
];

/// Create tables and indexes (default mode for LSP/incremental updates).
pub fn create_schema(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in TABLES {
        conn.execute_batch(stmt)?;
    }
    for stmt in INDEXES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

/// Create tables only, skip indexes (for bulk import mode).
pub fn create_schema_no_index(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in TABLES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

/// Create all indexes (called after bulk insert is complete).
pub fn create_indexes(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for stmt in INDEXES {
        conn.execute_batch(stmt)?;
    }
    Ok(())
}

/// Drop all user-created indexes (for bulk import mode).
pub fn drop_indexes(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for name in INDEX_NAMES {
        let _ = conn.execute_batch(&format!("DROP INDEX IF EXISTS `{}`;", name));
    }
    Ok(())
}
