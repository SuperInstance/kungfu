# kungfu-ai

Context retrieval and distillation engine for coding agents. Saves tokens, speeds up agent workflows.

## Install

```bash
npm install -g kungfu-ai
```

## Usage

```bash
# Initialize in your project
kungfu init
kungfu index

# Search and explore
kungfu find-symbol MyClass
kungfu explore-symbol Router
kungfu ask-context "how does auth work"
kungfu semantic-search "database connection"
kungfu callers handleRequest

# Start MCP server (for AI agents)
kungfu mcp
```

## What is Kungfu?

Kungfu is an MCP server that gives coding agents (Claude Code, Cursor, etc.) fast, focused access to your codebase — without reading entire files.

**21 MCP tools** including:
- Symbol search (exact + fuzzy + semantic)
- Composite tools (explore-symbol, investigate)
- Call graph (callers/callees)
- Git history (blame, file log)
- Context assembly with intent detection

**Supports:** Rust, TypeScript, JavaScript, Python, Go

## Alternative Install

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/denyzhirkov/kungfu/master/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/denyzhirkov/kungfu/master/install.ps1 | iex
```

## Links

- [GitHub](https://github.com/denyzhirkov/kungfu)
- [Benchmarks](https://github.com/denyzhirkov/kungfu/blob/master/BENCHMARKS.md)
