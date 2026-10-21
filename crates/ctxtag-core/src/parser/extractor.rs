use crate::model::{ScopeKind, Symbol, SymbolKind};
use tree_sitter::Node;

use super::tree::SourceTree;

/// Extracts symbols from a tree-sitter syntax tree.
///
/// Walks the CST and identifies functions, variables, structs, enums,
/// typedefs, parameters, members, methods, namespaces, and more.
pub struct SymbolExtractor {
    file_path: String,
    symbols: Vec<Symbol>,
    /// Stack of container names for building qualified names.
    container_stack: Vec<String>,
}

impl SymbolExtractor {
    pub fn new(file_path: impl Into<String>) -> Self {
        Self {
            file_path: file_path.into(),
            symbols: Vec::new(),
            container_stack: Vec::new(),
        }
    }

    /// Extract all symbols from a parsed source tree.
    pub fn extract(&mut self, tree: &SourceTree) -> Vec<Symbol> {
        self.symbols.clear();
        self.container_stack.clear();
        self.walk_node(tree.root_node(), tree);
        std::mem::take(&mut self.symbols)
    }

    /// Walk the CST depth-first, dispatching on node type.
    fn walk_node(&mut self, node: Node, tree: &SourceTree) {
        let kind = node.kind();

        match kind {
            // C function definition / C++ method outside class
            "function_definition" => self.extract_function_definition(node, tree),

            // Declarations: variables, structs, enums, typedefs
            "declaration" => self.extract_declaration(node, tree),

            // Type definition (typedef)
            "type_definition" => self.extract_type_definition(node, tree),

            // Struct / union specifier
            "struct_specifier" => self.extract_struct_specifier(node, tree),

            // Enum specifier
            "enum_specifier" => self.extract_enum_specifier(node, tree),

            // C++ class specifier
            "class_specifier" => self.extract_class_specifier(node, tree),

            // C++ namespace
            "namespace_definition" => self.extract_namespace(node, tree),

            // C++ method inside class
            "function_declarator" => {
                // Check if parent is a class/struct specifier (method)
                if let Some(parent) = node.parent() {
                    let parent_kind = parent.kind();
                    if parent_kind == "class_specifier"
                        || parent_kind == "struct_specifier"
                        || parent_kind == "field_declaration"
                    {
                        self.extract_method(node, parent, tree);
                    }
                }
            }

            // C++ using declarations and aliases
            "using_declaration" => self.extract_using_declaration(node, tree),
            "using_alias_declaration" => self.extract_using_alias(node, tree),

            _ => {}
        }

        // Recurse into named children via a cursor. Cursor sibling-steps are
        // O(1) amortized, whereas child(0)/next_sibling() re-derives the parent
        // on every step (ts_node_child_containing_descendant) and degrades to
        // O(children^2) on nodes with huge child lists — e.g. the translation
        // unit of generated register headers with tens of thousands of top-level
        // declarations. Anonymous children are tokens that never match the cases
        // above, so skipping them (named_children) is both safe and cheaper.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.walk_node(child, tree);
        }
    }

    // ─── Functions ────────────────────────────────────────────────────

    fn extract_function_definition(&mut self, node: Node, tree: &SourceTree) {
        // function_definition -> declarator + body
        let declarator = node.child_by_field_name("declarator");
        let decl_type = node.child_by_field_name("decl_type");

        let (name, return_type, storage_class, signature) =
            self.parse_function_declarator(declarator, decl_type, tree);

        if name.is_empty() {
            return;
        }

        let params_node = declarator
            .and_then(|d| d.child_by_field_name("parameters"))
            .and_then(|p| p.child_by_field_name("parameters"));

        // Extract parameters
        if let Some(params) = params_node {
            let mut param_cursor = params.walk();
            for child in params.named_children(&mut param_cursor) {
                if child.kind() == "parameter_declaration" {
                    if let Some(param) = self.extract_parameter(child, tree) {
                        self.symbols.push(param);
                    }
                }
            }
        }

        let pos = node.start_position();
        let end_pos = node.end_position();
        let qualified = self.make_qualified_name(&name);

        let mut symbol = Symbol::new(&qualified, SymbolKind::Function);
        symbol.name = name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = true;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.return_type = return_type.clone();
        symbol.declared_type = return_type;
        symbol.storage_class = storage_class;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.signature = signature;
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.definition_text = Some(tree.node_text(&node).to_string());
        symbol.documentation = self.extract_documentation(node, tree);

        self.symbols.push(symbol);
    }

    // ─── Declarations ─────────────────────────────────────────────────

    fn extract_declaration(&mut self, node: Node, tree: &SourceTree) {
        // declaration -> decl_type + init_declarators (comma-separated)
        let decl_type_node = node.child_by_field_name("decl_type");
        let decl_type_str = decl_type_node.map(|n| tree.node_text(&n)).unwrap_or("");

        // Check for struct/enum/union specifiers inside declaration
        let has_type_specifier = node.named_children(&mut node.walk()).any(|child| {
            matches!(child.kind(), "struct_specifier" | "enum_specifier" | "class_specifier")
        });

        // If this declaration introduces a named type, handle it via the type-specific extractor
        if has_type_specifier {
            return;
        }

        // Extract storage class from decl_type
        let storage_class = extract_storage_class(decl_type_str);

        // Check if this is inside a struct (i.e., a member)
        let is_member = self.is_inside_struct(node);

        // A `declaration` carries one or more declarators in the repeated
        // `declarator` field (comma-separated: `int a, b;`). A declarator with
        // an initializer is wrapped in an `init_declarator`; bare ones
        // (`int x;`, `T arr[30];`, `int *p;`) are not. Unwrap init_declarator to
        // its inner declarator for an accurate span, then let
        // extract_declarator_name peel array/pointer/parenthesized wrappers.
        let mut cursor = node.walk();
        for decl in node.children_by_field_name("declarator", &mut cursor) {
            let inner = if decl.kind() == "init_declarator" {
                decl.child_by_field_name("declarator").unwrap_or(decl)
            } else {
                decl
            };
            // A function prototype (`int foo(int);`) declares a function, not a
            // variable; skip it here so it isn't mislabeled as a Variable.
            if inner.kind() == "function_declarator" {
                continue;
            }
            self.extract_variable_from_declarator(
                inner,
                decl_type_str,
                storage_class.clone(),
                is_member,
                tree,
            );
        }
    }

    fn extract_variable_from_declarator(
        &mut self,
        declarator: Node,
        decl_type: &str,
        storage_class: Option<String>,
        is_member: bool,
        tree: &SourceTree,
    ) {
        let name = self.extract_declarator_name(&declarator, tree);
        if name.is_empty() {
            return;
        }

        // Only file-scope (global / static) variables become symbols. Function-
        // local declarations are intentionally skipped to keep the DB compact;
        // local jump-to-definition is resolved by scope at query time.
        if !is_member && self.is_inside_function(declarator) {
            return;
        }

        let (adjusted_type, kind) = if is_member {
            (decl_type.to_string(), SymbolKind::Member)
        } else if storage_class.as_deref() == Some("static") {
            (decl_type.to_string(), SymbolKind::StaticMember)
        } else {
            (decl_type.to_string(), SymbolKind::Variable)
        };

        let pos = declarator.start_position();
        let end_pos = declarator.end_position();
        let qualified = self.make_qualified_name(&name);

        let mut symbol = Symbol::new(&qualified, kind);
        symbol.name = name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = true;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.declared_type = Some(adjusted_type);
        symbol.storage_class = storage_class;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.access_modifier = self.infer_access_modifier(declarator);

        self.symbols.push(symbol);
    }

    // ─── Type Definitions ─────────────────────────────────────────────

    fn extract_type_definition(&mut self, node: Node, tree: &SourceTree) {
        // type_definition -> type (the aliased type) + declarator
        let declarator = node.child_by_field_name("declarator");
        let decl_type = node.child_by_field_name("type");

        if let Some(decl) = declarator {
            let name = self.extract_declarator_name(&decl, tree);
            if name.is_empty() {
                return;
            }

            let underlying_type = decl_type.map(|n| tree.node_text(&n).to_string());
            let pos = node.start_position();
            let end_pos = node.end_position();
            let qualified = self.make_qualified_name(&name);

            let mut symbol = Symbol::new(&qualified, SymbolKind::Typedef);
            symbol.name = name.clone();
            symbol.qualified_name = qualified;
            symbol.is_definition = true;
            symbol.file_path = self.file_path.clone();
            symbol.line_start = pos.row as u32;
            symbol.line_end = end_pos.row as u32;
            symbol.column_start = pos.column as u32;
            symbol.column_end = end_pos.column as u32;
            symbol.declared_type = underlying_type;
            symbol.scope_kind = self.current_scope_kind();
            symbol.scope_depth = self.container_stack.len() as u32;
            symbol.container_name = self.container_stack.last().cloned();
            symbol.selection_line = Some(pos.row as u32);
            symbol.selection_column = Some(pos.column as u32);
            symbol.declaration_text = Some(tree.node_text(&node).to_string());

            self.symbols.push(symbol);
        }
    }

    // ─── Struct / Union ───────────────────────────────────────────────

    fn extract_struct_specifier(&mut self, node: Node, tree: &SourceTree) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();

        let body = node.child_by_field_name("body");

        if name.is_empty() && body.is_none() {
            // Forward declaration with no body, skip if no name
            return;
        }

        // A bodyless elaborated type used inside another declaration
        // (`struct dmtimer *timer`, field types, parameters, ...) is a type
        // *usage*, not a declaration. Only genuine forward declarations
        // (`struct dmtimer;`) and definitions (with a body) become symbols.
        if body.is_none() && is_elaborated_type_usage(&node) {
            return;
        }

        // Determine if struct or union
        let kind = if is_union_context(&node) {
            (SymbolKind::Union, ScopeKind::Union)
        } else {
            (SymbolKind::Struct, ScopeKind::Struct)
        };

        let display_name = if name.is_empty() {
            // Anonymous struct/union — check if inside a declaration that gives it a name
            get_anonymous_type_name(&node, tree)
        } else {
            name.clone()
        };

        let pos = node.start_position();
        let end_pos = node.end_position();
        let qualified = self.make_qualified_name(&display_name);

        let mut symbol = Symbol::new(&qualified, kind.0);
        symbol.name = display_name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = body.is_some();
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.declaration_text = Some(tree.node_text(&node).to_string());

        self.symbols.push(symbol);

        // Push onto container stack for member extraction
        if !display_name.is_empty() {
            self.container_stack.push(display_name.clone());
            let old_kind = self.current_scope_kind();

            // Extract members
            if let Some(body_node) = body {
                let mut cursor = body_node.walk();
                for child in body_node.named_children(&mut cursor) {
                    if child.kind() == "field_declaration" {
                        self.extract_field_declaration(child, display_name.clone(), tree);
                    }
                }
            }

            self.container_stack.pop();
        }
    }

    fn extract_field_declaration(
        &mut self,
        node: Node,
        container: String,
        tree: &SourceTree,
    ) {
        // field_declaration -> type + one or more `declarator` fields
        // (each a _field_declarator: field_identifier, pointer_declarator,
        //  array_declarator, function_declarator, ...). Bit-fields such as
        //  `unsigned posted:1;` carry a separate `bitfield_clause` child.
        let type_node = node.child_by_field_name("type");
        let base_type = type_node
            .map(|n| tree.node_text(&n).to_string())
            .unwrap_or_default();

        let decl_text_full = tree.node_text(&node);
        let access = self.infer_access_modifier(node);

        let mut cursor = node.walk();
        for decl in node.children_by_field_name("declarator", &mut cursor) {
            let member_name = self.extract_declarator_name(&decl, tree);
            if member_name.is_empty() {
                continue;
            }

            // Reconstruct a decorated type string (pointer / array markers).
            let decl_text = tree.node_text(&decl);
            let mut declared_type = base_type.clone();
            let stars = decl_text.chars().filter(|&c| c == '*').count();
            for _ in 0..stars {
                declared_type.push_str(" *");
            }
            if decl_text.contains('[') {
                declared_type.push_str("[]");
            }
            if decl_text_full.contains(':') && !declared_type.is_empty() {
                // bit-field: keep the width in the displayed type
                if let Some(colon) = decl_text_full.rfind(':') {
                    let width = decl_text_full[colon + 1..]
                        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
                    if !width.is_empty() && width.chars().all(|c| c.is_ascii_digit()) {
                        declared_type.push_str(" :");
                        declared_type.push_str(width);
                    }
                }
            }

            let pos = decl.start_position();
            let end_pos = decl.end_position();
            let qualified = format!("{}::{}", container, member_name);

            let mut symbol = Symbol::new(&qualified, SymbolKind::Member);
            symbol.name = member_name;
            symbol.qualified_name = qualified;
            symbol.is_definition = true;
            symbol.file_path = self.file_path.clone();
            symbol.line_start = pos.row as u32;
            symbol.line_end = end_pos.row as u32;
            symbol.column_start = pos.column as u32;
            symbol.column_end = end_pos.column as u32;
            symbol.declared_type = Some(declared_type);
            symbol.scope_kind = ScopeKind::Struct;
            symbol.scope_depth = self.container_stack.len() as u32;
            symbol.container_name = Some(container.clone());
            symbol.access_modifier = access.clone();
            symbol.selection_line = Some(pos.row as u32);
            symbol.selection_column = Some(pos.column as u32);

            self.symbols.push(symbol);
        }
    }

    // ─── Enum ─────────────────────────────────────────────────────────

    fn extract_enum_specifier(&mut self, node: Node, tree: &SourceTree) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();

        let body = node.child_by_field_name("body");
        if body.is_none() && is_elaborated_type_usage(&node) {
            return;
        }
        let display_name = if name.is_empty() {
            get_anonymous_type_name(&node, tree)
        } else {
            name.clone()
        };

        let pos = node.start_position();
        let end_pos = node.end_position();
        let qualified = self.make_qualified_name(&display_name);

        let mut symbol = Symbol::new(&qualified, SymbolKind::Enum);
        symbol.name = display_name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = body.is_some();
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.declaration_text = Some(tree.node_text(&node).to_string());

        self.symbols.push(symbol);

        // Extract enum values
        if !display_name.is_empty() {
            self.container_stack.push(display_name.clone());

            // `body` IS the enumerator_list; iterate its `enumerator` children
            // directly (it has no nested `body` field).
            if let Some(enum_list) = body {
                let mut cursor = enum_list.walk();
                for child in enum_list.named_children(&mut cursor) {
                    if child.kind() == "enumerator" {
                        self.extract_enumerator(child, display_name.clone(), tree);
                    }
                }
            }

            self.container_stack.pop();
        }
    }

    fn extract_enumerator(&mut self, node: Node, container: String, tree: &SourceTree) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let name_len = name.len() as u32;
        let _value_node = node.child_by_field_name("value");

        let pos = node.start_position();
        let qualified = format!("{}::{}", container, name.clone());

        let mut symbol = Symbol::new(&qualified, SymbolKind::EnumValue);
        symbol.name = name;
        symbol.qualified_name = qualified;
        symbol.is_definition = true;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = pos.column as u32 + name_len;
        symbol.scope_kind = ScopeKind::Enum;
        symbol.scope_depth = self.container_stack.len() as u32 + 1;
        symbol.container_name = Some(container);
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.declaration_text = Some(tree.node_text(&node).to_string());

        self.symbols.push(symbol);
    }

    // ─── C++ Class ────────────────────────────────────────────────────

    fn extract_class_specifier(&mut self, node: Node, tree: &SourceTree) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();

        let body = node.child_by_field_name("body");

        if name.is_empty() && body.is_none() {
            return;
        }

        if body.is_none() && is_elaborated_type_usage(&node) {
            return;
        }

        let display_name = if name.is_empty() {
            get_anonymous_type_name(&node, tree)
        } else {
            name.clone()
        };

        // Extract base classes
        let base_classes = self.extract_base_classes(&node, tree);

        let pos = node.start_position();
        let end_pos = node.end_position();
        let qualified = self.make_qualified_name(&display_name);

        let mut symbol = Symbol::new(&qualified, SymbolKind::Class);
        symbol.name = display_name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = body.is_some();
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);
        symbol.declaration_text = Some(tree.node_text(&node).to_string());
        symbol.base_classes = base_classes;

        self.symbols.push(symbol);

        // Extract methods and members from class body
        if !display_name.is_empty() {
            self.container_stack.push(display_name.clone());

            if let Some(body_node) = body {
                let mut cursor = body_node.walk();
                for child in body_node.named_children(&mut cursor) {
                    match child.kind() {
                        "field_declaration" => {
                            // Could be a method declaration or data member
                            self.extract_class_field(child, display_name.clone(), tree);
                        }
                        "declaration" => {
                            // Static members or nested types
                            self.extract_declaration(child, tree);
                        }
                        "function_definition" => {
                            // Inline method definition
                            self.extract_function_definition(child, tree);
                        }
                        // Access specifiers are tracked via context, not extracted as symbols
                        "access_specifier" => {}
                        _ => {}
                    }
                }
            }

            self.container_stack.pop();
        }
    }

    fn extract_class_field(
        &mut self,
        node: Node,
        container: String,
        tree: &SourceTree,
    ) {
        let type_node = node.child_by_field_name("type");
        let decl_type_str = type_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();

        let declarators = node.child_by_field_name("declarators");
        if let Some(decls) = declarators {
            let mut cursor = decls.walk();
            for child in decls.named_children(&mut cursor) {
                if child.kind() == "function_declarator" {
                    self.extract_class_method(child, container.clone(), decl_type_str.clone(), tree);
                } else if child.kind() == "ptr_declarator"
                    || child.kind() == "identifier"
                    || child.kind() == "array_declarator"
                {
                    // Data member
                    let name = self.extract_declarator_name(&child, tree);
                    if !name.is_empty() {
                        let name_len = name.len() as u32;
                        let pos = child.start_position();
                        let qualified = format!("{}::{}", container, name);

                        let mut symbol = Symbol::new(&qualified, SymbolKind::Member);
                        symbol.name = name;
                        symbol.qualified_name = qualified;
                        symbol.is_definition = true;
                        symbol.file_path = self.file_path.clone();
                        symbol.line_start = pos.row as u32;
                        symbol.line_end = pos.row as u32;
                        symbol.column_start = pos.column as u32;
                        symbol.column_end = pos.column as u32 + name_len;
                        symbol.declared_type = Some(decl_type_str.clone());
                        symbol.scope_kind = ScopeKind::Class;
                        symbol.scope_depth = self.container_stack.len() as u32 + 1;
                        symbol.container_name = Some(container.clone());
                        symbol.access_modifier = self.infer_access_modifier(child);
                        symbol.selection_line = Some(pos.row as u32);
                        symbol.selection_column = Some(pos.column as u32);

                        self.symbols.push(symbol);
                    }
                }
            }
        }
    }

    fn extract_class_method(
        &mut self,
        node: Node,
        container: String,
        decl_type: String,
        tree: &SourceTree,
    ) {
        let name = self.extract_declarator_name(&node, tree);
        if name.is_empty() {
            return;
        }

        let name_len = name.len() as u32;

        // Determine method kind (constructor, destructor, operator, etc.)
        let kind = self.determine_method_kind(&name, &node, tree);

        let params_node = node.child_by_field_name("parameters");
        let params_str = params_node.map(|p| tree.node_text(&p)).unwrap_or("");

        let pos = node.start_position();
        let qualified = format!("{}::{}", container, name);

        // Check for virtual, override, final, const
        let (is_virtual, is_override, is_final, is_const) =
            self.extract_method_qualifiers(&node, tree);

        let signature = format!("{}{};{}", decl_type, name, params_str);

        let mut symbol = Symbol::new(&qualified, kind);
        symbol.name = name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = false;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = pos.column as u32 + name_len;
        symbol.return_type = Some(decl_type);
        symbol.scope_kind = ScopeKind::Class;
        symbol.scope_depth = self.container_stack.len() as u32 + 1;
        symbol.container_name = Some(container);
        symbol.access_modifier = self.infer_access_modifier(node);
        symbol.signature = Some(signature);
        symbol.is_virtual = is_virtual;
        symbol.is_override = is_override;
        symbol.is_final = is_final;
        symbol.is_const_method = is_const;
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);

        self.symbols.push(symbol);
    }

    fn extract_method(&mut self, node: Node, _parent: Node, tree: &SourceTree) {
        // Extract method from within a class body (function_declarator context)
        let name = self.extract_declarator_name(&node, tree);
        if name.is_empty() {
            return;
        }

        let kind = self.determine_method_kind(&name, &node, tree);

        let pos = node.start_position();
        let container = self.container_stack.last().cloned().unwrap_or_default();
        let qualified = format!("{}::{}", container, name);

        let mut symbol = Symbol::new(&qualified, kind);
        symbol.name = name;
        symbol.qualified_name = qualified;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.scope_kind = ScopeKind::Class;
        symbol.scope_depth = self.container_stack.len() as u32 + 1;
        symbol.container_name = Some(container);

        self.symbols.push(symbol);
    }

    // ─── C++ Namespace ────────────────────────────────────────────────

    fn extract_namespace(&mut self, node: Node, tree: &SourceTree) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_else(|| "_anonymous_".to_string());

        let pos = node.start_position();
        let end_pos = node.end_position();
        let qualified = self.make_qualified_name(&name);

        let mut symbol = Symbol::new(&qualified, SymbolKind::Namespace);
        symbol.name = name.clone();
        symbol.qualified_name = qualified;
        symbol.is_definition = true;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.line_end = end_pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.column_end = end_pos.column as u32;
        symbol.scope_kind = ScopeKind::Namespace;
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.container_name = self.container_stack.last().cloned();
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);

        self.symbols.push(symbol);

        // Recurse into namespace body
        self.container_stack.push(name.clone());
        let body = node.child_by_field_name("body");
        if let Some(body_node) = body {
            let mut cursor = body_node.walk();
            for child in body_node.named_children(&mut cursor) {
                self.walk_node(child, tree);
            }
        }
        self.container_stack.pop();
    }

    // ─── C++ Using ────────────────────────────────────────────────────

    fn extract_using_declaration(&mut self, node: Node, tree: &SourceTree) {
        // using Namespace::Name;
        let scope_resolution = node.child_by_field_name("declarator");
        if let Some(srd) = scope_resolution {
            let name = tree.node_text(&srd).to_string();
            let pos = srd.start_position();

            let mut symbol = Symbol::new(&name, SymbolKind::UsingAlias);
            symbol.file_path = self.file_path.clone();
            symbol.line_start = pos.row as u32;
            symbol.column_start = pos.column as u32;
            symbol.scope_kind = self.current_scope_kind();
            symbol.scope_depth = self.container_stack.len() as u32;

            self.symbols.push(symbol);
        }
    }

    fn extract_using_alias(&mut self, node: Node, tree: &SourceTree) {
        // using Alias = Type;
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| tree.node_text(&n).to_string()).unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let type_node = node.child_by_field_name("type");
        let alias_type = type_node.map(|n| tree.node_text(&n).to_string());

        let pos = node.start_position();
        let qualified = self.make_qualified_name(&name);

        let mut symbol = Symbol::new(&qualified, SymbolKind::UsingAlias);
        symbol.name = name;
        symbol.qualified_name = qualified;
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.scope_kind = self.current_scope_kind();
        symbol.scope_depth = self.container_stack.len() as u32;
        symbol.declared_type = alias_type;

        self.symbols.push(symbol);
    }

    // ─── Parameters ───────────────────────────────────────────────────

    fn extract_parameter(&mut self, node: Node, tree: &SourceTree) -> Option<Symbol> {
        // parameter_declaration -> decl_type + declarator
        let decl_type = node.child_by_field_name("decl_type");
        let declarator = node.child_by_field_name("declarator");

        let name = declarator
            .map(|d| self.extract_declarator_name(&d, tree))
            .unwrap_or_default();
        if name.is_empty() {
            return None;
        }

        let type_str = decl_type
            .map(|dt| tree.node_text(&dt).to_string())
            .unwrap_or_default();

        let pos = node.start_position();
        let container = self.container_stack.last().cloned().unwrap_or_default();

        let mut symbol = Symbol::new(name.clone(), SymbolKind::Parameter);
        symbol.file_path = self.file_path.clone();
        symbol.line_start = pos.row as u32;
        symbol.column_start = pos.column as u32;
        symbol.declared_type = Some(type_str);
        symbol.scope_kind = ScopeKind::Function;
        symbol.scope_depth = self.container_stack.len() as u32 + 1;
        symbol.container_name = if container.is_empty() {
            None
        } else {
            Some(container)
        };
        symbol.selection_line = Some(pos.row as u32);
        symbol.selection_column = Some(pos.column as u32);

        Some(symbol)
    }

    // ─── Helpers ──────────────────────────────────────────────────────

    fn parse_function_declarator(
        &self,
        declarator: Option<Node>,
        decl_type: Option<Node>,
        tree: &SourceTree,
    ) -> (String, Option<String>, Option<String>, Option<String>) {
        let name = declarator
            .map(|d| self.extract_declarator_name(&d, tree))
            .unwrap_or_default();

        let return_type = decl_type
            .map(|dt| tree.node_text(&dt).to_string())
            .filter(|s| !s.is_empty());

        let storage_class = return_type
            .as_ref()
            .and_then(|t| extract_storage_class(t));

        // Build signature
        let params_node = declarator
            .and_then(|d| d.child_by_field_name("parameters"))
            .and_then(|p| p.child_by_field_name("parameters"));

        let params_str = params_node.map(|p| tree.node_text(&p)).unwrap_or_default();
        let signature = if !name.is_empty() {
            let rt = return_type.as_deref().unwrap_or("");
            Some(format!("{} {}({})", rt, name, params_str))
        } else {
            None
        };

        (name, return_type, storage_class, signature)
    }

    fn extract_declarator_name(&self, node: &Node, tree: &SourceTree) -> String {
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" => {
                tree.node_text(node).to_string()
            }
            "function_declarator" => {
                node.child_by_field_name("declarator")
                    .map(|d| self.extract_declarator_name(&d, tree))
                    .unwrap_or_default()
            }
            // `(*name)(args)` — the inner declarator is an unnamed child, not a
            // `declarator` field, so peel via named_child(0). Without this,
            // function-pointer members (`int (*request)(void);`) yield no name
            // and are dropped entirely.
            "parenthesized_declarator" => node
                .named_child(0)
                .map(|d| self.extract_declarator_name(&d, tree))
                .unwrap_or_default(),
            "ptr_declarator"
            | "array_declarator"
            | "parameter_list"
            | "attributed_declarator"
            | "pointer_declarator" => {
                node.child_by_field_name("declarator")
                    .map(|d| self.extract_declarator_name(&d, tree))
                    .unwrap_or_default()
            }
            "operator_name" => tree.node_text(node).to_string(),
            "constructor_declarator" => {
                node.child_by_field_name("declarator")
                    .map(|d| self.extract_declarator_name(&d, tree))
                    .unwrap_or_default()
            }
            "destructor_declarator" => {
                let name_node = node.child_by_field_name("name");
                name_node.map(|n| format!("~{}", tree.node_text(&n))).unwrap_or_default()
            }
            "qualified_identifier" => tree.node_text(node).to_string(),
            "field_declarator" => {
                node.child_by_field_name("declarator")
                    .map(|d| self.extract_declarator_name(&d, tree))
                    .unwrap_or_default()
            }
            "init_declarator" => {
                node.child_by_field_name("declarator")
                    .map(|d| self.extract_declarator_name(&d, tree))
                    .unwrap_or_default()
            }
            "abstract_declarator" => String::new(),
            _ => {
                // Try to find an identifier child
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

    fn determine_method_kind(&self, name: &str, node: &Node, tree: &SourceTree) -> SymbolKind {
        if name.starts_with('~') {
            return SymbolKind::Destructor;
        }

        // Check if name matches container name (constructor)
        if let Some(container) = self.container_stack.last() {
            if name == container {
                return SymbolKind::Constructor;
            }
        }

        // Check for operator overloads
        if name.starts_with("operator") {
            return SymbolKind::Operator;
        }

        SymbolKind::Method
    }

    fn extract_method_qualifiers(
        &self,
        node: &Node,
        tree: &SourceTree,
    ) -> (bool, bool, bool, bool) {
        // Look for virtual, override, final, const qualifiers
        // These appear in the parent field_declaration context
        let mut is_virtual = false;
        let mut is_override = false;
        let mut is_final = false;
        let mut is_const = false;

        if let Some(parent) = node.parent() {
            let text = tree.node_text(&parent);
            is_virtual = text.contains("virtual");
            is_override = text.contains("override");
            is_final = text.contains("final");
            is_const = text.contains("const") && !text.contains("constexpr");
        }

        (is_virtual, is_override, is_final, is_const)
    }

    fn extract_base_classes(&self, node: &Node, tree: &SourceTree) -> Option<serde_json::Value> {
        let bases = node.child_by_field_name("bases");
        bases.map(|b| {
            let mut cursor = b.walk();
            let class_names: Vec<String> = b
                .named_children(&mut cursor)
                .filter_map(|child| {
                    let name_node = child.child_by_field_name("name");
                    name_node.map(|n| tree.node_text(&n).to_string())
                })
                .collect();
            serde_json::json!(class_names)
        })
    }

    fn infer_access_modifier(&self, node: Node) -> Option<String> {
        // Walk up to find the nearest access_specifier
        let mut current = node;
        while let Some(parent) = current.parent() {
            // Check siblings before this node for access_specifier
            let mut sibling = previous_sibling(&current);
            while let Some(s) = sibling {
                if s.kind() == "access_specifier" {
                    // The specifier text is the child identifier
                    if let Some(child) = s.child(0) {
                        return Some(child.kind().to_string());
                    }
                }
                sibling = previous_sibling(&s);
                break; // Only check immediate previous sibling
            }

            // Also check if there's an access_specifier as a direct sibling
            if let Some(prev) = previous_sibling(&current) {
                if prev.kind() == "access_specifier" {
                    if let Some(child) = prev.child(0) {
                        return Some(child.kind().to_string());
                    }
                }
            }

            current = parent;
        }
        None
    }

    fn is_inside_struct(&self, node: Node) -> bool {
        let mut current = node.parent();
        while let Some(parent) = current {
            match parent.kind() {
                "struct_body" | "class_body" | "enum_body" => return true,
                "translation_unit" | "program" => return false,
                _ => {}
            }
            current = parent.parent();
        }
        false
    }

    fn is_inside_function(&self, node: Node) -> bool {
        let mut current = node.parent();
        while let Some(parent) = current {
            match parent.kind() {
                "function_definition" | "compound_statement" => {
                    // Check if compound_statement is inside a function
                    if parent.kind() == "compound_statement" {
                        if let Some(gp) = parent.parent() {
                            if gp.kind() == "function_definition" {
                                return true;
                            }
                        }
                    } else {
                        return true;
                    }
                }
                "translation_unit" | "program" => return false,
                _ => {}
            }
            current = parent.parent();
        }
        false
    }

    fn current_scope_kind(&self) -> ScopeKind {
        if self.container_stack.is_empty() {
            return ScopeKind::Global;
        }
        // Peek at the last container to determine scope kind
        // We use a heuristic: if the last container was pushed by namespace -> namespace,
        // by class -> class, by struct -> struct, by enum -> enum, by function -> function
        ScopeKind::Block
    }

    fn make_qualified_name(&self, name: &str) -> String {
        if self.container_stack.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", self.container_stack.join("::"), name)
        }
    }

    fn extract_documentation(&self, node: Node, tree: &SourceTree) -> Option<String> {
        // Look for preceding comment block
        let prev = previous_sibling(&node);
        prev.as_ref().and_then(|p| {
            let text = tree.node_text(p);
            if text.starts_with("/**") || text.contains("*") {
                Some(
                    text.lines()
                        .map(|l| l.trim().strip_prefix('*').unwrap_or(l).trim())
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                None
            }
        })
    }
}

// ─── Utility functions ────────────────────────────────────────────────

fn previous_sibling<'a>(node: &'a Node<'a>) -> Option<Node<'a>> {
    node.prev_named_sibling().or_else(|| node.prev_sibling())
}

fn extract_storage_class(decl_type: &str) -> Option<String> {
    let tokens: Vec<&str> = decl_type.split_whitespace().collect();
    for token in tokens {
        match token {
            "static" | "extern" | "register" | "auto" | "thread_local" | "mutable" => {
                return Some(token.to_string());
            }
            _ => {}
        }
    }
    None
}

fn is_union_context(node: &Node) -> bool {
    // Check if this struct_specifier is preceded by "union" keyword
    // In tree-sitter-c, union types use struct_specifier with a preceding "union" token
    if let Some(prev) = node.prev_sibling() {
        if prev.kind() == "union_keyword" {
            return true;
        }
    }
    // Also check parent context
    if let Some(parent) = node.parent() {
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if std::ptr::eq(&child, node) {
                break;
            }
            if child.kind() == "union_keyword" {
                return true;
            }
        }
    }
    false
}

/// A struct/union/enum/class specifier without a body appears both as a genuine
/// forward declaration (`struct dmtimer;`) and as an elaborated type *usage*
/// embedded in another declaration (`struct dmtimer *timer`, parameters, field
/// types, casts, sizeof, ...). Only the standalone forward declaration should
/// become a symbol; usages are references, not declarations.
fn is_elaborated_type_usage(node: &Node) -> bool {
    let parent = match node.parent() {
        Some(p) => p,
        None => return false,
    };
    match parent.kind() {
        // `struct foo x;` has a declarator (usage); `struct foo;` does not
        // (genuine forward declaration).
        "declaration" => parent.child_by_field_name("declarator").is_some(),
        // Anywhere else — parameter_declaration, field_declaration,
        // type_definition, function return type, cast, sizeof — it is a usage.
        _ => true,
    }
}

fn get_anonymous_type_name(node: &Node, tree: &SourceTree) -> String {
    // For anonymous types, try to find a declarator that names them
    if let Some(parent) = node.parent() {
        // Anonymous struct inside a typedef: typedef struct { ... } MyType;
        if parent.kind() == "type_identifier" {
            return tree.node_text(&parent).to_string();
        }
        // Anonymous struct inside init_declarator
        if parent.kind() == "type_descriptor" {
            if let Some(gp) = parent.parent() {
                if gp.kind() == "decl_type" {
                    if let Some(ggp) = gp.parent() {
                        if ggp.kind() == "type_definition" {
                            if let Some(decl) = ggp.child_by_field_name("declarator") {
                                return extract_declarator_name_from_node(&decl, tree);
                            }
                        }
                    }
                }
            }
        }
    }
    // Fallback: generate a unique name
    let (line, col) = (node.start_position().row as u32, node.start_position().column as u32);
    format!("_anonymous_{}_{}", line, col)
}

fn extract_declarator_name_from_node(node: &Node, tree: &SourceTree) -> String {
    match node.kind() {
        "identifier" => tree.node_text(node).to_string(),
        "ptr_declarator" | "array_declarator" | "parameter_list" => {
            node.child_by_field_name("declarator")
                .map(|d| extract_declarator_name_from_node(&d, tree))
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
