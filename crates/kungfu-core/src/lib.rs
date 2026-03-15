use anyhow::{bail, Result};
use kungfu_index::Indexer;
use kungfu_project::Project;
use kungfu_rank::{build_context_packet, build_context_packet_full, ScoredSymbol};
use kungfu_search::{SearchEngine, SearchResult};
use kungfu_storage::JsonStore;
use kungfu_types::budget::Budget;
use kungfu_types::context::{ContextPacket, Intent};
use kungfu_types::file::FileEntry;
use kungfu_types::relation::RelationKind;
use kungfu_types::symbol::Symbol;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::info;

pub struct KungfuService {
    project: Project,
}

pub struct StatusInfo {
    pub project_name: String,
    pub root: String,
    pub indexed_files: usize,
    pub indexed_symbols: usize,
    pub languages: HashMap<String, usize>,
    pub has_git: bool,
}

pub struct RepoOutline {
    pub project_name: String,
    pub total_files: usize,
    pub total_symbols: usize,
    pub languages: HashMap<String, usize>,
    pub top_dirs: Vec<DirEntry>,
    pub entrypoints: Vec<String>,
}

pub struct DirEntry {
    pub path: String,
    pub file_count: usize,
}

pub struct FileOutline {
    pub path: String,
    pub language: Option<String>,
    pub symbols: Vec<SymbolOutline>,
}

pub struct SymbolOutline {
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub line: usize,
    pub exported: bool,
}

impl KungfuService {
    pub fn open(start_dir: &Path) -> Result<Self> {
        let project = Project::open(start_dir)?;
        Ok(Self { project })
    }

    pub fn config(&self) -> &kungfu_config::KungfuConfig {
        &self.project.config
    }

    fn store(&self) -> JsonStore {
        JsonStore::new(&self.project.index_dir())
    }

    fn search(&self) -> SearchEngine {
        SearchEngine::new(self.store())
    }

    pub fn status(&self) -> Result<StatusInfo> {
        let store = self.store();
        let files = store.load_files()?;
        let symbols = store.load_symbols()?;

        let mut languages: HashMap<String, usize> = HashMap::new();
        for f in &files {
            if let Some(ref lang) = f.language {
                *languages.entry(lang.clone()).or_default() += 1;
            }
        }

        Ok(StatusInfo {
            project_name: self.project.meta.name.clone(),
            root: self.project.root.to_string_lossy().to_string(),
            indexed_files: files.len(),
            indexed_symbols: symbols.len(),
            languages,
            has_git: kungfu_git::is_git_repo(&self.project.root),
        })
    }

    pub fn index_full(&self) -> Result<kungfu_index::indexer::IndexStats> {
        let store = self.store();
        let mut indexer = Indexer::new(&self.project.root, self.project.config.clone(), store);
        indexer.index_full()
    }

    pub fn index_incremental(&self) -> Result<kungfu_index::indexer::IndexStats> {
        let store = self.store();
        let mut indexer = Indexer::new(&self.project.root, self.project.config.clone(), store);
        indexer.index_incremental()
    }

    pub fn index_changed(&self) -> Result<kungfu_index::indexer::IndexStats> {
        if !kungfu_git::is_git_repo(&self.project.root) {
            bail!("--changed requires a git repository");
        }
        let changed = kungfu_git::changed_files(&self.project.root)?;
        if changed.is_empty() {
            return Ok(kungfu_index::indexer::IndexStats {
                total_files: 0,
                new_files: 0,
                changed_files: 0,
                removed_files: 0,
                symbols_extracted: 0,
            });
        }
        let store = self.store();
        let mut indexer = Indexer::new(&self.project.root, self.project.config.clone(), store);
        indexer.index_only(&changed)
    }

    pub fn repo_outline(&self, budget: Budget) -> Result<RepoOutline> {
        let store = self.store();
        let files = store.load_files()?;
        let symbols = store.load_symbols()?;

        let mut languages: HashMap<String, usize> = HashMap::new();
        let mut dirs: HashMap<String, usize> = HashMap::new();

        for f in &files {
            if let Some(ref lang) = f.language {
                *languages.entry(lang.clone()).or_default() += 1;
            }
            if let Some(dir) = Path::new(&f.path).parent() {
                let dir_str = dir.to_string_lossy().to_string();
                if !dir_str.is_empty() {
                    // Get top-level directory
                    let top = dir_str.split('/').next().unwrap_or(&dir_str).to_string();
                    *dirs.entry(top).or_default() += 1;
                }
            }
        }

        let mut top_dirs: Vec<DirEntry> = dirs
            .into_iter()
            .map(|(path, file_count)| DirEntry { path, file_count })
            .collect();
        top_dirs.sort_by(|a, b| b.file_count.cmp(&a.file_count));
        top_dirs.truncate(budget.top_k() * 2);

        // Detect entrypoints
        let entrypoints: Vec<String> = files
            .iter()
            .filter(|f| {
                let p = &f.path;
                p.ends_with("main.rs")
                    || p.ends_with("lib.rs")
                    || p.ends_with("index.ts")
                    || p.ends_with("index.js")
                    || p.ends_with("main.py")
                    || p.ends_with("main.go")
                    || p.ends_with("app.ts")
                    || p.ends_with("app.js")
                    || p == "package.json"
                    || p == "Cargo.toml"
                    || p == "go.mod"
                    || p == "pyproject.toml"
            })
            .map(|f| f.path.clone())
            .collect();

        Ok(RepoOutline {
            project_name: self.project.meta.name.clone(),
            total_files: files.len(),
            total_symbols: symbols.len(),
            languages,
            top_dirs,
            entrypoints,
        })
    }

    pub fn file_outline(&self, file_path: &str) -> Result<FileOutline> {
        let search = self.search();
        let files = search.get_all_files()?;

        let file = files
            .iter()
            .find(|f| f.path == file_path || f.path.ends_with(file_path))
            .ok_or_else(|| anyhow::anyhow!("file not found in index: {}", file_path))?;

        let symbols = search.get_symbols_for_file(&file.path)?;

        let outlines = symbols
            .iter()
            .map(|s| SymbolOutline {
                name: s.name.clone(),
                kind: s.kind.to_string(),
                signature: s.signature.clone(),
                line: s.span.start_line,
                exported: s.exported,
            })
            .collect();

        Ok(FileOutline {
            path: file.path.clone(),
            language: file.language.clone(),
            symbols: outlines,
        })
    }

    pub fn find_symbol(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<Symbol>>> {
        self.search().find_symbol(query, budget)
    }

    pub fn get_symbol(&self, name: &str) -> Result<Option<Symbol>> {
        self.search().get_symbol(name)
    }

    pub fn search_text(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        self.search().search_text(query, budget)
    }

    pub fn find_related(&self, file_path: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        self.search().find_related(file_path, budget)
    }

    pub fn context(&self, query: &str, budget: Budget) -> Result<ContextPacket> {
        let search = self.search();
        let query_lower = query.to_lowercase();
        let words: Vec<&str> = query_lower.split_whitespace().collect();

        // Search symbols
        let symbol_results = search.find_symbol(query, Budget::Full)?;

        let mut scored_symbols: Vec<(Symbol, f64)> = symbol_results
            .into_iter()
            .map(|r| (r.item, r.score))
            .collect();

        // Also search files and pull in their symbols for broader context
        let file_results = search.search_text(query, Budget::Full)?;
        let all_symbols = search.get_all_symbols()?;
        let seen_ids: std::collections::HashSet<String> =
            scored_symbols.iter().map(|(s, _)| s.id.clone()).collect();

        for fr in &file_results {
            let file_syms: Vec<_> = all_symbols
                .iter()
                .filter(|s| s.file_id == fr.item.id && !seen_ids.contains(&s.id))
                .collect();
            for sym in file_syms {
                scored_symbols.push((sym.clone(), fr.score * 0.7));
            }
        }

        // Query-aware bonuses
        let wants_tests = kungfu_search::query_wants_tests(&words);
        let wants_config = kungfu_search::query_wants_config(&words);

        for (sym, score) in &mut scored_symbols {
            // Test proximity bonus
            if wants_tests
                && (sym.path.contains("test")
                    || sym.path.contains("spec")
                    || sym.path.contains("tests/"))
            {
                *score += 0.15;
            }

            // Config proximity bonus
            if wants_config
                && (sym.path.ends_with(".toml")
                    || sym.path.ends_with(".json")
                    || sym.path.ends_with(".yaml")
                    || sym.path.ends_with(".yml")
                    || sym.path.contains("config"))
            {
                *score += 0.15;
            }
        }

        // Changed-file bonus: boost symbols from git-changed files
        if kungfu_git::is_git_repo(&self.project.root) {
            if let Ok(changed) = kungfu_git::changed_files(&self.project.root) {
                if !changed.is_empty() {
                    for (sym, score) in &mut scored_symbols {
                        let is_changed = changed
                            .iter()
                            .any(|c| sym.path.ends_with(c) || c.ends_with(&sym.path));
                        if is_changed {
                            *score += 0.2;
                        }
                    }
                }
            }
        }

        let mut packet = build_context_packet(query, scored_symbols, budget);

        let snippet_lines = budget.max_lines();
        if snippet_lines > 0 {
            self.fill_snippets(&mut packet, snippet_lines);
        }

        Ok(packet)
    }

    /// High-level context retrieval: parse intent, run multi-strategy search,
    /// rank with contextual signals, return compact packet.
    pub fn ask_context(&self, task: &str, budget: Budget) -> Result<ContextPacket> {
        let query_lower = task.to_lowercase();
        let words: Vec<&str> = query_lower.split_whitespace().collect();

        // 1. Detect intent
        let intent = detect_intent(&words);

        // 2. Extract search terms (filter out stop/intent words)
        let keywords: Vec<&str> = words
            .iter()
            .filter(|w| !is_stop_word(w))
            .copied()
            .collect();
        let keyword_query = keywords.join(" ");

        let search = self.search();
        let store = self.store();

        // 3. Determine primary language for weighting
        let files = store.load_files()?;
        let primary_lang = detect_primary_language(&files);

        // 4. Multi-strategy search
        let mut scored_symbols: Vec<ScoredSymbol> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        // Strategy A: symbol search
        let sym_results = search.find_symbol(&keyword_query, Budget::Full)?;
        for r in sym_results {
            seen_ids.insert(r.item.id.clone());
            scored_symbols.push(ScoredSymbol {
                symbol: r.item,
                score: r.score,
                reason: "symbol name match".to_string(),
            });
        }

        // Strategy B: text/file search
        let file_results = search.search_text(&keyword_query, Budget::Full)?;
        let all_symbols = search.get_all_symbols()?;
        for fr in &file_results {
            let file_syms: Vec<_> = all_symbols
                .iter()
                .filter(|s| s.file_id == fr.item.id && !seen_ids.contains(&s.id))
                .collect();
            for sym in file_syms {
                seen_ids.insert(sym.id.clone());
                scored_symbols.push(ScoredSymbol {
                    symbol: sym.clone(),
                    score: fr.score * 0.6,
                    reason: format!("in matched file {}", fr.item.path),
                });
            }
        }

        // Strategy C: sibling symbols from top match's file (important for impact/understand)
        if matches!(intent, Intent::Impact | Intent::Understand) {
            // Only use the file of the HIGHEST scoring direct match
            if let Some(top) = scored_symbols
                .iter()
                .filter(|s| s.reason == "symbol name match")
                .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            {
                let top_file_id = top.symbol.file_id.clone();

                // Boost existing symbols from that file
                for s in &mut scored_symbols {
                    if s.symbol.file_id == top_file_id && s.reason != "symbol name match" {
                        if s.score < 0.55 {
                            s.score = 0.55;
                            s.reason = "same file as matched symbol".to_string();
                        }
                    }
                }
                // Add new siblings, limited to 5 per file
                let mut added = 0;
                let siblings: Vec<_> = all_symbols
                    .iter()
                    .filter(|s| s.file_id == top_file_id && !seen_ids.contains(&s.id))
                    .collect();
                for sym in siblings {
                    if added >= 5 {
                        break;
                    }
                    seen_ids.insert(sym.id.clone());
                    scored_symbols.push(ScoredSymbol {
                        symbol: sym.clone(),
                        score: 0.55,
                        reason: "same file as matched symbol".to_string(),
                    });
                    added += 1;
                }
            }
        }

        // Strategy D: related files (for impact/debug intents)
        if matches!(intent, Intent::Impact | Intent::Debug) && !file_results.is_empty() {
            let top_file = &file_results[0].item;
            if let Ok(related) = search.find_related(&top_file.path, Budget::Small) {
                for r in related {
                    let rel_syms: Vec<_> = all_symbols
                        .iter()
                        .filter(|s| s.file_id == r.item.id && !seen_ids.contains(&s.id))
                        .take(3)
                        .collect();
                    for sym in rel_syms {
                        seen_ids.insert(sym.id.clone());
                        scored_symbols.push(ScoredSymbol {
                            symbol: sym.clone(),
                            score: r.score * 0.4,
                            reason: format!("related to {}", top_file.path),
                        });
                    }
                }
            }
        }

        // Strategy D: import chain (for impact intent)
        if intent == Intent::Impact {
            let relations = store.load_relations()?;
            let file_ids: HashSet<String> = file_results.iter().map(|r| r.item.id.clone()).collect();

            for rel in &relations {
                if rel.kind == RelationKind::Imports && file_ids.contains(&rel.target_id) {
                    let importer_syms: Vec<_> = all_symbols
                        .iter()
                        .filter(|s| s.file_id == rel.source_id && !seen_ids.contains(&s.id))
                        .take(1)
                        .collect();
                    for sym in importer_syms {
                        seen_ids.insert(sym.id.clone());
                        scored_symbols.push(ScoredSymbol {
                            symbol: sym.clone(),
                            score: 0.35,
                            reason: "imports affected file".to_string(),
                        });
                    }
                }
            }
        }

        // 4. Apply intent-specific bonuses
        let wants_tests = kungfu_search::query_wants_tests(&words);
        let wants_config = kungfu_search::query_wants_config(&words);

        for s in &mut scored_symbols {
            if wants_tests
                && (s.symbol.path.contains("test")
                    || s.symbol.path.contains("spec")
                    || s.symbol.path.contains("tests/"))
            {
                s.score += 0.15;
            }
            if wants_config
                && (s.symbol.path.ends_with(".toml")
                    || s.symbol.path.ends_with(".json")
                    || s.symbol.path.ends_with(".yaml")
                    || s.symbol.path.contains("config"))
            {
                s.score += 0.15;
            }
            if intent == Intent::Debug {
                let name_lower = s.symbol.name.to_lowercase();
                if name_lower.contains("error")
                    || name_lower.contains("err")
                    || name_lower.contains("handle")
                    || name_lower.contains("validate")
                {
                    s.score += 0.1;
                }
            }
        }

        // Language importance weighting
        if let Some(ref primary) = primary_lang {
            for s in &mut scored_symbols {
                let sym_lang = &s.symbol.language;
                if sym_lang == primary {
                    // Primary language: no change (×1.0)
                } else if is_code_language(sym_lang) {
                    // Secondary code language: slight penalty
                    s.score *= 0.85;
                }
            }
        }

        // Changed-file bonus
        if kungfu_git::is_git_repo(&self.project.root) {
            if let Ok(changed) = kungfu_git::changed_files(&self.project.root) {
                if !changed.is_empty() {
                    for s in &mut scored_symbols {
                        if changed.iter().any(|c| {
                            s.symbol.path.ends_with(c) || c.ends_with(&s.symbol.path)
                        }) {
                            s.score += 0.2;
                            s.reason = format!("{}, recently changed", s.reason);
                        }
                    }
                }
            }
        }

        // 5. Build packet
        let mut packet = build_context_packet_full(
            task,
            scored_symbols,
            budget,
            Some(intent),
        );

        // 6. Extract snippets based on budget
        let snippet_lines = budget.max_lines();
        if snippet_lines > 0 {
            self.fill_snippets(&mut packet, snippet_lines);
        }

        Ok(packet)
    }

    /// Fill snippet fields in context packet items by reading source files.
    fn fill_snippets(&self, packet: &mut ContextPacket, max_lines: usize) {
        let mut file_cache: HashMap<String, Vec<String>> = HashMap::new();

        for item in &mut packet.items {
            // Find the symbol's span from the index
            let span = self
                .search()
                .get_all_symbols()
                .ok()
                .and_then(|syms| {
                    syms.iter()
                        .find(|s| s.path == item.path && s.name == item.name)
                        .map(|s| (s.span.start_line, s.span.end_line))
                });

            let (start, end) = match span {
                Some(s) => s,
                None => continue,
            };

            // Read file (cached)
            let lines = file_cache
                .entry(item.path.clone())
                .or_insert_with(|| {
                    let abs_path = self.project.root.join(&item.path);
                    std::fs::read_to_string(&abs_path)
                        .map(|c| c.lines().map(String::from).collect())
                        .unwrap_or_default()
                });

            if lines.is_empty() {
                continue;
            }

            // Extract snippet: symbol body, capped at max_lines
            let start_idx = start.saturating_sub(1); // 1-based to 0-based
            let end_idx = end.min(lines.len());
            let symbol_lines = end_idx - start_idx;

            let snippet_lines: Vec<&str> = if symbol_lines <= max_lines {
                // Whole symbol fits
                lines[start_idx..end_idx].iter().map(|s| s.as_str()).collect()
            } else {
                // Take first max_lines
                lines[start_idx..start_idx + max_lines]
                    .iter()
                    .map(|s| s.as_str())
                    .collect()
            };

            if !snippet_lines.is_empty() {
                item.snippet = Some(snippet_lines.join("\n"));
            }
        }
    }

    pub fn diff_context(&self, budget: Budget) -> Result<ContextPacket> {
        if !kungfu_git::is_git_repo(&self.project.root) {
            bail!("not a git repository");
        }

        let changed = kungfu_git::changed_files(&self.project.root)?;
        if changed.is_empty() {
            return Ok(ContextPacket {
                query: "diff context".to_string(),
                budget,
                intent: None,
                items: Vec::new(),
            });
        }

        info!("building context for {} changed files", changed.len());

        let search = self.search();
        let all_symbols = search.get_all_symbols()?;

        let scored: Vec<(Symbol, f64)> = all_symbols
            .into_iter()
            .filter_map(|s| {
                let is_changed = changed.iter().any(|c| s.path.ends_with(c) || c.ends_with(&s.path));
                if is_changed {
                    Some((s, 0.9))
                } else {
                    None
                }
            })
            .collect();

        Ok(build_context_packet("diff context", scored, budget))
    }
}

fn detect_primary_language(files: &[FileEntry]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for f in files {
        if let Some(ref lang) = f.language {
            if is_code_language(lang) {
                *counts.entry(lang.clone()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(lang, _)| lang)
}

fn is_code_language(lang: &str) -> bool {
    matches!(
        lang,
        "rust" | "typescript" | "javascript" | "python" | "go"
    )
}

fn detect_intent(words: &[&str]) -> Intent {
    for w in words {
        match *w {
            "find" | "where" | "locate" | "show" | "get" | "lookup" | "search" => {
                return Intent::Lookup
            }
            "bug" | "fix" | "error" | "crash" | "broken" | "fail" | "debug" | "wrong"
            | "issue" | "panic" => return Intent::Debug,
            "how" | "explain" | "understand" | "what" | "why" | "does" | "works" | "overview" => {
                return Intent::Understand
            }
            "impact" | "affects" | "uses" | "calls" | "callers" | "consumers" | "depends"
            | "dependents" | "change" | "refactor" | "rename" | "remove" | "delete" => {
                return Intent::Impact
            }
            _ => {}
        }
    }
    Intent::Lookup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_lookup() {
        assert_eq!(detect_intent(&["find", "budget"]), Intent::Lookup);
        assert_eq!(detect_intent(&["where", "is", "config"]), Intent::Lookup);
        assert_eq!(detect_intent(&["show", "symbols"]), Intent::Lookup);
    }

    #[test]
    fn intent_debug() {
        assert_eq!(detect_intent(&["fix", "crash"]), Intent::Debug);
        assert_eq!(detect_intent(&["error", "parsing"]), Intent::Debug);
        assert_eq!(detect_intent(&["bug", "in", "indexer"]), Intent::Debug);
    }

    #[test]
    fn intent_understand() {
        assert_eq!(detect_intent(&["how", "does", "ranking"]), Intent::Understand);
        assert_eq!(detect_intent(&["explain", "budget"]), Intent::Understand);
        assert_eq!(detect_intent(&["what", "is", "context"]), Intent::Understand);
    }

    #[test]
    fn intent_impact() {
        assert_eq!(detect_intent(&["impact", "of", "change"]), Intent::Impact);
        assert_eq!(detect_intent(&["refactor", "budget"]), Intent::Impact);
        assert_eq!(detect_intent(&["rename", "symbol"]), Intent::Impact);
    }

    #[test]
    fn intent_default_is_lookup() {
        assert_eq!(detect_intent(&["foobar", "baz"]), Intent::Lookup);
    }

    #[test]
    fn stop_words_filtered() {
        assert!(is_stop_word("the"));
        assert!(is_stop_word("find"));
        assert!(is_stop_word("add"));
        assert!(!is_stop_word("budget"));
        assert!(!is_stop_word("parser"));
        assert!(!is_stop_word("language"));
    }

    #[test]
    fn primary_language_detection() {
        let files = vec![
            FileEntry {
                id: "1".into(), path: "a.rs".into(), extension: Some("rs".into()),
                language: Some("rust".into()), size: 100, hash: "h1".into(),
                indexed_at: Default::default(), tags: vec![],
            },
            FileEntry {
                id: "2".into(), path: "b.rs".into(), extension: Some("rs".into()),
                language: Some("rust".into()), size: 100, hash: "h2".into(),
                indexed_at: Default::default(), tags: vec![],
            },
            FileEntry {
                id: "3".into(), path: "c.py".into(), extension: Some("py".into()),
                language: Some("python".into()), size: 100, hash: "h3".into(),
                indexed_at: Default::default(), tags: vec![],
            },
        ];
        assert_eq!(detect_primary_language(&files), Some("rust".to_string()));
    }
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        // English stop words
        "the" | "a" | "an" | "is" | "are" | "was" | "were" | "in" | "on" | "at" | "to"
            | "for" | "of" | "with" | "by" | "from" | "and" | "or" | "not" | "it" | "this"
            | "that" | "be" | "has" | "have" | "do" | "does" | "did" | "will" | "would"
            | "could" | "should" | "can" | "may" | "i" | "me" | "my" | "we"
            // Intent trigger words (already captured by detect_intent, noise in search)
            | "find" | "where" | "locate" | "show" | "get" | "lookup" | "search"
            | "bug" | "fix" | "crash" | "broken" | "debug" | "wrong" | "issue"
            | "how" | "explain" | "understand" | "what" | "why" | "works" | "overview"
            | "impact" | "affects" | "uses" | "calls" | "callers" | "consumers"
            | "depends" | "dependents" | "change" | "refactor" | "rename"
            | "remove" | "delete" | "implemented" | "work" | "system" | "break"
            | "new" | "add" | "create" | "make" | "build" | "implement" | "support" | "need"
            | "want" | "like" | "also" | "just" | "all" | "every" | "each" | "some" | "any"
    )
}
