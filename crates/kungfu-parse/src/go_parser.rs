use kungfu_types::symbol::{Span, Symbol, SymbolKind};
use tree_sitter::Node;

pub fn extract(root: Node, source: &str, file_id: &str, file_path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let name_str = node_text(name, source);
                    let span = node_span(&child);
                    let id = format!("s:{}:{}:{}", file_id, span.start_line, &name_str);
                    symbols.push(Symbol {
                        id,
                        file_id: file_id.to_string(),
                        name: name_str.clone(),
                        kind: SymbolKind::Function,
                        language: "go".to_string(),
                        path: file_path.to_string(),
                        signature: extract_func_sig(&child, source),
                        span,
                        parent_symbol_id: None,
                        exported: name_str.chars().next().map_or(false, |c| c.is_uppercase()),
                        visibility: None,
                        doc_summary: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let name_str = node_text(name, source);
                    let span = node_span(&child);
                    let id = format!("s:{}:{}:{}", file_id, span.start_line, &name_str);
                    symbols.push(Symbol {
                        id,
                        file_id: file_id.to_string(),
                        name: name_str.clone(),
                        kind: SymbolKind::Method,
                        language: "go".to_string(),
                        path: file_path.to_string(),
                        signature: extract_func_sig(&child, source),
                        span,
                        parent_symbol_id: None,
                        exported: name_str.chars().next().map_or(false, |c| c.is_uppercase()),
                        visibility: None,
                        doc_summary: None,
                    });
                }
            }
            "type_declaration" => {
                let mut inner_cursor = child.walk();
                for spec in child.children(&mut inner_cursor) {
                    if spec.kind() == "type_spec" {
                        if let Some(name) = spec.child_by_field_name("name") {
                            let name_str = node_text(name, source);
                            let type_node = spec.child_by_field_name("type");
                            let sk = match type_node.map(|t| t.kind()) {
                                Some("struct_type") => SymbolKind::Struct,
                                Some("interface_type") => SymbolKind::Interface,
                                _ => SymbolKind::TypeAlias,
                            };
                            let span = node_span(&spec);
                            let id = format!("s:{}:{}:{}", file_id, span.start_line, &name_str);
                            symbols.push(Symbol {
                                id,
                                file_id: file_id.to_string(),
                                name: name_str.clone(),
                                kind: sk,
                                language: "go".to_string(),
                                path: file_path.to_string(),
                                signature: Some(format!("type {}", node_text(spec, source).lines().next().unwrap_or(""))),
                                span,
                                parent_symbol_id: None,
                                exported: name_str.chars().next().map_or(false, |c| c.is_uppercase()),
                                visibility: None,
                                doc_summary: None,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    symbols
}

fn extract_func_sig(node: &Node, source: &str) -> Option<String> {
    let start = node.start_byte();
    if let Some(body) = node.child_by_field_name("body") {
        let end = body.start_byte();
        Some(source[start..end].trim().to_string())
    } else {
        let end = node.end_byte().min(start + 200);
        let text = &source[start..safe_char_boundary(source, end)];
        Some(text.lines().next().unwrap_or(text).trim().to_string())
    }
}

fn node_text(node: Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn safe_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn node_span(node: &Node) -> Span {
    Span {
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_col: node.start_position().column,
        end_col: node.end_position().column,
    }
}
