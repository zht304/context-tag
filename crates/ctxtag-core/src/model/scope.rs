use super::symbol::{ScopeKind, Symbol};

/// Represents a scope in the scope chain.
#[derive(Debug, Clone)]
pub struct Scope {
    pub kind: ScopeKind,
    pub name: String,
    pub depth: u32,
    /// Symbols declared directly in this scope.
    pub symbols: Vec<Symbol>,
    /// Child scopes.
    pub children: Vec<Scope>,
}

impl Scope {
    pub fn new(kind: ScopeKind, name: impl Into<String>, depth: u32) -> Self {
        Self {
            kind,
            name: name.into(),
            depth,
            symbols: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Lookup a symbol by name in this scope only.
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().rev().find(|s| s.name == name)
    }

    /// Walk up the scope chain (this scope + all ancestors) to find a symbol.
    pub fn lookup_chain<'a>(scopes: &'a [Scope], name: &str) -> Option<&'a Symbol> {
        for scope in scopes.iter().rev() {
            if let Some(sym) = scope.lookup(name) {
                return Some(sym);
            }
        }
        None
    }
}
