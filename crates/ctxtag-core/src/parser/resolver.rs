use crate::model::{Reference, ReferenceKind, Scope, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

use super::tree::SourceTree;

/// Resolves identifier references to their declarations.
///
/// For each identifier node found in the CST, this resolver walks the
/// scope chain upward to find the matching declaration. It also handles
/// struct member access (`foo.bar`, `foo->bar`) and qualified names
/// (`ns::Class::member`).
pub struct ReferenceResolver {
    references: Vec<Reference>,
    /// Symbol ID counter for generating placeholder IDs.
    next_id: i64,
    /// Map from symbol name + container -> placeholder ID.
    symbol_id_map: HashMap<String, i64>,
}

impl ReferenceResolver {
    pub fn new() -> Self {
        Self {
            references: Vec::new(),
            next_id: 1,
            symbol_id_map: HashMap::new(),
        }
    }

    /// Build a symbol ID map from a collection of symbols.
    ///
    /// This should be called before `resolve` to populate the map
    /// with known definitions.
    pub fn index_symbols(&mut self, symbols: &[Symbol]) {
        for sym in symbols {
            let key = sym.qualified_name.clone();
            self.symbol_id_map.entry(key).or_insert_with(|| {
                let id = self.next_id;
                self.next_id += 1;
                id
            });

            // Also index by short name if it's not a duplicate
            self.symbol_id_map.entry(sym.name.clone()).or_insert_with(|| {
                let id = self.next_id;
                self.next_id += 1;
                id
            });
        }
    }

    /// Resolve all references in a parsed source tree.
    ///
    /// # Arguments
    /// * `tree` - The parsed source tree.
    /// * `scopes` - The scope chain built by ScopeBuilder (flattened).
    /// * `symbols` - All known symbols for cross-reference.
    pub fn resolve(
        &mut self,
        tree: &SourceTree,
        scopes: &[Scope],
        symbols: &[Symbol],
    ) -> Vec<Reference> {
        // Index symbols if not already done
        if self.symbol_id_map.is_empty() {
            self.index_symbols(symbols);
        }

        self.references.clear();
        self.walk_node(tree.root_node(), tree, scopes);
        std::mem::take(&mut self.references)
    }

    /// Collect all scopes into a flat chain (ancestors first, then current).
    fn build_scope_chain<'a>(scopes: &'a [Scope]) -> Vec<&'a Scope> {
        let mut chain = Vec::new();
        Self::collect_scopes(scopes, &mut chain);
        chain
    }

    fn collect_scopes<'a>(scopes: &'a [Scope], chain: &mut Vec<&'a Scope>) {
        for scope in scopes {
            chain.push(scope);
            Self::collect_scopes(&scope.children, chain);
        }
    }

    fn walk_node(&mut self, node: Node, tree: &SourceTree, scopes: &[Scope]) {
        let kind = node.kind();

        match kind {
            "identifier" => {
                // Identifier is a leaf node, resolve and don't recurse further
                self.resolve_identifier(node, tree, scopes);
                return;
            }
            "call_expression" => {
                // Resolve the call and skip recursing into children to avoid
                // double-resolving the function identifier inside the call
                self.resolve_call_expression(node, tree, scopes);
                return;
            }
            "qualified_identifier" => {
                self.resolve_qualified_identifier(node, tree, scopes);
            }
            "field_expression" => {
                self.resolve_field_expression(node, tree, scopes);
                // field_expression already resolves the operand identifier
            }
            _ => {}
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.walk_node(child, tree, scopes);
        }
    }

    // ─── Identifier Resolution ────────────────────────────────────────

    fn resolve_identifier(&mut self, node: Node, tree: &SourceTree, scopes: &[Scope]) {
        let name = tree.node_text(&node);
        let parent = match node.parent() {
            Some(p) => p,
            None => return,
        };

        let (ref_kind, context) = self.classify_identifier_reference(&node, &parent, tree);

        // Skip keywords that are parsed as identifiers
        if is_keyword(name) {
            return;
        }

        // Try to resolve against scope chain
        let scope_chain = Self::build_scope_chain(scopes);
        let symbol_id = self.lookup_symbol(&scope_chain, name, scopes);

        let pos = node.start_position();
        let end_pos = node.end_position();

        // Get context line
        let context_line = self.get_context_line(node, tree);

        let mut reference = Reference {
            id: None,
            symbol_id,
            symbol_name: name.to_string(),
            file_path: tree.file_path().to_string(),
            line: pos.row as u32,
            column: pos.column as u32,
            end_line: Some(end_pos.row as u32),
            end_column: Some(end_pos.column as u32),
            ref_kind,
            context,
            context_detail: None,
            context_line,
            is_definition: self.is_definition_context(&parent),
        };

        // Add container context
        if let Some(container) = self.get_container_name(&scope_chain, name) {
            reference.context_detail = Some(container);
        }

        self.references.push(reference);
    }

    // ─── Qualified Identifier (ns::Class::member) ─────────────────────

    fn resolve_qualified_identifier(&mut self, node: Node, tree: &SourceTree, scopes: &[Scope]) {
        let full_name = tree.node_text(&node);
        let pos = node.start_position();
        let end_pos = node.end_position();

        let symbol_id = self.lookup_qualified_symbol(&full_name, scopes);

        // Determine reference kind from context
        let parent = node.parent();
        let ref_kind = if let Some(p) = &parent {
            match p.kind() {
                "call_expression" => ReferenceKind::Call,
                "scope_resolution_operator" => ReferenceKind::ScopeResolution,
                _ => ReferenceKind::Access,
            }
        } else {
            ReferenceKind::Access
        };

        let context_line = self.get_context_line(node, tree);

        self.references.push(Reference {
            id: None,
            symbol_id,
            symbol_name: full_name.to_string(),
            file_path: tree.file_path().to_string(),
            line: pos.row as u32,
            column: pos.column as u32,
            end_line: Some(end_pos.row as u32),
            end_column: Some(end_pos.column as u32),
            ref_kind,
            context: Some("qualified".to_string()),
            context_detail: Some(full_name.to_string()),
            context_line,
            is_definition: false,
        });
    }

    // ─── Field Expression (foo.bar, foo->bar) ─────────────────────────

    fn resolve_field_expression(&mut self, node: Node, tree: &SourceTree, scopes: &[Scope]) {
        let operand = node.child_by_field_name("operand");
        let field = node.child_by_field_name("field");

        if let Some(field_node) = field {
            let field_name = tree.node_text(&field_node);
            let pos = field_node.start_position();
            let end_pos = field_node.end_position();

            // Try to determine the type of the operand to resolve the field
            let operand_type = self.infer_operand_type(&operand, tree, scopes);

            let symbol_id = if let Some(type_name) = &operand_type {
                // Look for a member with this name in the type's scope
                self.lookup_member_in_type(type_name, &field_name, scopes)
            } else {
                // Fallback: try general lookup
                self.lookup_symbol_general(&field_name, scopes)
            };

            // Determine if it's . or -> access
            let is_pointer_access = tree.node_text(&node).contains("->");
            let ref_kind = if is_pointer_access {
                ReferenceKind::Dereference
            } else {
                ReferenceKind::Access
            };

            let context_line = self.get_context_line(field_node, tree);

            self.references.push(Reference {
                id: None,
                symbol_id,
                symbol_name: field_name.to_string(),
                file_path: tree.file_path().to_string(),
                line: pos.row as u32,
                column: pos.column as u32,
                end_line: Some(end_pos.row as u32),
                end_column: Some(end_pos.column as u32),
                ref_kind,
                context: if is_pointer_access {
                    Some("pointer_member_access".to_string())
                } else {
                    Some("member_access".to_string())
                },
                context_detail: operand_type,
                context_line,
                is_definition: false,
            });
        }

        // Also resolve the operand (recursively)
        if let Some(op) = operand {
            if op.kind() == "identifier" {
                self.resolve_identifier(op, tree, scopes);
            }
        }
    }

    // ─── Call Expression ──────────────────────────────────────────────

    fn resolve_call_expression(&mut self, node: Node, tree: &SourceTree, scopes: &[Scope]) {
        let function = node.child_by_field_name("function");
        if let Some(func_node) = function {
            let name = tree.node_text(&func_node);
            let pos = func_node.start_position();
            let end_pos = func_node.end_position();

            let symbol_id = self.lookup_symbol_general(&name, scopes);
            let context_line = self.get_context_line(func_node, tree);

            self.references.push(Reference {
                id: None,
                symbol_id,
                symbol_name: name.to_string(),
                file_path: tree.file_path().to_string(),
                line: pos.row as u32,
                column: pos.column as u32,
                end_line: Some(end_pos.row as u32),
                end_column: Some(end_pos.column as u32),
                ref_kind: ReferenceKind::Call,
                context: Some("function_call".to_string()),
                context_detail: None,
                context_line,
                is_definition: false,
            });
        }
    }

    // ─── Lookup Helpers ────────────────────────────────────────────────

    fn lookup_symbol(&mut self, scope_chain: &[&Scope], name: &str, _all_scopes: &[Scope]) -> i64 {
        // Walk scope chain from innermost to outermost
        for scope in scope_chain.iter().rev() {
            if let Some(sym) = scope.lookup(name) {
                if let Some(id) = self.get_symbol_id(sym) {
                    return id;
                }
            }
        }

        // Fallback: generate placeholder ID
        self.get_or_create_id(name)
    }

    fn lookup_qualified_symbol(&mut self, full_name: &str, _scopes: &[Scope]) -> i64 {
        // Try exact match first
        if let Some(&id) = self.symbol_id_map.get(full_name) {
            return id;
        }

        // Try splitting on :: and looking up the last component
        let parts: Vec<&str> = full_name.split("::").collect();
        if let Some(last) = parts.last() {
            return self.get_or_create_id(last);
        }

        self.get_or_create_id(full_name)
    }

    fn lookup_member_in_type(
        &mut self,
        type_name: &str,
        member_name: &str,
        scopes: &[Scope],
    ) -> i64 {
        // Try to find the type in scopes and look up the member
        let qualified = format!("{}::{}", type_name, member_name);
        if let Some(&id) = self.symbol_id_map.get(&qualified) {
            return id;
        }

        // Search scope tree for the type
        for scope in scopes {
            if let Some(member_scope) = Self::find_scope_by_name(scopes, type_name) {
                if let Some(sym) = member_scope.lookup(member_name) {
                    if let Some(id) = self.get_symbol_id(sym) {
                        return id;
                    }
                }
                // Also search children
                for child in &member_scope.children {
                    if let Some(sym) = child.lookup(member_name) {
                        if let Some(id) = self.get_symbol_id(sym) {
                            return id;
                        }
                    }
                }
            }
        }

        self.get_or_create_id(member_name)
    }

    fn lookup_symbol_general(&mut self, name: &str, scopes: &[Scope]) -> i64 {
        if let Some(&id) = self.symbol_id_map.get(name) {
            return id;
        }

        // Search all scopes
        let scope_chain = Self::build_scope_chain(scopes);
        for scope in scope_chain.iter().rev() {
            if let Some(sym) = scope.lookup(name) {
                return self.get_symbol_id(sym).unwrap_or_else(|| self.get_or_create_id(name));
            }
        }

        self.get_or_create_id(name)
    }

    fn find_scope_by_name<'a>(scopes: &'a [Scope], name: &str) -> Option<&'a Scope> {
        for scope in scopes {
            if scope.name == name {
                return Some(scope);
            }
            if let Some(found) = Self::find_scope_by_name(&scope.children, name) {
                return Some(found);
            }
        }
        None
    }

    fn get_symbol_id(&self, sym: &Symbol) -> Option<i64> {
        if let Some(id) = sym.id {
            Some(id)
        } else {
            self.symbol_id_map.get(&sym.qualified_name).copied()
        }
    }

    fn get_or_create_id(&mut self, name: &str) -> i64 {
        *self.symbol_id_map.entry(name.to_string()).or_insert_with(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        })
    }

    // ─── Classification ───────────────────────────────────────────────

    fn classify_identifier_reference(
        &self,
        node: &Node,
        parent: &Node,
        _tree: &SourceTree,
    ) -> (ReferenceKind, Option<String>) {
        match parent.kind() {
            // Declaration context
            "type_identifier" => (ReferenceKind::TypeUse, Some("type".to_string())),
            "field_declaration" => (ReferenceKind::Declaration, Some("field".to_string())),
            "declaration" => (ReferenceKind::Declaration, Some("declaration".to_string())),
            "type_definition" => (ReferenceKind::Declaration, Some("typedef".to_string())),
            "parameter_declaration" => (ReferenceKind::Declaration, Some("parameter".to_string())),
            "struct_specifier" => (ReferenceKind::Declaration, Some("struct".to_string())),
            "enum_specifier" => (ReferenceKind::Declaration, Some("enum".to_string())),
            "class_specifier" => (ReferenceKind::Declaration, Some("class".to_string())),
            "namespace_definition" => (ReferenceKind::Declaration, Some("namespace".to_string())),

            // Call context
            "call_expression" => (ReferenceKind::Call, Some("call".to_string())),

            // Assignment
            "assignment_expression" => {
                let field_name = parent.child_by_field_name("left").map(|n| node == &n);
                if field_name == Some(true) {
                    (ReferenceKind::Assignment, Some("assignment_left".to_string()))
                } else {
                    (ReferenceKind::Access, Some("assignment_right".to_string()))
                }
            }

            // Return
            "return_statement" => (ReferenceKind::Return, Some("return".to_string())),

            // Address-of
            "unary_expression" => {
                let op = parent.child(0).map(|n| {
                    let t = _tree.node_text(&n);
                    t.to_string()
                });
                if op == Some(("&").to_string()) {
                    (ReferenceKind::AddressOf, Some("address_of".to_string()))
                } else if op == Some(("*").to_string()) {
                    (ReferenceKind::Dereference, Some("dereference".to_string()))
                } else {
                    (ReferenceKind::Access, None)
                }
            }

            // Scope resolution
            "scope_resolution_operator" => {
                (ReferenceKind::ScopeResolution, Some("scope_resolution".to_string()))
            }

            // Macro use
            "preproc_call" | "preproc_arg" => {
                (ReferenceKind::MacroUse, Some("macro".to_string()))
            }

            // Default
            _ => (ReferenceKind::Access, None),
        }
    }

    fn is_definition_context(&self, parent: &Node) -> bool {
        matches!(
            parent.kind(),
            "declaration"
                | "type_definition"
                | "function_definition"
                | "field_declaration"
                | "enum_specifier"
                | "struct_specifier"
                | "class_specifier"
                | "namespace_definition"
                | "parameter_declaration"
                | "init_declarator"
                | "field_declarator"
                | "enumerator"
        )
    }

    fn infer_operand_type(
        &self,
        operand: &Option<Node>,
        _tree: &SourceTree,
        _scopes: &[Scope],
    ) -> Option<String> {
        match operand {
            Some(node) => {
                if node.kind() == "identifier" {
                    // Could look up the type of this identifier in the scope chain
                    // For now, return the identifier name as a hint
                    Some(_tree.node_text(node).to_string())
                } else if node.kind() == "field_expression" {
                    // Nested access — get the innermost identifier
                    let field = node.child_by_field_name("field");
                    field.map(|f| _tree.node_text(&f).to_string())
                } else {
                    None
                }
            }
            None => None,
        }
    }

    fn get_container_name(&self, scope_chain: &[&Scope], name: &str) -> Option<String> {
        // Find the scope where this name is declared and return its parent's name
        for (idx, scope) in scope_chain.iter().enumerate().rev() {
            if scope.lookup(name).is_some() {
                if idx > 0 {
                    return Some(scope_chain[idx - 1].name.clone());
                }
                return Some(scope.name.clone());
            }
        }
        None
    }

    fn get_context_line(&self, node: Node, tree: &SourceTree) -> Option<String> {
        let pos = node.start_position();
        let line_start = pos.row as u32 as usize;

        let lines: Vec<&str> = tree.source_str().lines().collect();
        lines.get(line_start).map(|l| (*l).to_string())
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            // C++ keywords
            | "alignas"
            | "alignof"
            | "asm"
            | "bool"
            | "char8_t"
            | "char16_t"
            | "char32_t"
            | "class"
            | "co_await"
            | "co_return"
            | "co_yield"
            | "concept"
            | "consteval"
            | "constexpr"
            | "constinit"
            | "const_cast"
            | "decltype"
            | "delete"
            | "dynamic_cast"
            | "explicit"
            | "export"
            | "false"
            | "friend"
            | "nullptr"
            | "namespace"
            | "new"
            | "noexcept"
            | "not"
            | "not_eq"
            | "operator"
            | "or"
            | "or_eq"
            | "private"
            | "protected"
            | "public"
            | "reinterpret_cast"
            | "requires"
            | "static_assert"
            | "static_cast"
            | "template"
            | "this"
            | "thread_local"
            | "throw"
            | "true"
            | "try"
            | "typeid"
            | "typename"
            | "using"
            | "virtual"
            | "wchar_t"
            | "which"
            | "xor"
            | "xor_eq"
    )
}
