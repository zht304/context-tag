use serde::{Deserialize, Serialize};

/// Kind of symbol discovered in C/C++ source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Constructor,
    Destructor,
    Operator,
    Lambda,
    Parameter,
    Variable,
    LocalVariable,
    Member,
    StaticMember,
    Struct,
    Class,
    Union,
    Enum,
    EnumValue,
    Typedef,
    UsingAlias,
    Namespace,
    Macro,
    Label,
    Template,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Constructor => "constructor",
            SymbolKind::Destructor => "destructor",
            SymbolKind::Operator => "operator",
            SymbolKind::Lambda => "lambda",
            SymbolKind::Parameter => "parameter",
            SymbolKind::Variable => "variable",
            SymbolKind::LocalVariable => "local_variable",
            SymbolKind::Member => "member",
            SymbolKind::StaticMember => "static_member",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Union => "union",
            SymbolKind::Enum => "enum",
            SymbolKind::EnumValue => "enum_value",
            SymbolKind::Typedef => "typedef",
            SymbolKind::UsingAlias => "using_alias",
            SymbolKind::Namespace => "namespace",
            SymbolKind::Macro => "macro",
            SymbolKind::Label => "label",
            SymbolKind::Template => "template",
        }
    }

    /// LSP SymbolKind value for workspace/symbol.
    pub fn lsp_kind(&self) -> u32 {
        match self {
            SymbolKind::Function | SymbolKind::Lambda => 12,
            SymbolKind::Method
            | SymbolKind::Constructor
            | SymbolKind::Destructor
            | SymbolKind::Operator => 6,
            SymbolKind::Variable => 13,
            SymbolKind::LocalVariable => 13,
            SymbolKind::Member | SymbolKind::StaticMember => 10,
            SymbolKind::Parameter => 4,
            SymbolKind::Struct => 23,
            SymbolKind::Class => 5,
            SymbolKind::Union => 24,
            SymbolKind::Enum => 7,
            SymbolKind::EnumValue => 8,
            SymbolKind::Typedef | SymbolKind::UsingAlias => 21,
            SymbolKind::Namespace => 4,
            SymbolKind::Macro => 16,
            SymbolKind::Label => 29,
            SymbolKind::Template => 19,
        }
    }
}

/// Kind of scope a symbol belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Global,
    Namespace,
    Class,
    Struct,
    Union,
    Enum,
    Function,
    Block,
}

impl ScopeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeKind::Global => "global",
            ScopeKind::Namespace => "namespace",
            ScopeKind::Class => "class",
            ScopeKind::Struct => "struct",
            ScopeKind::Union => "union",
            ScopeKind::Enum => "enum",
            ScopeKind::Function => "function",
            ScopeKind::Block => "block",
        }
    }
}

/// A single symbol (declaration or definition) extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: Option<i64>,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub is_definition: bool,

    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,

    /// Where the cursor should land on "go to definition".
    pub selection_line: Option<u32>,
    pub selection_column: Option<u32>,

    pub scope_kind: ScopeKind,
    pub scope_depth: u32,
    pub parent_symbol_id: Option<i64>,
    pub container_name: Option<String>,

    pub declared_type: Option<String>,
    pub resolved_type: Option<String>,
    pub return_type: Option<String>,
    pub signature: Option<String>,

    pub access_modifier: Option<String>,
    pub storage_class: Option<String>,
    pub linkage: Option<String>,

    // C++ specific
    pub is_virtual: bool,
    pub is_override: bool,
    pub is_final: bool,
    pub is_const_method: bool,
    pub base_classes: Option<serde_json::Value>,
    pub template_params: Option<serde_json::Value>,

    pub declaration_text: Option<String>,
    pub definition_text: Option<String>,
    pub documentation: Option<String>,
}

impl Symbol {
    pub fn new(name: impl Into<String>, kind: SymbolKind) -> Self {
        let name = name.into();
        Self {
            id: None,
            qualified_name: name.clone(),
            kind,
            is_definition: false,
            file_path: String::new(),
            line_start: 0,
            line_end: 0,
            column_start: 0,
            column_end: 0,
            selection_line: None,
            selection_column: None,
            scope_kind: ScopeKind::Global,
            scope_depth: 0,
            parent_symbol_id: None,
            container_name: None,
            declared_type: None,
            resolved_type: None,
            return_type: None,
            signature: None,
            access_modifier: None,
            storage_class: None,
            linkage: None,
            is_virtual: false,
            is_override: false,
            is_final: false,
            is_const_method: false,
            base_classes: None,
            template_params: None,
            declaration_text: None,
            definition_text: None,
            documentation: None,
            name,
        }
    }
}
