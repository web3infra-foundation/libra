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
[`docs/development/commands/_general.md`](docs/development/commands/_general.md) and in each command
batch document.

## Top-level commands (from `src/cli.rs`)

| Command | Tier | Notes |
|---------|------|-------|
| init | partial | common initialization is supported; safe re-initialization/top-up of an existing Libra repo is not implemented |
| clone | partial | `--depth` and `--single-branch` supported; `--sparse` unsupported (see [docs/development/commands/_compatibility.md#d10-clone---sparse-与顶层-sparse-checkout-命令](docs/development/commands/_compatibility.md#d10-clone---sparse-与顶层-sparse-checkout-命令)); `--recurse-submodules` unsupported (see [docs/development/commands/_compatibility.md#d4-clone---recurse-submodules](docs/development/commands/_compatibility.md#d4-clone---recurse-submodules)) |
| code | intentionally-different | Libra AI extension, not a Git command |
| code-control | intentionally-different | Libra AI automation extension, not a Git command |
| automation | intentionally-different | Libra AI automation rules/history extension, not a Git command |
| usage | intentionally-different | Libra AI provider/model usage reporting extension, not a Git command |
| graph | intentionally-different | Libra AI graph inspection extension, not a Git command |
| sandbox | intentionally-different | Libra AI sandbox diagnostics extension, not a Git command |
| add | partial | sparse-checkout flag unsupported |
| rm | partial | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file-nul` supported; sparse-checkout flag unsupported; per-command `--quiet` not exposed (use global `--quiet`) |
| mv | partial | `-k` / `--skip-errors` supported; `--sparse` accepted as a no-op because Libra does not maintain sparse-checkout state |
| restore | partial | `--source`, `--staged`, `--worktree`, and path restore are supported; overlay/conflict/progress variants are not exposed |
| status | supported | common Git status surface plus `--porcelain` v1/v2, `--short`, `--branch`, `-z` NUL-terminated output, `--find-renames`, `--column`, and `--ahead-behind`/`--no-ahead-behind` supported |
| clean | partial | `-n`, `-f`, `-d`, `-x`, `-X`, and `--exclude` are supported; `-i` is intentionally different/not exposed and pathspec filtering remains deferred |
| stash | partial | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see [docs/development/commands/_compatibility.md#d8-stash-create](docs/development/commands/_compatibility.md#d8-stash-create) and [#d9-stash-store](docs/development/commands/_compatibility.md#d9-stash-store)) |
| lfs | partial | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/development/commands/_compatibility.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/development/commands/_compatibility.md#d5-git-lfs-gitattributes-filter--hooks-bridge)) |
| log | partial | common Git log surface plus `--range` revision expressions, `--all`, `--reverse`, `--follow`, and `-L` supported; positional revision range syntax and exact function-range tracking remain partial |
| shortlog | partial | basic author summary, email, count sorting, time filters, and single revision supported; grouping/format/stdin/no-merges/author filters are not exposed |
| show | partial | object/commit display, `--name-only`, `--stat`, `--oneline`, and path filters supported; extended pretty/raw/name-status formats are not exposed |
| show-ref | partial | branch/tag/HEAD listing, `--hash[=<n>]`, `--abbrev[=<n>]`, `--dereference`, `--verify`, `--exists`, and `--exclude-existing[=<pattern>]` supported; `--branches` / `--no-*` aliases are not exposed |
| for-each-ref | partial | `--heads` / `--tags` / `--remotes` / `--all` / `--format` / `--sort` / `--count` / `--points-at` / `<pattern>` supported; full Git atom language, `--contains` / `--merged` filters and shell quoting modes are not exposed |
| ls-remote | partial | heads/tags/refs filtering and patterns supported; `--symref`, `--get-url`, `--sort`, and `--exit-code` are not exposed |
| ls-tree | partial | Commit/tree listing, recursive listing, current-directory-relative path prefix filters, `--full-name`, `--full-tree`, JSON, and common output flags supported; `--format` and `REV:path` syntax are not exposed |
| symbolic-ref | partial | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| branch | partial | create/list/delete/rename/upstream set/current/contains filters supported; copy, unset-upstream, description, merged/points-at, sort/format, and ignore-case are not exposed |
| tag | partial | lightweight tags, message-based annotated tags, force, delete, list, and `-n` supported; explicit `-a`, filters, sorting, columns, signing, and verification are not exposed |
| commit | partial | common Git commit surface plus `--cleanup`, `--dry-run`, `--fixup`, `--squash`, `-C/-c`, `--trailer`, and `--reset-author` supported; `-e/--edit`, `-v/--verbose`, `--porcelain`, and `--status`/`--no-status` not yet exposed |
| switch | partial | `-C/--force-create`, `--orphan`, `--detach`, and `--track` supported; `-f/--discard-changes`, `--guess` / `--no-guess`, merge/conflict/submodule flags not exposed |
| rebase | partial | `--autosquash` / `--reapply-cherry-picks` not supported |
| merge | partial | fast-forward and single-head three-way merge supported; octopus/custom strategies/squash deferred |
| reset | partial | `--soft`, `--mixed`, `--hard`, and pathspec reset supported; `--merge`, `--keep`, pathspec-from-file, and no-refresh are not exposed |
| rev-parse | partial | basic revision parsing, `--short`, `--abbrev-ref`, and `--show-toplevel` supported; verify/default/repository-query/output-filter/parseopt modes are not exposed |
| rev-list | partial | single revision reachability listing plus `--count`, `-n`/`--max-count`, and `--skip` supported; ranges, exclusions, parent filters, and extended output are not exposed |
| describe | partial | basic describe, `--tags`, `--always`, `--abbrev`, `--exact-match`, and `--dirty[=<mark>]` supported; long/match/exclude/first-parent/contains/candidates/all are not exposed |
| notes | partial | `add` / `show` / `list` / `remove` supported; `--ref` supported; append/edit/copy/merge/prune and editor support not implemented |
| cherry-pick | partial | basic one-or-more commit replay, `-n/--no-commit`, and `-x` supported; edit/mainline/signoff/ff/sequencer/strategy/empty-policy surface remains incomplete |
| push | partial | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected — intentional (see [docs/development/commands/_compatibility.md#d2-本地-file-remote-的-push](docs/development/commands/_compatibility.md#d2-本地-file-remote-的-push)) |
| fetch | partial | repository/refspec, `--all`, and `--depth` supported; prune/dry-run/tags/force/append/refmap and shallow expansion flags are not exposed |
| pull | partial | fetch + fast-forward/three-way merge supported; `--ff-only` and `--rebase` exposed; `--squash` / `--no-ff` strategy flags not exposed |
| diff | partial | staged/old-new/pathspec/name/stat/numstat/output/algorithm supported; positional revspec, summary, word/binary diff, whitespace, and external diff are not exposed |
| grep | partial | tracked/index/tree search with common match/count/list/line flags supported; context, extended/Perl regex, untracked/no-index, and binary controls are not exposed |
| blame | partial | file blame with numeric `-L` ranges and ignore-rev inputs supported; porcelain, reverse, email, whitespace, incremental, and copy/move detection are not exposed |
| revert | partial | single-commit revert and `-n/--no-commit` supported; edit/mainline/sequencer/strategy surface remains incomplete |
| remote | partial | add/remove/rename/list/get-url/set-url/prune supported; detailed `remote show <name>` and `remote update` are not exposed |
| hash-object | partial | Blob hashing for files and `--stdin`; `-w` writes blob objects; `--path` and `--no-filters` are accepted for raw-byte hashing. Other object types and advanced Git hash-object flags are unsupported |
| open | supported | |
| config | partial | vault-backed local/global config is supported; system scope, editor round-trip, typed conversion, NUL output, section rename/remove, and includeIf are incomplete |
| db | intentionally-different | Libra repository database schema inspection/upgrade extension, not a Git command |
| reflog | partial | show/delete/exists and rich show filters supported; `reflog expire` is not exposed |
| worktree | intentionally-different | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refuses on a dirty worktree |
| cloud | intentionally-different | Libra cloud backup/restore extension, not a Git command |
| publish | intentionally-different | Libra Cloudflare publish extension, not a Git command |
| agent | intentionally-different | Libra external-agent capture extension, not a Git command |
| maintenance | partial | `run` / `register` / `unregister` / `status` exposed; lower-level maintenance tasks such as `commit-graph` and `prefetch` are skipped when unsupported |
| hooks | intentionally-different | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| archive | partial | Creates tar/tar.gz/tar.bz2/zip archives from a committed tree; `--format`, `--output`, `--prefix` supported |
| cat-file | partial | `-t`, `-s`, `-p`, `-e`, and AI object modes supported; batch modes and JSON/machine output for `-e` are not exposed |
| fsck | partial | object/ref/index/reflog/connectivity checks supported; JSON/machine output, strict mode, and pack verification surface remain incomplete |
| verify-pack | partial | validates one or more `.idx` files against matching `.pack` siblings; `-s` / `--stat-only` supported; `--pack` is available for a single explicit pack path |
| index-pack | partial | hidden plumbing command for pack file indexing; `--keep[=<msg>]` and Git-style `--progress` / `--no-progress` compatibility flags are accepted; stdin/fix-thin flags are not exposed |
| checkout | partial | visible branch compatibility surface plus `checkout <commit>` detached HEAD, `-b`/`-B` branch creation, and explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore` for new code; patch modes still partial |
| bisect | partial | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` supported; `replay` (see [docs/development/commands/_compatibility.md#d6-bisect-replay](docs/development/commands/_compatibility.md#d6-bisect-replay)) / `terms` (see [docs/development/commands/_compatibility.md#d7-bisect-terms](docs/development/commands/_compatibility.md#d7-bisect-terms)) deferred |

## Git commands intentionally absent from `src/cli.rs`

| Command | Tier | Notes |
|---------|------|-------|
| submodule | unsupported | intentional product boundary (see [docs/development/commands/_compatibility.md#d1-submodule-子命令族](docs/development/commands/_compatibility.md#d1-submodule-子命令族)) |
| sparse-checkout | unsupported | no public sparse checkout command (see [docs/development/commands/_compatibility.md#d10-clone---sparse-与顶层-sparse-checkout-命令](docs/development/commands/_compatibility.md#d10-clone---sparse-与顶层-sparse-checkout-命令)) |

## Hooks

- Stock Git hooks at `.git/hooks` / `core.hooksPath`: `unsupported` (see [docs/development/commands/_compatibility.md#d3-git-hooks-bridge-作为核心特性](docs/development/commands/_compatibility.md#d3-git-hooks-bridge-作为核心特性))
- AI provider hooks: `intentionally-different` (see [docs/development/commands/agent.md](docs/development/commands/agent.md))

## LFS compatibility notes

- `libra lfs`: `partial` command compatibility. Libra uses built-in pointer /
  lock management and `.libra_attributes`.
- Git LFS filter bridge (`.gitattributes` smudge/clean filters + `git-lfs` hook
  install): `intentionally-different` (see
  [docs/development/commands/_compatibility.md#d5-git-lfs-gitattributes-filter--hooks-bridge](docs/development/commands/_compatibility.md#d5-git-lfs-gitattributes-filter--hooks-bridge)).
- Repository asset storage policy: current committed binaries remain inline.
  Optional future Git LFS rules in `.gitattributes` are tracked as a repository
  governance decision, **not** as the `libra lfs` command status.
