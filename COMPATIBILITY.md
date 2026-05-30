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
| add | partial | sparse-checkout flag unsupported |
| rm | partial | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file-nul` supported; sparse-checkout flag unsupported; per-command `--quiet` not exposed (use global `--quiet`) |
| mv | partial | sparse-checkout flag unsupported; `--skip-errors` not exposed |
| restore | supported | |
| status | supported | |
| clean | supported | |
| stash | partial | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see [docs/improvement/compatibility/declined.md#d8-stash-create](docs/improvement/compatibility/declined.md#d8-stash-create) and [#d9-stash-store](docs/improvement/compatibility/declined.md#d9-stash-store)) |
| lfs | partial | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge)) |
| log | supported | |
| shortlog | supported | |
| show | supported | |
| show-ref | supported | |
| ls-remote | supported | |
| symbolic-ref | partial | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| branch | supported | |
| tag | supported | |
| commit | supported | |
| switch | supported | |
| rebase | partial | `--autosquash` / `--reapply-cherry-picks` not supported |
| merge | partial | fast-forward and single-head three-way merge supported; octopus/custom strategies/squash deferred |
| reset | supported | |
| rev-parse | supported | |
| rev-list | supported | |
| describe | supported | |
| cherry-pick | supported | |
| push | partial | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected — intentional (see [docs/improvement/compatibility/declined.md#d2-本地-file-remote-的-push](docs/improvement/compatibility/declined.md#d2-本地-file-remote-的-push)) |
| fetch | supported | `--depth` public flag |
| pull | partial | fetch + fast-forward/three-way merge supported; no `--ff-only` / `--rebase` / `--squash` strategy flags exposed |
| diff | supported | |
| grep | supported | |
| blame | supported | |
| revert | supported | |
| remote | supported | |
| hash-object | partial | Blob hashing for files and `--stdin`; `-w` writes blob objects. Other object types and advanced Git hash-object flags are unsupported |
| open | supported | |
| config | supported | vault-backed |
| db | intentionally-different | Libra repository database schema inspection/upgrade extension, not a Git command |
| reflog | supported | |
| worktree | intentionally-different | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refuses on a dirty worktree |
| cloud | intentionally-different | Libra cloud backup/restore extension, not a Git command |
| publish | intentionally-different | Libra Cloudflare publish extension, not a Git command |
| agent | intentionally-different | Libra external-agent capture extension, not a Git command |
| hooks | intentionally-different | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| cat-file | supported | `-e` does not support JSON |
| fsck | supported | |
| prune | supported | |
| verify-pack | partial | validates one `.idx` file against a matching `.pack`; Git's multi-index form and `-s` / `--stat-only` are not exposed |
| index-pack | supported | hidden plumbing command |
| checkout | partial | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore`; detached HEAD and patch modes still partial |
| bisect | partial | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` supported; `replay` (see [docs/improvement/compatibility/declined.md#d6-bisect-replay](docs/improvement/compatibility/declined.md#d6-bisect-replay)) / `terms` (see [docs/improvement/compatibility/declined.md#d7-bisect-terms](docs/improvement/compatibility/declined.md#d7-bisect-terms)) deferred |

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
