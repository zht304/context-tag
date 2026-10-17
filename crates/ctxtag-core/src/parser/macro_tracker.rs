use crate::model::Macro;
use serde_json::Value;
use tree_sitter::Node;

use super::tree::SourceTree;

/// Tracks preprocessor macro definitions in C/C++ source files.
///
/// Finds `#define` and `#undef` directives and extracts:
/// - Macro name
/// - Body (replacement text)
/// - Parameters (for function-like macros)
/// - Location
/// - Conditional context (which #ifdef guards this definition)
pub struct MacroTracker {
    macros: Vec<Macro>,
    /// Stack of active conditional directives for tracking context.
    conditional_stack: Vec<String>,
}

impl MacroTracker {
    pub fn new() -> Self {
        Self {
            macros: Vec::new(),
            conditional_stack: Vec::new(),
        }
    }

    /// Extract all macro definitions from a parsed source tree.
    pub fn track(&mut self, tree: &SourceTree) -> Vec<Macro> {
        self.macros.clear();
        self.conditional_stack.clear();
        self.walk_node(tree.root_node(), tree);
        std::mem::take(&mut self.macros)
    }

    fn walk_node(&mut self, node: Node, tree: &SourceTree) {
        match node.kind() {
            // #define directive
            "preproc_def" => {
                self.extract_define(node, tree);
            }

            // Function-like macro definition: #define FOO(x) ...
            "preproc_function_def" => {
                self.extract_define(node, tree);
            }

            // Conditional directives — track context
            "preproc_ifdef" => {
                let macro_name = Self::extract_conditional_macro_name(&node, tree);
                self.conditional_stack.push(format!("#ifdef {}", macro_name));
            }
            "preproc_ifndef" => {
                let macro_name = Self::extract_conditional_macro_name(&node, tree);
                self.conditional_stack.push(format!("#ifndef {}", macro_name));
            }
            "preproc_if" => {
                self.conditional_stack.push("#if".to_string());
            }
            "preproc_elif" => {
                // Replace the top of the stack
                if !self.conditional_stack.is_empty() {
                    *self.conditional_stack.last_mut().unwrap() = "#elif".to_string();
                }
            }
            "preproc_else" => {
                if !self.conditional_stack.is_empty() {
                    *self.conditional_stack.last_mut().unwrap() = "#else".to_string();
                }
            }
            "preproc_endif" => {
                self.conditional_stack.pop();
            }

            _ => {}
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.walk_node(child, tree);
        }
    }

    fn extract_define(&mut self, node: Node, tree: &SourceTree) {
        // preproc_def structure:
        //   name: identifier (the macro name)
        //   value: the replacement text (may be a slot for function-like macros)

        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => tree.node_text(&n).to_string(),
            None => return,
        };

        let pos = node.start_position();

        // Determine if this is a function-like macro
        let is_function_like = Self::is_function_like_macro(&node, tree);

        // Extract parameters for function-like macros
        let params = if is_function_like {
            Self::extract_macro_params(&node, tree)
        } else {
            Vec::new()
        };

        // Extract body
        let body = Self::extract_macro_body(&node, tree);

        // Build conditional context
        let conditional_context = if self.conditional_stack.is_empty() {
            None
        } else {
            Some(Value::Array(self.conditional_stack.iter().map(|s| Value::String(s.clone())).collect()))
        };

        let macro_def = Macro {
            id: None,
            name,
            is_function_like,
            params,
            body,
            file_path: tree.file_path().to_string(),
            line: pos.row as u32,
            conditional_context,
        };

        self.macros.push(macro_def);
    }

    /// Check if a macro is function-like (has parameters).
    ///
    /// In tree-sitter, function-like macros have a `preproc_params` child
    /// or a `parameters` field.
    fn is_function_like_macro(node: &Node, tree: &SourceTree) -> bool {
        // Check for preproc_params child
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "preproc_params" {
                return true;
            }
        }

        // Check by looking at the raw text for opening paren after name
        let text = tree.node_text(node);
        if let Some(name_node) = node.child_by_field_name("name") {
            let name_text = tree.node_text(&name_node);
            let name_end = name_node.end_byte();
            if name_end < text.len() {
                let after_name = &text[name_end..];
                let trimmed = after_name.trim_start();
                return trimmed.starts_with('(');
            }
        }

        false
    }

    /// Extract parameter names from a function-like macro.
    fn extract_macro_params(node: &Node, tree: &SourceTree) -> Vec<String> {
        let mut cursor = node.walk();
        let mut params = Vec::new();

        for child in node.named_children(&mut cursor) {
            if child.kind() == "preproc_params" {
                let mut param_cursor = child.walk();
                for param in child.named_children(&mut param_cursor) {
                    if param.kind() == "identifier" {
                        params.push(tree.node_text(&param).to_string());
                    }
                }
                break;
            }
        }

        // Also handle variadic macros (...)
        if params.len() >= 2 && *params.last().unwrap() == "..." {
            // The ... is already included as a param name, which is fine
        }

        params
    }

    /// Extract the macro body (replacement text).
    fn extract_macro_body(node: &Node, tree: &SourceTree) -> String {
        let value_node = node.child_by_field_name("value");
        match value_node {
            Some(v) => tree.node_text(&v).to_string(),
            None => String::new(),
        }
    }

    /// Extract the macro name from a conditional directive.
    fn extract_conditional_macro_name(node: &Node, tree: &SourceTree) -> String {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                return tree.node_text(&child).to_string();
            }
        }
        // Fallback: get from the raw text
        let text = tree.node_text(node);
        text.split_whitespace()
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Get the current conditional context as a string.
    pub fn conditional_context_string(context: &Option<Value>) -> String {
        match context {
            Some(Value::Array(arr)) => arr.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" > "),
            Some(other) => other.to_string(),
            None => "global".to_string(),
        }
    }

    /// Find a macro by name.
    pub fn find_macro<'a>(macros: &'a [Macro], name: &str) -> Option<&'a Macro> {
        macros.iter().rev().find(|m| m.name == name)
    }
}

impl Default for MacroTracker {
    fn default() -> Self {
        Self::new()
    }
}
