# Feature Backlog

Features that are planned or considered but not yet committed to implementation.

## Ideas — High Potential Impact

### Smart Snippet Extraction

**Problem:** Agent reads entire files (500+ lines) when it needs only 10-20 lines around the relevant code. `get_symbol` solves this for named symbols, but not for arbitrary code locations (e.g. "show me where the error handling is in this function").

**Idea:** Given a query + file, return only the minimal relevant snippet with surrounding context. Like `get_symbol` but for any code region, not just symbol definitions.

**Impact:** Could save 10-20x tokens per file read — the single biggest token sink in agent workflows.

### Context Deduplication Across Session

**Status:** Under question — may not be feasible

**Problem:** In long agent sessions, the same files get read 3-5 times. Each re-read burns the full token cost.

**Idea:** Track what context was already sent in a session. On repeated access, return only the delta or a short summary.

**Why uncertain:**
- MCP server doesn't know if the agent started a new session or continues an old one
- Agent's context may have been compressed/truncated — it genuinely needs the full re-read
- Returning "you already saw this" when the agent doesn't remember is worse than re-sending
- Would need explicit session management protocol that doesn't exist in MCP today

**Possible lighter approach:** Instead of dedup, focus on making reads cheaper — better snippets, summaries, diffs. Let the agent re-read but give it less to read each time.

### Targeted Impact Analysis

**Problem:** "If I change this function, what breaks?" is the most valuable question for an agent, but currently requires reading many files to figure out. Call graph was an attempt at this but produced too much noise.

**Idea:** Combine imports + test relations + symbol name search to answer impact queries without a full call graph. For a given symbol, find: (1) files that import the containing module, (2) tests that cover it, (3) symbols with the same name in type signatures. Lighter than call graph, more actionable.

**Impact:** Prevents agent from making breaking changes blindly — saves entire retry cycles (thousands of tokens each).

## Under Review

### Call Graph (Calls relations)

**Status:** Paused — needs design rethink

**What:** Extract function/method calls from AST, resolve them to known symbols, store as `Calls` relations. Enables `callers`/`callees` queries and impact analysis.

**Why paused:**
- Generates massive relation counts on real projects (Go stdlib: 1M calls, home-assistant: 440K)
- Most calls are noise (stdlib functions, common utilities like `push`, `len`, `unwrap`)
- Without filtering, call graph pollutes the index instead of helping the agent save tokens
- Ambiguous resolution: same function name in many files leads to false positives

**What needs to happen before resuming:**
1. Filter out stdlib/common function calls (need language-specific stop lists)
2. Only store calls that cross file boundaries (same-file calls are trivially discoverable)
3. Limit to unique/specific function names (skip names shorter than 4 chars or appearing in >N files)
4. Benchmark: measure how often callers/callees actually improve context retrieval vs adding noise
5. Consider making it opt-in via config

**Prototype code:** Removed in the commit that added this entry. See git history for the implementation (RawCall extraction in kungfu-parse, build_call_relations in indexer).

**Benchmark data (before removal):**

| Project | Files | Symbols | Calls | Ratio |
|---|---|---|---|---|
| go | 14,022 | 105,497 | 984,256 | 9.3x |
| home-assistant | 24,315 | 118,204 | 351,608 | 3.0x |
| deno | 11,068 | 87,539 | 145,225 | 1.7x |
| django | 6,907 | 42,917 | 118,491 | 2.8x |
| ruff | 9,702 | 42,239 | 132,349 | 3.1x |
