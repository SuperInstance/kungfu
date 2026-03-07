use anyhow::Result;
use kungfu_storage::JsonStore;
use kungfu_types::file::FileEntry;
use kungfu_types::symbol::Symbol;
use kungfu_types::Budget;

pub struct SearchEngine {
    store: JsonStore,
}

pub struct SearchResult<T> {
    pub item: T,
    pub score: f64,
}

impl SearchEngine {
    pub fn new(store: JsonStore) -> Self {
        Self { store }
    }

    pub fn find_symbol(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<Symbol>>> {
        let symbols = self.store.load_symbols()?;
        let query_lower = query.to_lowercase();
        let top_k = budget.top_k();

        // Split multi-word queries for broader matching
        let words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<SearchResult<Symbol>> = symbols
            .into_iter()
            .filter_map(|s| {
                let score = if words.len() > 1 {
                    score_symbol_multi_word(&s, &words)
                } else {
                    score_symbol_match(&s.name, &query_lower)
                };
                if score > 0.0 {
                    Some(SearchResult { item: s, score })
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        Ok(results)
    }

    pub fn get_symbol(&self, name: &str) -> Result<Option<Symbol>> {
        let symbols = self.store.load_symbols()?;
        let name_lower = name.to_lowercase();

        // Try exact match first
        if let Some(sym) = symbols.iter().find(|s| s.name.to_lowercase() == name_lower) {
            return Ok(Some(sym.clone()));
        }

        // Try dotted path match (e.g. "KungfuService.open")
        if let Some(dot_pos) = name.find('.') {
            let method_name = &name[dot_pos + 1..].to_lowercase();
            let parent_name = &name[..dot_pos].to_lowercase();
            if let Some(sym) = symbols.iter().find(|s| {
                s.name.to_lowercase() == *method_name
                    && s.parent_symbol_id
                        .as_ref()
                        .map_or(false, |pid| pid.to_lowercase().contains(parent_name))
            }) {
                return Ok(Some(sym.clone()));
            }
        }

        Ok(None)
    }

    pub fn search_text(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        let files = self.store.load_files()?;
        let symbols = self.store.load_symbols()?;
        let query_lower = query.to_lowercase();
        let top_k = budget.top_k();
        let words: Vec<&str> = query_lower.split_whitespace().collect();

        // Build a map of file_id -> symbol relevance
        let mut file_symbol_scores: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for sym in &symbols {
            let sym_score = if words.len() > 1 {
                score_symbol_multi_word(sym, &words)
            } else {
                score_symbol_match(&sym.name, &query_lower)
            };
            if sym_score > 0.0 {
                let entry = file_symbol_scores
                    .entry(sym.file_id.clone())
                    .or_insert(0.0);
                if sym_score > *entry {
                    *entry = sym_score;
                }
            }
        }

        let mut results: Vec<SearchResult<FileEntry>> = files
            .into_iter()
            .filter_map(|f| {
                let path_score = score_path_match(&f.path, &query_lower, &words);
                let sym_score = file_symbol_scores.get(&f.id).copied().unwrap_or(0.0) * 0.8;
                let combined = path_score.max(sym_score);
                if combined > 0.0 {
                    Some(SearchResult {
                        item: f,
                        score: combined,
                    })
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        Ok(results)
    }

    pub fn find_files(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        self.search_text(query, budget)
    }

    pub fn get_all_files(&self) -> Result<Vec<FileEntry>> {
        self.store.load_files()
    }

    pub fn get_all_symbols(&self) -> Result<Vec<Symbol>> {
        self.store.load_symbols()
    }

    pub fn get_symbols_for_file(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let symbols = self.store.load_symbols()?;
        let files = self.store.load_files()?;

        let file_id = files
            .iter()
            .find(|f| f.path == file_path)
            .map(|f| f.id.clone());

        match file_id {
            Some(fid) => Ok(symbols.into_iter().filter(|s| s.file_id == fid).collect()),
            None => Ok(Vec::new()),
        }
    }
}

fn score_symbol_match(name: &str, query: &str) -> f64 {
    let name_lower = name.to_lowercase();

    if name_lower == *query {
        return 1.0;
    }
    if name_lower.starts_with(query) {
        return 0.9;
    }
    if name_lower.contains(query) {
        return 0.7;
    }

    // Fuzzy: check if all query chars appear in order
    let mut query_chars = query.chars().peekable();
    for ch in name_lower.chars() {
        if let Some(&qch) = query_chars.peek() {
            if ch == qch {
                query_chars.next();
            }
        }
    }
    if query_chars.peek().is_none() && query.len() >= 3 {
        return 0.4;
    }

    0.0
}

/// Score a symbol against multiple query words (e.g. "refresh token")
fn score_symbol_multi_word(sym: &Symbol, words: &[&str]) -> f64 {
    let name_lower = sym.name.to_lowercase();
    let sig_lower = sym
        .signature
        .as_ref()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let path_lower = sym.path.to_lowercase();

    let mut matched = 0;
    for word in words {
        if name_lower.contains(word) || sig_lower.contains(word) || path_lower.contains(word) {
            matched += 1;
        }
    }

    if matched == 0 {
        return 0.0;
    }

    let ratio = matched as f64 / words.len() as f64;

    // Boost if name directly matches
    if words.iter().any(|w| name_lower.contains(w)) {
        ratio * 0.9
    } else {
        ratio * 0.6
    }
}

fn score_path_match(path: &str, query: &str, words: &[&str]) -> f64 {
    let path_lower = path.to_lowercase();

    // Single word: check path containment
    if words.len() <= 1 {
        if path_lower.contains(query) {
            // Boost for filename match vs directory match
            let filename = path.rsplit('/').next().unwrap_or(path).to_lowercase();
            if filename.contains(query) {
                return 0.9;
            }
            return 0.6;
        }
        return 0.0;
    }

    // Multi-word: check how many words match path
    let matched = words.iter().filter(|w| path_lower.contains(*w)).count();
    if matched == 0 {
        return 0.0;
    }

    matched as f64 / words.len() as f64
}
