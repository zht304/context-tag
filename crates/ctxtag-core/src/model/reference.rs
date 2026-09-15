use serde::{Deserialize, Serialize};

/// How a symbol is referenced at a specific location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Definition,
    Declaration,
    Call,
    Access,       // . or -> member access
    TypeUse,
    MacroUse,
    AddressOf,
    Dereference,
    ScopeResolution, // Class::member
    Assignment,
    Return,
}

impl ReferenceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReferenceKind::Definition => "definition",
            ReferenceKind::Declaration => "declaration",
            ReferenceKind::Call => "call",
            ReferenceKind::Access => "access",
            ReferenceKind::TypeUse => "type_use",
            ReferenceKind::MacroUse => "macro_use",
            ReferenceKind::AddressOf => "address_of",
            ReferenceKind::Dereference => "dereference",
            ReferenceKind::ScopeResolution => "scope_resolution",
            ReferenceKind::Assignment => "assignment",
            ReferenceKind::Return => "return",
        }
    }
}

/// A single reference (use) of a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub id: Option<i64>,
    pub symbol_id: i64,
    /// The name of the referenced symbol, stored for lookup when the symbol
    /// is not defined in the indexed codebase (e.g. stdlib functions).
    pub symbol_name: String,

    pub file_path: String,
    pub line: u32,
    pub column: u32,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,

    pub ref_kind: ReferenceKind,

    /// Context: "local_in_fn", "struct_member", "global", "parameter"
    pub context: Option<String>,

    /// Additional detail: parent scope name, struct type name, etc.
    pub context_detail: Option<String>,

    /// Surrounding source line for display.
    pub context_line: Option<String>,

    pub is_definition: bool,
}
