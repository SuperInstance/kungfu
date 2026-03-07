use kungfu_types::symbol::{Span, Symbol, SymbolKind};
use tree_sitter::Node;

pub fn extract(root: Node, source: &str, file_id: &str, file_path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        extract_node(child, source, file_id, file_path, None, &mut symbols);
    }

    symbols
}

fn extract_node(
    node: Node,
    source: &str,
    file_id: &str,
    file_path: &str,
    parent_id: Option<&str>,
    symbols: &mut Vec<Symbol>,
) {
    let kind = node.kind();

    let symbol_kind = match kind {
        "function_item" => Some(SymbolKind::Function),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        "impl_item" => Some(SymbolKind::Impl),
        "type_item" => Some(SymbolKind::TypeAlias),
        "const_item" => Some(SymbolKind::Constant),
        "static_item" => Some(SymbolKind::Constant),
        "mod_item" => Some(SymbolKind::Module),
        _ => None,
    };

    if let Some(sk) = symbol_kind {
        let name = find_name(&node, source, kind);
        if let Some(name) = name {
            let signature = extract_signature(&node, source, kind);
            let visibility = detect_visibility(&node, source);
            let span = node_span(&node);
            let id = format!("s:{}:{}:{}", file_id, span.start_line, &name);

            let sym = Symbol {
                id: id.clone(),
                file_id: file_id.to_string(),
                name,
                kind: sk,
                language: "rust".to_string(),
                path: file_path.to_string(),
                signature,
                span,
                parent_symbol_id: parent_id.map(String::from),
                exported: visibility.as_deref() == Some("pub"),
                visibility,
                doc_summary: None,
            };
            symbols.push(sym);

            // Extract children (e.g., methods in impl blocks)
            if kind == "impl_item" {
                let mut child_cursor = node.walk();
                for child in node.children(&mut child_cursor) {
                    if child.kind() == "declaration_list" {
                        let mut inner_cursor = child.walk();
                        for inner in child.children(&mut inner_cursor) {
                            if inner.kind() == "function_item" {
                                let method_name = find_name(&inner, source, "function_item");
                                if let Some(mname) = method_name {
                                    let mspan = node_span(&inner);
                                    let mid = format!("s:{}:{}:{}", file_id, mspan.start_line, &mname);
                                    let msig = extract_signature(&inner, source, "function_item");
                                    let mvis = detect_visibility(&inner, source);
                                    symbols.push(Symbol {
                                        id: mid,
                                        file_id: file_id.to_string(),
                                        name: mname,
                                        kind: SymbolKind::Method,
                                        language: "rust".to_string(),
                                        path: file_path.to_string(),
                                        signature: msig,
                                        span: mspan,
                                        parent_symbol_id: Some(id.clone()),
                                        exported: mvis.as_deref() == Some("pub"),
                                        visibility: mvis,
                                        doc_summary: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn find_name(node: &Node, source: &str, kind: &str) -> Option<String> {
    match kind {
        "impl_item" => {
            // For impl, get the type name
            node.child_by_field_name("type")
                .map(|n| node_text(n, source))
        }
        _ => node
            .child_by_field_name("name")
            .map(|n| node_text(n, source)),
    }
}

fn extract_signature(node: &Node, source: &str, kind: &str) -> Option<String> {
    match kind {
        "function_item" => {
            // Get everything up to the body block
            let start = node.start_byte();
            if let Some(body) = node.child_by_field_name("body") {
                let end = body.start_byte();
                let sig = &source[start..end];
                Some(sig.trim().to_string())
            } else {
                Some(node_text(*node, source))
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            let start = node.start_byte();
            let end = node.end_byte().min(start + 200);
            let text = &source[start..end];
            // Take first line or up to opening brace
            let sig = text
                .lines()
                .next()
                .unwrap_or(text)
                .trim()
                .trim_end_matches('{')
                .trim();
            Some(sig.to_string())
        }
        _ => None,
    }
}

fn detect_visibility(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn node_text(node: Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn node_span(node: &Node) -> Span {
    Span {
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_col: node.start_position().column,
        end_col: node.end_position().column,
    }
}
