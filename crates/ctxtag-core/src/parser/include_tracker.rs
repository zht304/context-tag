use crate::model::Include;
use tree_sitter::Node;

use super::tree::SourceTree;

/// Tracks `#include` directives in C/C++ source files.
///
/// Finds all preprocessor include directives and extracts:
/// - The included file path
/// - Line number (0-based)
/// - Whether it's a system include (`<>`) or local include (`""`)
pub struct IncludeTracker {
    includes: Vec<Include>,
}

impl IncludeTracker {
    pub fn new() -> Self {
        Self {
            includes: Vec::new(),
        }
    }

    /// Extract all #include directives from a parsed source tree.
    pub fn track(&mut self, tree: &SourceTree) -> Vec<Include> {
        self.includes.clear();
        self.walk_node(tree.root_node(), tree);
        std::mem::take(&mut self.includes)
    }

    fn walk_node(&mut self, node: Node, tree: &SourceTree) {
        if node.kind() == "preproc_include" {
            self.extract_include(node, tree);
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.walk_node(child, tree);
        }
    }

    fn extract_include(&mut self, node: Node, tree: &SourceTree) {
        // preproc_include has a child that is either:
        // - string_literal (for #include "file.h")
        // - system_lib_string (for #include <stdio.h>)

        let path_node = node.child(0);
        let content = match path_node {
            Some(n) => {
                let text = tree.node_text(&n);
                Self::extract_path_text(text, n.kind())
            }
            None => return,
        };

        let pos = node.start_position();
        let is_system = path_node.map(|n| n.kind() == "system_lib_string").unwrap_or(false);

        let include = Include {
            from_file: tree.file_path().to_string(),
            to_file: content.clone(),
            line: pos.row as u32,
            is_system,
            original_text: format!("#include {}", content),
        };

        self.includes.push(include);
    }

    /// Extract the raw path text from the include directive.
    ///
    /// For string literals, strips the surrounding quotes.
    /// For system_lib_string, strips the surrounding angle brackets.
    fn extract_path_text(text: &str, kind: &str) -> String {
        match kind {
            "string_literal" => {
                // Strip surrounding quotes
                text.strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| text.to_string())
            }
            "system_lib_string" => {
                // Strip surrounding angle brackets
                text.strip_prefix('<')
                    .and_then(|s| s.strip_suffix('>'))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| text.to_string())
            }
            _ => text.to_string(),
        }
    }

    /// Check if an include path is a system include.
    pub fn is_system_include(path: &str) -> bool {
        // System includes typically don't have path separators
        // or start with known system prefixes
        !path.starts_with('.') && !path.contains('/') && !path.contains('\\')
    }

    /// Get include as a display string.
    pub fn format_include(include: &Include) -> String {
        if include.is_system {
            format!("#include <{}> (line {})", include.to_file, include.line)
        } else {
            format!("#include \"{}\" (line {})", include.to_file, include.line)
        }
    }
}

impl Default for IncludeTracker {
    fn default() -> Self {
        Self::new()
    }
}
