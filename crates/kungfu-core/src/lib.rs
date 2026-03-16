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

    /// Resolve Budget::Auto to a concrete budget based on project size.
    pub fn resolve_budget(&self, budget: Budget) -> Budget {
        if budget != Budget::Auto {
            return budget;
        }
        let file_count = self.store().load_files().map(|f| f.len()).unwrap_or(0);
        budget.resolve(file_count)
    }

    /// Check if index is stale and auto-reindex if needed.
    /// Compares fingerprints.json mtime with project files.
    pub fn ensure_fresh_index(&self) -> Result<bool> {
        let fp_path = self.project.index_dir().join("fingerprints.json");
        if !fp_path.exists() {
            // No index at all — full index needed
            info!("no index found, running full index");
            self.index_full()?;
            return Ok(true);
        }

        let fp_mtime = std::fs::metadata(&fp_path)?.modified()?;

        // Sample a few key project files for staleness check (fast heuristic)
        let root = &self.project.root;
        let markers = ["Cargo.toml", "package.json", "go.mod", "pyproject.toml", "Cargo.lock", "package-lock.json", "bun.lock"];
        let mut stale = false;
        for marker in &markers {
            let p = root.join(marker);
            if p.exists() {
                if let Ok(meta) = std::fs::metadata(&p) {
                    if let Ok(mtime) = meta.modified() {
                        if mtime > fp_mtime {
                            stale = true;
                            break;
                        }
                    }
                }
            }
        }

        // Also check src/ directory for any file newer than index
        if !stale {
            let src_dirs = ["src", "crates", "packages", "lib", "app", "server", "client"];
            'outer: for dir in &src_dirs {
                let d = root.join(dir);
                if d.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&d) {
                        for entry in entries.take(20) {
                            if let Ok(entry) = entry {
                                if let Ok(meta) = entry.metadata() {
                                    if let Ok(mtime) = meta.modified() {
                                        if mtime > fp_mtime {
                                            stale = true;
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if stale {
            info!("index is stale, running incremental reindex");
            self.index_incremental()?;
            Ok(true)
        } else {
            Ok(false)
        }
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
        let budget = self.resolve_budget(budget);
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
        let budget = self.resolve_budget(budget);
        self.search().find_symbol(query, budget)
    }

    pub fn get_symbol(&self, name: &str) -> Result<Option<Symbol>> {
        self.search().get_symbol(name)
    }

    pub fn search_text(&self, query: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        let budget = self.resolve_budget(budget);
        self.search().search_text(query, budget)
    }

    pub fn find_related(&self, file_path: &str, budget: Budget) -> Result<Vec<SearchResult<FileEntry>>> {
        let budget = self.resolve_budget(budget);
        self.search().find_related(file_path, budget)
    }

    pub fn context(&self, query: &str, budget: Budget) -> Result<ContextPacket> {
        let budget = self.resolve_budget(budget);
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
            self.fill_snippets(&mut packet, snippet_lines, &[]);
        }

        Ok(packet)
    }

    /// High-level context retrieval: parse intent, run multi-strategy search,
    /// rank with contextual signals, return compact packet.
    pub fn ask_context(&self, task: &str, budget: Budget) -> Result<ContextPacket> {
        let budget = self.resolve_budget(budget);
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

        // Strategy B: text/file search — only add keyword-relevant symbols
        let file_results = search.search_text(&keyword_query, Budget::Full)?;
        let all_symbols = search.get_all_symbols()?;
        for fr in &file_results {
            let file_syms: Vec<_> = all_symbols
                .iter()
                .filter(|s| s.file_id == fr.item.id && !seen_ids.contains(&s.id))
                .filter(|s| {
                    let name_lower = s.name.to_lowercase();
                    let sig_lower = s.signature.as_deref().unwrap_or("").to_lowercase();
                    keywords
                        .iter()
                        .any(|kw| name_lower.contains(*kw) || sig_lower.contains(*kw))
                })
                .take(3)
                .collect();
            for sym in file_syms {
                seen_ids.insert(sym.id.clone());
                scored_symbols.push(ScoredSymbol {
                    symbol: sym.clone(),
                    score: fr.score * 0.7,
                    reason: format!("in matched file {}", fr.item.path),
                });
            }
        }

        // Strategy B2: content grep — search file bodies for keywords
        if scored_symbols.len() < budget.top_k() {
            let content_matches = self.grep_content(&keywords, &seen_ids, budget.top_k());
            for (sym, matched_line) in content_matches {
                seen_ids.insert(sym.id.clone());
                scored_symbols.push(ScoredSymbol {
                    symbol: sym,
                    score: 0.45,
                    reason: format!("content match: {}", matched_line),
                });
            }
        }

        // Strategy B3: semantic expansion — search with conceptually related terms
        if scored_symbols.len() < budget.top_k() {
            let expanded = kungfu_search::expand_query(&keywords);
            // Only use new terms (not original keywords)
            let new_terms: Vec<&str> = expanded
                .iter()
                .filter(|t| !keywords.contains(&t.as_str()))
                .map(|t| t.as_str())
                .collect();

            if !new_terms.is_empty() {
                let expanded_query = new_terms.join(" ");
                let sem_results = search.find_symbol(&expanded_query, Budget::Full)?;
                for r in sem_results {
                    if seen_ids.contains(&r.item.id) {
                        continue;
                    }
                    // Lower score for semantic matches — they're conceptual, not exact
                    if r.score >= 0.5 {
                        seen_ids.insert(r.item.id.clone());
                        scored_symbols.push(ScoredSymbol {
                            symbol: r.item,
                            score: r.score * 0.5,
                            reason: "semantic match (related concept)".to_string(),
                        });
                    }
                }
            }
        }

        // Strategy C: sibling symbols from top match's file (important for impact/understand)
        if matches!(intent, Intent::Impact | Intent::Understand) {
            if let Some(top) = scored_symbols
                .iter()
                .filter(|s| s.reason == "symbol name match")
                .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            {
                let top_file_id = top.symbol.file_id.clone();
                let top_score = top.score;

                // Add new siblings, scored by keyword relevance
                let mut siblings: Vec<_> = all_symbols
                    .iter()
                    .filter(|s| s.file_id == top_file_id && !seen_ids.contains(&s.id))
                    .map(|s| {
                        let name_lower = s.name.to_lowercase();
                        let sig_lower = s.signature.as_deref().unwrap_or("").to_lowercase();
                        let relevance: usize = keywords
                            .iter()
                            .filter(|kw| name_lower.contains(*kw) || sig_lower.contains(*kw))
                            .count();
                        (s, relevance)
                    })
                    .collect();
                siblings.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| b.0.exported.cmp(&a.0.exported))
                });
                // Impact/understand: allow more siblings since we want the full picture
                let max_siblings = if intent == Intent::Impact { 5 } else { 3 };
                for (sym, relevance) in siblings.iter().take(max_siblings) {
                    seen_ids.insert(sym.id.clone());
                    // Relevant siblings score close to the match; irrelevant ones much lower
                    let score = if *relevance > 0 {
                        top_score * 0.9
                    } else {
                        top_score * 0.4
                    };
                    scored_symbols.push(ScoredSymbol {
                        symbol: (*sym).clone(),
                        score,
                        reason: "same file as matched symbol".to_string(),
                    });
                }

                // Also boost existing symbols from that file if keyword-relevant
                for s in &mut scored_symbols {
                    if s.symbol.file_id == top_file_id
                        && s.reason != "symbol name match"
                        && s.reason != "same file as matched symbol"
                    {
                        let name_lower = s.symbol.name.to_lowercase();
                        let sig_lower =
                            s.symbol.signature.as_deref().unwrap_or("").to_lowercase();
                        let is_relevant = keywords
                            .iter()
                            .any(|kw| name_lower.contains(*kw) || sig_lower.contains(*kw));
                        if is_relevant && s.score < top_score * 0.9 {
                            s.score = top_score * 0.9;
                            s.reason = "same file as matched symbol".to_string();
                        }
                    }
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

        // Path/directory boost: if keyword matches a directory or filename, boost those symbols
        for s in &mut scored_symbols {
            let path_lower = s.symbol.path.to_lowercase();
            let path_match = keywords.iter().any(|kw| {
                kw.len() >= 3 && path_lower.split('/').any(|seg| {
                    seg.contains(kw) || seg.trim_end_matches(".ts").trim_end_matches(".js")
                        .trim_end_matches(".rs").trim_end_matches(".py").trim_end_matches(".go")
                        .contains(kw)
                })
            });
            if path_match {
                s.score += 0.15;
                if !s.reason.contains("path match") {
                    s.reason = format!("{}, path match", s.reason);
                }
            }
        }

        // File-level fallback: if best symbol score is weak, inject file-level results
        let best_score = scored_symbols.iter().map(|s| s.score).fold(0.0f64, f64::max);
        if best_score < 0.6 {
            for fr in &file_results {
                let path_lower = fr.item.path.to_lowercase();
                let path_match = keywords.iter().any(|kw| kw.len() >= 3 && path_lower.contains(kw));
                if path_match && !seen_ids.contains(&fr.item.id) {
                    // Pick the top exported symbol from this file as representative
                    if let Some(rep) = all_symbols
                        .iter()
                        .filter(|s| s.file_id == fr.item.id && !seen_ids.contains(&s.id))
                        .max_by_key(|s| (s.exported as u8, s.span.end_line - s.span.start_line))
                    {
                        seen_ids.insert(rep.id.clone());
                        scored_symbols.push(ScoredSymbol {
                            symbol: rep.clone(),
                            score: 0.55,
                            reason: format!("file path match: {}", fr.item.path),
                        });
                    }
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
        let changed = if kungfu_git::is_git_repo(&self.project.root) {
            kungfu_git::changed_files(&self.project.root).unwrap_or_default()
        } else {
            Vec::new()
        };

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

        // 5. Build packet
        let mut packet = build_context_packet_full(
            task,
            scored_symbols,
            budget,
            Some(intent),
        );

        // 6. Attach changed files list
        packet.changed_files = changed;

        // 7. Extract snippets based on budget
        let snippet_lines = budget.max_lines();
        if snippet_lines > 0 {
            self.fill_snippets(&mut packet, snippet_lines, &keywords);
        }

        Ok(packet)
    }

    /// Grep file contents for keywords, return matching symbols with the matched line.
    fn grep_content(
        &self,
        keywords: &[&str],
        seen_ids: &HashSet<String>,
        limit: usize,
    ) -> Vec<(Symbol, String)> {
        if keywords.is_empty() {
            return Vec::new();
        }

        let store = self.store();
        let files = store.load_files().unwrap_or_default();
        let symbols = store.load_symbols().unwrap_or_default();

        // Build file_id → symbols map
        let mut file_symbols: HashMap<&str, Vec<&Symbol>> = HashMap::new();
        for sym in &symbols {
            if !seen_ids.contains(&sym.id) {
                file_symbols.entry(sym.file_id.as_str()).or_default().push(sym);
            }
        }

        let mut results: Vec<(Symbol, String)> = Vec::new();

        // Only scan code files
        for f in &files {
            if results.len() >= limit {
                break;
            }

            let lang = f.language.as_deref().unwrap_or("");
            if !matches!(lang, "rust" | "typescript" | "javascript" | "python" | "go") {
                continue;
            }

            let abs_path = self.project.root.join(&f.path);
            let content = match std::fs::read_to_string(&abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Check if any keyword appears in file content
            let content_lower = content.to_lowercase();
            let matched_keyword = keywords.iter().find(|kw| {
                content_lower.contains(*kw)
                    || kungfu_search::simple_stem(kw)
                        .map_or(false, |s| content_lower.contains(&s))
            });

            let keyword = match matched_keyword {
                Some(kw) => *kw,
                None => continue,
            };

            // Find the best matched line
            let matched_line = content
                .lines()
                .find(|line| {
                    let ll = line.to_lowercase();
                    ll.contains(keyword)
                        || kungfu_search::simple_stem(keyword)
                            .map_or(false, |s| ll.contains(&s))
                })
                .unwrap_or("")
                .trim();

            if matched_line.is_empty() {
                continue;
            }

            let snippet = if matched_line.len() > 100 {
                format!("{}...", &matched_line[..100])
            } else {
                matched_line.to_string()
            };

            // Find the best symbol in this file to attach the match to
            if let Some(file_syms) = file_symbols.get(f.id.as_str()) {
                // Prefer symbol whose span contains the match
                let match_line_num = content
                    .lines()
                    .enumerate()
                    .find(|(_, line)| {
                        let ll = line.to_lowercase();
                        ll.contains(keyword)
                            || kungfu_search::simple_stem(keyword)
                                .map_or(false, |s| ll.contains(&s))
                    })
                    .map(|(i, _)| i + 1)
                    .unwrap_or(0);

                let best = file_syms
                    .iter()
                    .filter(|s| s.span.start_line <= match_line_num && s.span.end_line >= match_line_num)
                    .min_by_key(|s| s.span.end_line - s.span.start_line) // smallest containing symbol
                    .or_else(|| file_syms.first()); // fallback: first symbol in file

                if let Some(sym) = best {
                    if !seen_ids.contains(&sym.id) {
                        results.push(((*sym).clone(), snippet));
                    }
                }
            }
        }

        results
    }

    /// Fill snippet fields in context packet items by reading source files.
    /// If keywords are provided, extract lines containing those keywords with context.
    /// Falls back to first N lines of symbol if no keyword matches found.
    fn fill_snippets(&self, packet: &mut ContextPacket, max_lines: usize, keywords: &[&str]) {
        let mut file_cache: HashMap<String, Vec<String>> = HashMap::new();
        let all_symbols = self.search().get_all_symbols().unwrap_or_default();

        for item in &mut packet.items {
            let span = all_symbols
                .iter()
                .find(|s| s.path == item.path && s.name == item.name)
                .map(|s| (s.span.start_line, s.span.end_line));

            let (start, end) = match span {
                Some(s) => s,
                None => continue,
            };

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

            let start_idx = start.saturating_sub(1);
            let end_idx = end.min(lines.len());

            // Try keyword-relevant extraction first
            if !keywords.is_empty() && end_idx > start_idx {
                let relevant = extract_keyword_lines(lines, start_idx, end_idx, keywords, max_lines);
                if !relevant.is_empty() {
                    item.snippet = Some(relevant);
                    continue;
                }
            }

            // Fallback: first max_lines of symbol
            let symbol_len = end_idx - start_idx;
            let take = symbol_len.min(max_lines);
            let snippet: Vec<&str> = lines[start_idx..start_idx + take]
                .iter()
                .map(|s| s.as_str())
                .collect();
            if !snippet.is_empty() {
                item.snippet = Some(snippet.join("\n"));
            }
        }
    }

    /// Composite: explore a symbol — find + detail + related symbols + snippets in one call.
    pub fn explore_symbol(&self, name: &str, budget: Budget) -> Result<serde_json::Value> {
        let budget = self.resolve_budget(budget);
        let search = self.search();

        // 1. Find symbol candidates
        let candidates = search.find_symbol(name, budget)?;

        // 2. Pick best candidate — on tie prefer: definitions > variables, src > test, exported
        let (symbol, score) = if let Some(best) = candidates
            .iter()
            .max_by(|a, b| {
                a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        // Strongest signal: prefer source over test/example paths
                        fn is_non_src(path: &str) -> bool {
                            path.contains("test") || path.contains("example")
                                || path.contains("spec/") || path.contains("fixture")
                                || path.contains("evals/") || path.contains("/bench/")
                                || path.contains("__tests__") || path.contains("__mocks__")
                        }
                        let a_test = is_non_src(&a.item.path);
                        let b_test = is_non_src(&b.item.path);
                        b_test.cmp(&a_test)
                    })
                    .then_with(|| {
                        // Prefer exact case match
                        let a_exact = a.item.name == name;
                        let b_exact = b.item.name == name;
                        a_exact.cmp(&b_exact)
                    })
                    .then_with(|| {
                        // Prefer definition kinds over variables/modules
                        fn kind_rank(sym: &Symbol) -> u8 {
                            match sym.kind {
                                kungfu_types::symbol::SymbolKind::Class => 5,
                                kungfu_types::symbol::SymbolKind::Struct => 5,
                                kungfu_types::symbol::SymbolKind::Trait => 5,
                                kungfu_types::symbol::SymbolKind::Interface => 5,
                                kungfu_types::symbol::SymbolKind::Enum => 4,
                                kungfu_types::symbol::SymbolKind::Function => 3,
                                kungfu_types::symbol::SymbolKind::Method => 3,
                                kungfu_types::symbol::SymbolKind::Impl => 2,
                                kungfu_types::symbol::SymbolKind::Module => 1,
                                _ => 0,
                            }
                        }
                        kind_rank(&a.item).cmp(&kind_rank(&b.item))
                    })
                    .then_with(|| {
                        // Prefer larger symbols (class definition > getter/field)
                        let a_size = a.item.span.end_line.saturating_sub(a.item.span.start_line);
                        let b_size = b.item.span.end_line.saturating_sub(b.item.span.start_line);
                        a_size.cmp(&b_size)
                    })
                    .then_with(|| a.item.exported.cmp(&b.item.exported))
            })
        {
            (best.item.clone(), best.score)
        } else if let Some(sym) = search.get_symbol(name)? {
            (sym, 1.0)
        } else {
            return Ok(serde_json::json!({ "error": format!("Symbol '{}' not found", name) }));
        };

        // 3. File outline for related symbols
        let file_outline = self.file_outline(&symbol.path)?;
        let siblings: Vec<_> = file_outline
            .symbols
            .iter()
            .filter(|s| s.name != symbol.name)
            .take(budget.top_k())
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "kind": s.kind,
                    "line": s.line,
                    "exported": s.exported,
                })
            })
            .collect();

        // 4. Snippet for the primary symbol
        let snippet = self.symbol_snippet(&symbol, 30);

        // 5. Other candidates (if fuzzy matched)
        let other_matches: Vec<_> = candidates
            .iter()
            .filter(|c| c.item.id != symbol.id)
            .take(5)
            .map(|c| {
                serde_json::json!({
                    "name": c.item.name,
                    "kind": c.item.kind.to_string(),
                    "path": c.item.path,
                    "line": c.item.span.start_line,
                    "score": c.score,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "symbol": {
                "name": symbol.name,
                "kind": symbol.kind.to_string(),
                "path": symbol.path,
                "line": symbol.span.start_line,
                "end_line": symbol.span.end_line,
                "signature": symbol.signature,
                "exported": symbol.exported,
                "language": symbol.language,
                "score": score,
            },
            "snippet": snippet,
            "siblings_in_file": siblings,
            "other_matches": other_matches,
        }))
    }

    /// Composite: explore a file — outline + related files + key symbols in one call.
    pub fn explore_file(&self, path: &str, budget: Budget) -> Result<serde_json::Value> {
        let budget = self.resolve_budget(budget);
        // 1. File outline
        let outline = self.file_outline(path)?;

        // 2. Related files
        let related = self.find_related(path, budget).unwrap_or_default();
        let related_files: Vec<_> = related
            .iter()
            .map(|r| {
                serde_json::json!({
                    "path": r.item.path,
                    "language": r.item.language,
                    "score": r.score,
                })
            })
            .collect();

        // 3. Key symbols (exported first, then by line order)
        let mut key_symbols: Vec<_> = outline.symbols.iter().collect();
        key_symbols.sort_by(|a, b| b.exported.cmp(&a.exported).then(a.line.cmp(&b.line)));
        let key_symbols: Vec<_> = key_symbols
            .iter()
            .take(budget.top_k() * 2)
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "kind": s.kind,
                    "signature": s.signature,
                    "line": s.line,
                    "exported": s.exported,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "path": outline.path,
            "language": outline.language,
            "total_symbols": outline.symbols.len(),
            "key_symbols": key_symbols,
            "related_files": related_files,
        }))
    }

    /// Composite: investigate a query — ask_context + diff boost in one call.
    pub fn investigate(&self, query: &str, budget: Budget) -> Result<serde_json::Value> {
        let budget = self.resolve_budget(budget);
        // 1. Main context via ask_context
        let packet = self.ask_context(query, budget)?;

        // 2. Gather diff info if available
        let diff_info = if kungfu_git::is_git_repo(&self.project.root) {
            let changed = kungfu_git::changed_files(&self.project.root).unwrap_or_default();
            if changed.is_empty() {
                None
            } else {
                // Find which changed files overlap with context results
                let relevant_changes: Vec<_> = changed
                    .iter()
                    .filter(|c| {
                        packet.items.iter().any(|item| {
                            item.path.ends_with(c.as_str()) || c.ends_with(&item.path)
                        })
                    })
                    .cloned()
                    .collect();
                Some(serde_json::json!({
                    "total_changed_files": changed.len(),
                    "relevant_changed_files": relevant_changes,
                }))
            }
        } else {
            None
        };

        // 3. Build combined result
        let items: Vec<_> = packet
            .items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.name,
                    "type": item.item_type,
                    "path": item.path,
                    "score": item.score,
                    "why": item.why,
                    "signature": item.signature,
                    "snippet": item.snippet,
                })
            })
            .collect();

        let mut result = serde_json::json!({
            "query": packet.query,
            "intent": packet.intent.map(|i| format!("{:?}", i)),
            "budget": format!("{:?}", packet.budget),
            "items": items,
        });

        if let Some(diff) = diff_info {
            result.as_object_mut().unwrap().insert("diff".to_string(), diff);
        }

        Ok(result)
    }

    /// Extract a snippet for a single symbol (helper for composite tools).
    fn symbol_snippet(&self, symbol: &Symbol, max_lines: usize) -> Option<String> {
        let abs_path = self.project.root.join(&symbol.path);
        let content = std::fs::read_to_string(&abs_path).ok()?;
        let lines: Vec<&str> = content.lines().collect();

        let start = symbol.span.start_line.saturating_sub(1);
        let end = symbol.span.end_line.min(lines.len());
        if start >= end {
            return None;
        }

        let take = (end - start).min(max_lines);
        let snippet: Vec<&str> = lines[start..start + take].to_vec();
        if snippet.is_empty() {
            None
        } else {
            Some(snippet.join("\n"))
        }
    }

    /// Find all symbols that call the given symbol (callers / "who calls this?").
    pub fn callers(&self, name: &str, budget: Budget) -> Result<Vec<(Symbol, String)>> {
        let budget = self.resolve_budget(budget);
        let store = self.store();
        let relations = store.load_relations()?;
        let all_symbols = self.search().get_all_symbols()?;

        // Find target symbol IDs matching name
        let target_ids: HashSet<&str> = all_symbols
            .iter()
            .filter(|s| s.name == name)
            .map(|s| s.id.as_str())
            .collect();

        if target_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Find Calls relations where target is our symbol
        let caller_ids: Vec<&str> = relations
            .iter()
            .filter(|r| r.kind == RelationKind::Calls && target_ids.contains(r.target_id.as_str()))
            .map(|r| r.source_id.as_str())
            .collect();

        let mut results: Vec<(Symbol, String)> = Vec::new();
        let mut seen = HashSet::new();
        for caller_id in &caller_ids {
            if seen.contains(caller_id) {
                continue;
            }
            if let Some(sym) = all_symbols.iter().find(|s| s.id == *caller_id) {
                seen.insert(*caller_id);
                results.push((sym.clone(), format!("calls {}", name)));
            }
        }

        results.truncate(budget.top_k());
        Ok(results)
    }

    /// Find all symbols that the given symbol calls (callees / "what does this call?").
    pub fn callees(&self, name: &str, budget: Budget) -> Result<Vec<(Symbol, String)>> {
        let budget = self.resolve_budget(budget);
        let store = self.store();
        let relations = store.load_relations()?;
        let all_symbols = self.search().get_all_symbols()?;

        // Find source symbol IDs matching name
        let source_ids: HashSet<&str> = all_symbols
            .iter()
            .filter(|s| s.name == name)
            .map(|s| s.id.as_str())
            .collect();

        if source_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Find Calls relations where source is our symbol
        let callee_ids: Vec<&str> = relations
            .iter()
            .filter(|r| r.kind == RelationKind::Calls && source_ids.contains(r.source_id.as_str()))
            .map(|r| r.target_id.as_str())
            .collect();

        let mut results: Vec<(Symbol, String)> = Vec::new();
        let mut seen = HashSet::new();
        for callee_id in &callee_ids {
            if seen.contains(callee_id) {
                continue;
            }
            if let Some(sym) = all_symbols.iter().find(|s| s.id == *callee_id) {
                seen.insert(*callee_id);
                results.push((sym.clone(), format!("called by {}", name)));
            }
        }

        results.truncate(budget.top_k());
        Ok(results)
    }

    /// Semantic search: expand query with related concepts, then search symbols.
    pub fn semantic_search(&self, query: &str, budget: Budget) -> Result<serde_json::Value> {
        let budget = self.resolve_budget(budget);
        let query_lower = query.to_lowercase();
        let words: Vec<&str> = query_lower.split_whitespace().collect();

        let keywords: Vec<&str> = words
            .iter()
            .filter(|w| !is_stop_word(w))
            .copied()
            .collect();

        let expanded = kungfu_search::expand_query(&keywords);
        let new_terms: Vec<&str> = expanded
            .iter()
            .filter(|t| !keywords.contains(&t.as_str()))
            .map(|t| t.as_str())
            .collect();

        let search = self.search();
        let mut results = Vec::new();
        let mut seen = HashSet::new();

        // Search with original keywords
        let keyword_query = keywords.join(" ");
        for r in search.find_symbol(&keyword_query, Budget::Full)? {
            if seen.insert(r.item.id.clone()) {
                results.push(serde_json::json!({
                    "name": r.item.name,
                    "kind": r.item.kind.to_string(),
                    "path": r.item.path,
                    "line": r.item.span.start_line,
                    "score": r.score,
                    "match_type": "direct",
                }));
            }
        }

        // Search with expanded terms
        if !new_terms.is_empty() {
            let expanded_query = new_terms.join(" ");
            for r in search.find_symbol(&expanded_query, Budget::Full)? {
                if seen.insert(r.item.id.clone()) && r.score >= 0.5 {
                    results.push(serde_json::json!({
                        "name": r.item.name,
                        "kind": r.item.kind.to_string(),
                        "path": r.item.path,
                        "line": r.item.span.start_line,
                        "score": r.score * 0.6,
                        "match_type": "semantic",
                    }));
                }
            }
        }

        // Sort by score and truncate
        results.sort_by(|a, b| {
            b["score"].as_f64().unwrap_or(0.0)
                .partial_cmp(&a["score"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(budget.top_k());

        Ok(serde_json::json!({
            "query": query,
            "keywords": keywords,
            "expanded_terms": new_terms,
            "results": results,
        }))
    }

    /// Get git history for a file: recent commits.
    pub fn file_history(&self, path: &str, max_entries: usize) -> Result<serde_json::Value> {
        if !kungfu_git::is_git_repo(&self.project.root) {
            bail!("not a git repository");
        }
        let entries = kungfu_git::file_log(&self.project.root, path, max_entries)?;
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "hash": e.hash,
                    "date": e.date,
                    "author": e.author,
                    "message": e.message,
                })
            })
            .collect();
        Ok(serde_json::json!({ "path": path, "commits": items }))
    }

    /// Get git blame for a symbol: who changed its code and why.
    pub fn symbol_history(&self, name: &str) -> Result<serde_json::Value> {
        if !kungfu_git::is_git_repo(&self.project.root) {
            bail!("not a git repository");
        }
        let sym = self.search().get_symbol(name)?;
        let symbol = match sym {
            Some(s) => s,
            None => return Ok(serde_json::json!({ "error": format!("Symbol '{}' not found", name) })),
        };

        let blame = kungfu_git::blame_lines(
            &self.project.root,
            &symbol.path,
            symbol.span.start_line,
            symbol.span.end_line,
        )
        .unwrap_or_default();

        let blame_items: Vec<_> = blame
            .iter()
            .map(|b| {
                serde_json::json!({
                    "hash": b.hash,
                    "author": b.author,
                    "date": b.date,
                    "summary": b.summary,
                })
            })
            .collect();

        let log = kungfu_git::file_log(&self.project.root, &symbol.path, 5).unwrap_or_default();
        let log_items: Vec<_> = log
            .iter()
            .map(|e| {
                serde_json::json!({
                    "hash": e.hash,
                    "date": e.date,
                    "author": e.author,
                    "message": e.message,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "symbol": name,
            "path": symbol.path,
            "lines": format!("{}-{}", symbol.span.start_line, symbol.span.end_line),
            "blame": blame_items,
            "recent_commits": log_items,
        }))
    }

    pub fn diff_context(&self, budget: Budget) -> Result<ContextPacket> {
        let budget = self.resolve_budget(budget);
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
                changed_files: Vec::new(),
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

/// Extract lines from a symbol body that contain query keywords, with 1 line of context.
fn extract_keyword_lines(
    lines: &[String],
    start_idx: usize,
    end_idx: usize,
    keywords: &[&str],
    max_lines: usize,
) -> String {
    use kungfu_search::simple_stem;

    // Find line indices within symbol that contain any keyword (or stem)
    let mut hit_indices: Vec<usize> = Vec::new();
    for i in start_idx..end_idx {
        let line_lower = lines[i].to_lowercase();
        let matches = keywords.iter().any(|kw| {
            line_lower.contains(kw)
                || simple_stem(kw).map_or(false, |s| line_lower.contains(&s))
        });
        if matches {
            hit_indices.push(i);
        }
    }

    if hit_indices.is_empty() {
        return String::new();
    }

    // Always include first line (signature) + keyword-matched lines with 1 line context
    let mut include: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    include.insert(start_idx); // signature line

    for &idx in &hit_indices {
        let ctx_start = idx.saturating_sub(1).max(start_idx);
        let ctx_end = (idx + 2).min(end_idx);
        for i in ctx_start..ctx_end {
            include.insert(i);
        }
    }

    // Build snippet, inserting "..." for gaps
    let indices: Vec<usize> = include.into_iter().collect();
    let mut result = Vec::new();
    let mut prev: Option<usize> = None;

    for &i in &indices {
        if result.len() >= max_lines {
            break;
        }
        if let Some(p) = prev {
            if i > p + 1 {
                result.push("    ...".to_string());
            }
        }
        // Highlight keyword lines with >>> marker
        if hit_indices.contains(&i) {
            result.push(format!(">>> {}", highlight_keywords(&lines[i], keywords)));
        } else {
            result.push(lines[i].clone());
        }
        prev = Some(i);
    }

    result.join("\n")
}

/// Highlight keyword occurrences in a line by wrapping them with «» markers.
fn highlight_keywords(line: &str, keywords: &[&str]) -> String {
    use kungfu_search::simple_stem;

    let line_lower = line.to_lowercase();
    // Collect all match positions (start, end) in the original line
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for kw in keywords {
        // Find all occurrences of keyword (case-insensitive)
        let mut pos = 0;
        while let Some(idx) = line_lower[pos..].find(kw) {
            let start = pos + idx;
            let end = start + kw.len();
            matches.push((start, end));
            pos = end;
        }
        // Also try stem
        if let Some(stem) = simple_stem(kw) {
            pos = 0;
            while let Some(idx) = line_lower[pos..].find(&stem) {
                let start = pos + idx;
                let end = start + stem.len();
                matches.push((start, end));
                pos = end;
            }
        }
    }

    if matches.is_empty() {
        return line.to_string();
    }

    // Sort by start position and merge overlapping ranges
    matches.sort_by_key(|&(s, _)| s);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in matches {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }

    // Build highlighted string
    let mut result = String::with_capacity(line.len() + merged.len() * 4);
    let mut cursor = 0;
    for (s, e) in &merged {
        result.push_str(&line[cursor..*s]);
        result.push('«');
        result.push_str(&line[*s..*e]);
        result.push('»');
        cursor = *e;
    }
    result.push_str(&line[cursor..]);
    result
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
