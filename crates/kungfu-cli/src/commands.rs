use anyhow::Result;
use kungfu_core::KungfuService;
use kungfu_project::{find_project_root, init_project, KUNGFU_VERSION};
use kungfu_types::Budget;
use std::env;

pub fn init(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_project_root(&cwd)?;
    let project = init_project(&root)?;

    if json {
        let info = serde_json::json!({
            "status": "initialized",
            "root": project.root.to_string_lossy(),
            "project_name": project.meta.name,
        });
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("Initialized kungfu in {}", project.root.display());
        println!("  project: {}", project.meta.name);
        println!("  config:  .kungfu/config.toml");
        println!("\nRun 'kungfu index' to build the project index.");
    }
    Ok(())
}

pub fn status(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let info = service.status()?;

    if json {
        let out = serde_json::json!({
            "project_name": info.project_name,
            "root": info.root,
            "indexed_files": info.indexed_files,
            "indexed_symbols": info.indexed_symbols,
            "languages": info.languages,
            "has_git": info.has_git,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Project: {}", info.project_name);
        println!("Root:    {}", info.root);
        println!("Files:   {}", info.indexed_files);
        println!("Symbols: {}", info.indexed_symbols);
        println!("Git:     {}", if info.has_git { "yes" } else { "no" });
        if !info.languages.is_empty() {
            println!("Languages:");
            let mut langs: Vec<_> = info.languages.iter().collect();
            langs.sort_by(|a, b| b.1.cmp(a.1));
            for (lang, count) in langs {
                println!("  {}: {}", lang, count);
            }
        }
    }
    Ok(())
}

pub fn doctor(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let mut checks: Vec<(&str, bool, String)> = Vec::new();

    // Check version
    checks.push(("version", true, KUNGFU_VERSION.to_string()));

    // Check project root
    match find_project_root(&cwd) {
        Ok(root) => {
            let kungfu_dir = root.join(".kungfu");
            checks.push(("project_root", true, root.to_string_lossy().to_string()));
            checks.push((
                "kungfu_dir",
                kungfu_dir.exists(),
                if kungfu_dir.exists() {
                    ".kungfu exists".into()
                } else {
                    "missing — run 'kungfu init'".into()
                },
            ));

            if kungfu_dir.exists() {
                // Config
                let config_path = kungfu_dir.join("config.toml");
                let config_ok = config_path.exists();
                let config_detail = if config_ok {
                    match kungfu_config::KungfuConfig::load(&config_path) {
                        Ok(_) => "valid".to_string(),
                        Err(e) => format!("parse error: {}", e),
                    }
                } else {
                    "missing".to_string()
                };
                checks.push((
                    "config",
                    config_ok && !config_detail.starts_with("parse"),
                    config_detail,
                ));

                // Project metadata
                let project_path = kungfu_dir.join("project.json");
                let project_ok = project_path.exists();
                checks.push((
                    "project_meta",
                    project_ok,
                    if project_ok {
                        "project.json exists".into()
                    } else {
                        "missing".into()
                    },
                ));

                // Index
                let index_dir = kungfu_dir.join("index");
                let files_path = index_dir.join("files.json");
                let symbols_path = index_dir.join("symbols.json");
                let fp_path = index_dir.join("fingerprints.json");

                let has_files = files_path.exists();
                let has_symbols = symbols_path.exists();
                let has_fp = fp_path.exists();

                if has_files {
                    let file_count = std::fs::read_to_string(&files_path)
                        .ok()
                        .and_then(|c| {
                            serde_json::from_str::<Vec<serde_json::Value>>(&c)
                                .ok()
                                .map(|v| v.len())
                        })
                        .unwrap_or(0);
                    checks.push((
                        "index_files",
                        true,
                        format!("{} files indexed", file_count),
                    ));
                } else {
                    checks.push(("index_files", false, "not indexed — run 'kungfu index'".into()));
                }

                if has_symbols {
                    let sym_count = std::fs::read_to_string(&symbols_path)
                        .ok()
                        .and_then(|c| {
                            serde_json::from_str::<Vec<serde_json::Value>>(&c)
                                .ok()
                                .map(|v| v.len())
                        })
                        .unwrap_or(0);
                    checks.push((
                        "index_symbols",
                        true,
                        format!("{} symbols extracted", sym_count),
                    ));
                } else {
                    checks.push(("index_symbols", false, "no symbols".into()));
                }

                checks.push((
                    "index_fingerprints",
                    has_fp,
                    if has_fp {
                        "fingerprints tracked".into()
                    } else {
                        "no fingerprints".into()
                    },
                ));

                // Relations
                let relations_path = index_dir.join("relations.json");
                if relations_path.exists() {
                    let rel_count = std::fs::read_to_string(&relations_path)
                        .ok()
                        .and_then(|c| {
                            serde_json::from_str::<Vec<serde_json::Value>>(&c)
                                .ok()
                                .map(|v| v.len())
                        })
                        .unwrap_or(0);
                    checks.push((
                        "index_relations",
                        rel_count > 0,
                        format!("{} relations (imports, tests, configs)", rel_count),
                    ));
                } else {
                    checks.push(("index_relations", false, "no relations — reindex with 'kungfu index --full'".into()));
                }

                // Symbol coverage: % of code files that have symbols
                if has_files && has_symbols {
                    let file_count = std::fs::read_to_string(&files_path)
                        .ok()
                        .and_then(|c| serde_json::from_str::<Vec<serde_json::Value>>(&c).ok())
                        .unwrap_or_default();
                    let sym_data = std::fs::read_to_string(&symbols_path)
                        .ok()
                        .and_then(|c| serde_json::from_str::<Vec<serde_json::Value>>(&c).ok())
                        .unwrap_or_default();

                    let code_files: std::collections::HashSet<String> = file_count
                        .iter()
                        .filter(|f| {
                            let lang = f.get("language").and_then(|l| l.as_str()).unwrap_or("");
                            matches!(lang, "rust" | "typescript" | "javascript" | "python" | "go")
                        })
                        .filter_map(|f| f.get("id").and_then(|id| id.as_str()).map(String::from))
                        .collect();

                    let files_with_symbols: std::collections::HashSet<String> = sym_data
                        .iter()
                        .filter_map(|s| s.get("file_id").and_then(|id| id.as_str()).map(String::from))
                        .collect();

                    let covered = code_files.intersection(&files_with_symbols).count();
                    let total = code_files.len();
                    let pct = if total > 0 { covered * 100 / total } else { 0 };

                    checks.push((
                        "symbol_coverage",
                        pct >= 50,
                        format!("{}/{}  code files have symbols ({}%)", covered, total, pct),
                    ));
                }

                // Directories
                let dirs = ["cache", "logs", "state"];
                for dir in &dirs {
                    let d = kungfu_dir.join(dir);
                    checks.push((dir, d.exists(), if d.exists() { "ok".into() } else { "missing".into() }));
                }
            }
        }
        Err(e) => {
            checks.push(("project_root", false, e.to_string()));
        }
    }

    // Git
    checks.push((
        "git",
        kungfu_git::is_git_repo(&cwd),
        if kungfu_git::is_git_repo(&cwd) {
            "git repository detected".into()
        } else {
            "not a git repo (git features unavailable)".into()
        },
    ));

    // Parser support
    checks.push((
        "parsers",
        true,
        "rust, typescript, javascript, python, go".into(),
    ));

    if json {
        let items: Vec<_> = checks
            .iter()
            .map(|(name, ok, detail)| {
                serde_json::json!({
                    "check": name,
                    "ok": ok,
                    "detail": detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        let all_ok = checks.iter().all(|(_, ok, _)| *ok);
        for (name, ok, detail) in &checks {
            let icon = if *ok { "OK" } else { "!!" };
            println!("  [{}] {}: {}", icon, name, detail);
        }
        println!();
        if all_ok {
            println!("All checks passed.");
        } else {
            let failed = checks.iter().filter(|(_, ok, _)| !ok).count();
            println!("{} check(s) need attention.", failed);
        }
    }
    Ok(())
}

pub fn config_show(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let _ = KungfuService::open(&cwd)?;
    let root = find_project_root(&cwd)?;
    let config_path = root.join(".kungfu").join("config.toml");
    let config = kungfu_config::KungfuConfig::load_merged(Some(&config_path))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&config)?);
    } else {
        let toml_str = toml::to_string_pretty(&config)?;
        println!("{}", toml_str);
    }
    Ok(())
}

pub fn index(full: bool, changed: bool, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;

    let start = std::time::Instant::now();
    let stats = if full {
        service.index_full()?
    } else if changed {
        service.index_changed()?
    } else {
        service.index_incremental()?
    };
    let elapsed = start.elapsed();

    if json {
        let out = serde_json::json!({
            "total_files": stats.total_files,
            "new_files": stats.new_files,
            "changed_files": stats.changed_files,
            "removed_files": stats.removed_files,
            "symbols_extracted": stats.symbols_extracted,
            "elapsed_ms": elapsed.as_millis(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Indexed {} files ({} symbols) in {:.1}s",
            stats.total_files,
            stats.symbols_extracted,
            elapsed.as_secs_f64()
        );
        if stats.new_files > 0 {
            println!("  new:     {}", stats.new_files);
        }
        if stats.changed_files > 0 {
            println!("  changed: {}", stats.changed_files);
        }
        if stats.removed_files > 0 {
            println!("  removed: {}", stats.removed_files);
        }
    }
    Ok(())
}

pub fn clean(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_project_root(&cwd)?;
    let kungfu_dir = root.join(".kungfu");

    let index_dir = kungfu_dir.join("index");
    let cache_dir = kungfu_dir.join("cache");

    let mut cleaned = Vec::new();
    if index_dir.exists() {
        std::fs::remove_dir_all(&index_dir)?;
        std::fs::create_dir_all(&index_dir)?;
        cleaned.push("index");
    }
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)?;
        std::fs::create_dir_all(cache_dir.join("summaries"))?;
        std::fs::create_dir_all(cache_dir.join("queries"))?;
        cleaned.push("cache");
    }

    if json {
        println!("{}", serde_json::json!({ "cleaned": cleaned }));
    } else {
        println!("Cleaned: {}", cleaned.join(", "));
    }
    Ok(())
}

pub fn watch() -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let root = find_project_root(&cwd)?;
    let config = service.config().clone();
    let index_dir = root.join(".kungfu").join("index");

    kungfu_index::watcher::watch_and_index(&root, config, &index_dir)
}

pub fn repo_outline(budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let outline = service.repo_outline(budget)?;

    if json {
        let dirs: Vec<_> = outline
            .top_dirs
            .iter()
            .map(|d| serde_json::json!({"path": d.path, "files": d.file_count}))
            .collect();
        let out = serde_json::json!({
            "project": outline.project_name,
            "total_files": outline.total_files,
            "total_symbols": outline.total_symbols,
            "languages": outline.languages,
            "directories": dirs,
            "entrypoints": outline.entrypoints,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Project: {} ({} files, {} symbols)",
            outline.project_name, outline.total_files, outline.total_symbols
        );
        println!();
        println!("Languages:");
        let mut langs: Vec<_> = outline.languages.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1));
        for (lang, count) in langs {
            println!("  {}: {}", lang, count);
        }
        println!();
        println!("Top directories:");
        for dir in &outline.top_dirs {
            println!("  {}/ ({} files)", dir.path, dir.file_count);
        }
        if !outline.entrypoints.is_empty() {
            println!();
            println!("Entrypoints:");
            for ep in &outline.entrypoints {
                println!("  {}", ep);
            }
        }
    }
    Ok(())
}

pub fn file_outline(path: &str, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let outline = service.file_outline(path)?;

    if json {
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
        let out = serde_json::json!({
            "path": outline.path,
            "language": outline.language,
            "symbols": symbols,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "{} ({})",
            outline.path,
            outline.language.as_deref().unwrap_or("unknown")
        );
        println!();
        for s in &outline.symbols {
            let exported = if s.exported { " [pub]" } else { "" };
            if let Some(ref sig) = s.signature {
                println!("  L{} {} {}{}", s.line, s.kind, sig, exported);
            } else {
                println!("  L{} {} {}{}", s.line, s.kind, s.name, exported);
            }
        }
    }
    Ok(())
}

pub fn find_symbol(query: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let results = service.find_symbol(query, budget)?;

    if json {
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
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if results.is_empty() {
            println!("No symbols found for '{}'", query);
        } else {
            for r in &results {
                let sig = r.item.signature.as_deref().unwrap_or(&r.item.name);
                println!(
                    "  {:.2}  {}:{}  {} {}",
                    r.score, r.item.path, r.item.span.start_line, r.item.kind, sig
                );
            }
        }
    }
    Ok(())
}

pub fn get_symbol(name: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let symbol = service.get_symbol(name)?;

    match symbol {
        Some(sym) => {
            if json {
                let mut out = serde_json::to_value(&sym)?;
                // At medium+ budget, include sibling symbols from the same file
                if budget >= Budget::Medium {
                    let outline = service.file_outline(&sym.path)?;
                    let siblings: Vec<_> = outline
                        .symbols
                        .iter()
                        .filter(|s| s.name != sym.name)
                        .take(budget.top_k())
                        .map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "kind": s.kind,
                                "line": s.line,
                            })
                        })
                        .collect();
                    out["siblings"] = serde_json::json!(siblings);
                }
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("{} ({})", sym.name, sym.kind);
                println!("  path: {}:{}", sym.path, sym.span.start_line);
                if let Some(ref sig) = sym.signature {
                    println!("  sig:  {}", sig);
                }
                if sym.exported {
                    println!("  exported: yes");
                }
                if let Some(ref vis) = sym.visibility {
                    println!("  visibility: {}", vis);
                }
                if let Some(ref doc) = sym.doc_summary {
                    println!("  doc:  {}", doc);
                }
                // At medium+ budget, show sibling symbols
                if budget >= Budget::Medium {
                    let outline = service.file_outline(&sym.path)?;
                    let siblings: Vec<_> = outline
                        .symbols
                        .iter()
                        .filter(|s| s.name != sym.name)
                        .take(budget.top_k())
                        .collect();
                    if !siblings.is_empty() {
                        println!();
                        println!("  Siblings in {}:", sym.path);
                        for s in &siblings {
                            println!("    L{} {} {}", s.line, s.kind, s.name);
                        }
                    }
                }
            }
        }
        None => {
            if json {
                println!("null");
            } else {
                println!("Symbol '{}' not found", name);
            }
        }
    }
    Ok(())
}

pub fn search_text(query: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let results = service.search_text(query, budget)?;

    if json {
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
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if results.is_empty() {
            println!("No results for '{}'", query);
        } else {
            for r in &results {
                println!(
                    "  {:.2}  {} ({})",
                    r.score,
                    r.item.path,
                    r.item.language.as_deref().unwrap_or("?")
                );
            }
        }
    }
    Ok(())
}

pub fn related(path: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let results = service.find_related(path, budget)?;

    if json {
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
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if results.is_empty() {
            println!("No related files found for '{}'", path);
        } else {
            for r in &results {
                println!(
                    "  {:.2}  {} ({})",
                    r.score,
                    r.item.path,
                    r.item.language.as_deref().unwrap_or("?")
                );
            }
        }
    }
    Ok(())
}

pub fn context(query: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let packet = service.context(query, budget)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
    } else {
        println!("Query:  {}", packet.query);
        println!("Budget: {}", packet.budget);
        println!("Items:  {}", packet.items.len());
        println!();
        for item in &packet.items {
            println!(
                "  {:.2}  [{}] {} — {}",
                item.score, item.path, item.name, item.why
            );
            if let Some(ref sig) = item.signature {
                println!("        sig: {}", sig);
            }
        }
    }
    Ok(())
}

pub fn ask_context(task: &str, budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let packet = service.ask_context(task, budget)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
    } else {
        println!("Task:   {}", packet.query);
        if let Some(ref intent) = packet.intent {
            println!("Intent: {}", intent);
        }
        println!("Budget: {}", packet.budget);
        println!("Items:  {}", packet.items.len());
        println!();
        for item in &packet.items {
            println!(
                "  {:.2}  [{}] {} — {}",
                item.score, item.path, item.name, item.why
            );
            if let Some(ref sig) = item.signature {
                println!("        sig: {}", sig);
            }
            if let Some(ref snippet) = item.snippet {
                println!("        ---");
                for line in snippet.lines().take(10) {
                    println!("        {}", line);
                }
                let total = snippet.lines().count();
                if total > 10 {
                    println!("        ... ({} more lines)", total - 10);
                }
                println!();
            }
        }
    }
    Ok(())
}

pub fn diff_context(budget: Budget, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let service = KungfuService::open(&cwd)?;
    let packet = service.diff_context(budget)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
    } else {
        if packet.items.is_empty() {
            println!("No changed files or relevant symbols found.");
        } else {
            println!("Diff context ({} items):", packet.items.len());
            for item in &packet.items {
                println!(
                    "  {:.2}  [{}] {} — {}",
                    item.score, item.path, item.name, item.why
                );
            }
        }
    }
    Ok(())
}

pub fn mcp() -> Result<()> {
    let cwd = env::current_dir()?;
    let root = kungfu_project::find_project_root(&cwd)?;
    let _ = KungfuService::open(&cwd)?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(kungfu_mcp::run_stdio_server(root))?;
    Ok(())
}
