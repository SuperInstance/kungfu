use kungfu_types::budget::Budget;
use kungfu_types::context::{ContextItem, ContextItemType, ContextPacket};
use kungfu_types::symbol::Symbol;

pub fn build_context_packet(
    query: &str,
    symbols: Vec<(Symbol, f64)>,
    budget: Budget,
) -> ContextPacket {
    let top_k = budget.top_k();

    let mut items: Vec<ContextItem> = symbols
        .into_iter()
        .take(top_k)
        .map(|(sym, score)| ContextItem {
            item_type: ContextItemType::Symbol,
            path: sym.path,
            name: sym.name,
            signature: sym.signature,
            why: format!("matched query with score {:.2}", score),
            score,
            snippet: None,
        })
        .collect();

    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    ContextPacket {
        query: query.to_string(),
        budget,
        items,
    }
}
