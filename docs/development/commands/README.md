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
| [`archive`](archive.md) | `partial` | Creates tar/tar.gz/tar.bz2/zip archives from a committed tree; `--format`, `--output`, `--prefix`, `--list`, and `TREEISH <path>...` supported |
| [`agent`](agent.md) | `intentionally-different` | Libra external-agent capture extension, not a Git command |
| [`automation`](automation.md) | `intentionally-different` | Libra AI automation rules/history extension, not a Git command |
| [`bisect`](bisect.md) | `partial` | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` supported; `replay` (see [docs/development/comma... |
| [`blame`](blame.md) | `partial` | numeric `-L` ranges plus porcelain/line-porcelain supported; reverse/email/whitespace/incremental/copy-move detection remain incomplete |
| [`branch`](branch.md) | `partial` | create/list/delete/rename/upstream set+unset/current/contains/points-at/ignore-case supported; copy/description/merged/sort/format not exposed |
| [`cat-file`](cat-file.md) | `partial` | `-t` / `-s` / `-p` / `-e` plus default `--batch-check` / `--batch` supported; batch-command/all-objects/custom formats and `-e` JSON/machine output not exposed |
| [`checkout`](checkout.md) | `partial` | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore... |
| [`cherry-pick`](cherry-pick.md) | `partial` | commit replay, `-n`, and `-x` supported; edit/mainline/signoff/sequencer/strategy flags incomplete |
| [`clean`](clean.md) | `partial` | `-n` / `-f` / `-d` / `-x` / `-X` / `--exclude` / `<pathspec>...` supported; `-i` not exposed |
| [`clone`](clone.md) | `partial` | `--depth`, `--single-branch`, and `--tags`/`--no-tags` (clone fetches all tags by default) supported; `--sparse` unsupported (see [docs/development/commands/_compatibility.md#d... |
| [`cloud`](cloud.md) | `intentionally-different` | Libra cloud backup/restore extension, not a Git command |
| [`code`](code.md) | `intentionally-different` | Libra AI extension, not a Git command |
| [`code-control`](code-control.md) | `intentionally-different` | Libra AI automation extension, not a Git command |
| [`commit`](commit.md) | `partial` | common commit flags plus cleanup/fixup/squash/trailer and `-e/--edit` + `-v/--verbose` (editor) supported; `--porcelain`/`--status`/`-t/--template` not exposed |
| [`config`](config.md) | `partial` | vault-backed local/global config; system scope, editor round-trip, typed conversion, NUL output and section operations incomplete |
| [`describe`](describe.md) | `partial` | basic describe, `--tags`, `--always`, `--abbrev`, `--exact-match`, `--long`, `--dirty[=<mark>]`, `--first-parent`, `--match`, and `--exclude` supported; contains/candidates/all not exposed |
| [`diff`](diff.md) | `partial` | staged/old-new/pathspec/name/stat output supported; positional revspec, summary/word/binary/whitespace/ext-diff incomplete |
| [`fetch`](fetch.md) | `partial` | repository/refspec, `--all`, `--depth`, `--dry-run`, `-v`, `--porcelain`, tag auto-follow (default; `--tags`/`--no-tags`, `remote.<name>.tagOpt`), `-f`/`--force`, `FETCH_HEAD`, and `--append` supported; refmap/atomic/prune and shallow expansion flags not exposed |
| [`for-each-ref`](for-each-ref.md) | `partial` | `--heads` / `--tags` / `--remotes` / `--all` / `--format` / `--sort` / `--count` / `--points-at` / `--contains` / `--no-contains` / `<pattern>` supported; full Git atom language, `--merged` filters and shell quoting modes are not exposed |
| [`format-patch`](format-patch.md) | `partial` | `-o`/`--output-directory`, `--stdout`, `-n`/`--numbered`, `--start-number`, `--subject-prefix`, `--cover-letter`, `--thread`/`--no-thread`, `--in-reply-to`, `-v`/`--reroll-count`, `-s`/`--signoff`, `--full-index`, `--no-stat`, `--keep-subject`, and `A..B`/single-commit revision range supported; merge commits are skipped; `--attach`, `--inline`, `--suffix`, `--signature`, `--from`, `--to`, `--cc`, `--base`, `--interdiff`, `--range-diff`, `--notes`, `--encode-email-headers`, and `--force` are not exposed |
| [`fsck`](fsck.md) | `partial` | object/ref/index/reflog/connectivity checks supported; JSON/machine output, strict mode and pack verification surface incomplete |
| [`graph`](graph.md) | `intentionally-different` | Libra AI graph inspection extension, not a Git command |
| [`grep`](grep.md) | `partial` | tracked/index/tree search with common match flags, context lines, `-E`/`-G`, `-P` rejection, and `-a`/`-I` binary controls supported; untracked/no-index/heading/null output not exposed |
| [`hash-object`](hash-object.md) | `partial` | Blob hashing for files and `--stdin`; `-w` writes blob objects; `--path` / `--no-filters` accepted for raw-byte... |
| [`hooks`](hooks.md) | `intentionally-different` | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| [`index-pack`](index-pack.md) | `partial` | hidden plumbing command; `--stdin`, `--keep[=<MSG>]`, and progress flags supported; `--fix-thin` not exposed |
| [`init`](init.md) | `partial` | fresh repository initialization supported; safe re-initialization/top-up of existing repos not implemented |
| [`lfs`](lfs.md) | `partial` | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/development/commands/_comp... |
| [`log`](log.md) | `partial` | common log surface plus `--range`/`--all`/`--reverse`/`--follow`/`-L`; positional ranges and exact line history remain partial |
| [`ls-files`](ls-files.md) | `partial` | default cached listing plus modified/deleted/stage/untracked filters, `.libraignore`-aware `--others --exclude-standard`, pathspecs, `--error-unmatch`, `-z`, and JSON/machine output supported |
| [`ls-remote`](ls-remote.md) | `partial` | heads/tags/refs filtering, patterns, get-url, sort, and exit-code supported; symref not exposed |
| [`ls-tree`](ls-tree.md) | `partial` | Commit/tree listing, recursive listing, current-directory-relative path prefix filters, `--full-name`, `--full-tree`, JSON, common output flags, and partial `--format` atom support exposed; `REV:path` syntax is not exposed |
| [`maintenance`](maintenance.md) | `partial` | `run` / `register` / `unregister` / `status` / `start` / `stop` exposed; commit-graph and prefetch tasks implemented with documented Git semantic differences |
| [`merge`](merge.md) | `partial` | fast-forward and single-head three-way merge supported; octopus/custom strategies/squash deferred |
| [`mv`](mv.md) | `partial` | `-k` / `--skip-errors` supported; `--sparse` accepted as a no-op because Libra does not maintain sparse-checkout state |
| [`notes`](notes.md) | `partial` | `add` / `show` / `list` / `remove` supported; `--ref` supported; append/edit/copy/merge/prune and editor support not implemented |
| [`op`](op.md) | `intentionally-different` | Libra command-level operation history inspection/restore extension, not a Git command |
| [`open`](open.md) | `supported` | 见命令文档。 |
| [`publish`](publish.md) | `intentionally-different` | Libra Cloudflare publish extension, not a Git command |
| [`pull`](pull.md) | `partial` | fetch + fast-forward/three-way merge supported; `--ff-only` / `--rebase` / `--ff` / `--no-ff`, fetch `--depth`, `--squash`, and `--no-commit` exposed; `--commit` / `--autostash` not exposed |
| [`push`](push.md) | `partial` | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected — intentiona... |
| [`rebase`](rebase.md) | `partial` | `--onto <newbase> [<upstream>] [<branch>]` supported; `--autosquash` / `--reapply-cherry-picks` not supported |
| [`reflog`](reflog.md) | `supported` | show/delete/exists/expire supported; expire has documented intentional differences around no-ref handling, stale-fix depth, and updateref skips |
| [`remote`](remote.md) | `partial` | add/remove/rename/list/get-url/set-url/prune/set-branches/set-head (incl. `--auto`) supported; `remote show` queries the remote by default (`--no-query` for offline cached data); `remote update` not exposed |
| [`reset`](reset.md) | `partial` | soft/mixed/hard/path reset plus pathspec-from-file/pathspec-file-nul and no-refresh no-op supported; merge/keep not exposed |
| [`restore`](restore.md) | `partial` | source/staged/worktree path restore supported; overlay/conflict/progress variants not exposed |
| [`rev-list`](rev-list.md) | `partial` | multi-revision reachability, exclusions/ranges, count/limit controls, author/committer/message/path/time filters, parent filters/reset aliases, first-parent traversal, symmetric side/cherry filters including `--cherry`, parents/children and timestamp output supported; object/boundary traversal output remains incomplete |
| [`rev-parse`](rev-parse.md) | `partial` | basic revision parsing, `--verify`, `--short[=<n>]`, `--abbrev-ref`, `--show-toplevel`, `--show-prefix`, `--show-cdup`, work-tree/bare/git-dir queries supported; default/output-filter/parseopt modes remain incomplete |
| [`revert`](revert.md) | `partial` | single/multi-commit revert, `-n`, mainline, signoff, and conflict `--continue`/`--abort` supported; skip/multi-commit todo/edit/strategy flags incomplete |
| [`rm`](rm.md) | `partial` | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file... |
| [`sandbox`](sandbox.md) | `intentionally-different` | Libra AI sandbox diagnostics extension, not a Git command |
| [`shortlog`](shortlog.md) | `partial` | author summary, email, count sorting, time filters, single revision, committer grouping, no-merges, top/min-count/reverse, and author filter supported; trailer group/format/stdin/wrap width not exposed |
| [`show`](show.md) | `partial` | object/commit display and common name/stat flags supported; extended pretty/raw/name-status formats not exposed |
| [`show-ref`](show-ref.md) | `supported` | branch/tag/HEAD listing, scope filters, hash/abbrev/dereference/verify/exists/head reset aliases, and `--exclude-existing[=<pattern>]` stdin filter supported |
| [`stash`](stash.md) | `partial` | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see ... |
| [`status`](status.md) | `supported` | 见命令文档。 |
| [`switch`](switch.md) | `partial` | `-C/--force-create`、`--orphan`、`--detach`、`--track`、`--guess`/`--no-guess`（DWIM 远端跟踪猜测，默认开启，受 `checkout.guess` / `checkout.defaultRemote` 控制）已公开；`-f/--discard-changes`、merge/conflict/submodule 相关参数未公开。 |
| [`symbolic-ref`](symbolic-ref.md) | `partial` | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| [`tag`](tag.md) | `partial` | lightweight/message/annotated tags, force/delete/list/`-n`, points-at, contains/no-contains, merged/no-merged, sort, and vault-PGP sign/verify supported; column and Git GPG interop not exposed |
| [`usage`](usage.md) | `intentionally-different` | Libra AI provider/model usage reporting extension, not a Git command |
| [`verify-pack`](verify-pack.md) | `partial` | validates one or more `.idx` files against matching `.pack` siblings; `-s` / `--stat-only` supported; `--pack` is available for a single explicit pack path |
| [`worktree`](worktree.md) | `intentionally-different` | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refus... |

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
