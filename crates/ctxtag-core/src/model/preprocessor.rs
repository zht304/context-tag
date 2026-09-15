use serde::{Deserialize, Serialize};

/// A preprocessor macro (`#define`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Macro {
    pub id: Option<i64>,
    pub name: String,
    pub is_function_like: bool,
    pub params: Vec<String>,
    pub body: String,
    pub file_path: String,
    pub line: u32,
    /// JSON: which #ifdef guards this define.
    pub conditional_context: Option<serde_json::Value>,
}

/// A reference to a macro.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroReference {
    pub macro_id: i64,
    pub file_path: String,
    pub line: u32,
    pub column: u32,
    pub context_line: Option<String>,
}

/// An `#include` directive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Include {
    pub from_file: String,
    pub to_file: String,
    pub line: u32,
    pub is_system: bool,
    pub original_text: String,
}
