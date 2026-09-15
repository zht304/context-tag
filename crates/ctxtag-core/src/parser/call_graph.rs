use tree_sitter::Node;

use super::tree::SourceTree;

/// Represents a single function call edge in the call graph.
#[derive(Debug, Clone)]
pub struct CallEdge {
    /// The name of the callee function.
    pub callee_name: String,
    /// The qualified callee name (if it contains namespace::class::member).
    pub qualified_callee: String,
    /// Source file where the call occurs.
    pub file_path: String,
    /// Line number (0-based) of the call site.
    pub line: u32,
    /// Column number (0-based) of the call site.
    pub column: u32,
    /// End line of the call expression.
    pub end_line: u32,
    /// End column of the call expression.
    pub end_column: u32,
    /// The name of the containing function (caller).
    pub caller_name: Option<String>,
    /// The full call expression text.
    pub call_text: String,
}

/// Extracts function calls from a parsed source tree.
///
/// Finds all `call_expression` nodes and records the callee name,
/// call location, and caller context.
pub struct CallGraph {
    edges: Vec<CallEdge>,
    /// Stack tracking which function we're currently inside.
    function_stack: Vec<String>,
}

impl CallGraph {
    pub fn new() -> Self {
        Self {
            edges: Vec::new(),
            function_stack: Vec::new(),
        }
    }

    /// Extract all function calls from a parsed source tree.
    pub fn extract(&mut self, tree: &SourceTree) -> Vec<CallEdge> {
        self.edges.clear();
        self.function_stack.clear();
        self.walk_node(tree.root_node(), tree);
        std::mem::take(&mut self.edges)
    }

    fn walk_node(&mut self, node: Node, tree: &SourceTree) {
        match node.kind() {
            // Enter function
            "function_definition" => {
                let declarator = node.child_by_field_name("declarator");
                let name = declarator
                    .and_then(|d| d.child_by_field_name("declarator"))
                    .or(declarator)
                    .map(|d| extract_declarator_name(&d, tree))
                    .unwrap_or_default();

                if !name.is_empty() {
                    self.function_stack.push(name.clone());
                } else {
                    // Lambda or unnamed function
                    self.function_stack.push("<anonymous>".to_string());
                }

                // Walk children
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.walk_node(child, tree);
                }

                self.function_stack.pop();
            }

            // C++ method inside class
            "function_declarator" => {
                let parent = node.parent();
                if let Some(p) = &parent {
                    if p.kind() == "field_declaration" {
                        // This is a method declaration inside a class
                        let name = extract_declarator_name(&node, tree);
                        if !name.is_empty() {
                            // Check for method body
                            if let Some(body_field) = p.child_by_field_name("body") {
                                self.function_stack.push(format!("{}::{}", self.current_container(tree), name));
                                let mut cursor = body_field.walk();
                                for child in body_field.named_children(&mut cursor) {
                                    self.walk_node(child, tree);
                                }
                                self.function_stack.pop();
                                return;
                            }
                        }
                    }
                }

                // Not in a method body context, skip
            }

            // Lambda expression
            "lambda_expression" => {
                let body = node.child_by_field_name("body");
                if let Some(body_node) = body {
                    self.function_stack.push("<lambda>".to_string());
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree);
                    }
                    self.function_stack.pop();
                }
                return; // Don't recurse further after handling lambda
            }

            // Call expression
            "call_expression" => {
                self.extract_call(node, tree);
            }

            _ => {}
        }

        // Default recursion for unmatched nodes
        if !matches!(node.kind(), "function_definition" | "lambda_expression") {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                self.walk_node(child, tree);
            }
        }
    }

    fn extract_call(&mut self, node: Node, tree: &SourceTree) {
        let function_node = node.child_by_field_name("function");
        let args_node = node.child_by_field_name("arguments");

        let (callee_name, qualified_callee) = match function_node {
            Some(fn_node) => match fn_node.kind() {
                "identifier" => {
                    let name = tree.node_text(&fn_node).to_string();
                    (name.clone(), name)
                }
                "qualified_identifier" => {
                    let name = tree.node_text(&fn_node).to_string();
                    // Extract the last component as the simple name
                    let simple_name = name
                        .rsplit("::")
                        .next()
                        .map(|s| s.to_string())
                        .unwrap_or(name.clone());
                    (simple_name, name)
                }
                "field_expression" => {
                    // Method call: obj.method() or obj->method()
                    let field = fn_node.child_by_field_name("field");
                    if let Some(field_node) = field {
                        let name = tree.node_text(&field_node).to_string();
                        let operand_text = fn_node
                            .child_by_field_name("operand")
                            .map(|op| tree.node_text(&op).to_string())
                            .unwrap_or_default();
                        (name.clone(), format!("{}.{})", operand_text, name))
                    } else {
                        (String::new(), String::new())
                    }
                }
                _ => {
                    // Other callable types (function pointers, lambdas, etc.)
                    let text = tree.node_text(&fn_node);
                    let name = Self::extract_callee_name_from_expr(&fn_node, tree);
                    (name.clone(), text.to_string())
                }
            },
            None => return,
        };

        if callee_name.is_empty() {
            return;
        }

        let pos = function_node.map(|n| n.start_position()).unwrap_or(node.start_position());
        let end_pos = node.end_position();

        let args_text = args_node
            .map(|a| tree.node_text(&a).to_string())
            .unwrap_or_default();

        let call_text = if function_node.is_some() {
            let fn_text = tree.node_text(&function_node.unwrap());
            format!("{}{}", fn_text, args_text)
        } else {
            tree.node_text(&node).to_string()
        };

        let edge = CallEdge {
            callee_name,
            qualified_callee,
            file_path: tree.file_path().to_string(),
            line: pos.row as u32,
            column: pos.column as u32,
            end_line: end_pos.row as u32,
            end_column: end_pos.column as u32,
            caller_name: self.function_stack.last().cloned(),
            call_text,
        };

        self.edges.push(edge);
    }

    /// Try to extract a meaningful callee name from complex expressions.
    fn extract_callee_name_from_expr(node: &Node, tree: &SourceTree) -> String {
        match node.kind() {
            "identifier" => tree.node_text(node).to_string(),
            "field_expression" => {
                let field = node.child_by_field_name("field");
                field.map(|f| tree.node_text(&f).to_string()).unwrap_or_default()
            }
            "subscript_expression" => {
                // operator[] call
                "operator[]".to_string()
            }
            "cast_expression" => {
                // Could be a function pointer cast
                String::new()
            }
            _ => {
                // Try to find an identifier in the expression
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.kind() == "identifier" {
                        return tree.node_text(&child).to_string();
                    }
                }
                String::new()
            }
        }
    }

    fn current_container(&self, _tree: &SourceTree) -> String {
        self.function_stack
            .iter()
            .rev()
            .find(|n| !n.starts_with('<'))
            .map(|s| s.split("::").next().unwrap_or(s).to_string())
            .unwrap_or_default()
    }

    /// Get all unique callee names from the extracted calls.
    pub fn unique_callees(&self) -> Vec<String> {
        let mut names: Vec<String> = self.edges.iter().map(|e| e.callee_name.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    /// Get all unique caller names.
    pub fn unique_callers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.edges
            .iter()
            .filter_map(|e| e.caller_name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Get calls for a specific callee.
    pub fn calls_to(&self, callee: &str) -> &[CallEdge] {
        self.edges
            .iter()
            .position(|e| e.callee_name == callee || e.qualified_callee == callee)
            .map(|pos| &self.edges[pos..])
            .unwrap_or(&[])
    }

    /// Get calls made by a specific caller.
    pub fn calls_from(&self, caller: &str) -> Vec<&CallEdge> {
        self.edges
            .iter()
            .filter(|e| e.caller_name.as_deref() == Some(caller))
            .collect()
    }
}

impl Default for CallGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────

fn extract_declarator_name(node: &Node, tree: &SourceTree) -> String {
    match node.kind() {
        "identifier" => tree.node_text(node).to_string(),
        "function_declarator" | "ptr_declarator" | "array_declarator"
        | "parameter_list" | "pointer_declarator" => {
            node.child_by_field_name("declarator")
                .map(|d| extract_declarator_name(&d, tree))
                .unwrap_or_default()
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "identifier" {
                    return tree.node_text(&child).to_string();
                }
            }
            String::new()
        }
    }
}
