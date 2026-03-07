pub mod rust_parser;
pub mod typescript_parser;
pub mod python_parser;
pub mod go_parser;

use anyhow::{bail, Result};
use kungfu_types::file::Language;
use kungfu_types::symbol::Symbol;

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

        let symbols = match language {
            Language::Rust => rust_parser::extract(root, source, file_id, file_path),
            Language::TypeScript | Language::JavaScript => {
                typescript_parser::extract(root, source, file_id, file_path)
            }
            Language::Python => python_parser::extract(root, source, file_id, file_path),
            Language::Go => go_parser::extract(root, source, file_id, file_path),
            _ => Vec::new(),
        };

        Ok(symbols)
    }
}
