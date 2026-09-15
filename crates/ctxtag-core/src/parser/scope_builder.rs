use crate::model::{Scope, ScopeKind, Symbol, SymbolKind};
use tree_sitter::Node;

use super::tree::SourceTree;

/// Builds a nested scope tree from a tree-sitter CST.
///
/// Walks the syntax tree and creates Scope nodes for namespaces,
/// classes, structs, enums, functions, and compound statements (blocks).
pub struct ScopeBuilder {
    scopes: Vec<Scope>,
    current_depth: u32,
}

impl ScopeBuilder {
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            current_depth: 0,
        }
    }

    /// Build a scope tree from a parsed source tree.
    ///
    /// Returns the root (global) scope containing all nested scopes and symbols.
    pub fn build(&mut self, tree: &SourceTree) -> Scope {
        self.scopes.clear();
        self.current_depth = 0;

        // Create global scope
        let global = Scope::new(ScopeKind::Global, "global", 0);

        // Walk the translation unit
        self.walk_node(tree.root_node(), tree, &mut global.clone());

        global
    }

    /// Walk the CST and build scopes.
    fn walk_node(&self, node: Node, tree: &SourceTree, current_scope: &mut Scope) {
        match node.kind() {
            // Namespace
            "namespace_definition" => {
                let name_node = node.child_by_field_name("name");
                let name = name_node
                    .map(|n| tree.node_text(&n).to_string())
                    .unwrap_or_else(|| "_anonymous_".to_string());

                let child_depth = current_scope.depth + 1;
                let mut child_scope = Scope::new(ScopeKind::Namespace, name.clone(), child_depth);

                // Add namespace symbol to current scope
                let ns_pos = node.start_position();
                let ns_end = node.end_position();
                let mut ns_symbol = Symbol::new(name.clone(), SymbolKind::Namespace);
                ns_symbol.is_definition = true;
                ns_symbol.file_path = tree.file_path().to_string();
                ns_symbol.line_start = ns_pos.row as u32;
                ns_symbol.line_end = ns_end.row as u32;
                ns_symbol.column_start = ns_pos.column as u32;
                ns_symbol.column_end = ns_end.column as u32;
                ns_symbol.scope_kind = ScopeKind::Namespace;
                ns_symbol.scope_depth = child_depth;
                ns_symbol.container_name = current_scope.name.clone().if_empty_then_none();
                current_scope.symbols.push(ns_symbol);

                // Recurse into namespace body
                let body = node.child_by_field_name("body");
                if let Some(body_node) = body {
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree, &mut child_scope);
                    }
                }

                current_scope.children.push(child_scope);
            }

            // Struct
            "struct_specifier" => {
                let name_node = node.child_by_field_name("name");
                let name = name_node.map(|n| tree.node_text(&n).to_string());

                // Skip forward declarations with no name and no body
                if name.is_none() && node.child_by_field_name("body").is_none() {
                    return;
                }

                let display_name = name.unwrap_or_else(|| {
                    let pos = node.start_position();
                    format!("_anonymous_struct_{}", pos.row as u32)
                });

                let child_depth = current_scope.depth + 1;
                let mut child_scope = Scope::new(ScopeKind::Struct, display_name.clone(), child_depth);

                // Add struct symbol
                let pos = node.start_position();
                let end_pos = node.end_position();
                let mut struct_symbol = Symbol::new(display_name.clone(), SymbolKind::Struct);
                struct_symbol.is_definition = node.child_by_field_name("body").is_some();
                struct_symbol.file_path = tree.file_path().to_string();
                struct_symbol.line_start = pos.row as u32;
                struct_symbol.line_end = end_pos.row as u32;
                struct_symbol.column_start = pos.column as u32;
                struct_symbol.column_end = end_pos.column as u32;
                struct_symbol.scope_kind = ScopeKind::Struct;
                struct_symbol.scope_depth = child_depth;
                struct_symbol.container_name = current_scope.name.clone().if_empty_then_none();
                current_scope.symbols.push(struct_symbol);

                // Walk struct body for members
                if let Some(body_node) = node.child_by_field_name("body") {
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree, &mut child_scope);
                    }
                }

                current_scope.children.push(child_scope);
            }

            // Enum
            "enum_specifier" => {
                let name_node = node.child_by_field_name("name");
                let name = name_node.map(|n| tree.node_text(&n).to_string());

                if name.is_none() && node.child_by_field_name("body").is_none() {
                    return;
                }

                let display_name = name.unwrap_or_else(|| {
                    let pos = node.start_position();
                    format!("_anonymous_enum_{}", pos.row as u32)
                });

                let child_depth = current_scope.depth + 1;
                let mut child_scope = Scope::new(ScopeKind::Enum, display_name.clone(), child_depth);

                // Add enum symbol
                let pos = node.start_position();
                let end_pos = node.end_position();
                let mut enum_symbol = Symbol::new(display_name.clone(), SymbolKind::Enum);
                enum_symbol.is_definition = node.child_by_field_name("body").is_some();
                enum_symbol.file_path = tree.file_path().to_string();
                enum_symbol.line_start = pos.row as u32;
                enum_symbol.line_end = end_pos.row as u32;
                enum_symbol.column_start = pos.column as u32;
                enum_symbol.column_end = end_pos.column as u32;
                enum_symbol.scope_kind = ScopeKind::Enum;
                enum_symbol.scope_depth = child_depth;
                enum_symbol.container_name = current_scope.name.clone().if_empty_then_none();
                current_scope.symbols.push(enum_symbol);

                // Walk enum body for values
                if let Some(body_node) = node.child_by_field_name("body") {
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree, &mut child_scope);
                    }
                }

                current_scope.children.push(child_scope);
            }

            // C++ Class
            "class_specifier" => {
                let name_node = node.child_by_field_name("name");
                let name = name_node.map(|n| tree.node_text(&n).to_string());

                if name.is_none() && node.child_by_field_name("body").is_none() {
                    return;
                }

                let display_name = name.unwrap_or_else(|| {
                    let pos = node.start_position();
                    format!("_anonymous_class_{}", pos.row as u32)
                });

                let child_depth = current_scope.depth + 1;
                let mut child_scope = Scope::new(ScopeKind::Class, display_name.clone(), child_depth);

                // Add class symbol
                let pos = node.start_position();
                let end_pos = node.end_position();
                let mut class_symbol = Symbol::new(display_name.clone(), SymbolKind::Class);
                class_symbol.is_definition = node.child_by_field_name("body").is_some();
                class_symbol.file_path = tree.file_path().to_string();
                class_symbol.line_start = pos.row as u32;
                class_symbol.line_end = end_pos.row as u32;
                class_symbol.column_start = pos.column as u32;
                class_symbol.column_end = end_pos.column as u32;
                class_symbol.scope_kind = ScopeKind::Class;
                class_symbol.scope_depth = child_depth;
                class_symbol.container_name = current_scope.name.clone().if_empty_then_none();
                current_scope.symbols.push(class_symbol);

                // Walk class body
                if let Some(body_node) = node.child_by_field_name("body") {
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree, &mut child_scope);
                    }
                }

                current_scope.children.push(child_scope);
            }

            // Function definition
            "function_definition" => {
                let declarator = node.child_by_field_name("declarator");
                let name = declarator
                    .map(|d| extract_declarator_name(&d, tree))
                    .unwrap_or_default();

                if name.is_empty() {
                    // Still recurse for local scopes even if we can't extract the name
                    let body = node.child_by_field_name("body");
                    if let Some(body_node) = body {
                        let child_depth = current_scope.depth + 1;
                        let mut child_scope = Scope::new(
                            ScopeKind::Function,
                            "_unnamed_function".to_string(),
                            child_depth,
                        );
                        let mut cursor = body_node.walk();
                        for child in body_node.named_children(&mut cursor) {
                            self.walk_node(child, tree, &mut child_scope);
                        }
                        current_scope.children.push(child_scope);
                    }
                    return;
                }

                let decl_type = node.child_by_field_name("decl_type");
                let return_type = decl_type.map(|dt| tree.node_text(&dt).to_string());

                let child_depth = current_scope.depth + 1;
                let mut child_scope = Scope::new(ScopeKind::Function, name.clone(), child_depth);

                // Add function symbol
                let pos = node.start_position();
                let end_pos = node.end_position();
                let mut fn_symbol = Symbol::new(name.clone(), SymbolKind::Function);
                fn_symbol.is_definition = true;
                fn_symbol.file_path = tree.file_path().to_string();
                fn_symbol.line_start = pos.row as u32;
                fn_symbol.line_end = end_pos.row as u32;
                fn_symbol.column_start = pos.column as u32;
                fn_symbol.column_end = end_pos.column as u32;
                fn_symbol.return_type = return_type;
                fn_symbol.scope_kind = ScopeKind::Function;
                fn_symbol.scope_depth = child_depth;
                fn_symbol.container_name = current_scope.name.clone().if_empty_then_none();
                current_scope.symbols.push(fn_symbol);

                // Walk function body for local variables and nested blocks
                let body = node.child_by_field_name("body");
                if let Some(body_node) = body {
                    let mut cursor = body_node.walk();
                    for child in body_node.named_children(&mut cursor) {
                        self.walk_node(child, tree, &mut child_scope);
                    }
                }

                current_scope.children.push(child_scope);
            }

            // Compound statement (block scope) — only create scope if it contains declarations
            "compound_statement" => {
                // Avoid creating block scopes for function bodies (handled above)
                if let Some(parent) = node.parent() {
                    if parent.kind() == "function_definition" {
                        // Don't create extra block scope for the function body itself
                        // but do recurse into it
                        let mut cursor = node.walk();
                        for child in node.named_children(&mut cursor) {
                            self.walk_node(child, tree, current_scope);
                        }
                        return;
                    }
                }

                // Check if this block contains any declarations
                let has_declarations = has_declaration_children(&node);
                if !has_declarations {
                    let mut cursor = node.walk();
                    for child in node.named_children(&mut cursor) {
                        self.walk_node(child, tree, current_scope);
                    }
                    return;
                }

                let child_depth = current_scope.depth + 1;
                let pos = node.start_position();
                let mut child_scope = Scope::new(
                    ScopeKind::Block,
                    format!("block_{}", pos.row as u32),
                    child_depth,
                );

                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.walk_node(child, tree, &mut child_scope);
                }

                current_scope.children.push(child_scope);
            }

            // Declarations — extract symbol and add to current scope
            "declaration" => {
                self.extract_declaration_to_scope(node, tree, current_scope);
            }

            // Type definition
            "type_definition" => {
                self.extract_type_definition_to_scope(node, tree, current_scope);
            }

            // Enumerator
            "enumerator" => {
                self.extract_enumerator_to_scope(node, tree, current_scope);
            }

            // Field declaration (struct/class members)
            "field_declaration" => {
                self.extract_field_to_scope(node, tree, current_scope);
            }

            _ => {
                // Recurse into children for any nested scopes we might miss
                // But avoid going too deep into expressions
                let should_recurse = !is_expression_node(&node);
                if should_recurse {
                    let mut cursor = node.walk();
                    for child in node.named_children(&mut cursor) {
                        self.walk_node(child, tree, current_scope);
                    }
                }
            }
        }
    }

    fn extract_declaration_to_scope(
        &self,
        node: Node,
        tree: &SourceTree,
        scope: &mut Scope,
    ) {
        let decl_type_node = node.child_by_field_name("decl_type");
        let decl_type = decl_type_node.map(|n| tree.node_text(&n)).unwrap_or("");

        // Skip if this declaration is part of a type specifier (struct/enum/class)
        let has_type_specifier = node.named_children(&mut node.walk()).any(|child| {
            matches!(child.kind(), "struct_specifier" | "enum_specifier" | "class_specifier")
        });
        if has_type_specifier {
            return;
        }

        let storage_class = extract_storage_class(decl_type);
        let is_member = scope.kind == ScopeKind::Struct || scope.kind == ScopeKind::Class;
        let in_function = scope.kind == ScopeKind::Function || scope.kind == ScopeKind::Block;

        let init_declarators = node.child_by_field_name("declarators");
        if let Some(init_decl) = init_declarators {
            let mut cursor = init_decl.walk();
            for child in init_decl.named_children(&mut cursor) {
                if child.kind() == "init_declarator" {
                    let declarator = child.child_by_field_name("declarator");
                    if let Some(decl) = declarator {
                        let name = extract_declarator_name(&decl, tree);
                        if name.is_empty() {
                            continue;
                        }

                        let kind = if is_member {
                            SymbolKind::Member
                        } else if in_function {
                            SymbolKind::LocalVariable
                        } else {
                            SymbolKind::Variable
                        };

                        let pos = decl.start_position();
                        let mut symbol = Symbol::new(name, kind);
                        symbol.is_definition = true;
                        symbol.file_path = tree.file_path().to_string();
                        symbol.line_start = pos.row as u32;
                        symbol.column_start = pos.column as u32;
                        symbol.declared_type = Some(decl_type.to_string());
                        symbol.storage_class = storage_class.clone();
                        symbol.scope_kind = scope.kind.clone();
                        symbol.scope_depth = scope.depth;
                        symbol.container_name = scope.name.clone().if_empty_then_none();
                        scope.symbols.push(symbol);
                    }
                }
            }
        }
    }

    fn extract_type_definition_to_scope(
        &self,
        node: Node,
        tree: &SourceTree,
        scope: &mut Scope,
    ) {
        let declarator = node.child_by_field_name("declarator");
        let decl_type = node.child_by_field_name("decl_type");

        if let Some(decl) = declarator {
            let name = extract_declarator_name(&decl, tree);
            if name.is_empty() {
                return;
            }

            let underlying = decl_type.map(|dt| tree.node_text(&dt).to_string());
            let pos = node.start_position();

            let mut symbol = Symbol::new(name, SymbolKind::Typedef);
            symbol.is_definition = true;
            symbol.file_path = tree.file_path().to_string();
            symbol.line_start = pos.row as u32;
            symbol.column_start = pos.column as u32;
            symbol.declared_type = underlying;
            symbol.scope_kind = scope.kind.clone();
            symbol.scope_depth = scope.depth;
            symbol.container_name = scope.name.clone().if_empty_then_none();
            scope.symbols.push(symbol);
        }
    }

    fn extract_enumerator_to_scope(
        &self,
        node: Node,
        tree: &SourceTree,
        scope: &mut Scope,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let pos = node.start_position();
        let mut symbol = Symbol::new(name, SymbolKind::EnumValue);
        symbol.is_definition = true;
        symbol.file_path = tree.file_path().to_string();
        symbol.line_start = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.scope_kind = scope.kind.clone();
        symbol.scope_depth = scope.depth;
        symbol.container_name = scope.name.clone().if_empty_then_none();
        scope.symbols.push(symbol);
    }

    fn extract_field_to_scope(
        &self,
        node: Node,
        tree: &SourceTree,
        scope: &mut Scope,
    ) {
        let type_node = node.child_by_field_name("type");
        let decl_type = type_node.map(|n| tree.node_text(&n)).unwrap_or("");

        let declarators = node.child_by_field_name("declarators");
        if let Some(decls) = declarators {
            let mut cursor = decls.walk();
            for child in decls.named_children(&mut cursor) {
                if child.kind() == "field_declarator" || child.kind() == "function_declarator" {
                    let name = extract_declarator_name(&child, tree);
                    if name.is_empty() {
                        continue;
                    }

                    let kind = if child.kind() == "function_declarator" {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Member
                    };

                    let pos = child.start_position();
                    let mut symbol = Symbol::new(name, kind);
                    symbol.is_definition = true;
                    symbol.file_path = tree.file_path().to_string();
                    symbol.line_start = pos.row as u32;
                    symbol.column_start = pos.column as u32;
                    symbol.declared_type = Some(decl_type.to_string());
                    symbol.scope_kind = scope.kind.clone();
                    symbol.scope_depth = scope.depth;
                    symbol.container_name = scope.name.clone().if_empty_then_none();
                    scope.symbols.push(symbol);
                }
            }
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────

fn extract_declarator_name(node: &Node, tree: &SourceTree) -> String {
    match node.kind() {
        "identifier" => tree.node_text(node).to_string(),
        "function_declarator" | "ptr_declarator" | "array_declarator"
        | "parameter_list" | "pointer_declarator" | "field_declarator"
        | "init_declarator" => {
            node.child_by_field_name("declarator")
                .map(|d| extract_declarator_name(&d, tree))
                .unwrap_or_default()
        }
        "qualified_identifier" => tree.node_text(node).to_string(),
        "constructor_declarator" | "destructor_declarator" => {
            node.child_by_field_name("name")
                .map(|n| tree.node_text(&n).to_string())
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

fn extract_storage_class(decl_type: &str) -> Option<String> {
    for token in decl_type.split_whitespace() {
        match token {
            "static" | "extern" | "register" | "auto" | "thread_local" | "mutable" => {
                return Some(token.to_string());
            }
            _ => {}
        }
    }
    None
}

fn has_declaration_children(node: &Node) -> bool {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "declaration"
                | "type_definition"
                | "function_definition"
                | "compound_statement"
                | "for_statement"
                | "if_statement"
                | "while_statement"
                | "switch_statement"
        ) {
            return true;
        }
    }
    false
}

fn is_expression_node(node: &Node) -> bool {
    matches!(
        node.kind(),
        "expression_statement"
            | "return_statement"
            | "break_statement"
            | "continue_statement"
            | "goto_statement"
            | "call_expression"
            | "binary_expression"
            | "unary_expression"
            | "assignment_expression"
            | "string_literal"
            | "number_literal"
            | "char_literal"
            | "true"
            | "false"
            | "nil"
            | "new_expression"
            | "delete_expression"
            | "lambda_expression"
            | "field_expression"
            | "subscript_expression"
            | "cast_expression"
            | "conditional_expression"
            | "compound_literal_expression"
            | "sizeof_expression"
            | "alignof_expression"
            | "case_statement"
    )
}

// Helper trait for converting empty strings to None
trait IfEmptyThenNone {
    fn if_empty_then_none(self) -> Option<String>;
}

impl IfEmptyThenNone for String {
    fn if_empty_then_none(self) -> Option<String> {
        if self.is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

impl IfEmptyThenNone for &str {
    fn if_empty_then_none(self) -> Option<String> {
        if self.is_empty() {
            None
        } else {
            Some(self.to_string())
        }
    }
}
