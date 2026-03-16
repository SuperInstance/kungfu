# Kungfu Benchmarks

Benchmarks across 8 popular open-source projects. Measured on Apple Silicon (M-series).

## Projects

| Project | Language | Files | Symbols | Index Time |
|---------|----------|------:|--------:|-----------:|
| flask   | Python   |   217 |   1,629 |       0.6s |
| gin     | Go       |   118 |   1,487 |       0.7s |
| express | JS       |   201 |   1,948 |       0.5s |
| axum    | Rust     |   474 |   2,771 |       1.3s |
| cargo   | Rust     | 2,718 |  12,009 |      17.4s |
| react   | JS/TS    | 6,736 |  58,828 |      23.7s |
| django  | Python   | 6,907 |  42,917 |      37.9s |
| ruff    | Rust     | 9,702 |  42,239 |      67.2s |
| go      | Go       |14,022 | 105,497 |     186.8s |
| next.js | JS/TS    |25,110 |  91,306 |     146.2s |

## Tool Response Times

| Project | explore-symbol | ask-context | semantic-search | callers |
|---------|---------------:|------------:|----------------:|--------:|
| flask   |          158ms |       228ms |           169ms |   150ms |
| gin     |          147ms |       217ms |           167ms |   154ms |
| express |          152ms |       227ms |           169ms |   134ms |
| axum    |          180ms |       269ms |           215ms |   292ms |
| cargo   |          395ms |       783ms |           495ms | 2,539ms |
| django  |        1,102ms |     2,125ms |         1,487ms | 4,074ms |
| ruff    |        1,081ms |     2,300ms |         1,466ms |10,202ms |
| go      |        2,195ms |     4,661ms |         3,282ms |25,399ms |

## MCP Tools (21 total)

### Core Search
| Tool | Description |
|------|-------------|
| `find_symbol` | Search symbols by exact and fuzzy name match |
| `get_symbol` | Get detailed info about a specific symbol |
| `search_text` | Search files by path and name matching |
| `find_files` | Find files by path pattern or keywords |
| `semantic_search` | Find symbols by concept with query expansion |

### Context Retrieval
| Tool | Description |
|------|-------------|
| `ask_context` | Smart multi-strategy search with intent detection |
| `get_minimal_context` | Smallest high-confidence context set |
| `build_task_context` | Ranked context packet for a task |
| `diff_context` | Context focused on git-changed code |

### Composite (multi-tool in one call)
| Tool | Description |
|------|-------------|
| `explore_symbol` | find + detail + related + snippet |
| `explore_file` | outline + related files + key symbols |
| `investigate` | ask_context + diff awareness |

### Call Graph
| Tool | Description |
|------|-------------|
| `callers` | Who calls this symbol? |
| `callees` | What does this symbol call? |

### Structure
| Tool | Description |
|------|-------------|
| `project_status` | File count, symbol count, languages |
| `repo_outline` | Top directories, entrypoints |
| `file_outline` | Symbols, signatures, exports |
| `find_related_files` | Import/dependency relations |
| `find_related_symbols` | Structural proximity |

### History & Stats
| Tool | Description |
|------|-------------|
| `file_history` | Git log for a file |
| `symbol_history` | Git blame + commits for a symbol |
| `usage_stats` | Token savings, cache hit rate |

## Key Features

- **Query cache**: LRU cache (64 entries) with mtime-based invalidation
- **Adaptive budget**: Auto-resolves based on project size
- **Scope filtering**: Limit results to a subdirectory
- **Highlighted snippets**: `>>>` markers + `«keyword»` highlighting
- **Diff awareness**: Changed files boosted in results
- **Semantic search**: 40+ concept synonym mappings for query expansion
- **Call graph**: AST-based function call extraction (Rust, TS, JS, Python, Go)
