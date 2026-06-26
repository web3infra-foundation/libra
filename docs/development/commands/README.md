# Command Development Documentation

本目录是 Libra 命令开发设计、兼容性说明、实现历史和剩余缺口的唯一集中位置。每个命令文档都按同一结构维护：命令实现目标、对比 Git 与兼容性、设计方案、实现历史、当前状态、还未实现的功能。

## 事实来源

- 当前代码：`src/cli.rs`、`src/command/`。
- 用户行为说明：`docs/commands/`。
- Git 兼容承诺：`COMPATIBILITY.md`、本目录命令文档和 `_compatibility.md`。
- 历史背景：旧 `.omo` 计划、`docs/improvement` 和兼容性报告已迁入并改写，不再作为独立标准文件。

## 文档维护规则

- 改进任何命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 先以代码和测试确认事实，再更新本目录。
- 不再把旧文档全文粘贴进命令页；只保留结论、状态和可执行缺口。
- 命令行为变化必须同步用户文档、`COMPATIBILITY.md` 和相关测试。
- 未公开命令必须明确标记为未接入 CLI，避免用户误以为可用。

## 公开命令

| 命令 | 兼容级别 | 当前说明 |
|---|---|---|
| [`add`](add.md) | `partial` | sparse-checkout flag unsupported |
| [`archive`](archive.md) | `partial` | Creates tar/tar.gz/tar.bz2/zip archives from a committed tree; `--format`, `--output`, `--prefix`, `--list`, `-v`/`--verbose`, `--add-file=<file>` (inject an untracked file; repeatable), `--compression-level <0-9>` (Git's `-0`..`-9`), and `TREEISH <path>...` supported |
| [`agent`](agent.md) | `intentionally-different` | Libra external-agent capture extension, not a Git command |
| [`automation`](automation.md) | `intentionally-different` | Libra AI automation rules/history extension, not a Git command |
| [`bisect`](bisect.md) | `partial` | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` and `start --first-parent` supported; `replay` (see [docs/development/comma... |
| [`blame`](blame.md) | `partial` | `-L` ranges (numeric and `/regex/` start/end endpoints; single endpoint spans to EOF, like Git), porcelain/line-porcelain (`-p`), `-e`/`--show-email`, display flags `-l`/`-s`/`-t`/`-f`(`--show-name`)/`--abbrev`, and `--root` (no-op) supported; `-L :<funcname>`/reverse/whitespace/incremental/copy-move detection remain incomplete |
| [`branch`](branch.md) | `partial` | create/list/delete/rename/copy(`-c`/`-C`)/upstream set+unset/current/contains/points-at/merged/no-merged/sort(refname,version:refname,committerdate,creatordate)/ignore-case/`--column`/`--no-column`(countermands `--column`, last wins)/`-v`(`--verbose`, `-vv` adds upstream tracking)/`--edit-description`(edit `branch.<name>.description` in an editor; empty unsets) supported; custom-format not exposed |
| [`cat-file`](cat-file.md) | `partial` | `-t` / `-s` / `-p` / `-e` plus `--batch-check` / `--batch` / `--batch-command` / `--batch-all-objects` (with optional `=<format>`) supported; `-e --json`/`--machine` emits `{ exists: bool }` (exit code preserved) |
| [`checkout`](checkout.md) | `partial` | visible branch compatibility surface plus `-d`/`--detach`, `-t`/`--track` (accepted no-op; DWIM always tracks), `--no-overlay` (no-op — never in overlay mode), and explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore` for new code |
| [`cherry-pick`](cherry-pick.md) | `partial` | commit replay, `-n`, and `-x` supported; edit/mainline/signoff/sequencer/strategy flags incomplete |
| [`clean`](clean.md) | `partial` | `-n` / `-f` / `-d` / `-x` / `-X` / `-e`/`--exclude` / `<pathspec>...` supported; `-i` not exposed |
| [`clone`](clone.md) | `partial` | `--depth`, `--single-branch`/`--no-single-branch` (toggle; `--no-single-branch` countermands, last wins), `--tags`/`--no-tags` (clone fetches all tags by default), and `--no-progress` (suppresses the fetch progress meter) supported; `--sparse` unsupported (see [docs/development/commands/_compatibility.md#d... |
| [`cloud`](cloud.md) | `intentionally-different` | Libra cloud backup/restore extension, not a Git command |
| [`code`](code.md) | `intentionally-different` | Libra AI extension, not a Git command |
| [`code-control`](code-control.md) | `intentionally-different` | Libra AI automation extension, not a Git command |
| [`commit`](commit.md) | `partial` | common commit flags plus cleanup/fixup/squash/trailer, `-e/--edit` + `-v/--verbose` (editor), `--porcelain` (status v1), `--status`/`--no-status` (seed commented status into the editor template), and the `commit.cleanup`/`commit.verbose` config defaults (CLI flag wins) supported; `-t/--template` not exposed |
| [`config`](config.md) | `partial` | vault-backed local/global config plus section operations (`--remove-section`/`--rename-section`), `-z`/`--null` output, and read-time `--type`/`--bool`/`--int`/`--path` canonicalization; system scope, editor round-trip and set-time type validation incomplete |
| [`describe`](describe.md) | `partial` | basic describe, `--tags`, `--always`, `--abbrev`, `--exact-match`, `--long`, `--dirty[=<mark>]`, `--first-parent`, `--match`, `--exclude`, `--candidates` (0 ⇒ exact-match), `--all` (any ref, prefixed), and `--contains` (git name-rev: nearest descendant tag, `<tag>~<n>^<m>` form) supported |
| [`diff`](diff.md) | `partial` | staged/old-new/pathspec/name/stat/shortstat/summary output + `--exit-code`/`-s`/`--no-patch`/`-z`/`-U<n>`(`--unified`, context lines)/`-w`(`--ignore-all-space`, re-diff)/`-b`(`--ignore-space-change`)/`--ignore-space-at-eol`/`--ignore-blank-lines`(ignore blank-only changes; faithful `xdl_get_hunk` port)/`--check`/`-R`/`-a`(`--text`, no-op)/`--no-ext-diff`(no-op)/`--no-color-moved`(no-op)/`--relative[=<path>]`(restrict to a directory + strip prefix)/`--no-relative`(no-op alone; overrides `--relative` when both given)/`--no-renames`/`--no-indent-heuristic`/`--no-textconv`(no-ops) supported; positional revspec, word diff, `--color-moved`, `--renames`, `--indent-heuristic`, `--textconv`, `--binary` patch/ext-diff drivers incomplete |
| [`fetch`](fetch.md) | `partial` | repository/refspec, `--all`, `--depth`, `--dry-run`, `-v`, `--porcelain`, tag auto-follow (default; `--tags`/`--no-tags`, `remote.<name>.tagOpt`), `-f`/`--force`, `FETCH_HEAD`, `--append`, `--no-auto-gc`(no-op), `--no-progress`(suppresses the progress meter), and `--no-prune`(no-op — fetch never prunes) supported; refmap/atomic/prune(`--prune`/`-p`) and shallow expansion flags not exposed |
| [`for-each-ref`](for-each-ref.md) | `partial` | `--heads` / `--tags` / `--remotes` / `--all` / `--format` / `--sort` (`refname`/`objectname`/`version:refname`/`committerdate`/`authordate`/`creatordate`, each reversible) / `--count` / `--points-at` / `--contains` / `--no-contains` / `--merged` / `--no-merged` / `--exclude` / `<pattern>` and `--shell`/`--perl`/`--python`/`--tcl` output quoting modes supported; full Git atom language and the `objectsize`/`*objectname` sort keys are not exposed |
| [`format-patch`](format-patch.md) | `partial` | `-o`/`--output-directory`, `--stdout`, `-n`/`--numbered`, `--start-number`, `--subject-prefix`, `--cover-letter`, `--thread`/`--no-thread`, `--in-reply-to`, `-v`/`--reroll-count`, `-s`/`--signoff`, `--full-index`, `--no-stat`, `--keep-subject`, `--suffix`, `--zero-commit`, `--signature`/`--no-signature`, `--signature-file`, `--encode-email-headers`/`--no-encode-email-headers`, `--numbered-files`, and `A..B`/single-commit revision range `--to`/`--cc` (repeatable recipient headers, folded like git; placed after the MIME headers and on the cover letter), and `--no-to`/`--no-cc` (suppress them — Libra has no `format.to`/`format.cc` config to reset) supported; merge commits are skipped; `--attach`, `--inline`, `--from`, `--base`, `--interdiff`, `--range-diff`, and `--notes` are not exposed (`--force` is not a Git format-patch flag) |
| [`fsck`](fsck.md) | `partial` | object/ref/index/reflog/connectivity checks supported; JSON/machine output, strict mode and pack verification surface incomplete |
| [`graph`](graph.md) | `intentionally-different` | Libra AI graph inspection extension, not a Git command |
| [`grep`](grep.md) | `partial` | tracked/index/tree search with common match flags, context lines, `-E`/`-G`, `-P` rejection, `-a`/`-I` binary controls, `--heading`/`--break`/`-z` output grouping, `-m`/`--max-count`, `-o`/`--only-matching`, `--untracked` (search untracked non-ignored files too), `--no-index` (no-repo recursive filesystem grep) supported; function display and max-depth not exposed |
| [`hash-object`](hash-object.md) | `partial` | Hashing for files, `--stdin`, and `--stdin-paths`; `-t blob/commit/tree/tag` typed hashing (Git-identical oid) with `--literally`; `-w` writes the object; `--path` / `--no-filters` accepted for raw-byte hashing; path filters/attributes unsupported |
| [`hooks`](hooks.md) | `intentionally-different` | Hidden compatibility entry for AI provider hook configs installed by `libra agent enable`; not a Git hooks bridge (`.git/hooks` / `core.hooksPath` rejected by D3) |
| [`index-pack`](index-pack.md) | `partial` | hidden plumbing command; `--stdin`, `--keep[=<MSG>]`, and progress flags supported; `--fix-thin` not exposed |
| [`init`](init.md) | `partial` | fresh repository initialization plus Git-style safe re-initialization/top-up of existing repos (`Reinitialized existing ...`, layout top-up, `--shared` re-apply, DB/config/refs preserved) supported; recursive submodule init not implemented |
| [`lfs`](lfs.md) | `partial` | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/development/commands/_comp... |
| [`log`](log.md) | `partial` | common log surface plus `--range`/`--all`/`--reverse`/`--author-date-order`/`--date-order`/`--no-expand-tabs`(no-op)/`--no-notes`(no-op)/`--no-mailmap`(no-op)/`--no-show-signature`(no-op)/`--follow`/`-L`/`--parents`/`--children`/`-i`/`--invert-grep`/`--patch-with-stat`(`-p --stat`); `--expand-tabs`, `--show-signature`, positional ranges, and exact line history remain partial |
| [`ls-files`](ls-files.md) | `partial` | default cached listing plus modified/deleted/stage/untracked filters (`-c`/`-o` shorts), `--abbrev[=<n>]`, `.libraignore`-aware `--others --exclude-standard`, `-i`/`--ignored` (ignored set; `-i -o` ignored untracked, `-i -c` tracked-matching-exclude; needs `-o`/`-c` + `--exclude-standard`), pathspecs, `--error-unmatch`, `-z`, status tags `-t` (H/R/C/?/M), unmerged-only `-u`/`--unmerged`, `--full-name` (accepted no-op; Libra always prints repo-root-relative paths), and JSON/machine output supported |
| [`ls-remote`](ls-remote.md) | `partial` | heads/tags/refs filtering, patterns, get-url, sort, and exit-code supported; symref not exposed |
| [`ls-tree`](ls-tree.md) | `partial` | Commit/tree listing, recursive listing, current-directory-relative path prefix filters, `--full-name`, `--full-tree`, `REV:path` tree-ish syntax, JSON, common output flags, and partial `--format` atom support exposed; full Git pathspec magic remains incomplete |
| [`maintenance`](maintenance.md) | `partial` | `run` / `register` / `unregister` / `status` / `start` / `stop` exposed; commit-graph and prefetch tasks implemented with documented Git semantic differences |
| [`merge`](merge.md) | `partial` | fast-forward and single-head three-way merge supported; `-m`/`--ff-only`/`--no-ff`/`--squash`/`--no-commit`/`--no-edit`/`--stat`(prints post-merge diffstat)/`-n`(`--no-stat`)/`--verify-signatures`(verify merged tip's PGP sig vs the local vault key)/`--no-verify-signatures`(default; toggle)/`--no-rerere-autoupdate`(no-op) supported; octopus/custom strategies/`--rerere-autoupdate` deferred |
| [`mv`](mv.md) | `partial` | `-k` / `--skip-errors` supported; `--sparse` accepted as a no-op because Libra does not maintain sparse-checkout state |
| [`notes`](notes.md) | `partial` | `add` / `append` / `copy` / `edit` / `show` / `list` / `remove` / `merge` (2-way flat-row merge, `--strategy=manual\|ours\|theirs\|union\|cat_sort_uniq`) supported; `--ref` supported; `prune`, `get-ref`, and the interactive editor not implemented |
| [`op`](op.md) | `intentionally-different` | Libra command-level operation history inspection/restore extension, not a Git command |
| [`open`](open.md) | `supported` | 见命令文档。 |
| [`publish`](publish.md) | `intentionally-different` | Libra Cloudflare publish extension, not a Git command |
| [`pull`](pull.md) | `partial` | fetch + fast-forward/three-way merge supported; `--ff-only` / `--rebase` / `--no-rebase` (countermands `--rebase`, last wins) / `--ff` / `--no-ff`, fetch `--depth`, `--squash`, `--no-commit`, `--commit`, `--autostash`, and `--no-progress` (forwarded to the fetch) exposed |
| [`push`](push.md) | `partial` | branch/tag update, multi-refspec, delete (`-d`/`--delete`), `--tags`, and `--mirror` supported; local file remote rejected — intentiona... |
| [`rebase`](rebase.md) | `partial` | `--onto <newbase> [<upstream>] [<branch>]`, `--autosquash`, `--reapply-cherry-picks`, `--no-autostash` (no-op — never autostashes), `--no-rerere-autoupdate` (no-op — no rerere), `--keep-empty` (no-op — already keeps empty commits), and `--no-keep-empty` (drop start-empty commits) supported; interactive / `--rebase-merges` / `--autostash` / `--rerere-autoupdate` / `--empty=drop` not supported |
| [`reflog`](reflog.md) | `supported` | show/delete/exists/expire supported; expire has documented intentional differences around no-ref handling, stale-fix depth, and updateref skips |
| [`remote`](remote.md) | `partial` | add (incl. `-f`/`--fetch` and the cold-config flags `-t`/`--track`, `-m`/`--master`, `--tags`/`--no-tags`)/remove/rename/list/get-url/set-url/prune/set-branches/set-head (incl. `--auto`)/update supported; `remote show` queries the remote by default (`--no-query` for offline cached data); `remote update [-p/--prune] [<group>|<remote>...]` fetches all/named remotes (groups expanded), and `-p`/`--prune` prunes stale remote-tracking refs once every fetch succeeds |
| [`reset`](reset.md) | `partial` | soft/mixed/hard/path reset plus pathspec-from-file/pathspec-file-nul and no-refresh no-op supported; merge/keep not exposed |
| [`restore`](restore.md) | `partial` | source/staged/worktree path restore + conflict-stage `--ours`/`-2` & `--theirs`/`-3` (worktree-only, index left unmerged) + `--ignore-unmerged` (unmerged guard: plain restore of an unmerged path → `LBR-CONFLICT-001`/128) + `--overlay`/`--no-overlay` (real toggle — overlay never removes paths absent from the source) + `--no-progress`(no-op) + `--merge`/`--conflict=merge|diff3` (rebuild conflict markers from index stages — Libra's whole-file marker format, not Git's line-level) supported; only the `--progress` meter not exposed |
| [`rev-list`](rev-list.md) | `partial` | multi-revision reachability, exclusions/ranges, count/limit controls, author/committer/message/path/time filters, parent filters/reset aliases, first-parent traversal, symmetric side/cherry filters including `--cherry`, parents/children, timestamp, `--reverse` ordering, `--all` (every ref + HEAD), `--date-order` (no-op for default committer-date order; no Git topo constraint), and `--boundary` (frontier commits — parents of listed commits not themselves listed, including the `--max-count` cut point — `-`-prefixed with metadata) output supported; object-enumeration output (`--objects`) remains incomplete |
| [`rev-parse`](rev-parse.md) | `partial` | basic revision parsing, `--verify`, `--short[=<n>]`, `--abbrev-ref`, `--symbolic-full-name` (spec → full ref name), `--show-toplevel`, `--show-prefix`, `--show-cdup`, work-tree/inside-git-dir/bare/git-dir/absolute-git-dir queries, and `--sq` supported; remaining output-filter (`--symbolic`/`--flags`/`--abbrev=<n>`)/parseopt modes incomplete |
| [`revert`](revert.md) | `partial` | single/multi-commit revert, `-n`, mainline, signoff, `--no-edit` (accepted no-op), `--no-rerere-autoupdate` (no-op), and conflict `--continue`/`--abort` supported; skip/multi-commit todo/`--edit`/`--rerere-autoupdate`/strategy flags incomplete |
| [`rm`](rm.md) | `partial` | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file... |
| [`sandbox`](sandbox.md) | `intentionally-different` | Libra AI sandbox diagnostics extension, not a Git command |
| [`shortlog`](shortlog.md) | `partial` | author summary, email, count sorting, time filters, single revision, committer grouping, `--group=author\|committer\|trailer:<key>`, merges/no-merges, top/min-count/reverse, author filter, and `-w` subject wrapping, `--format` (custom per-commit template) supported; stdin not exposed |
| [`show`](show.md) | `partial` | object/commit display, common name/stat flags, `--patch-with-stat` (diffstat + patch, Git's `-p --stat`), `--summary` (create/delete file mode summary, like `diff --summary`), `--pretty` / `--format`, `--abbrev-commit`/`--no-abbrev-commit` (toggle; `--no-abbrev-commit` countermands, last wins), and `--no-expand-tabs`/`--no-notes`/`--no-mailmap`/`--no-show-signature` (no-ops) supported; named pretty presets (short/full/fuller/raw), the raw format, and `--expand-tabs`/`--notes`/`--mailmap`/`--show-signature` not exposed |
| [`show-ref`](show-ref.md) | `supported` | branch/tag/HEAD listing, scope filters, hash/abbrev/dereference/verify/exists/head reset aliases, and `--exclude-existing[=<pattern>]` stdin filter supported |
| [`stash`](stash.md) | `partial` | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see ... |
| [`status`](status.md) | `supported` | 见命令文档。 |
| [`switch`](switch.md) | `partial` | `-C/--force-create`、`--orphan`、`--detach`、`--track`、`-f`/`--force`（别名 `--discard-changes`）、`--guess`/`--no-guess`（DWIM 远端跟踪猜测，默认开启，受 `checkout.guess` / `checkout.defaultRemote` 控制）、`--no-progress`（接受式 no-op：Libra 的 switch 从不渲染进度条）已公开；merge/conflict/submodule 相关参数未公开。 |
| [`symbolic-ref`](symbolic-ref.md) | `partial` | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| [`tag`](tag.md) | `partial` | lightweight/message/annotated tags, `-F`/`--file` (message from file or stdin), force/delete/list/`-n`, points-at, contains/no-contains, merged/no-merged, sort, `--column` (always/auto/never; `--no-column` countermands it, last wins), and vault-PGP `-s`/`--sign` (`--no-sign` countermands it; last wins)/`-v`/`--verify` supported; editor (`-e`) and Git GPG interop not exposed |
| [`usage`](usage.md) | `intentionally-different` | Libra AI provider/model usage reporting extension, not a Git command |
| [`verify-pack`](verify-pack.md) | `partial` | validates one or more `.idx` files against matching `.pack` siblings; `-s` / `--stat-only` supported; `--pack` is available for a single explicit pack path |
| [`worktree`](worktree.md) | `intentionally-different` | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refuses on a dirty worktree. `list --porcelain` emits Git-style machine-readable output |

## 未公开或未纳入用户承诺的命令资料

以下命令曾有开发设计资料，但已明确决定不接入公开 CLI；它们降级为内部历史资料，不承诺用户可见兼容面：

- `gc`：功能由 `libra maintenance run --task gc` 覆盖（见 `docs/development/internal/gc.md`）
- `package`：内部设计资料保留（见 `docs/development/internal/package.md`）
- `prune`：内部设计资料保留（见 `docs/development/internal/prune.md`）
- `stats`：内部设计资料保留（见 `docs/development/internal/stats.md`）

若未来需要发布其中任一命令，必须重新走完整的 CLI 接入、`COMPATIBILITY.md` 登记、用户文档和回归测试流程。

## 汇总文档

- [`_compatibility.md`](_compatibility.md)：Git 兼容治理、D1-D10 拒绝/延后决策、参数级缺口状态。
- [`_general.md`](_general.md)：跨命令实现规范、CLIG 现代化、测试和文档维护要求。
