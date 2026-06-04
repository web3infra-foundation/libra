# Libra Compatibility Matrix

> 4 tiers: `supported` / `partial` / `unsupported` / `intentionally-different`
> Source of truth: top-level `Commands` variants in [`src/cli.rs`](src/cli.rs).

This file declares which Git command surfaces Libra promises to support, and at
what level. The four tiers have the following user-facing semantics:

| Tier | Meaning | What users should expect |
|------|---------|--------------------------|
| `supported` | Command/flag behavior matches stock Git or is functionally equivalent | Use as you would in Git |
| `partial` | Command is exposed but the subcommand surface or flag set is incomplete | Common paths work; advanced paths may be missing |
| `unsupported` | Not implemented, no public plumbing | Use stock Git or look for an equivalent Libra command |
| `intentionally-different` | Behavior deliberately diverges from Git; documented | Read the migration notes before relying on it |

The tier here describes **Git surface** compatibility only. It does not
describe whether a command has been modernized for CLIG (`--json` / `--machine`
/ stable error codes / `run_<cmd>()` split); that work is tracked in
[`docs/improvement/README.md`](docs/improvement/README.md) and in each command
batch document.

## Top-level commands (from `src/cli.rs`)

| Command | Tier | Notes |
|---------|------|-------|
| init | supported | |
| clone | partial | `--depth` and `--single-branch` supported; `--sparse` unsupported (see [docs/improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令](docs/improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令)); `--recurse-submodules` unsupported (see [docs/improvement/compatibility/declined.md#d4-clone---recurse-submodules](docs/improvement/compatibility/declined.md#d4-clone---recurse-submodules)) |
| code | intentionally-different | Libra AI extension, not a Git command |
| code-control | intentionally-different | Libra AI automation extension, not a Git command |
| automation | intentionally-different | Libra AI automation rules/history extension, not a Git command |
| usage | intentionally-different | Libra AI provider/model usage reporting extension, not a Git command |
| graph | intentionally-different | Libra AI graph inspection extension, not a Git command |
| sandbox | intentionally-different | Libra AI sandbox diagnostics extension, not a Git command |
| package | intentionally-different | Libra AI capability-package install/list/diff extension, not a Git command |
| add | partial | `--chmod=(+\|-)x` supported (index mode only, not worktree perms); `--renormalize` intentionally-different (force-rewrites tracked entries' blobs; no CRLF/EOL normalization — git-internal has no clean filter); `--pathspec-from-file` / `--pathspec-file-nul` partial (no quoted/escaped pathspec — raw bytes; 128 MiB cap); `--ignore-missing` partial/intentionally-different (dry-run only; missing paths skipped with a warning rather than the Git ignored-even-if-missing check); `add.ignoreErrors` config honored; `--sparse` unsupported (declined); `-N` / `--intent-to-add` deferred (on-disk index has no intent-to-add bit) |
| rm | partial | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file-nul` supported; sparse-checkout flag unsupported; per-command `--quiet` not exposed (use global `--quiet`) |
| mv | partial | sparse-checkout flag unsupported |
| restore | supported | |
| status | supported | |
| clean | partial | `-n`/`-f`/`-d`/`-x`/`-X`/`--exclude` supported; `-e` short alias for `--exclude`; `force` is a repeatable count (`-ff` required to also remove nested `.git`/`.libra` repositories, which are otherwise skipped with a `Skipping repository <path>` warning); `clean.requireForce` config honored (default `true`, blocks a mode-less invocation; set `false` to allow `clean` with no flag); `-i`/`--interactive` selection loop (`clean`/filter-by-pattern/select-by-numbers/ask-each/quit/help) — intentionally-different: mutually exclusive with `--json` and `-n` (`LBR-CLI-002`); removal is tolerant (a per-path failure is warned and recorded in the JSON `failed` field, cleanup continues, exit 128). Remaining gap: `<pathspec>...` positional filter is **not supported** (declined — would be a separate plan) |
| stash | partial | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see [docs/improvement/compatibility/declined.md#d8-stash-create](docs/improvement/compatibility/declined.md#d8-stash-create) and [#d9-stash-store](docs/improvement/compatibility/declined.md#d9-stash-store)) |
| lfs | partial | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge)) |
| log | supported | |
| shortlog | supported | |
| show | supported | |
| show-ref | supported | |
| ls-remote | supported | |
| symbolic-ref | partial | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| branch | partial | SQLite-backed refs (reference table, not `.git/refs`). List filters `--contains`/`--no-contains`/`--merged`/`--no-merged`/`--points-at` and `--ignore-case` sort supported; upstream set (`-u`) / unset (`--unset-upstream`) with tracking shown in list; copy (`-c`/`-C`) and transaction-hardened rename with config + conditional reflog migration; `--edit-description` supported; `-f`/`--force` resets existing create/copy targets (locked branches always refused); `--create-reflog` accepted-but-no-op (no per-branch reflog). `--track`/`--no-track` declined (intentionally-different — use `libra switch --track` or `-u`); `--sort`/`--format` unsupported (accepted-but-ignored, use `--json`). Locked branches (main/intent/agent-traces) are protected from destructive ops (intentionally-different) |
| tag | supported | |
| commit | supported | |
| switch | supported | |
| rebase | partial | `--autosquash` / `--reapply-cherry-picks` not supported |
| merge | supported | fast-forward, best-base single-head three-way with recursive virtual merge bases for criss-cross histories, clean disjoint octopus, `--squash`, `--no-ff`/`--ff-only`/`merge.ff`, `--no-commit`/`merge.commit`, `-m`/`-F`/`--signoff`/`--log`/`--into-name`, `-e`/`--edit`, `ours` strategy, `-X ours/theirs`, diff3 markers, `--stat`/`merge.stat`, `--autostash`/`merge.autoStash`, `-S`/`--gpg-sign` vault signing, `--verify-signatures`/`merge.verifySignatures`, whitespace-insensitive merge, rename detection (`--find-renames`/`merge.renames`), and `--diff-algorithm`/`--cleanup` validation. Remaining gaps: subtree/custom strategies and merge drivers, conflicted-octopus resolution, and directory-rename tracking are deferred; signature verification is a presence check, not cryptographic (see [docs/improvement/compatibility/merge.md](docs/improvement/compatibility/merge.md)) |
| reset | supported | |
| rev-parse | supported | |
| rev-list | supported | |
| describe | supported | |
| cherry-pick | partial | `-x` (off by default), `-s`/`--signoff`, `-e`/`--edit`, `--allow-empty`/`--allow-empty-message`/`--keep-redundant-commits`, `-m <parent-number>` (merge commits apply along the named parent only), `--ff`, and `-S`/`--gpg-sign` (reuses the vault chain, same as `merge`) supported; multi-commit `--no-commit` accumulates into the index. Conflict sequencer `--continue`/`--skip`/`--abort`/`--quit` persists in the SQLite `cherry_pick_state` table — not a `.git`/`.libra` sequencer file (intentionally-different); conflict detection is path-level, not Git's line-level hunk merge. `--strategy`/`-X`, `--empty`, `--cleanup`, `--rerere-autoupdate`, and `--commit` are unsupported (`LBR-UNSUPPORTED-001`). A `--no-commit` multi-commit conflict is terminal (no continuation) |
| push | partial | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected — intentional (see [docs/improvement/compatibility/declined.md#d2-本地-file-remote-的-push](docs/improvement/compatibility/declined.md#d2-本地-file-remote-的-push)) |
| fetch | supported | `--depth` public flag |
| pull | partial | fetch + fast-forward/three-way merge supported; `--ff-only` and `--rebase` (`-r`) strategy flags exposed; `--squash` deferred |
| diff | supported | |
| grep | supported | |
| blame | supported | `-L` ranges, `--json`/`--machine`, and display flags `-l`/`-t`/`-f`/`-n`/`-s`/`-e`/`-w` supported; `--porcelain`/`-p` supported; `-M`/`-C` partial (flags parsed but cross-file move/copy detection is not implemented — blame still walks this file) |
| revert | supported | |
| remote | supported | |
| hash-object | partial | Blob hashing for files, `--stdin`, and `--stdin-paths`; `-w` writes blob objects. Other object types and advanced Git hash-object flags are unsupported |
| open | supported | |
| config | supported | vault-backed; partial Git config parity with documented intentional differences (see [docs/commands/config.md](docs/commands/config.md) Git Config Compatibility Matrix) |
| db | intentionally-different | Libra repository database schema inspection/upgrade extension, not a Git command |
| reflog | supported | |
| worktree | intentionally-different | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refuses on a dirty worktree |
| cloud | intentionally-different | Libra cloud backup/restore extension, not a Git command |
| publish | intentionally-different | Libra Cloudflare publish extension, not a Git command |
| agent | intentionally-different | Libra external-agent capture extension, not a Git command |
| hooks | intentionally-different | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| cat-file | partial | Single-object `-t`/`-s`/`-p`/`-e` and `--ai*` supported. Batch protocol supported: `--batch`/`--batch-check` (with `=<format>` and `%(objectname)`/`%(objecttype)`/`%(objectsize)` atoms), `--batch-all-objects` (local loose+pack only — not un-fetched cloud-tiered objects), `--unordered`, `-Z` (NUL records), `--buffer`, `--follow-symlinks`. Batch per-line states: only `missing`/`ambiguous` (filtered/submodule deferred). `--follow-symlinks` resolves in the object graph only (never the working tree); escaping the tree root yields `missing` rather than Git's `symlink <size>` line (intentionally-different); link-follow depth capped at 32. 4 KiB cap on a batch input record (over-long → `LBR-CLI-003`/129; Libra hardening). `--textconv`/`--filters` rejected (`LBR-UNSUPPORTED-001`/128, intentionally-different). `--batch-command`/`--filter`/`--use-mailmap`/`--path`/bare `<type> <object>` form deferred. Deprecated lowercase `-z` deferred (only `-Z`). `-e` does not support JSON |
| fsck | supported | |
| verify-pack | partial | validates `.idx` files against matching `.pack` files; multi-index and Git's `-s` / `--stat-only` are supported; `--pack` is a Libra-only explicit pack path |
| index-pack | supported | hidden plumbing command |
| checkout | partial | branch compatibility surface plus path restoration. Branch modes `-b`, `-B` (create/reset), `--detach`, `--orphan`, and `-f`/`--force` supported; `--ours`/`--theirs` conflict-path checkout partial (restores one merge stage and collapses to stage 0; no `-p`/`--patch`, no `--conflict`); explicit `checkout [<tree-ish>] -- <path>` restoration alias supported. Plain `checkout <commit-ish>`/`<tag>` without `--detach`, patch mode (`-p`), and `--conflict` deferred. Internal `intent`/`agent-traces` branches protected. Prefer `switch` / `restore` |
| bisect | partial | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` supported; `start` accepts `<bad> <good>...` positional bounds (multiple good commits); `replay` (see [docs/improvement/compatibility/declined.md#d6-bisect-replay](docs/improvement/compatibility/declined.md#d6-bisect-replay)) / `terms` (see [docs/improvement/compatibility/declined.md#d7-bisect-terms](docs/improvement/compatibility/declined.md#d7-bisect-terms)) deferred; `start -- <pathspec>` rejected (path-limited bisect unsupported) |

## Git commands intentionally absent from `src/cli.rs`

| Command | Tier | Notes |
|---------|------|-------|
| submodule | unsupported | intentional product boundary (see [docs/improvement/compatibility/declined.md#d1-submodule-子命令族](docs/improvement/compatibility/declined.md#d1-submodule-子命令族)) |
| sparse-checkout | unsupported | no public sparse checkout command (see [docs/improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令](docs/improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令)) |

## Hooks

- Stock Git hooks at `.git/hooks` / `core.hooksPath`: `unsupported` (see [docs/improvement/compatibility/declined.md#d3-git-hooks-bridge-作为核心特性](docs/improvement/compatibility/declined.md#d3-git-hooks-bridge-作为核心特性))
- AI provider hooks: `intentionally-different` (see [docs/improvement/agent.md](docs/improvement/agent.md))

## LFS compatibility notes

- `libra lfs`: `partial` command compatibility. Libra uses built-in pointer /
  lock management and `.libra_attributes`.
- Git LFS filter bridge (`.gitattributes` smudge/clean filters + `git-lfs` hook
  install): `intentionally-different` (see
  [docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge)).
- Repository asset storage policy: current committed binaries remain inline.
  Optional future Git LFS rules in `.gitattributes` are tracked as a repository
  governance decision, **not** as the `libra lfs` command status.
