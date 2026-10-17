use crate::model::{Symbol, SymbolKind};
use std::collections::HashMap;

/// Resolves typedef chains to their underlying types.
///
/// Builds a map of typedef alias -> underlying type by analyzing
/// type definition symbols, and follows chains to find the
/// ultimate base type.
///
/// Example:
/// ```ignore
/// typedef int32_t my_int;       // my_int -> int32_t
/// typedef my_int my_counter;    // my_counter -> my_int -> int32_t
/// ```
/// After resolution:
/// - my_int => int32_t
/// - my_counter => int32_t
pub struct TypeResolver {
    /// Direct typedef mappings: alias -> immediate underlying type.
    typedef_map: HashMap<String, String>,
    /// Resolution cache: alias -> fully resolved base type.
    resolved_cache: HashMap<String, String>,
}

impl TypeResolver {
    pub fn new() -> Self {
        Self {
            typedef_map: HashMap::new(),
            resolved_cache: HashMap::new(),
        }
    }

    /// Build the typedef map from a list of symbols.
    ///
    /// Extracts all `Typedef` kind symbols and maps their names
    /// to their declared types.
    pub fn build(&mut self, symbols: &[Symbol]) {
        self.typedef_map.clear();
        self.resolved_cache.clear();

        for sym in symbols {
            if sym.kind == SymbolKind::Typedef {
                if let Some(declared_type) = &sym.declared_type {
                    // The declared_type may contain extra keywords like "struct", "enum", etc.
                    let cleaned_type = self.clean_type_string(declared_type);

                    // Index by both simple name and qualified name
                    self.typedef_map.insert(sym.name.clone(), cleaned_type.clone());
                    if sym.qualified_name != sym.name {
                        self.typedef_map.insert(sym.qualified_name.clone(), cleaned_type);
                    }
                }
            } else if sym.kind == SymbolKind::UsingAlias {
                if let Some(declared_type) = &sym.declared_type {
                    let cleaned_type = self.clean_type_string(declared_type);
                    self.typedef_map.insert(sym.name.clone(), cleaned_type.clone());
                    if sym.qualified_name != sym.name {
                        self.typedef_map.insert(sym.qualified_name.clone(), cleaned_type);
                    }
                }
            }
        }
    }

    /// Resolve a single type name to its underlying base type.
    ///
    /// Follows the typedef chain until reaching a type that is not
    /// itself a typedef alias. Returns the original name if no
    /// resolution is found.
    pub fn resolve(&mut self, type_name: &str) -> String {
        // Check cache first
        if let Some(resolved) = self.resolved_cache.get(type_name) {
            return resolved.clone();
        }

        // Follow the chain with cycle detection
        let mut current = type_name.to_string();
        let mut visited = Vec::new();

        while let Some(underlying) = self.typedef_map.get(&current) {
            // Cycle detection
            if visited.contains(&underlying.as_str()) {
                // Circular typedef — return current
                break;
            }
            visited.push(underlying.as_str());
            current = underlying.clone();
        }

        // Cache the result
        self.resolved_cache.insert(type_name.to_string(), current.clone());
        current
    }

    /// Resolve all known typedef aliases and return the complete map.
    pub fn resolve_all(&mut self) -> HashMap<String, String> {
        let aliases: Vec<String> = self.typedef_map.keys().cloned().collect();
        let mut resolved = HashMap::new();

        for alias in &aliases {
            let base = self.resolve(alias);
            // Only record if resolution changed the type (i.e., it was a chain)
            if base != *alias {
                resolved.insert(alias.clone(), base);
            }
        }

        resolved
    }

    /// Get the raw typedef map without resolution.
    pub fn typedef_map(&self) -> &HashMap<String, String> {
        &self.typedef_map
    }

    /// Check if a type name is a known typedef alias.
    pub fn is_typedef(&self, type_name: &str) -> bool {
        self.typedef_map.contains_key(type_name)
    }

    /// Get the resolution depth (chain length) for a type.
    pub fn resolution_depth(&self, type_name: &str) -> usize {
        let mut current = type_name;
        let mut depth = 0;
        let mut visited = Vec::new();

        while let Some(underlying) = self.typedef_map.get(current) {
            if visited.contains(&underlying.as_str()) {
                break;
            }
            visited.push(underlying.as_str());
            current = underlying;
            depth += 1;
        }

        depth
    }

    /// Print the resolution chain for debugging.
    pub fn resolution_chain(&self, type_name: &str) -> Vec<String> {
        let mut chain = vec![type_name.to_string()];
        let mut current = type_name;
        let mut visited = Vec::new();

        while let Some(underlying) = self.typedef_map.get(current) {
            if visited.contains(&underlying.as_str()) {
                chain.push("(circular)".to_string());
                break;
            }
            visited.push(underlying.as_str());
            chain.push(underlying.clone());
            current = underlying;
        }

        chain
    }

    // ─── Type String Cleaning ─────────────────────────────────────────

    /// Clean a type string by removing elaborated-type keywords and
    /// cv-qualifiers, leaving the bare type name(s).
    fn clean_type_string(&self, type_str: &str) -> String {
        // Word-level filtering so any ordering works (e.g. "const struct Foo").
        type_str
            .split_whitespace()
            .filter(|w| {
                !matches!(
                    *w,
                    "struct" | "enum" | "union" | "class" | "const" | "volatile" | "restrict" | "inline"
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Try to extract a simple type name from a complex type descriptor.
    ///
    /// Handles cases like:
    /// - "const int *" -> "int"
    /// - "struct Foo" -> "Foo"
    /// - "unsigned long long" -> "unsigned_long_long"
    pub fn extract_simple_type_name(type_str: &str) -> String {
        // Strip trailing pointer/reference indicators, then filter out
        // elaborated-type keywords and cv-qualifiers at the word level.
        let cleaned = type_str.trim().trim_end_matches('*').trim_end_matches('&').trim();

        let filtered: Vec<&str> = cleaned
            .split_whitespace()
            .filter(|w| {
                !matches!(
                    *w,
                    "struct" | "enum" | "union" | "class" | "const" | "volatile" | "restrict"
                )
            })
            .collect();

        // For compound types (e.g., "unsigned int"), join with underscore
        if filtered.len() > 1 {
            filtered.join("_")
        } else {
            filtered.join(" ")
        }
    }
}

impl Default for TypeResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_typedef(name: &str, declared_type: &str) -> Symbol {
        let mut sym = Symbol::new(name, SymbolKind::Typedef);
        sym.qualified_name = name.to_string();
        sym.declared_type = Some(declared_type.to_string());
        sym
    }

    #[test]
    fn test_simple_typedef() {
        let resolver = &mut TypeResolver::new();
        let symbols = vec![make_typedef("my_int", "int")];
        resolver.build(&symbols);

        assert_eq!(resolver.resolve("my_int"), "int");
        assert_eq!(resolver.resolve("unknown"), "unknown");
    }

    #[test]
    fn test_typedef_chain() {
        let resolver = &mut TypeResolver::new();
        let symbols = vec![
            make_typedef("my_int", "int"),
            make_typedef("my_counter", "my_int"),
            make_typedef("pixel_count", "my_counter"),
        ];
        resolver.build(&symbols);

        assert_eq!(resolver.resolve("my_int"), "int");
        assert_eq!(resolver.resolve("my_counter"), "int");
        assert_eq!(resolver.resolve("pixel_count"), "int");
    }

    #[test]
    fn test_chain_depth() {
        let resolver = &mut TypeResolver::new();
        let symbols = vec![
            make_typedef("a", "int"),
            make_typedef("b", "a"),
            make_typedef("c", "b"),
        ];
        resolver.build(&symbols);

        assert_eq!(resolver.resolution_depth("a"), 1);
        assert_eq!(resolver.resolution_depth("b"), 2);
        assert_eq!(resolver.resolution_depth("c"), 3);
    }

    #[test]
    fn test_resolution_chain() {
        let resolver = &mut TypeResolver::new();
        let symbols = vec![
            make_typedef("a", "int"),
            make_typedef("b", "a"),
        ];
        resolver.build(&symbols);

        let chain = resolver.resolution_chain("b");
        assert_eq!(chain, vec!["b", "a", "int"]);
    }

    #[test]
    fn test_struct_typedef() {
        let resolver = &mut TypeResolver::new();
        let symbols = vec![make_typedef("Point", "struct PointImpl")];
        resolver.build(&symbols);

        assert_eq!(resolver.resolve("Point"), "PointImpl");
    }

    #[test]
    fn test_extract_simple_type_name() {
        assert_eq!(TypeResolver::extract_simple_type_name("const int *"), "int");
        assert_eq!(TypeResolver::extract_simple_type_name("struct Foo"), "Foo");
        assert_eq!(TypeResolver::extract_simple_type_name("struct Foo *"), "Foo");
        assert_eq!(
            TypeResolver::extract_simple_type_name("unsigned long long"),
            "unsigned_long_long"
        );
    }
}
