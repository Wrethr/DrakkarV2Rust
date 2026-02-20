#!/usr/bin/env bash
# ci/acceptance_tests.sh
# Acceptance tests for drakkar. Requires gcc, g++, and a built drakkar binary.
# Usage: ./ci/acceptance_tests.sh [path/to/drakkar]

set -euo pipefail

DRAKKAR="${1:-./target/release/drakkar}"
WORKSPACE=$(mktemp -d)
PASS=0
FAIL=0
SKIP=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}✓ PASS${NC}: $1"; ((PASS++)) || true; }
fail() { echo -e "${RED}✗ FAIL${NC}: $1"; echo "  → $2"; ((FAIL++)) || true; }
skip() { echo -e "${YELLOW}~ SKIP${NC}: $1 — $2"; ((SKIP++)) || true; }

cleanup() { rm -rf "$WORKSPACE"; }
trap cleanup EXIT

header() { echo -e "\n\033[1m=== $1 ===\033[0m"; }

require_tool() {
    command -v "$1" >/dev/null 2>&1 || { echo "Skipping tests: '$1' not found."; exit 0; }
}

require_tool gcc
require_tool g++

if [ ! -x "$DRAKKAR" ]; then
    echo "drakkar binary not found at '$DRAKKAR'. Build with: cargo build --release"
    exit 1
fi

# ─────────────────────────────────────────────────────────────
header "Test 1: Create project skeleton"
# ─────────────────────────────────────────────────────────────
T1="$WORKSPACE/t1"
mkdir -p "$T1"
cd "$T1"
OUT=$("$DRAKKAR" create demo 2>&1)
if [ -d "demo/src" ] && [ -d "demo/out" ] && [ -d "demo/target" ] && \
   [ -f "demo/config.txt" ] && [ -f "demo/README.md" ]; then
    pass "Create project skeleton"
else
    fail "Create project skeleton" "Missing directories/files: $(ls demo/ 2>/dev/null || echo 'demo/ not found')"
fi

# ─────────────────────────────────────────────────────────────
header "Test 2: Simple single-file build"
# ─────────────────────────────────────────────────────────────
T2="$WORKSPACE/t2"
mkdir -p "$T2/src" "$T2/out" "$T2/target"
cat > "$T2/src/main.cpp" <<'EOF'
#include <iostream>
int main() {
    std::cout << "hello drakkar" << std::endl;
    return 0;
}
EOF
cat > "$T2/config.txt" <<'EOF'
app_name = "hello"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
cxx_standard = "c++17"
incremental = "true"
parallel_jobs = "1"
EOF
cd "$T2"
if "$DRAKKAR" build >/dev/null 2>&1; then
    if [ -f "target/main.o" ] && [ -f "out/hello" ]; then
        OUTPUT=$(./out/hello)
        if [ "$OUTPUT" = "hello drakkar" ]; then
            pass "Simple single-file build and run"
        else
            fail "Simple build: wrong output" "Expected 'hello drakkar', got '$OUTPUT'"
        fi
    else
        fail "Simple build" "target/main.o or out/hello missing"
    fi
else
    fail "Simple build" "drakkar build returned non-zero"
fi

# ─────────────────────────────────────────────────────────────
header "Test 3: Name collision (utils in two dirs)"
# ─────────────────────────────────────────────────────────────
T3="$WORKSPACE/t3"
mkdir -p "$T3/src/math" "$T3/src/network" "$T3/out" "$T3/target"
echo 'int math_add(int a, int b) { return a + b; }' > "$T3/src/math/utils.cpp"
echo 'int net_connect(int p) { return p; }' > "$T3/src/network/utils.cpp"
cat > "$T3/src/main.cpp" <<'EOF'
#include <iostream>
int math_add(int, int);
int net_connect(int);
int main() {
    std::cout << math_add(1, 2) << " " << net_connect(80) << std::endl;
    return 0;
}
EOF
cat > "$T3/config.txt" <<'EOF'
app_name = "coll_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "2"
EOF
cd "$T3"
if "$DRAKKAR" build >/dev/null 2>&1; then
    if [ -f "target/math/utils.o" ] && [ -f "target/network/utils.o" ]; then
        pass "Name collision prevention"
    else
        fail "Name collision" "target/math/utils.o or target/network/utils.o missing"
    fi
else
    fail "Name collision build" "Build failed"
fi

# ─────────────────────────────────────────────────────────────
header "Test 4: Incremental build (header change triggers recompile)"
# ─────────────────────────────────────────────────────────────
T4="$WORKSPACE/t4"
mkdir -p "$T4/src" "$T4/out" "$T4/target"
echo -e '#define VERSION 1\n' > "$T4/src/common.h"
echo '#include "common.h"' > "$T4/src/a.cpp"
echo 'int a_func() { return VERSION; }' >> "$T4/src/a.cpp"
echo '#include "common.h"' > "$T4/src/b.cpp"
echo 'int b_func() { return VERSION; }' >> "$T4/src/b.cpp"
cat > "$T4/src/main.cpp" <<'EOF'
#include <iostream>
int a_func(); int b_func();
int main() { std::cout << a_func() + b_func(); return 0; }
EOF
cat > "$T4/config.txt" <<'EOF'
app_name = "incr_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "2"
EOF
cd "$T4"
"$DRAKKAR" build >/dev/null 2>&1

A_MTIME1=$(stat -c %Y target/a.o 2>/dev/null || stat -f %m target/a.o)
B_MTIME1=$(stat -c %Y target/b.o 2>/dev/null || stat -f %m target/b.o)

sleep 1.1
echo '#define VERSION 2' > "$T4/src/common.h"

"$DRAKKAR" build >/dev/null 2>&1
A_MTIME2=$(stat -c %Y target/a.o 2>/dev/null || stat -f %m target/a.o)
B_MTIME2=$(stat -c %Y target/b.o 2>/dev/null || stat -f %m target/b.o)

if [ "$A_MTIME2" -gt "$A_MTIME1" ] && [ "$B_MTIME2" -gt "$B_MTIME1" ]; then
    pass "Incremental: header change recompiles dependents"
else
    fail "Incremental" "a.o or b.o not recompiled after header change"
fi

# Check "All up-to-date" when nothing changes
STDOUT3=$("$DRAKKAR" build 2>&1)
if echo "$STDOUT3" | grep -q "up-to-date"; then
    pass "Incremental: up-to-date message when nothing changed"
else
    fail "Incremental: up-to-date" "Expected 'up-to-date' in output, got: $STDOUT3"
fi

# ─────────────────────────────────────────────────────────────
header "Test 5: Mixed .c and .cpp compilation"
# ─────────────────────────────────────────────────────────────
T5="$WORKSPACE/t5"
mkdir -p "$T5/src" "$T5/out" "$T5/target"
cat > "$T5/src/utils.c" <<'EOF'
#include <stdio.h>
void c_hello(void) { printf("C says hello\n"); }
EOF
cat > "$T5/src/main.cpp" <<'EOF'
#include <iostream>
extern "C" void c_hello(void);
int main() { c_hello(); return 0; }
EOF
cat > "$T5/config.txt" <<'EOF'
app_name = "mixed_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
c_flags = "-Wall"
cxx_flags = "-Wall"
c_standard = "c11"
cxx_standard = "c++17"
incremental = "true"
parallel_jobs = "2"
EOF
cd "$T5"
if "$DRAKKAR" build >/dev/null 2>&1; then
    if [ -f "target/utils.o" ] && [ -f "target/main.o" ]; then
        pass "Mixed .c and .cpp compilation"
    else
        fail "Mixed compilation" "Object files missing"
    fi
else
    fail "Mixed compilation" "Build failed"
fi

# ─────────────────────────────────────────────────────────────
header "Test 6: Comma-containing flags (rpath) not split"
# ─────────────────────────────────────────────────────────────
T6="$WORKSPACE/t6"
mkdir -p "$T6/src" "$T6/out" "$T6/target"
echo 'int main() { return 0; }' > "$T6/src/main.cpp"
cat > "$T6/config.txt" <<'EOF'
app_name = "rpath_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall -Wextra"
ld_flags = "-Wl,-O1"
incremental = "true"
parallel_jobs = "1"
EOF
cd "$T6"
if "$DRAKKAR" build >/dev/null 2>&1; then
    pass "Comma-containing flags not split"
else
    fail "Comma flags" "Build failed — -Wl,-O1 may have been incorrectly split"
fi

# ─────────────────────────────────────────────────────────────
header "Test 7: Parallel build with 20 files"
# ─────────────────────────────────────────────────────────────
T7="$WORKSPACE/t7"
mkdir -p "$T7/src" "$T7/out" "$T7/target"
DECLS=""
CALLS=""
for i in $(seq 0 19); do
    echo "int func$i() { return $i; }" > "$T7/src/mod$i.cpp"
    DECLS="$DECLS int func$i();"
    CALLS="$CALLS total += func$i();"
done
cat > "$T7/src/main.cpp" <<EOF
#include <iostream>
$DECLS
int main() {
    int total = 0;
    $CALLS
    std::cout << total << std::endl;
    return 0;
}
EOF
cat > "$T7/config.txt" <<'EOF'
app_name = "parallel_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "8"
EOF
cd "$T7"
if "$DRAKKAR" build >/dev/null 2>&1; then
    MISSING=0
    for i in $(seq 0 19); do
        [ -f "target/mod$i.o" ] || MISSING=$((MISSING+1))
    done
    if [ "$MISSING" -eq 0 ]; then
        RESULT=$(./out/parallel_test)
        EXPECTED=190  # 0+1+...+19 = 190
        if [ "$RESULT" = "$EXPECTED" ]; then
            pass "Parallel build correctness"
        else
            fail "Parallel build" "Expected output $EXPECTED, got $RESULT"
        fi
    else
        fail "Parallel build" "$MISSING object files missing"
    fi
else
    fail "Parallel build" "Build failed"
fi

# ─────────────────────────────────────────────────────────────
header "Test 8: Error handling (compile error)"
# ─────────────────────────────────────────────────────────────
T8="$WORKSPACE/t8"
mkdir -p "$T8/src" "$T8/out" "$T8/target"
echo 'THIS IS NOT VALID C++' > "$T8/src/broken.cpp"
cat > "$T8/config.txt" <<'EOF'
app_name = "err_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "1"
EOF
cd "$T8"
if ! "$DRAKKAR" build >/dev/null 2>&1; then
    pass "Compile error returns non-zero exit code"
else
    fail "Error handling" "Expected non-zero exit, got success"
fi

# ─────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════"
echo -e "  Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}"
echo "═══════════════════════════════════"

[ "$FAIL" -eq 0 ]
