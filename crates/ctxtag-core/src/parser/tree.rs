use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};

/// Language kind detected from file extension or explicitly set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageKind {
    C,
    Cpp,
}

impl LanguageKind {
    /// Detect language from a file path's extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        match ext {
            "c" | "h" => Some(LanguageKind::C),
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hxx" | "hh" | "h++" => Some(LanguageKind::Cpp),
            _ => None,
        }
    }

    /// Get the tree-sitter Language for this language kind.
    pub fn language(self) -> Language {
        match self {
            LanguageKind::C => get_c_language(),
            LanguageKind::Cpp => get_cpp_language(),
        }
    }
}

/// Load the C grammar.
///
/// The grammar is compiled as a static library by build.rs (`libtree-sitter-c.a`).
/// The symbol `tree_sitter_c` is exported by the compiled `parser.c`.
fn get_c_language() -> Language {
    extern "C" {
        fn tree_sitter_c() -> *const ();
    }
    // SAFETY: The grammar library is compiled and linked by build.rs.
    unsafe { Language::from_raw(tree_sitter_c() as *const tree_sitter::ffi::TSLanguage) }
}

/// Load the C++ grammar.
///
/// The grammar is compiled as a static library by build.rs (`libtree-sitter-cpp.a`).
fn get_cpp_language() -> Language {
    extern "C" {
        fn tree_sitter_cpp() -> *const ();
    }
    // SAFETY: Same as get_c_language.
    unsafe { Language::from_raw(tree_sitter_cpp() as *const tree_sitter::ffi::TSLanguage) }
}

/// A parsed source file wrapped around tree-sitter structures.
///
/// Provides convenient access to the syntax tree and source content.
pub struct SourceTree {
    parser: Parser,
    tree: Tree,
    source: Vec<u8>,
    file_path: String,
    language: LanguageKind,
}

impl SourceTree {
    /// Parse source code with the given language kind.
    pub fn parse(language: LanguageKind, source: &[u8]) -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&language.language())
            .context("Failed to set parser language")?;

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("Failed to parse source code"))?;

        Ok(Self {
            parser,
            tree,
            source: source.to_vec(),
            file_path: String::new(),
            language,
        })
    }

    /// Parse source code using an existing parser (for thread-local parser reuse).
    pub fn parse_with_parser(parser: &mut Parser, language: LanguageKind, source: &[u8]) -> Result<Self> {
        parser
            .set_language(&language.language())
            .context("Failed to set parser language")?;

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("Failed to parse source code"))?;

        // We need to keep a parser in the SourceTree for reparsing,
        // so create a minimal one (the caller's parser is reused for the next file).
        let mut inner_parser = Parser::new();
        inner_parser.set_language(&language.language()).ok();

        Ok(Self {
            parser: inner_parser,
            tree,
            source: source.to_vec(),
            file_path: String::new(),
            language,
        })
    }

    /// Parse source code, detecting language from file extension.
    pub fn parse_from_path(path: &Path, source: &[u8]) -> Result<Self> {
        let language = LanguageKind::from_path(path)
            .ok_or_else(|| anyhow!("Cannot detect language from file extension: {}", path.display()))?;
        let mut st = Self::parse(language, source)?;
        st.file_path = path.display().to_string();
        Ok(st)
    }

    /// Parse source code from file on disk.
    pub fn parse_file(path: &Path) -> Result<Self> {
        let source = std::fs::read(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;
        Self::parse_from_path(path, &source)
    }

    /// Re-parse updated source content (incremental).
    pub fn reparse(&mut self, new_source: &[u8], _old_source: Option<&[u8]>) -> Result<&Tree> {
        self.tree = self
            .parser
            .parse(new_source, None)
            .ok_or_else(|| anyhow!("Failed to reparse source code"))?;
        self.source = new_source.to_vec();
        Ok(&self.tree)
    }

    /// Get the root node of the syntax tree.
    pub fn root_node(&self) -> tree_sitter::Node {
        self.tree.root_node()
    }

    /// Get the tree-sitter Tree.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Get the source bytes.
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// Get the source as a string (lossy UTF-8 conversion).
    pub fn source_str(&self) -> &str {
        std::str::from_utf8(&self.source).unwrap_or("")
    }

    /// Get the file path if set.
    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    /// Set the file path.
    pub fn set_file_path(&mut self, path: String) {
        self.file_path = path;
    }

    /// Get the detected language kind.
    pub fn language(&self) -> LanguageKind {
        self.language
    }

    /// Convert a byte offset to a (line, column) pair, both 0-based.
    pub fn offset_to_position(&self, byte_offset: usize) -> (u32, u32) {
        let source = &self.source[..byte_offset.min(self.source.len())];
        let line = source.iter().filter(|&&b| b == b'\n').count();
        let column = source.iter().rposition(|&b| b == b'\n')
            .map(|i| source.len() - i - 1)
            .unwrap_or(source.len());
        (line as u32, column as u32)
    }

    /// Extract source text for a node.
    pub fn node_text(&self, node: &tree_sitter::Node) -> &str {
        node.utf8_text(&self.source).unwrap_or("")
    }

    /// Check if parsing produced any errors.
    pub fn has_errors(&self) -> bool {
        self.root_node().has_error()
    }
}
