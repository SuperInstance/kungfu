# kungfu

Context retrieval and distillation engine for coding agents.

Indexes codebases locally, understands structure at file and symbol level, and delivers the smallest useful context packet — so agents read fewer files and waste fewer tokens.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/denyzhirkov/kungfu/master/install.sh | sh
```

Or build from source:

```sh
git clone https://github.com/denyzhirkov/kungfu.git
cd kungfu
cargo build --release
cp target/release/kungfu ~/.local/bin/
```

## Quick start

```sh
kungfu init          # initialize in project root
kungfu index         # build the index
kungfu status        # check project health
```

## CLI

```sh
# Structure
kungfu repo-outline                    # compact repo map
kungfu file-outline src/auth/service.ts # symbols in a file

# Symbols
kungfu find-symbol AuthService         # search by name
kungfu get-symbol refreshToken         # detailed info

# Search
kungfu search-text "refresh token"     # text search across files
kungfu related src/auth/service.ts     # find related files

# Context for agents
kungfu context "where is token rotation?" --budget small
kungfu diff-context                    # context from git changes

# Maintenance
kungfu doctor                          # validate everything
kungfu watch                           # auto re-index on changes
kungfu clean                           # wipe index and cache
```

All commands support `--json` for machine-readable output and `--budget small|medium|full` where applicable.

## MCP server

```sh
kungfu mcp
```

Runs an MCP server over stdio. Add to your agent config:

```json
{
  "mcpServers": {
    "kungfu": {
      "command": "kungfu",
      "args": ["mcp"]
    }
  }
}
```

### Tools

| Tool | Description |
|------|-------------|
| `project_status` | File count, symbol count, languages, git status |
| `repo_outline` | Top directories, language distribution, entrypoints |
| `file_outline` | Symbols, signatures, exports for a file |
| `find_symbol` | Search symbols by name (exact + fuzzy) |
| `get_symbol` | Detailed symbol info |
| `search_text` | Text search across indexed files |
| `find_files` | Find files by path pattern |
| `find_related_files` | Related files by import/path proximity |
| `find_related_symbols` | Related symbols in same file |
| `get_minimal_context` | Smallest high-confidence context set |
| `build_task_context` | Ranked context packet for a task |
| `diff_context` | Context focused on git changes |

## How it works

- Scans project files respecting `.gitignore` and configurable ignore rules
- Parses code with [tree-sitter](https://tree-sitter.github.io/) (Rust, TypeScript, JavaScript, Python, Go)
- Extracts symbols: functions, classes, structs, methods, traits, interfaces, types
- Stores index locally in `.kungfu/` as JSON
- Incremental re-indexing via blake3 file fingerprints
- Ranks results by exact match, fuzzy match, path proximity, and symbol relevance
- Assembles bounded context packets with configurable budget (`small` / `medium` / `full`)

## Configuration

Project config lives in `.kungfu/config.toml`:

```toml
project_name = "my-project"

[ignore]
paths = ["node_modules", "dist", "build", ".git", "target"]

[languages]
enabled = ["typescript", "javascript", "rust", "go", "python", "json", "markdown", "yaml", "toml"]

[search]
default_budget = "small"
default_top_k = 5

[index]
incremental = true

[git]
enabled = true
```

## License

MIT
