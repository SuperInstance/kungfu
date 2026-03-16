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

/// Raw function call extracted from a symbol body.
#[derive(Debug, Clone)]
pub struct RawCall {
    /// Symbol ID of the caller (the function/method containing the call).
    pub caller_id: String,
    /// Name of the called function/method (last segment, e.g. "foo" from "self.foo()").
    pub callee_name: String,
    /// Line number of the call.
    pub line: usize,
}

pub struct ParseResult {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<RawImport>,
    pub calls: Vec<RawCall>,
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

        let calls = extract_calls(&root, source, &symbols);

        Ok(ParseResult { symbols, imports, calls })
    }
}

/// Extract function calls from symbol bodies using tree-sitter.
/// Works across all languages by looking for call_expression/call nodes.
fn extract_calls(root: &tree_sitter::Node, source: &str, symbols: &[Symbol]) -> Vec<RawCall> {
    let mut calls = Vec::new();

    // Build line→symbol_id mapping for resolving which symbol contains a call
    let mut line_to_sym: Vec<(&str, usize, usize)> = symbols
        .iter()
        .filter(|s| matches!(
            s.kind,
            kungfu_types::symbol::SymbolKind::Function
                | kungfu_types::symbol::SymbolKind::Method
        ))
        .map(|s| (s.id.as_str(), s.span.start_line, s.span.end_line))
        .collect();
    // Sort by span size (smallest first) so we find the most specific containing function
    line_to_sym.sort_by_key(|&(_, start, end)| end - start);

    let mut seen = std::collections::HashSet::new();
    collect_calls(root, source, &line_to_sym, &mut calls, &mut seen);
    calls
}

fn collect_calls(
    node: &tree_sitter::Node,
    source: &str,
    line_to_sym: &[(&str, usize, usize)],
    calls: &mut Vec<RawCall>,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let kind = node.kind();
    // call_expression (Rust, Go, TS, JS) or call (Python)
    if kind == "call_expression" || kind == "call" {
        if let Some(callee_name) = extract_callee_name(node, source) {
            let line = node.start_position().row + 1;

            // Find containing symbol (smallest span that contains this line)
            if let Some(&(caller_id, _, _)) = line_to_sym
                .iter()
                .find(|&&(_, start, end)| line >= start && line <= end)
            {
                let key = (caller_id.to_string(), callee_name.clone());
                if seen.insert(key) {
                    calls.push(RawCall {
                        caller_id: caller_id.to_string(),
                        callee_name,
                        line,
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls(&child, source, line_to_sym, calls, seen);
    }
}

/// Extract the callee function name from a call expression node.
/// Handles: foo(), self.foo(), obj.method(), Module::func(), etc.
fn extract_callee_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let func_node = node.child_by_field_name("function")?;
    let text = source.get(func_node.start_byte()..func_node.end_byte())?;

    // Get the last segment: "self.foo" → "foo", "Module::bar" → "bar", "foo" → "foo"
    let name = text
        .rsplit_once('.')
        .map(|(_, r)| r)
        .or_else(|| text.rsplit_once("::").map(|(_, r)| r))
        .unwrap_or(text)
        .trim();

    // Filter out noise: too short, numeric, or common non-function patterns
    if name.is_empty() || name.len() > 60 || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    Some(name.to_string())
}
