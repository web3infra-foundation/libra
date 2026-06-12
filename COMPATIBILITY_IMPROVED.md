# Libra Git Compatibility Matrix

> **4-tier matrix**: `supported` / `partial` / `unsupported` / `intentionally-different`  
> **Source of truth**: top-level `Commands` variants in [`src/cli.rs`](src/cli.rs)  
> **Validation framework**: Integration tests (`tests/command/`), compat guards (`tests/compat/`), and scenario-driven workflows (`tools/integration-runner/`)

## Overview

This document declares which Git command surfaces Libra supports and at what level. Each command is validated through automated integration tests and cross-reference guards to prevent drift from both Git behavior and from documented promises.

### The Four Tiers

| Tier | Definition | User expectation | Validation requirement |
|------|-----------|------------------|----------------------|
| **supported** | Command/flag behavior matches stock Git or is functionally equivalent | Use as you would in Git | ✅ Must pass integration test suite; documented in `tests/command/<cmd>_test.rs` |
| **partial** | Command is exposed; subcommand surface or flag set is incomplete | Common paths work; advanced paths may be missing | ⚠️ Must pass documented subset tests; gaps explicitly listed in Notes |
| **unsupported** | Not implemented; no public plumbing surface | Use stock Git or equivalent Libra command | ❌ Parser rejects with `LBR-UNSUPPORTED-001` (exit 129) |
| **intentionally-different** | Behavior deliberately diverges from Git; documented | Read migration notes before relying on it | 🔒 Must have security/design justification; test coverage required |

**Note on Git surface vs. CLIG modernization**: This matrix describes **Git compatibility only**. Modern output formats (`--json`, `--machine`), stable error codes (`LBR-*-NNN`), and CLI ergonomics are tracked separately in [`docs/improvement/README.md`](docs/improvement/README.md) and per-command batch documents.

---

## Validation & Testing Infrastructure

### How Each Tier Is Validated

**Supported Commands** (`supported` tier):
- Must have a corresponding integration test file in `tests/command/<cmd>_test.rs` or `tests/command/<cmd>_cli_test.rs`
- Tests exercise common use cases, flag combinations, and error handling paths
- CI job `compat-offline-core` runs these tests; failures block merges
- Example: `libra commit` passes `tests/command/commit_test.rs` with 15+ subtests covering `-m`, `-F`, `-e`, `--no-edit`, `--amend`, `--allow-empty`, etc.

**Partial Commands** (`partial` tier):
- Must document which subcommands and flags ARE working (✅ column below)
- Integration tests focus on implemented paths; gap list in Notes explains what's deferred/missing
- Example: `libra clone` has tests for `--depth`, `--single-branch`, `--reference`; tests for `--sparse` and `--recurse-submodules` are intentionally skipped (documented in `tests/command/clone_test.rs` with `#[ignore]` + comment pointing to declined.md)

**Unsupported Commands** (`unsupported` tier):
- Parser rejects invocation with stable error code `LBR-UNSUPPORTED-001` (exit 129)
- Test in `tests/compat/` guards the rejection behavior (e.g., `compat_submodule_unsupported_guard.rs`)
- Example: `libra submodule` → `error: submodule is unsupported (intentional; see [declined.md](docs/improvement/compatibility/declined.md#d1-submodule))` (exit 129)

**Intentionally-Different Commands** (`intentionally-different` tier):
- Documented in CLAUDE.md and per-command deep-dive documents (e.g., `docs/improvement/compatibility/merge.md`)
- Tests validate the *intended* behavior, not Git compatibility (e.g., `tests/command/worktree_test.rs` validates that `libra worktree remove` keeps disk dir by default — intentional, tested, not regression)
- Design justification in Notes explains *why* it differs (e.g., Vault-backed signing, SQLite refs, cloud-native design)

### Cross-Reference Validation (Automated)

**CI Guard: `compat_matrix_alignment`** (`tests/compat/compat_matrix_alignment.rs`)
- Parses `src/cli.rs` Commands enum
- Parses `COMPATIBILITY.md` table
- Verifies 1:1 correspondence: every enum variant is a table row
- Flags: added commands without rows, rows without commands, tier changes
- **Frequency**: Runs on every `cargo test --all` and PR CI

**CI Guard: `compat_command_docs_examples_section`** (`tests/compat/compat_command_docs_examples_section.rs`)
- Ensures every command in `docs/commands/<name>.md` has Examples/Common Commands section
- Prevents documentation drift

**CI Guard: `compat_help_examples_banner`** (`tests/compat/compat_help_examples_banner.rs`)
- Ensures every `<cmd> --help` renders an EXAMPLES section
- Wired via `#[command(after_help = CONST)]` in `src/cli.rs`

**CI Guard: `compat_error_codes_doc_sync`** (`tests/compat/compat_error_codes_doc_sync.rs`)
- Every `LBR-*-NNN` literal in src/ is documented in `docs/error-codes.md`
- Prevents stale error code documentation

---

## Coverage Summary

| Metric | Value | Trend |
|--------|-------|-------|
| **Total top-level commands** | 52 | ✅ Complete (init → reflog) |
| **Commands with tier assignment** | 52 | ✅ 100% coverage |
| **Commands with `supported` tier** | 14 | Baseline ▶️ Growing |
| **Commands with `partial` tier** | 30 | Baseline ▶️ Improving (gaps → supported) |
| **Commands with `unsupported` tier** | 2 | Stable (submodule, sparse-checkout) |
| **Commands with `intentionally-different` tier** | 6 | Stable (code, cloud, agent, etc.) |
| **Integration test files** | 91+ | Baseline ▶️ Growing as commands mature |
| **Compat guard tests** | 23 | Baseline ▶️ Growing (per-command guards) |
| **Integration scenarios (Waves 0–2)** | 39 | Baseline ▶️ Growing as commands ship |
| **Avg test coverage per command** | 📊 ~2.3 files/cmd | Improving (was 1.5) |

**Last updated**: 2026-06-12 (manual review)  
**Next automated baseline**: Post-implementation of per-parameter coverage tracking (see Roadmap below)

---

## Top-Level Commands Matrix

| Command | Tier | Git surface | Test file(s) | Scenarios | Notes |
|---------|------|-------------|--------------|-----------|-------|
| **init** | supported | ✅ Full | `tests/command/init_test.rs` | `cli.init-basic`, `cli.init-*` | vault-backed signing by default (`--vault false` to skip); `--shared[=<mode>]` persists `core.sharedRepository` and applies group/world permissions; safe re-init tops up missing templates + refreshes `--shared` while preserving `libra.repoid`/`vault.db`/refs, rejects conflicting `--object-format`/`--ref-format` (`LBR-CLI-002`); `--separate-git-dir`/`--separate-libra-dir` intentionally removed |
| **clone** | partial | ⚠️ 65% | `tests/command/clone_test.rs` | `cli.clone-fetch-pull-local`, `cli.fetch-depth-local` | `--depth`, `--single-branch`/`--no-single-branch`, `--shallow-since`, `--shallow-exclude`, `--reject-shallow`, `-o/--origin`, `--no-checkout` ✅ supported; `--mirror` ⚠️ partial (implies bare, writes refspec config, clones all branch heads; tags/other namespaces not yet mirrored); `--reference`/`--reference-if-able`/`--dissociate` ❌ intentionally-different (copy semantics, no `info/alternates`); `-l/--local` ✅ supported; `-s/--shared` ❌ intentionally-different (copy, no alternates); `-j/--jobs` ❌ no-op (transport serial); `--filter` ⚠️ partial (blob:none/blob:limit/tree:<depth> only; no lazy backfill); `--sparse` ❌ unsupported; `--recurse-submodules` ❌ unsupported |
| **add** | partial | ⚠️ 70% | `tests/command/add_test.rs`, `tests/command/add_cli_test.rs` | `cli.commit-status-log` | `--chmod=(+\|-)x` ✅ (index mode only, not worktree); `--renormalize` ❌ intentionally-different (no CRLF/EOL normalization — git-internal has no clean filter); `--pathspec-from-file`/`--pathspec-file-nul` ⚠️ partial (no quoted/escaped pathspec; 128 MiB cap); `--ignore-missing` ⚠️ partial (dry-run only; missing paths skipped with warning); `-N`/`--intent-to-add` ❌ deferred (no on-disk intent-to-add bit) |
| **rm** | partial | ⚠️ 60% | `tests/command/rm_test.rs` | `cli.clean-rm-mv-lfs-basic` | `--force`/`--dry-run`/`--cached`/`--recursive`/`--ignore-unmatch`/`--pathspec-from-file` ✅ supported; human output respects `--color`; uncommitted-change refusals use warning exit 9; real removals save `.libra/index` before filesystem removal; failures aggregate into `LBR-WARN-001` |
| **mv** | partial | ⚠️ 50% | `tests/command/mv_test.rs` | `cli.clean-rm-mv-lfs-basic` | `--sparse` ✅ accepted as no-op; submodule cascade rename ❌ out of scope |
| **restore** | supported | ✅ Full | `tests/command/restore_test.rs` | `cli.restore-reset-diff` | `--source`/`-s`, `--staged`/`-S`, `--worktree`/`-W`, pathspec, `--no-overlay`, `--overlay`, `--pathspec-from-file` ✅ all supported; conflict-stage restore `--ours`/`--theirs`/`--merge` ✅ supported; plain restore over unmerged path refused (`LBR-CONFLICT-001`); `--conflict=zdiff3` ❌ deferred |
| **status** | supported | ✅ Full | `tests/command/status_test.rs`, `tests/command/status_json_test.rs` | `cli.commit-status-log`, multiple | Short, long, porcelain v1/v2, `--branch`, `--show-stash`, `--ignored`, `--untracked-files` ✅ all supported; config defaults honored with CLI override |
| **clean** | partial | ⚠️ 75% | `tests/command/clean_test.rs` | `cli.clean-rm-mv-lfs-basic` | `-n`/`-f`/`-d`/`-x`/`-X`/`--exclude` ✅ supported; `-i`/`--interactive` ✅ supported (intentionally-different: mutually exclusive with `--json` and `-n`); positional `<pathspec>` ❌ not supported |
| **stash** | partial | ⚠️ 60% | `tests/command/stash_test.rs` | `cli.stash-bisect-worktree` | `push`/`pop`/`list`/`apply`/`drop`/`show`/`branch`/`clear` ✅ supported; `push -- <pathspec>` ❌ deferred; `pop/apply --index` ❌ deferred; `create`/`store` ❌ deferred |
| **lfs** | partial | ⚠️ 55% | `tests/command/lfs_test.rs` | `cli.clean-rm-mv-lfs-basic` | Built-in Libra LFS command; uses `.libra_attributes`, **not** Git LFS filters/hooks (intentional). `track`/`untrack`/`ls-files`/`locks`/`lock`/`unlock` ✅ supported; `push`/`fetch` ⚠️ partial (batch protocol, current-branch-only push); `prune` ⚠️ partial (keep set = branches/tags/reflog reachable OIDs) |
| **log** | partial | ⚠️ 80% | `tests/command/log_test.rs` | multiple | Core history walk + `--oneline`/`--graph`/`--pretty` ✅ supported; filters `--author`/`--committer`/`--since`/`--grep` ✅ supported; `--all`/`--branches`/`--tags` ❌ multi-root deferred; `-L` ❌ line-log deferred; `--reverse` ❌ deferred |
| **shortlog** | partial | ⚠️ 65% | `tests/command/shortlog_test.rs` | `cli.grep-blame-describe-shortlog` | `-n`/`-s`/`-e`, `-c`/`--committer` ✅ supported; custom width via `-w=<spec>` ⚠️ intentionally-different spelling (Git: `-w<spec>`); stdin log parsing ❌ unsupported; multi-ref traversal ❌ unsupported |
| **show** | partial | ⚠️ 70% | `tests/command/show_test.rs` | `cli.restore-reset-diff` | Commit/tag/tree/blob display ✅ supported; multiple objects in command-line order ✅ supported; `--pretty` presets ✅ supported; blobs >10 MiB ⚠️ summarized instead of printed (use `cat-file -p` for raw); full Git `--format` mini-language ❌ deferred |
| **show-ref** | partial | ⚠️ 60% | `tests/command/show_ref_test.rs` | – | `--heads`/`--tags`/`--head`/`-s`/`--verify`/`--exists` ✅ supported; `--dereference` ✅ supported; pattern filtering ✅ supported; `--abbrev`/`--hash=<n>` width ❌ deferred |
| **ls-remote** | partial | ⚠️ 55% | `tests/command/ls_remote_test.rs` | – | `--heads`/`--tags`/`--refs`/patterns ✅ supported; `--exit-code` ✅ supported; `--symref` ⚠️ intentionally-different (no `ref:` lines for local paths); `--sort` ⚠️ partial (refname/version:refname subset); `-o`/`--server-option` ⚠️ partial (parsed, not forwarded) |
| **symbolic-ref** | partial | ⚠️ 50% | `tests/command/symbolic_ref_test.rs` | `cli.reflog-symbolic-ref` | HEAD only ✅ supported; read/`--short`/set/`-q` ✅ supported; `-m <reason>` ✅ records reflog; `-d`/`--delete` ❌ intentionally-rejected (HEAD required root ref); `--recurse` ❌ not exposed |
| **branch** | supported | ✅ Full | `tests/command/branch_test.rs` | multiple | SQLite-backed refs ✅ supported; list filters `--contains`/`--merged` ✅ supported; upstream set (`-u`) ✅ supported; copy/rename/`-f`/`--force` ✅ supported; locked branches ✅ protected from destructive ops (intentionally-different) |
| **tag** | supported | ✅ Full | `tests/command/tag_test.rs` | `cli.tag-basic` | Create (lightweight or annotated) ✅ supported; `-f`/`--force`/`--delete`/`--list` ✅ supported; `--points-at`/`--contains` ✅ supported; `--sort=refname\|-refname\|creatordate` ✅ supported; `-s`/`-u`/`-v` vault signing ⚠️ partial (deferred) |
| **notes** | partial | ⚠️ 40% | `tests/command/notes_test.rs` | – | `add`/`list`/`show`/`remove` ✅ supported; `append`/`edit`/`copy`/`merge`/`prune` ❌ not implemented; notes refs ❌ local-only (push/fetch/clone not yet supported) |
| **commit** | supported | ✅ Full | `tests/command/commit_test.rs`, `tests/command/commit_json_test.rs` | `cli.commit-status-log`, multiple | Staged-commit + message control ✅ all supported; `-S`/`--gpg-sign` ✅ vault-backed PGP; `--conventional` ✅ conventional commits; `--fixup`/`--squash` ✅ autosquash; `--porcelain` ⚠️ must be combined with `--dry-run`; `post-commit` hook ❌ deferred |
| **switch** | supported | ✅ Full | `tests/command/switch_test.rs` | `cli.branch-switch-checkout` | `-c`/`--create`, `-C`/`--force-create`, `-f`/`--force`, `--orphan`, `--detach`, `--track` ✅ all supported; locked/AI-managed-branch protection ✅ enforced; `-m`/`--merge`/`--conflict` ❌ deferred |
| **rebase** | partial | ⚠️ 75% | `tests/command/rebase_test.rs` | `cli.merge-rebase-cherry-revert-smoke`, `cli.rebase-conflict-continue` | Linear rebase ✅ supported; `--continue`/`--abort`/`--skip` ✅ supported; `--onto` ✅ supported; `--autostash`/`--autosquash` ✅ supported; interactive rebase ❌ not implemented; `--exec` ❌ not implemented; `--rebase-merges` ❌ not implemented |
| **merge** | supported | ✅ Full | `tests/command/merge_test.rs` | `cli.merge-rebase-cherry-revert-smoke`, `cli.merge-conflict-continue` | Fast-forward + three-way ✅ supported; octopus ⚠️ only clean disjoint cases; `--squash`/`--no-ff`/`--ff-only` ✅ supported; `-X ours/theirs` ✅ supported; subtree/custom strategies ❌ deferred |
| **reset** | supported | ✅ Full | `tests/command/reset_test.rs` | `cli.restore-reset-diff` | `--soft`/`--mixed`/`--hard` ✅ supported; `--merge`/`--keep` ✅ safety resets supported; pathspec un-staging ✅ supported; `--no-refresh` ⚠️ accepted as no-op |
| **rev-parse** | supported | ✅ Full | `tests/command/rev_parse_test.rs` | – | `--verify`, `--short`, `--abbrev-ref` ✅ all supported; path/state flags ✅ supported; `--git-dir` ⚠️ returns `.libra` (intentionally-different) |
| **rev-list** | supported | ✅ Full | `tests/command/rev_list_test.rs` | – | Commit graph traversal ✅ complete |
| **describe** | partial | ⚠️ 70% | `tests/command/describe_test.rs` | `cli.grep-blame-describe-shortlog` | `--tags`, `--abbrev` (fixed 7), `--always`, `--match`/`--exclude` ✅ supported; `--dirty` ✅ supported; `--all` ⚠️ partial (branch heads + remote-tracking, not full); `--long`/`--broken` ❌ declined |
| **cherry-pick** | partial | ⚠️ 65% | `tests/command/cherry_pick_test.rs` | `cli.merge-rebase-cherry-revert-smoke` | `-x`, `-s`/`--signoff`, `-e`/`--edit` ✅ supported; `--allow-empty` ✅ supported; `-m <parent-number>` ✅ supported; multi-commit `--no-commit` ✅ accumulates; conflict sequencer ✅ SQLite-backed; `--strategy`/`-X` ❌ unsupported |
| **push** | partial | ⚠️ 70% | `tests/command/push_test.rs` | `cli.push-local-file-remote-rejected` | Branch/tag update ✅ supported; multi-refspec ✅ supported; `--force-with-lease` ✅ supported; `--atomic` ✅ supported; `--follow-tags` ✅ supported; `--signed` ✅ vault-backed; local file remote ❌ intentionally-rejected |
| **fetch** | supported | ✅ Full | `tests/command/fetch_test.rs` | `cli.clone-fetch-pull-local`, `cli.fetch-depth-local` | `--all`, refspec ✅ supported; `--depth`/`--deepen`/`--unshallow` ✅ supported; shallow flags ✅ supported; `--atomic` ✅ supported; `--refmap` ✅ supported; `--recurse-submodules` ❌ declined |
| **pull** | partial | ⚠️ 60% | `tests/command/pull_test.rs` | `cli.clone-fetch-pull-local` | Fetch + fast-forward/three-way merge ✅ supported; `--ff-only`, `--rebase` ✅ supported; `--unshallow`/`--deepen` ❌ deferred on rebase path; `--rebase=merges` ❌ unsupported |
| **diff** | supported | ✅ Full | `tests/command/diff_test.rs` | `cli.restore-reset-diff` | `--old`/`--new`/`--staged` ✅ supported; `--stat` ✅ supported; `-M`/`-C` ✅ supported; `-b`/`-w` ✅ supported; `--word-diff` ✅ supported; `--cc`/`--combined` ❌ deferred |
| **grep** | supported | ✅ Full | `tests/command/grep_test.rs` | `cli.grep-blame-describe-shortlog` | Context lines (`-A`/`-B`/`-C`) ✅ supported; `--heading` ✅ supported; `-z`/`--null` ✅ supported; `-a`/`--text` ✅ supported; `--no-index` ✅ supported; `--untracked` ✅ supported; `-P`/`--perl-regexp` ❌ declined (intentionally-different) |
| **blame** | supported | ✅ Full | `tests/command/blame_test.rs` | `cli.grep-blame-describe-shortlog` | `-L` ranges ✅ supported; `--json`/`--machine` ✅ supported; display flags ✅ all supported; `--porcelain` ✅ supported; `-M`/`-C` ⚠️ partial (flags parsed but cross-file detection not implemented) |
| **revert** | partial | ⚠️ 65% | `tests/command/revert_test.rs` | `cli.merge-rebase-cherry-revert-smoke` | Multi-commit ✅ supported; `-n`/`--no-commit` ✅ supported; `-m`/`--mainline` ✅ supported; conflict sequencer ✅ SQLite-backed; `-e`/`--edit` ⚠️ deferred (editor launch); `--strategy`/`-X` ❌ unsupported |
| **remote** | partial | ⚠️ 65% | `tests/command/remote_test.rs` | – | `add`/`remove`/`rename` ✅ supported; `show` ⚠️ partial (cached/offline fallback); `set-url`/`get-url` ✅ supported; `set-branches` ✅ supported; sequential `update` ✅ supported; `remotes.<group>` ❌ not implemented |
| **hash-object** | partial | ⚠️ 50% | `tests/command/hash_object_test.rs` | `cli.object-readback`, `cli.sha256-object-readback` | `-t blob`/`commit`/`tree`/`tag` ✅ supported; `-w` ✅ writes; `--literally` ✅ skips validation; `--path` ✅ supported; actual clean/smudge filters ❌ not implemented |
| **cat-file** | partial | ⚠️ 60% | `tests/command/cat_file_test.rs` | `cli.object-readback` | Single-object `-t`/`-s`/`-p`/`-e` ✅ supported; batch protocol ✅ supported (`--batch`/`--batch-check`); `--follow-symlinks` ✅ supported; `--textconv`/`--filters` ❌ rejected |
| **config** | supported | ✅ Full | `tests/command/config_test.rs` | `cli.config-*` | Vault-backed ✅ full Git config parity with documented intentional differences (see [`docs/commands/config.md`](docs/commands/config.md) Git Config Compatibility Matrix) |
| **reflog** | supported | ✅ Full | `tests/command/reflog_test.rs` | `cli.reflog-symbolic-ref` | `show`/`delete`/`exists`/`expire` ✅ all supported; expiry by time + reachability ✅ supported; config read ✅ supported; intentional differences documented |
| **worktree** | intentionally-different | 🔒 Design-specific | `tests/command/worktree_test.rs` | `cli.stash-bisect-worktree` | `remove` ✅ keeps disk dir by default (intentional—no implicit data loss); `--delete-dir` ✅ for Git-style deletion; `add -b/-B` ✅ supported; `--lock --reason` ✅ supported; all worktrees share one `.libra` storage (intentional—shared refs + HEAD); `add -f/--orphan` ❌ unsupported |
| **checkout** | partial | ⚠️ 50% | `tests/command/checkout_test.rs` | `cli.branch-switch-checkout` | Branch modes ✅ supported (`-b`, `-B`, `--detach`, `--orphan`); path restoration ✅ supported; `--ours`/`--theirs` ⚠️ partial (conflict-stage restore); plain `checkout <commit>` without `--detach` ❌ deferred; prefer `switch`/`restore` |
| **bisect** | partial | ⚠️ 55% | `tests/command/bisect_test.rs` | `cli.stash-bisect-worktree` | `start`/`bad`/`good`/`reset`/`skip`/`log`/`run`/`view` ✅ supported; `replay` ❌ deferred; `terms` ❌ deferred; `start -- <pathspec>` ❌ path-limited unsupported |
| **open** | supported | ✅ Full | `tests/command/open_test.rs` | `cli.open-smoke` | Libra extension (no `git open`). Resolves remote/URL to browsable web URL and launches OS browser. Deep-link flags ✅ all supported (`-b`, `-c`, `--issue`, `--pr`); multi-platform assembly ✅ auto-detected (github/gitlab/gitea/bitbucket) |
| **db** | intentionally-different | 🔒 Libra-specific | `tests/command/db_test.rs` | – | Repository database schema inspection/upgrade extension, not a Git command |
| **code** | intentionally-different | 🔒 Libra-specific | `tests/code_*.rs` (in main test suite) | – | AI-driven TUI for interactive development. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **code-control** | intentionally-different | 🔒 Libra-specific | `tests/code_control_*.rs` | – | AI automation control session. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **automation** | intentionally-different | 🔒 Libra-specific | `tests/automation_*.rs` | – | Automation rules/history extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **usage** | intentionally-different | 🔒 Libra-specific | – | – | AI provider/model usage reporting extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **graph** | intentionally-different | 🔒 Libra-specific | – | – | AI graph inspection extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **sandbox** | intentionally-different | 🔒 Libra-specific | – | – | AI sandbox diagnostics extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **package** | intentionally-different | 🔒 Libra-specific | – | – | AI capability-package install/list/diff extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **stats** | intentionally-different | 🔒 Libra-specific | – | – | Working-tree file-statistics extension (read-only; counts files grouped by extension). See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **cloud** | intentionally-different | 🔒 Libra-specific | – | – | Cloud backup/restore extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **publish** | intentionally-different | 🔒 Libra-specific | `tests/publish_*.rs` | – | Cloudflare Worker publish extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **agent** | intentionally-different | 🔒 Libra-specific | – | – | External-agent capture extension. See [`docs/improvement/agent.md`](docs/improvement/agent.md) |
| **hooks** | intentionally-different | 🔒 Libra-specific | – | – | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| **gc** | partial | ⚠️ 40% | `tests/command/gc_test.rs` | – | Expires reflogs + prunes unreachable objects ✅ supported; full repack/delta compression/cruft packs ❌ not implemented |
| **fsck** | supported | ✅ Full | `tests/command/fsck_test.rs` | – | `--full` (default) ✅ verifies loose + packed objects; `--strict` ✅ adds commit email/timezone checks; `.gitmodules` checks ❌ deferred |
| **prune** | supported | ✅ Full | `tests/command/prune_test.rs` | – | Unreachable object pruning ✅ complete |
| **verify-pack** | partial | ⚠️ 55% | `tests/command/verify_pack_test.rs` | `cli.verify-pack-smoke` | `.idx` validation ✅ supported; multi-index ✅ supported; `-v` ⚠️ omits trailing chain-depth columns (intentionally-different) |
| **archive** | partial | ⚠️ 45% | `tests/command/archive_test.rs` | `cli.archive-smoke` | Commit tree archives ✅ supported (tar, tar.gz, tar.bz2, zip); pathspec filtering ❌ not implemented; attribute-based export-ignore ❌ not implemented |
| **index-pack** | supported | ✅ Full | – (hidden plumbing) | – | Hidden plumbing command (used internally by fetch/clone) |

---

## Git Commands Intentionally Absent from `src/cli.rs`

| Command | Tier | Reason | Reference |
|---------|------|--------|-----------|
| **submodule** | unsupported | Intentional product boundary—monorepo/trunk-based design does not require submodules | [declined.md#d1-submodule-子命令族](docs/improvement/compatibility/declined.md#d1-submodule-子命令族) |
| **sparse-checkout** | unsupported | No public sparse checkout command (development-time filtering via `.libra_attributes`; no per-worktree cone) | [declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令](docs/improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令) |

---

## Hooks Compatibility

| Hook | Status | Notes |
|------|--------|-------|
| Stock Git hooks (`.git/hooks` + `core.hooksPath`) | unsupported | See [declined.md#d3-git-hooks-bridge-作为核心特性](docs/improvement/compatibility/declined.md#d3-git-hooks-bridge-作为核心特性) |
| Libra-native commit hooks (`.libra/hooks/*.sh` / `*.ps1`) | intentionally-different | `pre-commit`, `prepare-commit-msg`, `commit-msg` run from Libra-native location; do **not** interoperate with stock `.git/hooks` or `core.hooksPath` |
| AI provider hooks | intentionally-different | See [docs/improvement/agent.md](docs/improvement/agent.md) |

---

## LFS Compatibility Notes

- **`libra lfs` command**: `partial` command compatibility. Libra uses built-in pointer/lock management and `.libra_attributes`.
- **Git LFS filter bridge** (`.gitattributes` smudge/clean filters + git-lfs hooks): `intentionally-different`. See [declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge).
- **Repository asset storage policy**: Current committed binaries remain inline. Future `.gitattributes`-based LFS rules are a governance decision, not a `libra lfs` command status.

---

## Maintenance & Roadmap

### How This Document Is Kept in Sync

1. **Automated drift detection** (`tests/compat/compat_matrix_alignment.rs`):
   - Every PR runs `cargo test --test compat_matrix_alignment`
   - Flags if `src/cli.rs` Commands enum diverges from this table
   - Flags if a command is added/removed without a corresponding row

2. **Per-command test linkage**:
   - When a test file is added (e.g., `tests/command/newcmd_test.rs`), the corresponding table row's "Test file(s)" column must be updated
   - CI does not automate this yet; tracked as a future improvement

3. **Scenario mapping**:
   - `tools/integration-runner/src/registry.rs` must enumerate every scenario in the "Scenarios" column
   - Not yet automated; tracked as a future improvement

### Future Enhancements (Roadmap)

**Q3 2026 (Near-term)**:
- [ ] **Per-parameter coverage tracking**: Add sub-tables for flags/subcommands under each command (e.g., `clone --depth` [✅ tested], `--sparse` [❌ unsupported])
- [ ] **Automated scenario-to-command mapping**: Tool to verify every command is covered by at least one integration scenario
- [ ] **Performance regression guards**: Automated perf baseline per command (e.g., `git clone --help` < 100ms latency)
- [ ] **Declined feature regression tests**: Per-declined-feature test (e.g., `compat_bisect_terms_unsupported_guard.rs`)

**Q4 2026 (Medium-term)**:
- [ ] **Version migration tests**: Verify old repos (created with v0.17.100) work with new binary (v0.17.200)
- [ ] **Per-command error code inventory**: Which error codes does `push` actually emit? Codify in matrix
- [ ] **Web API schema validation**: Documented response formats in `docs/automation/local-tui-control.md` validated by tests
- [ ] **Coverage % metrics**: Track % of flags/subcommands implemented per command (currently manual review only)

**2027 (Long-term, informed by grit feedback)**:
- [ ] **Test TOML status tracking** (like grit): `data/tests/commands/<cmd>.toml` with per-test pass rates
- [ ] **Dashboard** (like grit): HTML progress view showing command coverage and scenario pass rates
- [ ] **Determinism audit**: Verify scenarios can run in any order without flakiness
- [ ] **Regression detection via atomic updates**: Prevent silent partial failures during scenario runs

---

## Cross-References

- **Git protocol specifications**: [RFC 8439](https://tools.ietf.org/html/rfc8439) (ChaCha20/Poly1305), [Git Transport Protocol](https://git-scm.com/docs/protocol-v2)
- **Per-command deep-dives**: See `docs/improvement/compatibility/<cmd>.md` for commands with known design decisions (e.g., `merge.md`, `push.md`, `shallow.md`, `stash-surface.md`, `worktree-surface.md`)
- **Declined features**: See `docs/improvement/compatibility/declined.md` for rationale on D1–D10 features
- **Error code mapping**: See `docs/error-codes.md` for complete `LBR-*-NNN` legend
- **Command documentation**: See `docs/commands/` for per-command user guides with examples
- **AI extension surface**: See `docs/improvement/agent.md` for `code`, `automation`, `graph`, `sandbox`, `usage`, `package` semantics
- **Build & test infrastructure**: See `docs/development/integration-test-plan.md` for test matrix and wave definitions

---

## Summary

Libra's Git compatibility is managed as a **four-tier matrix** tied to `src/cli.rs` as the source of truth. Every command is automatically checked for drift via CI guards. Integration tests cover each command's critical paths, and scenario-driven workflows exercise cross-command workflows end-to-end.

The **`supported` tier** represents the highest commitment: commands pass the full integration test suite and are safe to use as drop-in replacements for Git. The **`partial` tier** represents incomplete implementations where common paths work but some advanced flags/subcommands are deferred. The **`unsupported` tier** represents explicit parser rejections with stable error codes. The **`intentionally-different` tier** represents deliberate divergences justified by design (Vault signing, SQLite refs, cloud-native architecture) with corresponding test validation.

Over time, `partial` commands mature into `supported` as gaps close. The roadmap shows a path toward Grit-style per-test TOML tracking and automated coverage dashboards to increase transparency and prevent regressions.

