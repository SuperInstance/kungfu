use anyhow::{bail, Result};
use kungfu_index::Indexer;
use kungfu_project::Project;
use kungfu_rank::build_context_packet;
use kungfu_search::{SearchEngine, SearchResult};
use kungfu_storage::JsonStore;
use kungfu_types::budget::Budget;
use kungfu_types::context::ContextPacket;
use kungfu_types::file::FileEntry;
use kungfu_types::symbol::Symbol;
use std::collections::HashMap;
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

        // Search symbols
        let symbol_results = search.find_symbol(query, Budget::Full)?;

        let scored_symbols: Vec<(Symbol, f64)> = symbol_results
            .into_iter()
            .map(|r| (r.item, r.score))
            .collect();

        Ok(build_context_packet(query, scored_symbols, budget))
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
