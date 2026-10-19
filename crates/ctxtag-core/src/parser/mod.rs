pub mod tree;
pub mod extractor;
pub mod scope_builder;
pub mod resolver;
pub mod include_tracker;
pub mod macro_tracker;
pub mod call_graph;
pub mod type_resolver;
pub mod scope_query;

pub use tree::{SourceTree, LanguageKind};
pub use extractor::SymbolExtractor;
pub use scope_builder::ScopeBuilder;
pub use resolver::ReferenceResolver;
pub use include_tracker::IncludeTracker;
pub use macro_tracker::MacroTracker;
pub use call_graph::CallGraph;
pub use type_resolver::TypeResolver;
pub use scope_query::{find_local_declaration, LocalDecl};
