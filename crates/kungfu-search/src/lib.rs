use anyhow::Result;
use kungfu_storage::JsonStore;
use kungfu_types::file::FileEntry;
use kungfu_types::relation::RelationKind;
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

    /// Find files related to the given file by import relations, directory proximity,
    /// shared symbols, and naming patterns.
    pub fn find_related(&self, file_path: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        let files = self.store.load_files()?;
        let symbols = self.store.load_symbols()?;
        let relations = self.store.load_relations()?;
        let top_k = budget.top_k();

        // Normalize: find the file in the index
        let target = files
            .iter()
            .find(|f| f.path == file_path || f.path.ends_with(file_path) || file_path.ends_with(&f.path));

        let target = match target {
            Some(t) => t.clone(),
            None => return Ok(Vec::new()),
        };

        let target_path = &target.path;

        // Collect import relations for the target file
        let mut relation_scores: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for rel in &relations {
            if rel.source_id == target.id {
                match rel.kind {
                    RelationKind::Imports => {
                        *relation_scores.entry(rel.target_id.clone()).or_default() += 0.7;
                    }
                    RelationKind::TestFor => {
                        // This test file tests that source file
                        *relation_scores.entry(rel.target_id.clone()).or_default() += 0.8;
                    }
                    RelationKind::ConfigFor => {
                        *relation_scores.entry(rel.target_id.clone()).or_default() += 0.4;
                    }
                    _ => {}
                }
            }
            if rel.target_id == target.id {
                match rel.kind {
                    RelationKind::Imports => {
                        *relation_scores.entry(rel.source_id.clone()).or_default() += 0.6;
                    }
                    RelationKind::TestFor => {
                        // Source file has this test
                        *relation_scores.entry(rel.source_id.clone()).or_default() += 0.8;
                    }
                    RelationKind::ConfigFor => {
                        *relation_scores.entry(rel.source_id.clone()).or_default() += 0.4;
                    }
                    _ => {}
                }
            }
        }

        // Collect symbol names from the target file
        let target_symbols: std::collections::HashSet<String> = symbols
            .iter()
            .filter(|s| s.file_id == target.id)
            .map(|s| s.name.to_lowercase())
            .collect();

        // Path components for proximity scoring
        let target_parts: Vec<&str> = target_path.split('/').collect();
        let target_dir = target_parts[..target_parts.len().saturating_sub(1)].join("/");
        let target_stem = std::path::Path::new(target_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Build symbol sets per file for cross-referencing
        let mut file_symbols: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for sym in &symbols {
            file_symbols
                .entry(sym.file_id.clone())
                .or_default()
                .insert(sym.name.to_lowercase());
        }

        let mut results: Vec<SearchResult<FileEntry>> = files
            .iter()
            .filter(|f| f.path != target.path)
            .filter_map(|f| {
                let mut score = 0.0_f64;

                // 0. Import/dependency relation (strongest signal)
                if let Some(&rel_score) = relation_scores.get(&f.id) {
                    score += rel_score;
                }

                let f_parts: Vec<&str> = f.path.split('/').collect();
                let f_dir = f_parts[..f_parts.len().saturating_sub(1)].join("/");
                let f_stem = std::path::Path::new(&f.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                // 1. Same directory
                if f_dir == target_dir && !target_dir.is_empty() {
                    score += 0.3;
                }

                // 2. Shared parent directory
                let shared_depth = target_parts
                    .iter()
                    .zip(f_parts.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                if shared_depth > 0 {
                    let depth_score = shared_depth as f64 / target_parts.len().max(1) as f64;
                    score += depth_score * 0.2;
                }

                // 3. Shared symbol names
                if let Some(f_syms) = file_symbols.get(&f.id) {
                    let shared = target_symbols.intersection(f_syms).count();
                    if shared > 0 {
                        let sym_score = (shared as f64 / target_symbols.len().max(1) as f64).min(1.0);
                        score += sym_score * 0.3;
                    }
                }

                // 4. Test file pattern
                if f_stem.contains(&target_stem) || target_stem.contains(&f_stem) {
                    if f.path.contains("test") || f.path.contains("spec") {
                        score += 0.3;
                    } else if target_path.contains("test") || target_path.contains("spec") {
                        score += 0.3;
                    } else {
                        score += 0.1;
                    }
                }

                // 5. Same language bonus
                if f.language == target.language {
                    score += 0.05;
                }

                if score > 0.05 {
                    Some(SearchResult {
                        item: f.clone(),
                        score,
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

    // Stem match: "ranking" matches "rank"
    if let Some(stem) = simple_stem(query) {
        if name_lower.contains(&stem) {
            return 0.6;
        }
    }
    // Reverse: query "rank" matches name "ranking"
    if let Some(stem) = simple_stem(&name_lower) {
        if stem.contains(query) || query.contains(&stem) {
            return 0.5;
        }
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

/// Score a symbol against multiple query words (e.g. "refresh token").
/// Supports exact phrase matching when query contains the full phrase in symbol name/signature.
fn score_symbol_multi_word(sym: &Symbol, words: &[&str]) -> f64 {
    let name_lower = sym.name.to_lowercase();
    let sig_lower = sym
        .signature
        .as_ref()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let path_lower = sym.path.to_lowercase();

    // Exact phrase bonus: join words back and check if the phrase appears as-is
    let phrase = words.join(" ");
    let phrase_underscore = words.join("_");
    let phrase_camel = words_to_camel(words);

    let phrase_camel_lower = phrase_camel.to_lowercase();
    let has_exact_phrase = name_lower.contains(&phrase)
        || name_lower.contains(&phrase_underscore)
        || name_lower.contains(&phrase_camel_lower)
        || sig_lower.contains(&phrase)
        || sig_lower.contains(&phrase_underscore);

    if has_exact_phrase {
        return 0.95;
    }

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

/// Convert words to camelCase for phrase matching (e.g. ["refresh", "token"] → "refreshtoken")
fn words_to_camel(words: &[&str]) -> String {
    let mut result = String::new();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            result.push_str(word);
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                result.extend(first.to_uppercase());
                result.push_str(chars.as_str());
            }
        }
    }
    result
}

fn score_path_match(path: &str, query: &str, words: &[&str]) -> f64 {
    let path_lower = path.to_lowercase();

    // Single word: check path containment (exact or stem)
    if words.len() <= 1 {
        if path_lower.contains(query) {
            let filename = path.rsplit('/').next().unwrap_or(path).to_lowercase();
            if filename.contains(query) {
                return 0.9;
            }
            return 0.6;
        }
        // Stem match: "ranking" → "rank", "indexing" → "index"
        if let Some(stem) = simple_stem(query) {
            if path_lower.contains(&stem) {
                return 0.5;
            }
        }
        return 0.0;
    }

    // Multi-word: check how many words match path (with stem fallback)
    let matched = words
        .iter()
        .filter(|w| {
            path_lower.contains(*w)
                || simple_stem(w).map_or(false, |stem| path_lower.contains(&stem))
        })
        .count();
    if matched == 0 {
        return 0.0;
    }

    matched as f64 / words.len() as f64
}

/// Simple English stemming: strip common suffixes.
fn simple_stem(word: &str) -> Option<String> {
    if word.len() < 5 {
        return None;
    }
    for suffix in &["ing", "tion", "sion", "ment", "ness", "ity", "able", "ible", "ous", "ive", "er", "ed", "es", "ly", "al", "ful", "less", "ize", "ise"] {
        if let Some(stem) = word.strip_suffix(suffix) {
            if stem.len() >= 3 {
                return Some(stem.to_string());
            }
        }
    }
    None
}

/// Check if the query suggests interest in test files.
pub fn query_wants_tests(words: &[&str]) -> bool {
    words.iter().any(|w| {
        matches!(
            *w,
            "test" | "tests" | "spec" | "specs" | "testing" | "unittest" | "unit_test"
        )
    })
}

/// Check if the query suggests interest in config files.
pub fn query_wants_config(words: &[&str]) -> bool {
    words.iter().any(|w| {
        matches!(
            *w,
            "config"
                | "configuration"
                | "env"
                | "settings"
                | "setup"
                | "cargo"
                | "package"
                | "toml"
                | "yaml"
                | "dockerfile"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kungfu_types::symbol::{Span, Symbol, SymbolKind};

    fn make_symbol(name: &str, path: &str, sig: Option<&str>) -> Symbol {
        Symbol {
            id: format!("s:test:1:{}", name),
            file_id: "f:test".to_string(),
            name: name.to_string(),
            kind: SymbolKind::Function,
            language: "rust".to_string(),
            path: path.to_string(),
            signature: sig.map(String::from),
            span: Span { start_line: 1, end_line: 10, start_col: 0, end_col: 0 },
            parent_symbol_id: None,
            exported: true,
            visibility: None,
            doc_summary: None,
        }
    }

    #[test]
    fn exact_match_scores_highest() {
        assert_eq!(score_symbol_match("Budget", "budget"), 1.0);
    }

    #[test]
    fn prefix_match() {
        assert!(score_symbol_match("BudgetParam", "budget") > 0.8);
    }

    #[test]
    fn contains_match() {
        assert!(score_symbol_match("parse_budget", "budget") > 0.6);
    }

    #[test]
    fn no_match_returns_zero() {
        assert_eq!(score_symbol_match("KungfuService", "budget"), 0.0);
    }

    #[test]
    fn fuzzy_match() {
        let score = score_symbol_match("build_context_packet", "bcp");
        assert!(score > 0.3, "fuzzy match should score > 0.3, got {}", score);
    }

    #[test]
    fn stem_match_ranking_to_rank() {
        // "ranking" stems to "rank", and "build_context_packet" doesn't contain "rank"
        // This correctly returns 0 — stem matching applies to path, not symbol name substring
        let score = score_symbol_match("build_context_packet", "ranking");
        assert_eq!(score, 0.0);

        // But a name containing "rank" should match via stem
        let score2 = score_symbol_match("rank_results", "ranking");
        assert!(score2 > 0.0, "stem should match rank_results, got {}", score2);
    }

    #[test]
    fn exact_phrase_snake_case() {
        let sym = make_symbol("build_context_packet", "src/rank.rs", None);
        let score = score_symbol_multi_word(&sym, &["context", "packet"]);
        assert_eq!(score, 0.95, "snake_case phrase should score 0.95");
    }

    #[test]
    fn exact_phrase_camel_case() {
        let sym = make_symbol("contextPacket", "src/types.rs", None);
        let score = score_symbol_multi_word(&sym, &["context", "packet"]);
        // Both words match in name → ratio=1.0 * 0.9 = 0.9
        // camelCase check: words_to_camel returns "contextPacket" which matches
        assert_eq!(score, 0.95, "camelCase phrase should score 0.95");
    }

    #[test]
    fn multi_word_partial() {
        let sym = make_symbol("parse_budget", "src/cli.rs", None);
        let score = score_symbol_multi_word(&sym, &["budget", "validation"]);
        assert!(score > 0.0 && score < 0.95, "partial multi-word should be between 0 and 0.95");
    }

    #[test]
    fn multi_word_no_match() {
        let sym = make_symbol("KungfuService", "src/core.rs", None);
        let score = score_symbol_multi_word(&sym, &["database", "migration"]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn path_match_filename() {
        let score = score_path_match("src/budget.rs", "budget", &["budget"]);
        assert_eq!(score, 0.9, "filename match should score 0.9");
    }

    #[test]
    fn path_match_directory() {
        let score = score_path_match("budget/types/mod.rs", "budget", &["budget"]);
        assert_eq!(score, 0.6, "directory-only match should score 0.6");
    }

    #[test]
    fn path_no_match() {
        let score = score_path_match("src/search.rs", "budget", &["budget"]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn path_stem_match() {
        let score = score_path_match("src/ranking.rs", "ranking", &["ranking"]);
        assert_eq!(score, 0.9, "direct filename match");

        let stem_score = score_path_match("src/rank.rs", "ranking", &["ranking"]);
        assert!(stem_score > 0.0, "stem match should find rank.rs");
    }

    #[test]
    fn simple_stem_works() {
        assert_eq!(simple_stem("ranking"), Some("rank".to_string()));
        assert_eq!(simple_stem("indexing"), Some("index".to_string()));
        assert_eq!(simple_stem("configuration"), Some("configura".to_string()));
        assert_eq!(simple_stem("abc"), None); // too short
    }

    #[test]
    fn words_to_camel_works() {
        assert_eq!(words_to_camel(&["context", "packet"]), "contextPacket");
        assert_eq!(words_to_camel(&["a"]), "a");
        assert_eq!(words_to_camel(&["foo", "bar", "baz"]), "fooBarBaz");
    }

    #[test]
    fn query_wants_tests_detection() {
        assert!(query_wants_tests(&["test", "budget"]));
        assert!(query_wants_tests(&["run", "specs"]));
        assert!(!query_wants_tests(&["budget", "parsing"]));
    }

    #[test]
    fn query_wants_config_detection() {
        assert!(query_wants_config(&["config", "file"]));
        assert!(query_wants_config(&["cargo", "toml"]));
        assert!(!query_wants_config(&["budget", "parsing"]));
    }
}
