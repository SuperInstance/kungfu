pub mod rust_parser;
pub mod typescript_parser;
pub mod python_parser;
pub mod go_parser;

use anyhow::{bail, Result};
use kungfu_types::file::Language;
use kungfu_types::symbol::Symbol;

/// Raw import extracted from source code.
#[derive(Debug, Clone)]
pub struct RawImport {
    /// The import path as written in source (e.g. "crate::scanner", "./bar", "fmt").
    pub path: String,
    /// Specific names imported (e.g. ["Result", "Context"]), empty for wildcard/module imports.
    pub names: Vec<String>,
    /// Line number of the import statement.
    pub line: usize,
}

pub struct ParseResult {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<RawImport>,
}

pub struct Parser {
    ts_parser: tree_sitter::Parser,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            ts_parser: tree_sitter::Parser::new(),
        }
    }

    pub fn extract_symbols(
        &mut self,
        source: &str,
        language: Language,
        file_id: &str,
        file_path: &str,
    ) -> Result<Vec<Symbol>> {
        Ok(self.parse(source, language, file_id, file_path)?.symbols)
    }

    pub fn parse(
        &mut self,
        source: &str,
        language: Language,
        file_id: &str,
        file_path: &str,
    ) -> Result<ParseResult> {
        let ts_language = match language {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            _ => bail!("no parser for language: {}", language),
        };

        self.ts_parser.set_language(&ts_language)?;

        let tree = self
            .ts_parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("failed to parse {}", file_path))?;

        let root = tree.root_node();

        let (symbols, imports) = match language {
            Language::Rust => (
                rust_parser::extract(root, source, file_id, file_path),
                rust_parser::extract_imports(root, source),
            ),
            Language::TypeScript | Language::JavaScript => (
                typescript_parser::extract(root, source, file_id, file_path),
                typescript_parser::extract_imports(root, source),
            ),
            Language::Python => (
                python_parser::extract(root, source, file_id, file_path),
                python_parser::extract_imports(root, source),
            ),
            Language::Go => (
                go_parser::extract(root, source, file_id, file_path),
                go_parser::extract_imports(root, source),
            ),
            _ => (Vec::new(), Vec::new()),
        };

        Ok(ParseResult { symbols, imports })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_and_imports_present() {
        let mut parser = Parser::new();
        let result = parser.parse(r#"
use std::path::Path;

pub fn hello() {
    println!("hi");
}

struct Foo {
    x: i32,
}
"#, Language::Rust, "f:test", "test.rs").unwrap();
        assert!(result.symbols.iter().any(|s| s.name == "hello"));
        assert!(result.symbols.iter().any(|s| s.name == "Foo"));
        assert!(!result.imports.is_empty());
    }
}
