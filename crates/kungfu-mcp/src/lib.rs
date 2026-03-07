use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::info;

use kungfu_core::KungfuService;
use kungfu_types::Budget;

#[derive(Clone)]
pub struct KungfuMcp {
    project_root: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl KungfuMcp {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            tool_router: Self::tool_router(),
        }
    }

    fn service(&self) -> std::result::Result<KungfuService, String> {
        KungfuService::open(&self.project_root).map_err(|e| e.to_string())
    }
}

// --- Parameter types ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BudgetParam {
    /// Budget level: "small", "medium", or "full". Default: "small"
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
    /// Budget level: "small", "medium", or "full". Default: "small"
    pub budget: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolNameParam {
    /// Exact symbol name
    pub name: String,
    /// Budget level: "small", "medium", or "full". Default: "small"
    pub budget: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilePathBudgetParam {
    /// Path to the file (relative to project root)
    pub path: String,
    /// Budget level: "small", "medium", or "full". Default: "small"
    pub budget: Option<String>,
}

fn parse_budget(s: Option<&str>) -> Budget {
    s.and_then(|s| s.parse().ok()).unwrap_or(Budget::Small)
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
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let results = service
            .find_symbol(&params.query, budget)
            .map_err(|e| e.to_string())?;

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
    }

    #[tool(description = "Get detailed info about a specific symbol by exact name")]
    fn get_symbol(&self, Parameters(params): Parameters<SymbolNameParam>) -> Result<String, String> {
        let service = self.service()?;
        match service.get_symbol(&params.name).map_err(|e| e.to_string())? {
            Some(sym) => serde_json::to_string_pretty(&sym).map_err(|e| e.to_string()),
            None => Ok(format!("Symbol '{}' not found", params.name)),
        }
    }

    #[tool(description = "Search text across indexed files by path and name matching")]
    fn search_text(&self, Parameters(params): Parameters<QueryParam>) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let results = service
            .search_text(&params.query, budget)
            .map_err(|e| e.to_string())?;

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
    }

    #[tool(description = "Find files by path pattern or keywords")]
    fn find_files(&self, Parameters(params): Parameters<QueryParam>) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let results = service
            .search_text(&params.query, budget)
            .map_err(|e| e.to_string())?;

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
    }

    #[tool(description = "Find files related to the given file by import graph, test proximity, and path heuristics")]
    fn find_related_files(
        &self,
        Parameters(params): Parameters<FilePathBudgetParam>,
    ) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let results = service
            .search_text(&params.path, budget)
            .map_err(|e| e.to_string())?;

        let items: Vec<_> = results
            .iter()
            .filter(|r| r.item.path != params.path)
            .map(|r| serde_json::json!({"path": r.item.path, "score": r.score}))
            .collect();

        serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
    }

    #[tool(description = "Given a symbol, path, or query, return the smallest high-confidence context set for an agent")]
    fn get_minimal_context(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let packet = service
            .context(&params.query, budget)
            .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
    }

    #[tool(description = "Given a task description, assemble a ranked context packet with relevant symbols, files, and explanations")]
    fn build_task_context(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let packet = service
            .context(&params.query, budget)
            .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
    }

    #[tool(description = "Build context focused on changed code and nearby dependencies using git diff")]
    fn diff_context(&self, Parameters(params): Parameters<BudgetParam>) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;
        let packet = service.diff_context(budget).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&packet).map_err(|e| e.to_string())
    }

    #[tool(description = "Find symbols related to a given symbol by name, path, or structural proximity")]
    fn find_related_symbols(
        &self,
        Parameters(params): Parameters<SymbolNameParam>,
    ) -> Result<String, String> {
        let budget = parse_budget(params.budget.as_deref());
        let service = self.service()?;

        let sym = service.get_symbol(&params.name).map_err(|e| e.to_string())?;
        match sym {
            Some(s) => {
                let file_outline = service.file_outline(&s.path).map_err(|e| e.to_string())?;
                let items: Vec<_> = file_outline
                    .symbols
                    .iter()
                    .filter(|os| os.name != params.name)
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
            None => Ok(format!("Symbol '{}' not found", params.name)),
        }
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
