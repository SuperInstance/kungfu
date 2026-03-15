# kungfu

Local context retrieval engine for coding agents. Indexes codebases, resolves dependencies, and delivers the smallest useful context packet — so agents read fewer files and waste fewer tokens.

## Why

Agents burn tokens exploring codebases: reading files, grepping, guessing structure. Kungfu replaces that with one call:

```
$ kungfu ask-context "add C++ language support" --budget tiny
Task:   add C++ language support
Intent: lookup
Budget: tiny
Items:  3

  0.50  [crates/kungfu-parse/src/lib.rs] extract_symbols
  0.45  [crates/kungfu-config/src/lib.rs] LanguagesConfig
  0.45  [crates/kungfu-types/src/file.rs] Language
```

**275 tokens** instead of reading files manually (~7,500+ tokens per file).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/denyzhirkov/kungfu/master/install.sh | sh
```

Supports macOS (ARM64, x86_64) and Linux (x86_64, ARM64).

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
kungfu doctor        # verify everything works
```

## CLI commands

### Core retrieval

```sh
# Smart context — the main command
kungfu ask-context "find where JWT refresh is implemented" --budget small
kungfu ask-context "impact of changing Budget enum" --budget medium
kungfu ask-context "fix crash in python parser" --budget tiny

# Simple context (no intent detection)
kungfu context "incremental indexing" --budget small

# Context from git changes
kungfu diff-context --budget small
```

### Search

```sh
kungfu find-symbol AuthService          # search symbols by name
kungfu get-symbol refreshToken          # exact symbol lookup
kungfu search-text "refresh token"      # text search across files
kungfu related src/auth/service.ts      # find related files (imports, tests, configs)
```

### Structure

```sh
kungfu repo-outline                     # compact repo map
kungfu file-outline src/auth/service.ts # symbols in a file
```

### Maintenance

```sh
kungfu index --full                     # full rebuild
kungfu index --changed                  # reindex only git-changed files
kungfu doctor                           # validate installation and index
kungfu watch                            # auto re-index on file changes
kungfu clean                            # wipe index and cache
kungfu config                           # show current configuration
```

All commands support `--json` for machine output and `--budget tiny|small|medium|full`.

## ask-context

The highest-value command. Given a task description, it:

1. **Detects intent** — lookup, debug, understand, or impact
2. **Runs multiple search strategies** — symbol search, text search, related files, import chains
3. **Applies contextual bonuses** — changed files (+0.2), test/config proximity (+0.15), language weighting
4. **Returns a ranked packet** with signatures and code snippets

### Intent detection

| Intent | Triggers | Extra strategies |
|--------|----------|-----------------|
| `lookup` | find, where, show, search | symbol + text search |
| `debug` | bug, fix, error, crash | + related files, error symbol boost |
| `understand` | how, explain, what, why | + sibling symbols from same file |
| `impact` | impact, refactor, rename, delete | + import chain, sibling symbols |

### Budget levels

| Budget | Items | Snippets | Use case |
|--------|-------|----------|----------|
| `tiny` | 3 | none | Quick pointers — "where to look" |
| `small` | 5 | 20 lines | Default — signatures + context |
| `medium` | 8 | 40 lines | Deeper exploration |
| `full` | 12 | 100 lines | Complete picture |

## MCP server

```sh
kungfu mcp
```

Add to your agent config (Claude Code, Cursor, etc.):

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

### 13 MCP tools

| Tool | Description |
|------|-------------|
| `project_status` | File count, symbol count, languages, git status |
| `repo_outline` | Top directories, language distribution, entrypoints |
| `file_outline` | Symbols, signatures, exports for a file |
| `find_symbol` | Search symbols by name (exact + fuzzy + stem) |
| `get_symbol` | Detailed symbol info by exact name |
| `search_text` | Text search across indexed files |
| `find_files` | Find files by path pattern |
| `find_related_files` | Related files by imports, tests, configs, proximity |
| `find_related_symbols` | Related symbols in same file |
| `get_minimal_context` | Smallest high-confidence context set |
| `build_task_context` | Ranked context packet for a task |
| `ask_context` | Smart retrieval: intent detection + multi-strategy search |
| `diff_context` | Context focused on git changes |

## Agent rules

Add to `CLAUDE.md` or system prompt of your project:

```markdown
## Context retrieval
- Before reading files, use `kungfu` MCP tools to understand the codebase.
- Use `ask_context` with the task description to get a ranked context packet.
- Use `find_symbol` / `get_symbol` to locate code by name instead of reading whole files.
- Use `file_outline` before reading a file — often the outline is enough.
- Only read full files when the above tools confirm you need them.
- After git changes, use `diff_context` to focus on what changed.
- Prefer tiny/small budget. Escalate to medium/full only when needed.
```

### Recommended workflow

```
Task received
    │
    ▼
ask_context("task description", budget: "tiny")
    │
    ├── Got enough? → Start working
    │
    ├── Need more detail? → ask_context(..., budget: "small")
    │
    ├── Need specific symbol? → find_symbol / get_symbol
    │
    ├── Need file structure? → file_outline
    │
    └── Only then → Read full file
```

### Token savings (measured on real projects)

| Project size | Without kungfu | With kungfu (small) | Savings |
|-------------|---------------|-------------------|---------|
| ~50 files | ~7,500 tokens | ~275 tokens | **27x** |
| ~120 files | ~3,500 tokens | ~205 tokens | **17x** |
| ~170 files (TS) | ~19,000 tokens | ~615 tokens | **31x** |
| ~170 files (game) | ~64,000 tokens | ~895 tokens | **71x** |

## How it works

### Indexing
- Scans project files respecting `.gitignore` and configurable ignore rules
- Parses code with [tree-sitter](https://tree-sitter.github.io/) — Rust, TypeScript, JavaScript, Python, Go
- Extracts symbols: functions, classes, structs, methods, traits, interfaces, types, constants
- Extracts imports from AST and resolves them to actual files in the project
- Builds relations: `imports`, `test_for`, `config_for`
- Incremental re-indexing via blake3 file fingerprints

### Search & ranking
- Exact match (1.0), prefix (0.9), contains (0.7), stem (0.6), fuzzy (0.4)
- Exact phrase matching: `snake_case`, `camelCase`, space-separated
- Simple English stemming: "ranking" finds "rank", "indexing" finds "index"
- Path matching with filename boost (0.9) vs directory match (0.6)

### Context assembly
- Multi-strategy search: symbols, text, related files, import chains
- Deduplication by (path, name)
- Changed-file bonus from git
- Test/config proximity bonuses based on query intent
- Language importance weighting (primary language prioritized)
- Budget-controlled output with code snippets

### Storage
```
.kungfu/
  config.toml          # project configuration
  project.json         # project metadata
  index/
    files.json         # indexed files with hashes
    symbols.json       # extracted symbols with spans
    relations.json     # import/test/config relations
    fingerprints.json  # blake3 hashes for incremental rebuilds
```

## Configuration

`.kungfu/config.toml`:

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

## Benchmarks

Tested on the kungfu codebase itself (48 files, 270 symbols):

| Metric | Result |
|--------|--------|
| Precision (11 queries) | 24/24 expected items found |
| Edge cases | 7/7 — unicode, special chars, empty queries |
| Unit tests | 36/36 |
| Token savings (tiny) | ~275 tokens vs ~7,500 reading one file (27x) |
| Index time | ~0.1s for 48 files |
| Binary size | ~11MB |

## License

MIT
