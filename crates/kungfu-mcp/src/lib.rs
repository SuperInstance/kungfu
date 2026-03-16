use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tracing::info;

use kungfu_core::KungfuService;
use kungfu_types::Budget;

const CACHE_CAPACITY: usize = 64;

struct CacheState {
    entries: HashMap<u64, String>,
    order: Vec<u64>,
    index_mtime: Option<SystemTime>,
    hits: u64,
    misses: u64,
    /// Total bytes returned to agent via kungfu
    bytes_served: u64,
    /// Total MCP tool calls served
    calls_served: u64,
}

impl CacheState {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            index_mtime: None,
            hits: 0,
            misses: 0,
            bytes_served: 0,
            calls_served: 0,
        }
    }

    fn get(&mut self, key: u64) -> Option<&String> {
        if let Some(val) = self.entries.get(&key) {
            self.hits += 1;
            // Move to end (most recent)
            self.order.retain(|k| *k != key);
            self.order.push(key);
            Some(val)
        } else {
            self.misses += 1;
            None
        }
    }

    fn put(&mut self, key: u64, value: String) {
        if self.entries.len() >= CACHE_CAPACITY && !self.entries.contains_key(&key) {
            // Evict oldest
            if let Some(oldest) = self.order.first().copied() {
                self.order.remove(0);
                self.entries.remove(&oldest);
            }
        }
        self.order.retain(|k| *k != key);
        self.order.push(key);
        self.entries.insert(key, value);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

fn cache_key(tool: &str, query: &str, budget: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool.hash(&mut hasher);
    query.hash(&mut hasher);
    budget.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone)]
pub struct KungfuMcp {
    project_root: PathBuf,
    tool_router: ToolRouter<Self>,
    cache: Arc<Mutex<CacheState>>,
}

impl KungfuMcp {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            tool_router: Self::tool_router(),
            cache: Arc::new(Mutex::new(CacheState::new())),
        }
    }

    fn service(&self) -> std::result::Result<KungfuService, String> {
        let svc = KungfuService::open(&self.project_root).map_err(|e| e.to_string())?;
        // Auto-reindex if stale (best-effort, don't fail on reindex errors)
        let reindexed = svc.ensure_fresh_index().unwrap_or(false);
        if reindexed {
            if let Ok(mut cache) = self.cache.lock() {
                cache.clear();
                info!("cache cleared after reindex");
            }
        }
        Ok(svc)
    }

    /// Check if index has changed since last cache validation, clear cache if so.
    fn validate_cache(&self) {
        let fp_path = self.project_root.join(".kungfu").join("index").join("fingerprints.json");
        let current_mtime = std::fs::metadata(&fp_path)
            .and_then(|m| m.modified())
            .ok();

        if let Ok(mut cache) = self.cache.lock() {
            if cache.index_mtime != current_mtime {
                cache.clear();
                cache.index_mtime = current_mtime;
            }
        }
    }

    /// Try to get a cached result, or compute and cache it.
    /// If scope is provided, it's included in the cache key and applied as a post-filter.
    fn cached(
        &self,
        tool: &str,
        query: &str,
        budget: &str,
        compute: impl FnOnce() -> std::result::Result<String, String>,
    ) -> std::result::Result<String, String> {
        self.cached_scoped(tool, query, budget, None, compute)
    }

    fn cached_scoped(
        &self,
        tool: &str,
        query: &str,
        budget: &str,
        scope: Option<&str>,
        compute: impl FnOnce() -> std::result::Result<String, String>,
    ) -> std::result::Result<String, String> {
        self.validate_cache();
        // Include scope in cache key
        let full_key = match scope {
            Some(s) if !s.is_empty() => format!("{}:{}:{}:{}", tool, query, budget, s),
            _ => format!("{}:{}:{}", tool, query, budget),
        };
        let key = cache_key(&full_key, "", "");

        // Check cache
        if let Ok(mut cache) = self.cache.lock() {
            if let Some(val) = cache.get(key) {
                let val = val.clone();
                cache.bytes_served += val.len() as u64;
                cache.calls_served += 1;
                return Ok(val);
            }
        }

        // Compute
        let result = compute()?;

        // Apply scope filter
        let result = apply_scope(&result, scope);

        // Store + track
        if let Ok(mut cache) = self.cache.lock() {
            cache.bytes_served += result.len() as u64;
            cache.calls_served += 1;
            cache.put(key, result.clone());
        }

        Ok(result)
    }
}

// --- Parameter types ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BudgetParam {
    /// Budget level: "tiny", "small", "medium", "full", or "auto". Default: "auto"
    pub budget: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilePathParam {
    /// Path to the file (relative to project root)
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryParam {
    /// Search query or symbol name
    pub query: String,
    /// Budget level: "tiny", "small", "medium", "full", or "auto". Default: "auto" (adapts to project size)
    pub budget: Option<String>,
    /// Limit results to files under this directory path prefix (e.g. "src/", "crates/kungfu-core")
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolNameParam {
    /// Exact symbol name
    pub name: String,
    /// Budget level: "tiny", "small", "medium", "full", or "auto". Default: "auto"
    pub budget: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilePathBudgetParam {
    /// Path to the file (relative to project root)
    pub path: String,
    /// Budget level: "tiny", "small", "medium", "full", or "auto". Default: "auto"
    pub budget: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolBudgetParam {
    /// Symbol name to explore
    pub name: String,
    /// Budget level: "tiny", "small", "medium", "full", or "auto". Default: "auto"
    pub budget: Option<String>,
    /// Limit results to files under this directory path prefix (e.g. "src/", "crates/kungfu-core")
    pub scope: Option<String>,
}

fn parse_budget(s: Option<&str>) -> Budget {
    s.and_then(|s| s.parse().ok()).unwrap_or(Budget::Auto)
}

/// Filter a JSON array result by scope: keep only items where "path" starts with the scope prefix.
fn apply_scope(json_str: &str, scope: Option<&str>) -> String {
    let scope = match scope {
        Some(s) if !s.is_empty() => s,
        _ => return json_str.to_string(),
    };

    // Try parsing as array
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(json_str) {
        let filtered: Vec<_> = arr
            .into_iter()
            .filter(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.starts_with(scope))
                    .unwrap_or(true)
            })
            .collect();
        return serde_json::to_string_pretty(&filtered).unwrap_or_else(|_| json_str.to_string());
    }

    // Try parsing as object with "items" array
    if let Ok(mut obj) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(items) = obj.get_mut("items").and_then(|v| v.as_array_mut()) {
            items.retain(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.starts_with(scope))
                    .unwrap_or(true)
            });
            return serde_json::to_string_pretty(&obj).unwrap_or_else(|_| json_str.to_string());
        }
        // Also check "key_symbols" and "related_files" in explore_file result
        if let Some(syms) = obj.get_mut("siblings_in_file").and_then(|v| v.as_array_mut()) {
            syms.retain(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.starts_with(scope))
                    .unwrap_or(true)
            });
        }
        if let Some(related) = obj.get_mut("related_files").and_then(|v| v.as_array_mut()) {
            related.retain(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.starts_with(scope))
                    .unwrap_or(true)
            });
        }
        if let Some(others) = obj.get_mut("other_matches").and_then(|v| v.as_array_mut()) {
            others.retain(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.starts_with(scope))
                    .unwrap_or(true)
            });
        }
        return serde_json::to_string_pretty(&obj).unwrap_or_else(|_| json_str.to_string());
    }

    json_str.to_string()
}

#[tool_router]
impl KungfuMcp {
    #[tool(description = "Show project status: file count, symbol count, languages, git status")]
    fn project_status(&self) -> Result<String, String> {
        let service = self.service()?;
        let info = service.status().map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&serde_json::json!({
            "project_name": info.project_name,
            "root": info.root,
            "indexed_files": info.indexed_files,
            "indexed_symbols": info.indexed_symbols,
            "languages": info.languages,
            "has_git": info.has_git,
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Return compact repo map: top directories, language distribution, entrypoints")]
    fn repo_outline(&self, Parameters(params): Parameters<BudgetParam>) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let outline = service.repo_outline(budget).map_err(|e| e.to_string())?;

        let dirs: Vec<_> = outline
            .top_dirs
            .iter()
            .map(|d| serde_json::json!({"path": d.path, "files": d.file_count}))
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "project": outline.project_name,
            "total_files": outline.total_files,
            "total_symbols": outline.total_symbols,
            "languages": outline.languages,
            "directories": dirs,
            "entrypoints": outline.entrypoints,
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Return compact file structure: symbols, signatures, exports")]
    fn file_outline(&self, Parameters(params): Parameters<FilePathParam>) -> Result<String, String> {
        let service = self.service()?;
        let outline = service.file_outline(&params.path).map_err(|e| e.to_string())?;

        let symbols: Vec<_> = outline
            .symbols
            .iter()
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

        serde_json::to_string_pretty(&serde_json::json!({
            "path": outline.path,
            "language": outline.language,
            "symbols": symbols,
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Search symbols by exact and fuzzy name match")]
    fn find_symbol(&self, Parameters(params): Parameters<QueryParam>) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("find_symbol", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.find_symbol(&query, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.item.name,
                        "kind": r.item.kind.to_string(),
                        "path": r.item.path,
                        "signature": r.item.signature,
                        "line": r.item.span.start_line,
                        "score": r.score,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Get detailed info about a specific symbol by exact name")]
    fn get_symbol(&self, Parameters(params): Parameters<SymbolNameParam>) -> Result<String, String> {
        let name = params.name.clone();
        self.cached("get_symbol", &name, "", || {
            let service = self.service()?;
            match service.get_symbol(&name).map_err(|e| e.to_string())? {
                Some(sym) => serde_json::to_string_pretty(&sym).map_err(|e| e.to_string()),
                None => Ok(format!("Symbol '{}' not found", name)),
            }
        })
    }

    #[tool(description = "Search text across indexed files by path and name matching")]
    fn search_text(&self, Parameters(params): Parameters<QueryParam>) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("search_text", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.search_text(&query, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "path": r.item.path,
                        "language": r.item.language,
                        "score": r.score,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Find files by path pattern or keywords")]
    fn find_files(&self, Parameters(params): Parameters<QueryParam>) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("find_files", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.search_text(&query, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "path": r.item.path,
                        "language": r.item.language,
                        "score": r.score,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Find files related to the given file by import/dependency relations, directory proximity, shared symbols, and test patterns")]
    fn find_related_files(
        &self,
        Parameters(params): Parameters<FilePathBudgetParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let path = params.path.clone();
        self.cached("find_related_files", &path, &budget_str, || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.find_related(&path, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "path": r.item.path,
                        "language": r.item.language,
                        "score": r.score,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Given a symbol, path, or query, return the smallest high-confidence context set for an agent")]
    fn get_minimal_context(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("get_minimal_context", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let packet = service.context(&query, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Given a task description, assemble a ranked context packet with relevant symbols, files, and explanations")]
    fn build_task_context(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("build_task_context", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let packet = service.context(&query, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Smart context retrieval: parse task intent, run multi-strategy search (symbols, text, related files, import chains), return ranked context packet")]
    fn ask_context(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("ask_context", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let packet = service.ask_context(&query, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Build context focused on changed code and nearby dependencies using git diff")]
    fn diff_context(&self, Parameters(params): Parameters<BudgetParam>) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let packet = service.diff_context(budget).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
    }

    #[tool(description = "Composite: explore a symbol in one call — find + detail + related symbols in same file + code snippet. Replaces find_symbol → get_symbol → find_related_symbols chain")]
    fn explore_symbol(
        &self,
        Parameters(params): Parameters<SymbolBudgetParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let name = params.name.clone();
        let scope = params.scope.clone();
        self.cached_scoped("explore_symbol", &name, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let result = service.explore_symbol(&name, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Composite: explore a file in one call — outline + related files + key symbols. Replaces file_outline → find_related_files chain")]
    fn explore_file(
        &self,
        Parameters(params): Parameters<FilePathBudgetParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let path = params.path.clone();
        self.cached("explore_file", &path, &budget_str, || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let result = service.explore_file(&path, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Composite: investigate a query in one call — smart context retrieval + diff awareness + snippets. Replaces ask_context → diff_context chain")]
    fn investigate(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("investigate", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let result = service.investigate(&query, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Find all symbols that call the given symbol (callers / 'who calls this?')")]
    fn callers(
        &self,
        Parameters(params): Parameters<SymbolBudgetParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let name = params.name.clone();
        let scope = params.scope.clone();
        self.cached_scoped("callers", &name, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.callers(&name, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|(sym, reason)| {
                    serde_json::json!({
                        "name": sym.name,
                        "kind": sym.kind.to_string(),
                        "path": sym.path,
                        "line": sym.span.start_line,
                        "signature": sym.signature,
                        "reason": reason,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Find all symbols that the given symbol calls (callees / 'what does this call?')")]
    fn callees(
        &self,
        Parameters(params): Parameters<SymbolBudgetParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let name = params.name.clone();
        let scope = params.scope.clone();
        self.cached_scoped("callees", &name, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let results = service.callees(&name, budget).map_err(|e| e.to_string())?;
            let items: Vec<_> = results
                .iter()
                .map(|(sym, reason)| {
                    serde_json::json!({
                        "name": sym.name,
                        "kind": sym.kind.to_string(),
                        "path": sym.path,
                        "line": sym.span.start_line,
                        "signature": sym.signature,
                        "reason": reason,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Semantic search: find symbols by concept, not just name. Expands query with related terms (e.g. 'auth' finds verify_token, login, session)")]
    fn semantic_search(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let query = params.query.clone();
        let scope = params.scope.clone();
        self.cached_scoped("semantic_search", &query, &budget_str, scope.as_deref(), || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let result = service.semantic_search(&query, budget).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Get git history for a file: recent commits with date, author, message")]
    fn file_history(
        &self,
        Parameters(params): Parameters<FilePathParam>,
    ) -> Result<String, String> {
        let service = self.service()?;
        let result = service.file_history(&params.path, 10).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
    }

    #[tool(description = "Get git blame + recent commits for a symbol: who changed it and why")]
    fn symbol_history(
        &self,
        Parameters(params): Parameters<SymbolNameParam>,
    ) -> Result<String, String> {
        let name = params.name.clone();
        self.cached("symbol_history", &name, "", || {
            let service = self.service()?;
            let result = service.symbol_history(&name).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        })
    }

    #[tool(description = "Show usage statistics: token savings, cache hit rate, calls served")]
    fn usage_stats(&self) -> Result<String, String> {
        let cache = self.cache.lock().map_err(|e| e.to_string())?;
        let total_cache = cache.hits + cache.misses;
        let hit_rate = if total_cache > 0 {
            (cache.hits as f64 / total_cache as f64) * 100.0
        } else {
            0.0
        };

        // Estimate raw size: each call would read ~8KB (avg file) without kungfu
        // Token estimate: ~4 chars per token
        let estimated_raw_bytes = cache.calls_served * 8192;
        let kungfu_bytes = cache.bytes_served;
        let savings_ratio = if kungfu_bytes > 0 {
            estimated_raw_bytes as f64 / kungfu_bytes as f64
        } else {
            0.0
        };
        let estimated_tokens_saved = estimated_raw_bytes.saturating_sub(kungfu_bytes) / 4;

        serde_json::to_string_pretty(&serde_json::json!({
            "calls_served": cache.calls_served,
            "bytes_served": kungfu_bytes,
            "estimated_raw_bytes": estimated_raw_bytes,
            "compression_ratio": format!("{:.1}x", savings_ratio),
            "estimated_tokens_saved": estimated_tokens_saved,
            "cache": {
                "entries": cache.entries.len(),
                "capacity": CACHE_CAPACITY,
                "hits": cache.hits,
                "misses": cache.misses,
                "hit_rate_pct": format!("{:.1}", hit_rate),
            }
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Find symbols related to a given symbol by name, path, or structural proximity")]
    fn find_related_symbols(
        &self,
        Parameters(params): Parameters<SymbolNameParam>,
    ) -> Result<String, String> {
        let budget_str = params.budget.as_deref().unwrap_or("small").to_string();
        let name = params.name.clone();
        self.cached("find_related_symbols", &name, &budget_str, || {
            let budget = parse_budget(Some(&budget_str));
            let service = self.service()?;
            let sym = service.get_symbol(&name).map_err(|e| e.to_string())?;
            match sym {
                Some(s) => {
                    let file_outline = service.file_outline(&s.path).map_err(|e| e.to_string())?;
                    let items: Vec<_> = file_outline
                        .symbols
                        .iter()
                        .filter(|os| os.name != name)
                        .take(budget.top_k())
                        .map(|os| {
                            serde_json::json!({
                                "name": os.name,
                                "kind": os.kind,
                                "path": s.path,
                                "line": os.line,
                            })
                        })
                        .collect();
                    serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
                }
                None => Ok(format!("Symbol '{}' not found", name)),
            }
        })
    }
}

#[tool_handler]
impl ServerHandler for KungfuMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Kungfu is a context retrieval and distillation engine for coding agents. Use its tools to explore project structure, find symbols, search code, and get minimal context packets.")
    }
}

pub async fn run_stdio_server(project_root: PathBuf) -> Result<()> {
    info!("starting kungfu MCP server (stdio)");

    let server = KungfuMcp::new(project_root);
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}
