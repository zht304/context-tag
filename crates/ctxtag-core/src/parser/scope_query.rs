//! Query-time local-binding resolution.
//!
//! Go-to-definition resolves a bare word by name against the project symbol
//! table. When the word is a function-local variable or parameter, that name
//! lookup wrongly matches unrelated globals (e.g. a local `attribute` matching
//! `struct attribute`). Locals are intentionally not stored as symbols, so this
//! module re-derives the binding by parsing the single file on demand: if the
//! identifier at the cursor is bound by a declaration or parameter of the
//! enclosing function, we return that declaration's location instead.

use std::collections::HashSet;

use tree_sitter::Node;

use super::tree::{LanguageKind, SourceTree};

/// Location of a local declaration, in 0-based line/column (matching the
/// tree-sitter positions the rest of the codebase reports to the editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDecl {
    pub line_start: u32,
    pub col_start: u32,
    pub line_end: u32,
    pub col_end: u32,
}

/// A candidate binding: (scope depth, declaration start position, location).
/// Deeper blocks and later declarations win, so an inner binding shadows an
/// outer one and any body declaration shadows a parameter (depth -1).
type Candidate = (i32, (usize, usize), LocalDecl);

/// If the identifier `name` at `(line, col)` is bound by a local declaration or
/// a parameter of the enclosing function, return that declaration's identifier
/// location. Returns `None` when the name is not a function-local at this
/// position, so the caller falls back to project-wide symbol lookup.
pub fn find_local_declaration(
    source: &[u8],
    language: LanguageKind,
    line: u32,
    col: u32,
    name: &str,
) -> Option<LocalDecl> {
    let tree = SourceTree::parse(language, source).ok()?;
    let root = tree.root_node();
    let cursor = tree_sitter::Point {
        row: line as usize,
        column: col as usize,
    };
    let cursor_node = root.descendant_for_point_range(cursor, cursor)?;

    // C/C++ functions don't nest, so the nearest enclosing function_definition
    // is the whole scope for locals and parameters.
    let chain = ancestor_chain(cursor_node);
    let func = chain
        .iter()
        .find(|n| n.kind() == "function_definition")
        .copied()?;

    // Ancestor ids of the cursor, used to test whether a declaration's block
    // actually contains the cursor position.
    let cursor_ancestors: HashSet<usize> = chain.iter().map(|n| n.id()).collect();

    let mut candidates: Vec<Candidate> = Vec::new();

    // Signature parameters: in scope throughout the function body.
    if let Some(declarator) = func.child_by_field_name("declarator") {
        collect_params(declarator, &tree, name, &mut candidates);
    }

    // Body declarations: in scope only when the enclosing block contains the
    // cursor and the declaration precedes it.
    if let Some(body) = func.child_by_field_name("body") {
        collect_body_decls(
            body,
            &tree,
            name,
            &cursor_ancestors,
            (cursor.row, cursor.column),
            &mut candidates,
        );
    }

    candidates
        .into_iter()
        .max_by_key(|(depth, start, _)| (*depth, *start))
        .map(|(_, _, loc)| loc)
}

/// Walk a function declarator's parameter list, adding each parameter whose
/// declared name matches `name` as a depth -1 (function-scoped) candidate.
fn collect_params<'t>(
    node: Node<'t>,
    tree: &SourceTree,
    name: &str,
    out: &mut Vec<Candidate>,
) {
    if node.kind() == "parameter_declaration" {
        if let Some(ident) = declared_ident_matching(node, tree, name) {
            let s = node.start_position();
            out.push((-1, (s.row, s.column), loc_of(ident)));
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_params(child, tree, name, out);
    }
}

/// Walk a function body, adding each `declaration` of `name` that is in scope
/// at the cursor (its enclosing block contains the cursor and it starts at or
/// before the cursor).
fn collect_body_decls<'t>(
    node: Node<'t>,
    tree: &SourceTree,
    name: &str,
    cursor_ancestors: &HashSet<usize>,
    cursor: (usize, usize),
    out: &mut Vec<Candidate>,
) {
    if node.kind() == "declaration" {
        let start = node.start_position();
        if (start.row, start.column) <= cursor {
            if let Some(ident) = declared_ident_matching(node, tree, name) {
                if let Some(block) = enclosing_block(node) {
                    if cursor_ancestors.contains(&block.id()) {
                        out.push((block_depth(block), (start.row, start.column), loc_of(ident)));
                    }
                }
            }
        }
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        collect_body_decls(child, tree, name, cursor_ancestors, cursor, out);
    }
}

/// The innermost declared identifier of a declaration/parameter whose name
/// matches `name`. Handles pointer, array, parenthesized and initializer
/// declarator wrappers.
fn declared_ident_matching<'t>(node: Node<'t>, tree: &SourceTree, name: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    for d in node.children_by_field_name("declarator", &mut cursor) {
        if let Some(ident) = declarator_ident(d) {
            if tree.node_text(&ident) == name {
                return Some(ident);
            }
        }
    }
    None
}

fn declarator_ident<'t>(node: Node<'t>) -> Option<Node<'t>> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => Some(node),
        "parenthesized_declarator" => node.named_child(0).and_then(declarator_ident),
        "pointer_declarator" | "array_declarator" | "attributed_declarator"
        | "init_declarator" => node
            .child_by_field_name("declarator")
            .and_then(declarator_ident),
        _ => None,
    }
}

/// Nearest enclosing `compound_statement` (block) of a node.
fn enclosing_block<'t>(node: Node<'t>) -> Option<Node<'t>> {
    ancestor_chain(node)
        .into_iter()
        .skip(1)
        .find(|n| n.kind() == "compound_statement")
}

/// Nesting depth of a block, counted in enclosing `compound_statement` nodes.
/// Only the relative ordering matters (inner blocks outrank outer ones).
fn block_depth(block: Node) -> i32 {
    ancestor_chain(block)
        .into_iter()
        .filter(|n| n.kind() == "compound_statement")
        .count() as i32
}

/// A node and all of its ancestors, innermost first.
fn ancestor_chain<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        out.push(n);
        cur = n.parent();
    }
    out
}

fn loc_of(ident: Node) -> LocalDecl {
    let s = ident.start_position();
    let e = ident.end_position();
    LocalDecl {
        line_start: s.row as u32,
        col_start: s.column as u32,
        line_end: e.row as u32,
        col_end: e.column as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
struct attribute { int x; };
int global_counter;

void use(struct attribute *attribute, int n) {
    attribute->x = n;
    {
        int inner = 5;
        inner++;
    }
    global_counter = n;
}
";

    fn pos(line_idx: usize, needle: &str) -> (u32, u32) {
        let line = SRC.lines().nth(line_idx).unwrap();
        (line_idx as u32, line.find(needle).unwrap() as u32)
    }

    fn text_at(loc: LocalDecl) -> String {
        let line = SRC.lines().nth(loc.line_start as usize).unwrap();
        line[loc.col_start as usize..loc.col_end as usize].to_string()
    }

    #[test]
    fn resolves_parameter_over_same_named_global() {
        // `attribute` used in the body binds to the parameter, not the struct.
        let (l, c) = pos(4, "attribute");
        let loc = find_local_declaration(SRC.as_bytes(), LanguageKind::C, l, c, "attribute")
            .expect("parameter should resolve");
        assert_eq!(loc.line_start, 3, "jumps to the signature line");
        assert_eq!(text_at(loc), "attribute");
    }

    #[test]
    fn resolves_block_local() {
        let (l, c) = pos(7, "inner");
        let loc = find_local_declaration(SRC.as_bytes(), LanguageKind::C, l, c, "inner")
            .expect("block local should resolve");
        assert_eq!(loc.line_start, 6);
        assert_eq!(text_at(loc), "inner");
    }

    #[test]
    fn global_is_not_a_local() {
        let (l, c) = pos(9, "global_counter");
        assert!(find_local_declaration(
            SRC.as_bytes(),
            LanguageKind::C,
            l,
            c,
            "global_counter"
        )
        .is_none());
    }

    #[test]
    fn declaration_after_use_is_not_in_scope() {
        let src = "void f(void) {\n    y = 1;\n    int y = 2;\n}\n";
        // Cursor on the first `y` (line 1); the declaration is on line 2.
        let loc = find_local_declaration(src.as_bytes(), LanguageKind::C, 1, 4, "y");
        assert!(loc.is_none());
    }

    #[test]
    fn inner_block_shadows_outer() {
        let src = "\
void f(void) {
    int v = 1;
    v++;
    {
        int v = 2;
        v++;
    }
}
";
        // Outer use (line 2) binds to the outer declaration (line 1).
        let outer = find_local_declaration(src.as_bytes(), LanguageKind::C, 2, 4, "v").unwrap();
        assert_eq!(outer.line_start, 1);
        // Inner use (line 5) binds to the inner declaration (line 4).
        let inner = find_local_declaration(src.as_bytes(), LanguageKind::C, 5, 8, "v").unwrap();
        assert_eq!(inner.line_start, 4);
    }
}

