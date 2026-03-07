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

## Examples

### Status

```
$ kungfu status
Project: kungfu
Root:    /Users/denis/Projects/kungfu
Files:   44
Symbols: 228
Git:     yes
Languages:
  rust: 28
  toml: 14
  markdown: 1
```

### Repo outline

```
$ kungfu repo-outline
Project: kungfu (44 files, 228 symbols)

Languages:
  rust: 28
  toml: 14

Top directories:
  crates/ (41 files)

Entrypoints:
  Cargo.toml
  crates/kungfu-cli/src/main.rs
```

### File outline

```
$ kungfu file-outline crates/kungfu-core/src/lib.rs
crates/kungfu-core/src/lib.rs (rust)

  L15 struct pub struct KungfuService [pub]
  L56 impl KungfuService
  L57 method pub fn open(start_dir: &Path) -> Result<Self> [pub]
  L74 method pub fn status(&self) -> Result<StatusInfo> [pub]
  L96 method pub fn index_full(&self) -> Result<IndexStats> [pub]
  L108 method pub fn repo_outline(&self, budget: Budget) -> Result<RepoOutline> [pub]
  L197 method pub fn find_symbol(&self, query: &str, budget: Budget) [pub]
  L209 method pub fn context(&self, query: &str, budget: Budget) [pub]
```

### Find symbol

```
$ kungfu find-symbol Indexer
  1.00  crates/kungfu-index/src/indexer.rs:14  struct pub struct Indexer
  1.00  crates/kungfu-index/src/indexer.rs:29  impl Indexer
  1.00  crates/kungfu-index/src/lib.rs:2       module indexer
```

### Search text

```
$ kungfu search-text parser
  0.90  crates/kungfu-parse/src/rust_parser.rs (rust)
  0.90  crates/kungfu-parse/src/go_parser.rs (rust)
  0.90  crates/kungfu-parse/src/typescript_parser.rs (rust)
  0.90  crates/kungfu-parse/src/python_parser.rs (rust)
  0.80  crates/kungfu-parse/src/lib.rs (rust)
```

### Context packet

```
$ kungfu context "incremental indexing" --budget small
Query:  incremental indexing
Budget: small
Items:  2

  0.45  [crates/kungfu-index/src/indexer.rs] index_incremental
        sig: pub fn index_incremental(&mut self) -> Result<IndexStats>
  0.45  [crates/kungfu-core/src/lib.rs] index_incremental
        sig: pub fn index_incremental(&self) -> Result<IndexStats>
```

### Doctor

```
$ kungfu doctor
  [OK] version: 0.1.0
  [OK] project_root: /Users/denis/Projects/kungfu
  [OK] kungfu_dir: .kungfu exists
  [OK] config: valid
  [OK] index_files: 44 files indexed
  [OK] index_symbols: 228 symbols extracted
  [OK] index_fingerprints: fingerprints tracked
  [OK] git: git repository detected
  [OK] parsers: rust, typescript, javascript, python, go

All checks passed.
```

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

## Agent rules

Add to your agent system prompt or `CLAUDE.md` / project rules:

```
Before reading project files, use kungfu MCP tools to understand the codebase:
1. Start with `repo_outline` to see project structure.
2. Use `find_symbol` / `get_symbol` to locate code by name instead of reading whole files.
3. Use `build_task_context` with your task description to get a ranked minimal context packet.
4. Use `file_outline` before reading a file — often the outline is enough.
5. Only read full files when the above tools confirm you need them.
6. After git changes, use `diff_context` to focus on what changed.
Prefer small budget. Escalate to medium/full only when small is insufficient.
```

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
