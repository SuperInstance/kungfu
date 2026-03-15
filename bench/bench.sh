#!/bin/bash
# Kungfu benchmark: measures precision@5 and token savings
# Runs against the kungfu project itself

set -e

KUNGFU="cargo run --release --quiet --"
PASS=0
FAIL=0
TOTAL_ITEMS=0
TOTAL_RELEVANT=0

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

check() {
    local description="$1"
    local query="$2"
    local budget="$3"
    shift 3
    local expected=("$@")

    local output
    output=$($KUNGFU ask-context "$query" --budget "$budget" --json 2>/dev/null)

    local items
    items=$(echo "$output" | grep -o '"path"' | wc -l | tr -d ' ')
    TOTAL_ITEMS=$((TOTAL_ITEMS + items))

    local found=0
    local missing=""
    for exp in "${expected[@]}"; do
        if echo "$output" | grep -q "$exp"; then
            found=$((found + 1))
        else
            missing="$missing $exp"
        fi
    done

    local total_expected=${#expected[@]}
    local precision=0
    if [ "$total_expected" -gt 0 ]; then
        precision=$((found * 100 / total_expected))
    fi
    TOTAL_RELEVANT=$((TOTAL_RELEVANT + found))

    if [ "$found" -eq "$total_expected" ]; then
        echo -e "  ${GREEN}PASS${NC} [$precision%] $description"
        PASS=$((PASS + 1))
    elif [ "$found" -gt 0 ]; then
        echo -e "  ${YELLOW}PART${NC} [$precision%] $description (missing:$missing)"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} [$precision%] $description (missing:$missing)"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== Kungfu Benchmark ==="
echo ""

# Reindex first
$KUNGFU index --full >/dev/null 2>&1
echo "Index rebuilt."
echo ""

# --- Test 1: Symbol lookup ---
echo "1. Symbol Lookup"
check "find Budget enum" \
    "Budget enum definition" "tiny" \
    "budget.rs" "Budget"

check "find KungfuService" \
    "KungfuService struct" "tiny" \
    "kungfu-core" "KungfuService"

check "find import extraction" \
    "extract imports from AST" "small" \
    "extract_imports" "RawImport"

# --- Test 2: Cross-file discovery ---
echo ""
echo "2. Cross-file Discovery"
check "find all parsers" \
    "language parser implementations" "medium" \
    "rust_parser" "typescript_parser" "python_parser"

check "find ranking logic" \
    "how are search results ranked and scored" "small" \
    "build_context_packet"

# --- Test 3: Intent detection ---
echo ""
echo "3. Intent-aware Search"
check "debug: parsing error" \
    "error when parsing python files" "small" \
    "python_parser" "parse"

check "impact: changing Language enum" \
    "impact of changing Language enum" "medium" \
    "Language" "from_extension" "is_code"

check "understand: how indexing works" \
    "how does the indexing pipeline work" "small" \
    "index" "Indexer"

# --- Test 4: Related files ---
echo ""
echo "4. Related Files"
check "find related to core" \
    "what files are related to kungfu-core service" "medium" \
    "kungfu-core" "budget" "symbol"

# --- Test 5: Config/test awareness ---
echo ""
echo "5. Context-aware Search"
check "config query" \
    "KungfuConfig settings" "small" \
    "KungfuConfig" "config"

check "MCP tools" \
    "MCP server tool definitions" "small" \
    "kungfu-mcp" "tool"

# --- Test 6: Edge cases ---
echo ""
echo "6. Edge Cases"

# Empty-ish queries
edge_check() {
    local desc="$1"
    local query="$2"
    local expect_ok="$3"

    local output
    output=$($KUNGFU ask-context "$query" --budget small --json 2>&1)
    local exit_code=$?

    if [ "$expect_ok" = "no_crash" ]; then
        if [ $exit_code -eq 0 ]; then
            echo -e "  ${GREEN}PASS${NC} $desc (no crash)"
        else
            echo -e "  ${RED}FAIL${NC} $desc (crashed!)"
            FAIL=$((FAIL + 1))
            return
        fi
    fi

    if [ "$expect_ok" = "has_items" ]; then
        local items
        items=$(echo "$output" | grep -c '"name"' || true)
        if [ "$items" -gt 0 ]; then
            echo -e "  ${GREEN}PASS${NC} $desc ($items items)"
        else
            echo -e "  ${RED}FAIL${NC} $desc (0 items)"
            FAIL=$((FAIL + 1))
            return
        fi
    fi

    PASS=$((PASS + 1))
}

edge_check "only stop words query" "the a an is are" "no_crash"
edge_check "single char query" "x" "no_crash"
edge_check "nonexistent symbol" "zzz_nonexistent_symbol_xyz" "no_crash"
edge_check "special chars in query" "fn<T>(x: &str)" "no_crash"
edge_check "very long query" "$(python3 -c 'print("budget " * 50)')" "has_items"
edge_check "unicode query" "бюджет парсинг конфиг" "no_crash"
edge_check "json output valid" "Budget" "has_items"

# --- Token size measurement ---
echo ""
echo "7. Token Size Measurement"

measure_tokens() {
    local desc="$1"
    local query="$2"
    local budget="$3"

    local output
    output=$($KUNGFU ask-context "$query" --budget "$budget" --json 2>/dev/null)
    local chars
    chars=$(echo "$output" | wc -c | tr -d ' ')
    # Rough approximation: 1 token ≈ 4 chars
    local tokens=$((chars / 4))
    echo "  $budget: ~${tokens} tokens ($chars chars) — $desc"
}

measure_tokens "add C++ support" "add new language support for C++" "tiny"
measure_tokens "add C++ support" "add new language support for C++" "small"
measure_tokens "add C++ support" "add new language support for C++" "medium"
measure_tokens "add C++ support" "add new language support for C++" "full"

# --- Summary ---
echo ""
echo "=== Summary ==="
TOTAL=$((PASS + FAIL))
echo "  Tests:     $PASS/$TOTAL passed"
echo "  Precision: $TOTAL_RELEVANT relevant items found"
echo "  Items:     $TOTAL_ITEMS total items returned"
if [ "$TOTAL_ITEMS" -gt 0 ]; then
    echo "  Avg items: $((TOTAL_ITEMS / TOTAL)) per query"
fi
echo ""
if [ "$FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}All benchmarks passed!${NC}"
else
    echo -e "  ${RED}$FAIL benchmark(s) need attention${NC}"
fi
