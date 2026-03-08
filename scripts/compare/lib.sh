#!/usr/bin/env bash
# ============================================================================
# lib.sh — Shared helper functions for git/jj/libra comparison tests
# ============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Color codes
# ---------------------------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

# ---------------------------------------------------------------------------
# Global state
# ---------------------------------------------------------------------------
SANDBOX=""
REPORT_FILE=""
REPORT_DIR=""
LIBRA_BIN="${LIBRA_BIN:-}"
TOOLS_AVAILABLE=()    # populated by check_tools
ENABLED_TOOLS=()      # populated by parse_args or defaults

# Counters per tool per category (bash 3 compatible key/value emulation)
# Stored as dynamic vars named CMP_<map>_<sanitized_key>
counter_var_name() {
    local map="$1"
    local key="$2"
    local raw="${map}_${key}"
    local safe
    safe="$(printf '%s' "$raw" | tr -c 'A-Za-z0-9_' '_')"
    printf 'CMP_%s' "$safe"
}

counter_set() {
    local map="$1"
    local key="$2"
    local value="$3"
    local var
    var="$(counter_var_name "$map" "$key")"
    eval "$var='$value'"
}

counter_get() {
    local map="$1"
    local key="$2"
    local var
    var="$(counter_var_name "$map" "$key")"
    eval "printf '%s' \"\${$var:-0}\""
}

counter_inc() {
    local map="$1"
    local key="$2"
    local delta="${3:-1}"
    local current
    current="$(counter_get "$map" "$key")"
    counter_set "$map" "$key" "$(( current + delta ))"
}

CURRENT_CATEGORY=""
TOTAL_TESTS=0

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
log_info()    { printf "${CYAN}[INFO]${RESET} %s\n" "$*"; }
log_success() { printf "${GREEN}[PASS]${RESET} %s\n" "$*"; }
log_fail()    { printf "${RED}[FAIL]${RESET} %s\n" "$*"; }
log_na()      { printf "${DIM}[N/A ]${RESET} %s\n" "$*"; }
log_expected(){ printf "${YELLOW}[XFAIL]${RESET} %s\n" "$*"; }
log_warn()    { printf "${YELLOW}[WARN]${RESET} %s\n" "$*"; }
log_section() { printf "\n${BOLD}${CYAN}═══════════════════════════════════════════════════════════════${RESET}\n"; printf "${BOLD}  %s${RESET}\n" "$*"; printf "${BOLD}${CYAN}═══════════════════════════════════════════════════════════════${RESET}\n\n"; }
log_subsect() { printf "\n${BOLD}  ── %s ──${RESET}\n\n" "$*"; }

# ---------------------------------------------------------------------------
# Sandbox management
# ---------------------------------------------------------------------------
setup_sandbox() {
    SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/libra-compare.XXXXXX")"
    mkdir -p "$SANDBOX/out" "$SANDBOX/repos" "$SANDBOX/home" "$SANDBOX/bare"
    REPORT_DIR="${REPORT_DIR:-$SANDBOX}"
    REPORT_FILE="$REPORT_DIR/report.md"

    # Isolated HOME so we don't pick up real ~/.gitconfig / ~/.jjconfig.toml
    export HOME="$SANDBOX/home"
    export GIT_CONFIG_NOSYSTEM=1
    export GIT_AUTHOR_NAME="Test User"
    export GIT_AUTHOR_EMAIL="test@example.com"
    export GIT_COMMITTER_NAME="Test User"
    export GIT_COMMITTER_EMAIL="test@example.com"
    # jj uses these or its own config
    export JJ_USER="Test User"
    export JJ_EMAIL="test@example.com"

    # Ensure git doesn't prompt
    export GIT_TERMINAL_PROMPT=0

    log_info "Sandbox: $SANDBOX"
    log_info "Report:  $REPORT_FILE"
}

cleanup_sandbox() {
    if [[ -n "$SANDBOX" && -d "$SANDBOX" ]]; then
        rm -rf "$SANDBOX"
        log_info "Cleaned up sandbox"
    fi
}

# ---------------------------------------------------------------------------
# Tool detection
# ---------------------------------------------------------------------------
check_tools() {
    TOOLS_AVAILABLE=()

    # git
    if command -v git &>/dev/null; then
        TOOLS_AVAILABLE+=("git")
        log_info "git found: $(git --version)"
    else
        log_warn "git not found — will be skipped"
    fi

    # jj
    if command -v jj &>/dev/null; then
        TOOLS_AVAILABLE+=("jj")
        log_info "jj found: $(jj --version 2>/dev/null || echo 'unknown')"
    else
        log_warn "jj not found — will be skipped"
    fi

    # libra
    if [[ -z "$LIBRA_BIN" ]]; then
        # Try to find in workspace
        local script_dir
        script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
        local workspace_root
        workspace_root="$(cd "$script_dir/../.." && pwd)"
        if [[ -x "$workspace_root/target/debug/libra" ]]; then
            LIBRA_BIN="$workspace_root/target/debug/libra"
        elif [[ -x "$workspace_root/target/release/libra" ]]; then
            LIBRA_BIN="$workspace_root/target/release/libra"
        fi
    fi

    if [[ -n "$LIBRA_BIN" && -x "$LIBRA_BIN" ]]; then
        TOOLS_AVAILABLE+=("libra")
        log_info "libra found: $LIBRA_BIN"
    else
        log_warn "libra binary not found — will be skipped"
        log_warn "  Set LIBRA_BIN env or build with: cargo build"
    fi

    # Set enabled tools (default: all available, or filtered by --tools)
    if [[ ${#ENABLED_TOOLS[@]} -eq 0 ]]; then
        ENABLED_TOOLS=("${TOOLS_AVAILABLE[@]}")
    else
        local filtered=()
        for t in "${ENABLED_TOOLS[@]}"; do
            if is_tool_available "$t"; then
                filtered+=("$t")
            else
                log_warn "Tool '$t' requested but not available — skipping"
            fi
        done
        ENABLED_TOOLS=("${filtered[@]}")
    fi

    log_info "Enabled tools: ${ENABLED_TOOLS[*]}"
    echo ""
}

is_tool_available() {
    local tool="$1"
    for t in "${TOOLS_AVAILABLE[@]}"; do
        [[ "$t" == "$tool" ]] && return 0
    done
    return 1
}

is_tool_enabled() {
    local tool="$1"
    for t in "${ENABLED_TOOLS[@]}"; do
        [[ "$t" == "$tool" ]] && return 0
    done
    return 1
}

# ---------------------------------------------------------------------------
# Tool command execution
# ---------------------------------------------------------------------------
# get_tool_bin <tool>  — returns the binary path for a tool
get_tool_bin() {
    case "$1" in
        git)   echo "git" ;;
        jj)    echo "jj" ;;
        libra) echo "$LIBRA_BIN" ;;
        *)     echo "$1" ;;
    esac
}

# run_tool <tool> <label> <args...>
#   Runs a command for the given tool, captures stdout/stderr/exit code.
#   Returns the exit code.
#   Output is stored in $SANDBOX/out/<label>.<tool>.{stdout,stderr,rc}
run_tool() {
    local tool="$1"; shift
    local label="$1"; shift
    local bin
    bin="$(get_tool_bin "$tool")"

    local out_prefix="$SANDBOX/out/${label}.${tool}"
    local rc=0

    # Capture timing
    local start_time
    start_time=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || echo 0)

    "$bin" "$@" >"${out_prefix}.stdout" 2>"${out_prefix}.stderr" || rc=$?

    local end_time
    end_time=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || echo 0)

    echo "$rc" > "${out_prefix}.rc"
    if [[ "$start_time" != "0" && "$end_time" != "0" ]]; then
        local elapsed_ms=$(( (end_time - start_time) / 1000000 ))
        echo "$elapsed_ms" > "${out_prefix}.time_ms"
    fi

    return $rc
}

# ---------------------------------------------------------------------------
# Test execution framework
# ---------------------------------------------------------------------------
set_category() {
    CURRENT_CATEGORY="$1"
    for tool in git jj libra; do
        local key="${CURRENT_CATEGORY}_${tool}"
        counter_set pass "$key" 0
        counter_set fail "$key" 0
        counter_set na "$key" 0
        counter_set xfail "$key" 0
        counter_set total "$key" 0
    done
}

# record_result <tool> <label> <actual_rc> <expect_success|expect_fail>
record_result() {
    local tool="$1"
    local label="$2"
    local actual_rc="$3"
    local expectation="${4:-expect_success}"   # expect_success | expect_fail

    local key="${CURRENT_CATEGORY}_${tool}"
    counter_inc total "$key" 1
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if [[ "$expectation" == "expect_fail" ]]; then
        if [[ "$actual_rc" -ne 0 ]]; then
            counter_inc xfail "$key" 1
            log_expected "$tool | $label (exit=$actual_rc, expected failure)"
            md_result_row "$label" "$tool" "XFAIL" "$actual_rc"
        else
            counter_inc fail "$key" 1
            log_fail "$tool | $label (exit=0, expected failure but succeeded!)"
            md_result_row "$label" "$tool" "UNEXPECTED_PASS" "$actual_rc"
        fi
    else
        if [[ "$actual_rc" -eq 0 ]]; then
            counter_inc pass "$key" 1
            log_success "$tool | $label"
            md_result_row "$label" "$tool" "PASS" "$actual_rc"
        else
            counter_inc fail "$key" 1
            local stderr_snippet
            stderr_snippet="$(head -c 200 "$SANDBOX/out/${label}.${tool}.stderr" 2>/dev/null || echo '(no stderr)')"
            log_fail "$tool | $label (exit=$actual_rc) — $stderr_snippet"
            md_result_row "$label" "$tool" "FAIL" "$actual_rc" "$stderr_snippet"
        fi
    fi
}

record_na() {
    local tool="$1"
    local label="$2"
    local reason="${3:-no equivalent command}"
    local key="${CURRENT_CATEGORY}_${tool}"
    counter_inc total "$key" 1
    counter_inc na "$key" 1
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    log_na "$tool | $label ($reason)"
    md_result_row "$label" "$tool" "N/A" "-" "$reason"
}

# ---------------------------------------------------------------------------
# Convenience: run_compare <label> <expect> <git_args> <jj_args_or_NA> <libra_args>
#   Each args string is eval'd — use "NA" for jj to mark N/A
# ---------------------------------------------------------------------------
run_compare() {
    local label="$1"
    local expectation="$2"   # expect_success | expect_fail
    local git_args="$3"
    local jj_args="$4"
    local libra_args="$5"

    printf "  %-45s" "$label"

    for tool in git jj libra; do
        if ! is_tool_enabled "$tool"; then
            printf "  ${DIM}skip${RESET}"
            continue
        fi

        local args_var="${tool}_args"
        local args="${!args_var}"

        if [[ "$args" == "NA" ]]; then
            record_na "$tool" "$label"
            printf "  ${DIM}N/A${RESET} "
            continue
        fi

        local rc=0
        # Run in current directory (caller should cd to appropriate repo)
        eval "run_tool $tool '$label' $args" || rc=$?
        record_result "$tool" "$label" "$rc" "$expectation"

        if [[ "$expectation" == "expect_fail" ]]; then
            if [[ "$rc" -ne 0 ]]; then
                printf "  ${YELLOW}XFAIL${RESET}"
            else
                printf "  ${RED}UPASS${RESET}"
            fi
        else
            if [[ "$rc" -eq 0 ]]; then
                printf "  ${GREEN}PASS${RESET} "
            else
                printf "  ${RED}FAIL${RESET} "
            fi
        fi
    done
    printf "\n"
}

# ---------------------------------------------------------------------------
# Repository setup helpers
# ---------------------------------------------------------------------------
# make_temp_repo <name> — creates $SANDBOX/repos/<name> and returns the path
make_temp_repo() {
    local name="$1"
    local dir="$SANDBOX/repos/$name"
    mkdir -p "$dir"
    echo "$dir"
}

# create_bare_remote <name> — creates a bare git repo for push/fetch tests
create_bare_remote() {
    local name="${1:-remote}"
    local dir="$SANDBOX/bare/$name"
    mkdir -p "$dir"
    git init --bare "$dir" &>/dev/null
    echo "$dir"
}

# setup_git_repo <dir> [--no-config]
setup_git_repo() {
    local dir="$1"
    local no_config="${2:-}"
    (
        cd "$dir"
        git init &>/dev/null
        if [[ "$no_config" != "--no-config" ]]; then
            git config user.name "Test User"
            git config user.email "test@example.com"
        fi
    )
}

# setup_jj_repo <dir> [--no-config]
setup_jj_repo() {
    local dir="$1"
    local no_config="${2:-}"
    (
        cd "$dir"
        jj git init &>/dev/null 2>&1 || jj init &>/dev/null 2>&1 || true
        if [[ "$no_config" != "--no-config" ]]; then
            jj config set --repo user.name "Test User" 2>/dev/null || true
            jj config set --repo user.email "test@example.com" 2>/dev/null || true
        fi
    )
}

# setup_libra_repo <dir> [--no-config]
setup_libra_repo() {
    local dir="$1"
    local no_config="${2:-}"
    (
        cd "$dir"
        "$LIBRA_BIN" init &>/dev/null
        if [[ "$no_config" != "--no-config" ]]; then
            "$LIBRA_BIN" config --add --local user.name "Test User" &>/dev/null || true
            "$LIBRA_BIN" config --add --local user.email "test@example.com" &>/dev/null || true
        fi
    )
}

# setup_all_repos <base_name> [--no-config]
#   Creates 3 repos: <base>_git, <base>_jj, <base>_libra
#   Returns nothing; sets global GIT_REPO, JJ_REPO, LIBRA_REPO
setup_all_repos() {
    local base="$1"
    local no_config="${2:-}"
    GIT_REPO="$(make_temp_repo "${base}_git")"
    JJ_REPO="$(make_temp_repo "${base}_jj")"
    LIBRA_REPO="$(make_temp_repo "${base}_libra")"

    if is_tool_enabled git;  then setup_git_repo  "$GIT_REPO"   "$no_config"; fi
    if is_tool_enabled jj;   then setup_jj_repo   "$JJ_REPO"    "$no_config"; fi
    if is_tool_enabled libra; then setup_libra_repo "$LIBRA_REPO" "$no_config"; fi
}

# get_repo <tool> — returns the repo dir for the current test context
get_repo() {
    case "$1" in
        git)   echo "$GIT_REPO" ;;
        jj)    echo "$JJ_REPO" ;;
        libra) echo "$LIBRA_REPO" ;;
    esac
}

# create_file_in_repos <filename> <content>
#   Creates the same file in all 3 repos
create_file_in_repos() {
    local filename="$1"
    local content="$2"
    for tool in git jj libra; do
        if is_tool_enabled "$tool"; then
            local repo
            repo="$(get_repo "$tool")"
            local dir
            dir="$(dirname "$repo/$filename")"
            mkdir -p "$dir"
            echo "$content" > "$repo/$filename"
        fi
    done
}

# add_and_commit_in_repos <message> [files...]
#   Stages and commits in all 3 repos
add_and_commit_in_repos() {
    local msg="$1"; shift
    local files=("$@")

    if is_tool_enabled git; then
        (
            cd "$GIT_REPO"
            if [[ ${#files[@]} -gt 0 ]]; then
                git add "${files[@]}" &>/dev/null
            else
                git add -A &>/dev/null
            fi
            git commit -m "$msg" &>/dev/null
        )
    fi

    if is_tool_enabled jj; then
        (
            cd "$JJ_REPO"
            # jj auto-tracks; just commit
            jj commit -m "$msg" &>/dev/null 2>&1 || true
        )
    fi

    if is_tool_enabled libra; then
        (
            cd "$LIBRA_REPO"
            if [[ ${#files[@]} -gt 0 ]]; then
                "$LIBRA_BIN" add "${files[@]}" &>/dev/null
            else
                "$LIBRA_BIN" add -A &>/dev/null
            fi
            "$LIBRA_BIN" commit -m "$msg" &>/dev/null
        )
    fi
}

# get_head_sha <tool> <repo_dir> — returns the HEAD commit SHA
get_head_sha() {
    local tool="$1"
    local repo="$2"
    (
        cd "$repo"
        case "$tool" in
            git)   git rev-parse HEAD 2>/dev/null ;;
            jj)    jj log -r @ --no-graph -T 'commit_id' 2>/dev/null | head -1 ;;
            libra) "$LIBRA_BIN" log -n 1 --oneline 2>/dev/null | awk '{print $1}' ;;
        esac
    )
}

# ---------------------------------------------------------------------------
# Markdown report generation
# ---------------------------------------------------------------------------
md_init() {
    cat > "$REPORT_FILE" << 'EOF'
# Git / jj / Libra Command Comparison Report

> Auto-generated by `scripts/compare/run.sh`

## Legend

| Symbol | Meaning |
|--------|---------|
| PASS   | Command succeeded as expected |
| FAIL   | Command unexpectedly failed |
| XFAIL  | Command failed as expected (error case) |
| UPASS  | Command unexpectedly passed (expected failure) |
| N/A    | No equivalent command in this tool |
| skip   | Tool not enabled/available |

---

EOF
}

md_section() {
    echo "" >> "$REPORT_FILE"
    echo "## $1" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "| Test | Tool | Result | Exit Code | Notes |" >> "$REPORT_FILE"
    echo "|------|------|--------|-----------|-------|" >> "$REPORT_FILE"
}

md_result_row() {
    local label="$1"
    local tool="$2"
    local result="$3"
    local exit_code="$4"
    local notes="${5:-}"
    # Escape pipe chars in notes
    notes="${notes//|/\\|}"
    # Truncate notes
    if [[ ${#notes} -gt 100 ]]; then
        notes="${notes:0:100}..."
    fi
    echo "| $label | $tool | $result | $exit_code | $notes |" >> "$REPORT_FILE"
}

md_category_summary() {
    local category="$1"
    echo "" >> "$REPORT_FILE"
    echo "### Summary: $category" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "| Tool | Pass | Fail | XFail | N/A | Total |" >> "$REPORT_FILE"
    echo "|------|------|------|-------|-----|-------|" >> "$REPORT_FILE"
    for tool in git jj libra; do
        local key="${category}_${tool}"
        local p f x n t
        p="$(counter_get pass "$key")"
        f="$(counter_get fail "$key")"
        x="$(counter_get xfail "$key")"
        n="$(counter_get na "$key")"
        t="$(counter_get total "$key")"
        echo "| $tool | $p | $f | $x | $n | $t |" >> "$REPORT_FILE"
    done
}

# ---------------------------------------------------------------------------
# Final scoreboard (terminal + markdown)
# ---------------------------------------------------------------------------
declare -a ALL_CATEGORIES=()

register_category() {
    ALL_CATEGORIES+=("$1")
}

print_scoreboard() {
    log_section "Final Scoreboard"

    # Terminal header
    printf "${BOLD}%-25s" "Category"
    for tool in git jj libra; do
        printf "│ %-18s" "$tool"
    done
    printf "${RESET}\n"
    printf "%-25s" "─────────────────────────"
    for _ in git jj libra; do
        printf "┼──────────────────"
    done
    printf "\n"

    local grand_pass_git=0 grand_pass_jj=0 grand_pass_libra=0
    local grand_total_git=0 grand_total_jj=0 grand_total_libra=0

    for cat in "${ALL_CATEGORIES[@]}"; do
        printf "%-25s" "$cat"
        for tool in git jj libra; do
            local key="${cat}_${tool}"
            local p x f n t
            p="$(counter_get pass "$key")"
            x="$(counter_get xfail "$key")"
            f="$(counter_get fail "$key")"
            n="$(counter_get na "$key")"
            t="$(counter_get total "$key")"
            local ok=$((p + x))

            # Color based on score
            local color="$GREEN"
            if [[ $f -gt 0 ]]; then color="$RED"; fi

            printf "│ ${color}%d+%d${RESET}/${DIM}%d${RESET} (${DIM}%dNA${RESET}) " "$p" "$x" "$t" "$n"

            # Accumulate
            eval "grand_pass_${tool}=\$((grand_pass_${tool} + ok))"
            eval "grand_total_${tool}=\$((grand_total_${tool} + t))"
        done
        printf "\n"
    done

    printf "%-25s" "─────────────────────────"
    for _ in git jj libra; do
        printf "┼──────────────────"
    done
    printf "\n"
    printf "${BOLD}%-25s" "TOTAL"
    for tool in git jj libra; do
        local gp gf gt
        eval "gp=\$grand_pass_${tool}"
        eval "gt=\$grand_total_${tool}"
        gf=$((gt - gp))
        local color="$GREEN"
        if [[ $gf -gt 0 ]]; then color="$YELLOW"; fi
        printf "│ ${color}${BOLD}%d/%d${RESET}           " "$gp" "$gt"
    done
    printf "${RESET}\n\n"

    # Markdown final summary
    echo "" >> "$REPORT_FILE"
    echo "---" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "## Final Summary" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "| Category | git | jj | libra |" >> "$REPORT_FILE"
    echo "|----------|-----|-----|-------|" >> "$REPORT_FILE"
    for cat in "${ALL_CATEGORIES[@]}"; do
        printf "| %s" "$cat" >> "$REPORT_FILE"
        for tool in git jj libra; do
            local key="${cat}_${tool}"
            local p x f t
            p="$(counter_get pass "$key")"
            x="$(counter_get xfail "$key")"
            f="$(counter_get fail "$key")"
            t="$(counter_get total "$key")"
            printf " | %d+%dxf/%d" "$p" "$x" "$t" >> "$REPORT_FILE"
        done
        echo " |" >> "$REPORT_FILE"
    done
    echo "| **TOTAL** | **$grand_pass_git/$grand_total_git** | **$grand_pass_jj/$grand_total_jj** | **$grand_pass_libra/$grand_total_libra** |" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "_Generated: $(date -u '+%Y-%m-%d %H:%M:%S UTC')_" >> "$REPORT_FILE"
}
