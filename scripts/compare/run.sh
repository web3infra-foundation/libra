#!/usr/bin/env bash
# ============================================================================
# run.sh — End-to-end git / jj / libra comparison runner
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./lib.sh
source "$SCRIPT_DIR/lib.sh"

KEEP_SANDBOX=0
SKIP_GITHUB_PUSH=0

# Backup for identity-related env vars (bash 3 compatible).
IDENTITY_ENV_UNSET_SENTINEL="__LIBRA_COMPARE_ENV_UNSET__"

usage() {
    cat <<'USAGE'
Usage:
  scripts/compare/run.sh [options]

Options:
  --tools <list>       Comma-separated tool list: git,jj,libra
  --report-dir <dir>   Directory where report.md is written
  --keep-sandbox       Keep sandbox directory for inspection
  --skip-github-push   Skip the GitHub push failure scenario
  -h, --help           Show this help

Examples:
  scripts/compare/run.sh
  scripts/compare/run.sh --tools git,libra --keep-sandbox
  scripts/compare/run.sh --report-dir /tmp/libra-compare-report
USAGE
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --tools)
                [[ $# -ge 2 ]] || { echo "missing value for --tools" >&2; exit 2; }
                IFS=',' read -r -a ENABLED_TOOLS <<< "$2"
                shift 2
                ;;
            --report-dir)
                [[ $# -ge 2 ]] || { echo "missing value for --report-dir" >&2; exit 2; }
                REPORT_DIR="$2"
                shift 2
                ;;
            --keep-sandbox)
                KEEP_SANDBOX=1
                shift
                ;;
            --skip-github-push)
                SKIP_GITHUB_PUSH=1
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                echo "unknown option: $1" >&2
                usage >&2
                exit 2
                ;;
        esac
    done
}

backup_identity_env() {
    local vars=(
        GIT_AUTHOR_NAME
        GIT_AUTHOR_EMAIL
        GIT_COMMITTER_NAME
        GIT_COMMITTER_EMAIL
        EMAIL
        JJ_USER
        JJ_EMAIL
    )

    for var in "${vars[@]}"; do
        local backup_var="IDENTITY_BACKUP_${var}"
        if [[ "${!var+x}" == "x" ]]; then
            printf -v "$backup_var" '%s' "${!var}"
        else
            printf -v "$backup_var" '%s' "$IDENTITY_ENV_UNSET_SENTINEL"
        fi
    done
}

unset_identity_env() {
    # Force deterministic "missing identity" behavior.
    export GIT_AUTHOR_NAME=""
    export GIT_AUTHOR_EMAIL=""
    export GIT_COMMITTER_NAME=""
    export GIT_COMMITTER_EMAIL=""
    export EMAIL=""
    export JJ_USER=""
    export JJ_EMAIL=""
}

restore_identity_env() {
    local vars=(
        GIT_AUTHOR_NAME
        GIT_AUTHOR_EMAIL
        GIT_COMMITTER_NAME
        GIT_COMMITTER_EMAIL
        EMAIL
        JJ_USER
        JJ_EMAIL
    )

    for var in "${vars[@]}"; do
        local backup_var="IDENTITY_BACKUP_${var}"
        local val="${!backup_var:-$IDENTITY_ENV_UNSET_SENTINEL}"
        if [[ "$val" == "$IDENTITY_ENV_UNSET_SENTINEL" ]]; then
            unset "$var"
        else
            export "$var=$val"
        fi
    done
}

configure_identities_for_current_repos() {
    if is_tool_enabled git; then
        (
            cd "$GIT_REPO"
            git config user.name "Compare User"
            git config user.email "compare@example.com"
        )
    fi

    if is_tool_enabled jj; then
        (
            cd "$JJ_REPO"
            jj config set --repo user.name "Compare User" >/dev/null 2>&1 || true
            jj config set --repo user.email "compare@example.com" >/dev/null 2>&1 || true
        )
    fi

    if is_tool_enabled libra; then
        (
            cd "$LIBRA_REPO"
            "$LIBRA_BIN" config --add --local user.name "Compare User" >/dev/null 2>&1 || true
            "$LIBRA_BIN" config --add --local user.email "compare@example.com" >/dev/null 2>&1 || true
        )
    fi
}

print_badge() {
    local expectation="$1"
    local rc="$2"

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
}

# run_case_<scope> <label>
#   <git_expect> <git_args_or_NA>
#   <jj_expect> <jj_args_or_NA>
#   <libra_expect> <libra_args_or_NA>
run_case_internal() {
    local scope="$1"; shift
    local label="$1"; shift

    local git_expect="$1"; shift
    local git_args="$1"; shift
    local jj_expect="$1"; shift
    local jj_args="$1"; shift
    local libra_expect="$1"; shift
    local libra_args="$1"; shift

    printf "  %-48s" "$label"

    for tool in git jj libra; do
        if ! is_tool_enabled "$tool"; then
            printf "  ${DIM}skip${RESET}"
            continue
        fi

        local expect args
        case "$tool" in
            git)
                expect="$git_expect"
                args="$git_args"
                ;;
            jj)
                expect="$jj_expect"
                args="$jj_args"
                ;;
            libra)
                expect="$libra_expect"
                args="$libra_args"
                ;;
            *)
                expect="expect_success"
                args="NA"
                ;;
        esac

        if [[ "$args" == "NA" ]]; then
            record_na "$tool" "$label"
            printf "  ${DIM}N/A${RESET} "
            continue
        fi

        local rc=0
        if [[ "$scope" == "repo" ]]; then
            local repo
            repo="$(get_repo "$tool")"
            (
                cd "$repo"
                eval "run_tool $tool '$label' $args"
            ) || rc=$?
        else
            (
                cd "$SANDBOX"
                eval "run_tool $tool '$label' $args"
            ) || rc=$?
        fi

        record_result "$tool" "$label" "$rc" "$expect"
        print_badge "$expect" "$rc"
    done

    printf "\n"
}

run_case_here() {
    run_case_internal "here" "$@"
}

run_case_repo() {
    run_case_internal "repo" "$@"
}

quote_for_shell() {
    printf '%q' "$1"
}

run_command_surface_category() {
    local category="Command Surface"
    log_section "$category"
    set_category "$category"
    register_category "$category"
    md_section "$category"

    local git_lfs_available=0
    if is_tool_enabled git && git lfs version >/dev/null 2>&1; then
        git_lfs_available=1
    fi

    local -a command_specs=(
        "init|init|git init|init"
        "clone|clone|git clone|clone"
        "add|add|file track|add"
        "rm|rm|file untrack|rm"
        "restore|restore|restore|restore"
        "status|status|status|status"
        "clean|clean|NA|clean"
        "stash|stash|NA|stash"
        "lfs|lfs|NA|lfs"
        "log|log|log|log"
        "shortlog|shortlog|NA|shortlog"
        "show|show|show|show"
        "show_ref|show-ref|NA|show-ref"
        "branch|branch|bookmark|branch"
        "tag|tag|tag|tag"
        "commit|commit|commit|commit"
        "switch|switch|edit|switch"
        "rebase|rebase|rebase|rebase"
        "merge|merge|NA|merge"
        "reset|reset|NA|reset"
        "mv|mv|NA|mv"
        "describe|describe|describe|describe"
        "cherry_pick|cherry-pick|NA|cherry-pick"
        "push|push|git push|push"
        "fetch|fetch|git fetch|fetch"
        "pull|pull|NA|pull"
        "diff|diff|diff|diff"
        "blame|blame|file annotate|blame"
        "revert|revert|revert|revert"
        "remote|remote|git remote|remote"
        "config|config|config|config"
        "reflog|reflog|operation log|reflog"
        "worktree|worktree|workspace|worktree"
        "cat_file|cat-file|NA|cat-file"
        "checkout|checkout|edit|checkout"
        "index_pack|index-pack|NA|index-pack"
    )

    local invalid_flag="--__libra_compare_invalid_flag__"

    for spec in "${command_specs[@]}"; do
        IFS='|' read -r name git_cmd jj_cmd libra_cmd <<< "$spec"

        if [[ "$name" == "lfs" && "$git_lfs_available" -eq 0 ]]; then
            git_cmd="NA"
        fi

        local git_help="NA" jj_help="NA" libra_help="NA"
        local git_invalid="NA" jj_invalid="NA" libra_invalid="NA"

        if [[ "$git_cmd" != "NA" ]]; then
            git_help="$git_cmd --help"
            git_invalid="$git_cmd $invalid_flag"
        fi
        if [[ "$jj_cmd" != "NA" ]]; then
            jj_help="$jj_cmd --help"
            jj_invalid="$jj_cmd $invalid_flag"
        fi
        if [[ "$libra_cmd" != "NA" ]]; then
            libra_help="$libra_cmd --help"
            libra_invalid="$libra_cmd $invalid_flag"
        fi

        run_case_here "surface_${name}_help" \
            "expect_success" "$git_help" \
            "expect_success" "$jj_help" \
            "expect_success" "$libra_help"

        run_case_here "surface_${name}_invalid" \
            "expect_fail" "$git_invalid" \
            "expect_fail" "$jj_invalid" \
            "expect_fail" "$libra_invalid"
    done

    md_category_summary "$category"
}

run_identity_config_category() {
    local category="Identity Config"
    log_section "$category"
    set_category "$category"
    register_category "$category"
    md_section "$category"

    setup_all_repos "identity" "--no-config"

    create_file_in_repos "identity.txt" "identity without config"

    run_case_repo "identity_add_no_config" \
        "expect_success" "add identity.txt" \
        "expect_success" "file track identity.txt" \
        "expect_success" "add identity.txt"

    backup_identity_env
    unset_identity_env

    run_case_repo "identity_commit_no_config" \
        "expect_fail" "commit -m identity_no_config" \
        "expect_success" "commit -m identity_no_config" \
        "expect_success" "commit -m identity_no_config"

    restore_identity_env

    configure_identities_for_current_repos

    create_file_in_repos "identity.txt" "identity with config"

    run_case_repo "identity_add_with_config" \
        "expect_success" "add identity.txt" \
        "expect_success" "file track identity.txt" \
        "expect_success" "add identity.txt"

    run_case_repo "identity_commit_with_config" \
        "expect_success" "commit -m identity_with_config" \
        "expect_success" "commit -m identity_with_config" \
        "expect_success" "commit -m identity_with_config"

    md_category_summary "$category"
}

run_behavior_matrix_category() {
    local category="Behavior Matrix"
    log_section "$category"
    set_category "$category"
    register_category "$category"
    md_section "$category"

    setup_all_repos "behavior"
    configure_identities_for_current_repos

    create_file_in_repos "README.md" "line1"

    run_case_repo "behavior_add_initial" \
        "expect_success" "add README.md" \
        "expect_success" "file track README.md" \
        "expect_success" "add README.md"

    run_case_repo "behavior_commit_initial" \
        "expect_success" "commit -m behavior_initial" \
        "expect_success" "commit -m behavior_initial" \
        "expect_success" "commit -m behavior_initial"

    run_case_repo "behavior_status_clean" \
        "expect_success" "status --short" \
        "expect_success" "status" \
        "expect_success" "status --short"

    run_case_repo "behavior_add_missing" \
        "expect_fail" "add DOES_NOT_EXIST.txt" \
        "expect_success" "file track DOES_NOT_EXIST.txt" \
        "expect_fail" "add DOES_NOT_EXIST.txt"

    run_case_repo "behavior_log_head" \
        "expect_success" "log -n 1 --oneline" \
        "expect_success" "log -n 1 --no-graph" \
        "expect_success" "log -n 1 --oneline"

    run_case_repo "behavior_shortlog" \
        "expect_success" "shortlog -s HEAD" \
        "expect_success" "NA" \
        "expect_success" "shortlog -s"

    run_case_repo "behavior_show" \
        "expect_success" "show --stat --oneline -1" \
        "expect_success" "show @-" \
        "expect_success" "show --stat"

    run_case_repo "behavior_show_ref" \
        "expect_success" "show-ref" \
        "expect_success" "NA" \
        "expect_success" "show-ref"

    run_case_repo "behavior_branch_create" \
        "expect_success" "branch feature" \
        "expect_success" "bookmark create feature -r @-" \
        "expect_success" "branch feature"

    run_case_repo "behavior_branch_invalid_base" \
        "expect_fail" "branch bad_branch badref" \
        "expect_fail" "bookmark create bad_branch -r badref" \
        "expect_fail" "branch bad_branch badref"

    run_case_repo "behavior_switch_feature" \
        "expect_success" "switch feature" \
        "expect_success" "edit feature" \
        "expect_success" "switch feature"

    run_case_repo "behavior_switch_missing" \
        "expect_fail" "switch no_such_branch_123" \
        "expect_fail" "edit no_such_branch_123" \
        "expect_fail" "switch no_such_branch_123"

    create_file_in_repos "README.md" $'line1\nline2 behavior'

    run_case_repo "behavior_diff" \
        "expect_success" "diff" \
        "expect_success" "diff" \
        "expect_success" "diff"

    run_case_repo "behavior_add_second" \
        "expect_success" "add README.md" \
        "expect_success" "file track README.md" \
        "expect_success" "add README.md"

    run_case_repo "behavior_commit_second" \
        "expect_success" "commit -m behavior_second" \
        "expect_success" "commit -m behavior_second" \
        "expect_success" "commit -m behavior_second"

    run_case_repo "behavior_blame" \
        "expect_success" "blame README.md" \
        "expect_success" "file annotate README.md" \
        "expect_success" "blame README.md"

    run_case_repo "behavior_cat_file_type" \
        "expect_success" "cat-file -t HEAD" \
        "expect_success" "NA" \
        "expect_success" "cat-file -t HEAD"

    run_case_repo "behavior_cat_file_invalid" \
        "expect_fail" "cat-file -t deadbeef" \
        "expect_fail" "NA" \
        "expect_fail" "cat-file -t deadbeef"

    run_case_repo "behavior_tag_create" \
        "expect_success" "tag behavior-v1" \
        "expect_success" "tag set behavior-v1 -r @-" \
        "expect_success" "tag behavior-v1"

    run_case_repo "behavior_tag_delete_missing" \
        "expect_fail" "tag -d no_such_behavior_tag" \
        "expect_success" "tag delete no_such_behavior_tag" \
        "expect_fail" "tag -d no_such_behavior_tag"

    create_file_in_repos "stash.txt" "stash me"

    run_case_repo "behavior_stash_push" \
        "expect_success" "stash push -m behavior_stash" \
        "expect_success" "NA" \
        "expect_success" "stash push --message behavior_stash"

    run_case_repo "behavior_stash_pop_missing" \
        "expect_fail" "stash pop stash@{99}" \
        "expect_fail" "NA" \
        "expect_fail" "stash pop stash@{99}"

    run_case_repo "behavior_restore_missing" \
        "expect_fail" "restore no_such_file.txt" \
        "expect_success" "restore no_such_file.txt" \
        "expect_fail" "restore no_such_file.txt"

    create_file_in_repos "reset.txt" "reset me"

    run_case_repo "behavior_add_reset" \
        "expect_success" "add reset.txt" \
        "expect_success" "file track reset.txt" \
        "expect_success" "add reset.txt"

    run_case_repo "behavior_reset_path" \
        "expect_success" "reset HEAD reset.txt" \
        "expect_success" "NA" \
        "expect_success" "reset HEAD reset.txt"

    create_file_in_repos "move_src.txt" "move me"

    run_case_repo "behavior_add_move_src" \
        "expect_success" "add move_src.txt" \
        "expect_success" "file track move_src.txt" \
        "expect_success" "add move_src.txt"

    run_case_repo "behavior_mv_success" \
        "expect_success" "mv move_src.txt move_dst.txt" \
        "expect_success" "NA" \
        "expect_success" "mv move_src.txt move_dst.txt"

    run_case_repo "behavior_mv_missing" \
        "expect_fail" "mv missing_src.txt move_x.txt" \
        "expect_fail" "NA" \
        "expect_fail" "mv missing_src.txt move_x.txt"

    run_case_repo "behavior_rm_missing" \
        "expect_fail" "rm missing_rm.txt" \
        "expect_success" "file untrack missing_rm.txt" \
        "expect_fail" "rm missing_rm.txt"

    run_case_repo "behavior_reflog" \
        "expect_success" "reflog -n 1" \
        "expect_success" "operation log -n 1" \
        "expect_success" "reflog show"

    run_case_repo "behavior_worktree_list" \
        "expect_success" "worktree list" \
        "expect_success" "workspace list" \
        "expect_success" "worktree list"

    run_case_repo "behavior_config_list" \
        "expect_success" "config --list" \
        "expect_success" "config list" \
        "expect_success" "config --list"

    run_case_repo "behavior_describe" \
        "expect_success" "describe --tags --always" \
        "expect_success" "describe -m behavior_describe" \
        "expect_success" "describe --tags"

    run_case_repo "behavior_rebase_invalid" \
        "expect_fail" "rebase no_such_base_123" \
        "expect_fail" "rebase -b @ -o no_such_base_123" \
        "expect_success" "rebase no_such_base_123"

    run_case_repo "behavior_merge_invalid" \
        "expect_fail" "merge no_such_branch_123" \
        "expect_fail" "NA" \
        "expect_fail" "merge no_such_branch_123"

    run_case_repo "behavior_revert_invalid" \
        "expect_fail" "revert --no-edit deadbeef" \
        "expect_fail" "revert -r deadbeef" \
        "expect_fail" "revert deadbeef"

    run_case_repo "behavior_cherry_pick_invalid" \
        "expect_fail" "cherry-pick deadbeef" \
        "expect_fail" "NA" \
        "expect_fail" "cherry-pick deadbeef"

    run_case_repo "behavior_checkout_feature" \
        "expect_success" "checkout feature" \
        "expect_success" "NA" \
        "expect_fail" "checkout feature"

    create_file_in_repos "junk.tmp" "junk"

    run_case_repo "behavior_clean_dry_run" \
        "expect_success" "clean -n" \
        "expect_success" "NA" \
        "expect_success" "clean -n"

    local origin
    origin="$(create_bare_remote "behavior-origin.git")"
    local origin_q
    origin_q="$(quote_for_shell "$origin")"

    run_case_repo "behavior_remote_add_origin" \
        "expect_success" "remote add origin $origin_q" \
        "expect_success" "git remote add origin $origin_q" \
        "expect_success" "remote add origin $origin_q"

    run_case_repo "behavior_remote_add_origin_duplicate" \
        "expect_fail" "remote add origin $origin_q" \
        "expect_fail" "git remote add origin $origin_q" \
        "expect_success" "remote add origin $origin_q"

    run_case_repo "behavior_fetch_missing" \
        "expect_fail" "fetch missing" \
        "expect_fail" "git fetch --remote missing" \
        "expect_fail" "fetch missing"

    run_case_repo "behavior_fetch_origin" \
        "expect_success" "fetch origin" \
        "expect_success" "git fetch --remote origin" \
        "expect_success" "fetch origin"

    run_case_repo "behavior_pull_no_tracking" \
        "expect_fail" "pull" \
        "expect_fail" "NA" \
        "expect_fail" "pull"

    md_category_summary "$category"
}

run_flow_experience_category() {
    local category="Flow Experience"
    log_section "$category"
    set_category "$category"
    register_category "$category"
    md_section "$category"

    # Start from brand-new directories (no pre-init) to simulate first-time flow.
    GIT_REPO="$(make_temp_repo "flow_git")"
    JJ_REPO="$(make_temp_repo "flow_jj")"
    LIBRA_REPO="$(make_temp_repo "flow_libra")"

    run_case_repo "flow_01_init_repo" \
        "expect_success" "init" \
        "expect_success" "git init" \
        "expect_success" "init"

    run_case_repo "flow_02_status_empty" \
        "expect_success" "status --short" \
        "expect_success" "status" \
        "expect_success" "status --short"

    run_case_repo "flow_03_add_missing" \
        "expect_fail" "add README.md" \
        "expect_success" "file track README.md" \
        "expect_fail" "add README.md"

    create_file_in_repos "README.md" "flow line 1"

    run_case_repo "flow_04_add_readme" \
        "expect_success" "add README.md" \
        "expect_success" "file track README.md" \
        "expect_success" "add README.md"

    backup_identity_env
    unset_identity_env

    run_case_repo "flow_05_commit_no_config" \
        "expect_fail" "commit -m flow_first_without_config" \
        "expect_success" "commit -m flow_first_without_config" \
        "expect_success" "commit -m flow_first_without_config"

    restore_identity_env

    run_case_repo "flow_06_set_name" \
        "expect_success" "config user.name FlowUser" \
        "expect_success" "config set --repo user.name FlowUser" \
        "expect_success" "config --add --local user.name FlowUser"

    run_case_repo "flow_07_set_email" \
        "expect_success" "config user.email flow@example.com" \
        "expect_success" "config set --repo user.email flow@example.com" \
        "expect_success" "config --add --local user.email flow@example.com"

    create_file_in_repos "README.md" $'flow line 1\nflow line 2'

    run_case_repo "flow_08_add_readme_again" \
        "expect_success" "add README.md" \
        "expect_success" "file track README.md" \
        "expect_success" "add README.md"

    run_case_repo "flow_09_commit_success" \
        "expect_success" "commit -m flow_second_with_config" \
        "expect_success" "commit -m flow_second_with_config" \
        "expect_success" "commit -m flow_second_with_config"

    run_case_repo "flow_10_cat_file_head" \
        "expect_success" "cat-file -t HEAD" \
        "expect_success" "NA" \
        "expect_success" "cat-file -t HEAD"

    run_case_repo "flow_11_tag" \
        "expect_success" "tag flow-v1" \
        "expect_success" "tag set flow-v1 -r @-" \
        "expect_success" "tag flow-v1"

    run_case_repo "flow_12_blame" \
        "expect_success" "blame README.md" \
        "expect_success" "file annotate README.md" \
        "expect_success" "blame README.md"

    local flow_remote
    flow_remote="$(create_bare_remote "flow-origin.git")"
    local flow_remote_q
    flow_remote_q="$(quote_for_shell "$flow_remote")"

    run_case_repo "flow_13_remote_add_origin" \
        "expect_success" "remote add origin $flow_remote_q" \
        "expect_success" "git remote add origin $flow_remote_q" \
        "expect_success" "remote add origin $flow_remote_q"

    run_case_repo "flow_14_prepare_push_ref" \
        "expect_success" "branch -f flow-main-git" \
        "expect_success" "bookmark create flow-main-jj -r @-" \
        "expect_success" "branch flow-main-libra"

    run_case_repo "flow_15_push_local_remote" \
        "expect_success" "push -u origin HEAD:refs/heads/flow-main-git" \
        "expect_success" "git push --bookmark flow-main-jj" \
        "expect_fail" "push origin flow-main-libra"

    run_case_repo "flow_16_fetch_origin" \
        "expect_success" "fetch origin" \
        "expect_success" "git fetch --remote origin" \
        "expect_success" "fetch origin"

    run_case_repo "flow_17_pull_without_tracking" \
        "expect_success" "pull" \
        "expect_fail" "NA" \
        "expect_fail" "pull"

    if [[ "$SKIP_GITHUB_PUSH" -eq 0 ]]; then
        local github_fail_url="https://github.com/nonexistent-owner/libra-compare-nonexistent.git"
        local github_fail_q
        github_fail_q="$(quote_for_shell "$github_fail_url")"

        run_case_repo "flow_18_remote_add_github_fail" \
            "expect_success" "remote add github-fail $github_fail_q" \
            "expect_success" "git remote add github-fail $github_fail_q" \
            "expect_success" "remote add github-fail $github_fail_q"

        run_case_repo "flow_19_push_github_fail" \
            "expect_fail" "push github-fail HEAD:refs/heads/flow-main-git" \
            "expect_fail" "git push --remote github-fail --bookmark flow-main-jj" \
            "expect_fail" "push github-fail flow-main-libra"
    else
        log_warn "Skipping GitHub push failure scenario (--skip-github-push)."
    fi

    run_case_repo "flow_20_fetch_missing_remote" \
        "expect_fail" "fetch missing" \
        "expect_fail" "git fetch --remote missing" \
        "expect_fail" "fetch missing"

    local clone_git_dir="$SANDBOX/repos/flow_clone_git"
    local clone_jj_dir="$SANDBOX/repos/flow_clone_jj"
    local clone_libra_dir="$SANDBOX/repos/flow_clone_libra"

    rm -rf "$clone_git_dir" "$clone_jj_dir" "$clone_libra_dir"

    local clone_git_q clone_jj_q clone_libra_q
    clone_git_q="$(quote_for_shell "$clone_git_dir")"
    clone_jj_q="$(quote_for_shell "$clone_jj_dir")"
    clone_libra_q="$(quote_for_shell "$clone_libra_dir")"

    run_case_here "flow_21_clone_from_origin" \
        "expect_success" "clone $flow_remote_q $clone_git_q" \
        "expect_success" "git clone $flow_remote_q $clone_jj_q" \
        "expect_success" "clone $flow_remote_q $clone_libra_q"

    run_case_here "flow_22_clone_missing_remote" \
        "expect_fail" "clone /tmp/libra-compare-no-such-remote $clone_git_q.missing" \
        "expect_fail" "git clone /tmp/libra-compare-no-such-remote $clone_jj_q.missing" \
        "expect_fail" "clone /tmp/libra-compare-no-such-remote $clone_libra_q.missing"

    md_category_summary "$category"
}

main() {
    parse_args "$@"
    setup_sandbox

    check_tools
    if [[ ${#ENABLED_TOOLS[@]} -eq 0 ]]; then
        echo "No tools enabled or available. Nothing to run." >&2
        exit 1
    fi

    md_init

    run_command_surface_category
    run_identity_config_category
    run_behavior_matrix_category
    run_flow_experience_category

    print_scoreboard

    log_info "Report generated: $REPORT_FILE"
    log_info "Raw outputs:      $SANDBOX/out"
}

cleanup_on_exit() {
    local rc=$?
    if [[ "$KEEP_SANDBOX" -eq 1 ]]; then
        if [[ -n "${SANDBOX:-}" ]]; then
            log_info "Sandbox kept at: $SANDBOX"
        fi
    else
        cleanup_sandbox
    fi
    exit $rc
}

trap cleanup_on_exit EXIT

main "$@"
