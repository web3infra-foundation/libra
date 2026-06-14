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
| [`archive`](archive.md) | `partial` | Creates tar/tar.gz/tar.bz2/zip archives from a committed tree; `--format`, `--output`, `--prefix` supported |
| [`agent`](agent.md) | `intentionally-different` | Libra external-agent capture extension, not a Git command |
| [`automation`](automation.md) | `intentionally-different` | Libra AI automation rules/history extension, not a Git command |
| [`bisect`](bisect.md) | `partial` | `start` / `bad` / `good` / `reset` / `skip` / `log` / `run` / `view` supported; `replay` (see [docs/development/comma... |
| [`blame`](blame.md) | `supported` | 见命令文档。 |
| [`branch`](branch.md) | `supported` | 见命令文档。 |
| [`cat-file`](cat-file.md) | `supported` | `-e` does not support JSON |
| [`checkout`](checkout.md) | `partial` | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore... |
| [`cherry-pick`](cherry-pick.md) | `supported` | 见命令文档。 |
| [`clean`](clean.md) | `supported` | 见命令文档。 |
| [`clone`](clone.md) | `partial` | `--depth` and `--single-branch` supported; `--sparse` unsupported (see [docs/development/commands/_compatibility.md#d... |
| [`cloud`](cloud.md) | `intentionally-different` | Libra cloud backup/restore extension, not a Git command |
| [`code`](code.md) | `intentionally-different` | Libra AI extension, not a Git command |
| [`code-control`](code-control.md) | `intentionally-different` | Libra AI automation extension, not a Git command |
| [`commit`](commit.md) | `supported` | 见命令文档。 |
| [`config`](config.md) | `supported` | vault-backed |
| [`db`](db.md) | `intentionally-different` | Libra repository database schema inspection/upgrade extension, not a Git command |
| [`describe`](describe.md) | `supported` | 见命令文档。 |
| [`diff`](diff.md) | `supported` | 见命令文档。 |
| [`fetch`](fetch.md) | `supported` | `--depth` public flag |
| [`for-each-ref`](for-each-ref.md) | `partial` | `--heads` / `--tags` / `--remotes` / `--all` / `--format` / `--sort` / `--count` / `<pattern>` supported; full Git atom language, `--contains` / `--merged` / `--points-at` and shell quoting modes are not exposed |
| [`fsck`](fsck.md) | `supported` | 见命令文档。 |
| [`graph`](graph.md) | `intentionally-different` | Libra AI graph inspection extension, not a Git command |
| [`grep`](grep.md) | `supported` | 见命令文档。 |
| [`hash-object`](hash-object.md) | `partial` | Blob hashing for files and `--stdin`; `-w` writes blob objects. Other object types and advanced Git hash-object flags... |
| [`hooks`](hooks.md) | `intentionally-different` | Hidden compatibility entry for hook configs installed by `libra agent enable` |
| [`index-pack`](index-pack.md) | `supported` | hidden plumbing command |
| [`init`](init.md) | `supported` | 见命令文档。 |
| [`lfs`](lfs.md) | `partial` | built-in Libra LFS command; uses `.libra_attributes`, not Git LFS filters/hooks (see [docs/development/commands/_comp... |
| [`log`](log.md) | `supported` | 见命令文档。 |
| [`ls-remote`](ls-remote.md) | `supported` | 见命令文档。 |
| [`ls-tree`](ls-tree.md) | `partial` | Commit/tree listing, recursive listing, path prefix filters, JSON, and common output flags supported; `--full-name` / `--full-tree` / `--format` and `REV:path` syntax are not exposed |
| [`maintenance`](maintenance.md) | `partial` | `run` / `register` / `unregister` / `status` exposed; lower-level maintenance tasks such as `commit-graph` and `prefe... |
| [`merge`](merge.md) | `partial` | fast-forward and single-head three-way merge supported; octopus/custom strategies/squash deferred |
| [`mv`](mv.md) | `partial` | sparse-checkout flag unsupported; `--skip-errors` not exposed |
| [`notes`](notes.md) | `partial` | `add` / `show` / `list` / `remove` supported; `--ref` supported; append/edit/copy/merge/prune and editor support not implemented |
| [`open`](open.md) | `supported` | 见命令文档。 |
| [`publish`](publish.md) | `intentionally-different` | Libra Cloudflare publish extension, not a Git command |
| [`pull`](pull.md) | `partial` | fetch + fast-forward/three-way merge supported; `--ff-only` / `--rebase` exposed; `--squash` / `--no-ff` not exposed |
| [`push`](push.md) | `partial` | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected — intentiona... |
| [`rebase`](rebase.md) | `partial` | `--autosquash` / `--reapply-cherry-picks` not supported |
| [`reflog`](reflog.md) | `supported` | 见命令文档。 |
| [`remote`](remote.md) | `supported` | 见命令文档。 |
| [`reset`](reset.md) | `supported` | 见命令文档。 |
| [`restore`](restore.md) | `supported` | 见命令文档。 |
| [`rev-list`](rev-list.md) | `supported` | 见命令文档。 |
| [`rev-parse`](rev-parse.md) | `supported` | 见命令文档。 |
| [`revert`](revert.md) | `supported` | 见命令文档。 |
| [`rm`](rm.md) | `partial` | `--force` / `--dry-run` / `--cached` / `--recursive` / `--ignore-unmatch` / `--pathspec-from-file` / `--pathspec-file... |
| [`sandbox`](sandbox.md) | `intentionally-different` | Libra AI sandbox diagnostics extension, not a Git command |
| [`shortlog`](shortlog.md) | `supported` | 见命令文档。 |
| [`show`](show.md) | `supported` | 见命令文档。 |
| [`show-ref`](show-ref.md) | `supported` | 见命令文档。 |
| [`stash`](stash.md) | `partial` | `push` / `pop` / `list` / `apply` / `drop` / `show` / `branch` / `clear` supported; `create` / `store` deferred (see ... |
| [`status`](status.md) | `supported` | 见命令文档。 |
| [`switch`](switch.md) | `supported` | 见命令文档。 |
| [`symbolic-ref`](symbolic-ref.md) | `partial` | Supports local `HEAD` only; other symbolic refs are rejected because Libra stores refs in SQLite |
| [`tag`](tag.md) | `supported` | 见命令文档。 |
| [`usage`](usage.md) | `intentionally-different` | Libra AI provider/model usage reporting extension, not a Git command |
| [`verify-pack`](verify-pack.md) | `partial` | validates one `.idx` file against a matching `.pack`; Git's multi-index form and `-s` / `--stat-only` are not exposed |
| [`worktree`](worktree.md) | `intentionally-different` | `remove` keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior; the flag refus... |

## 未公开或未纳入用户承诺的命令资料

| 文档 | 当前状态 | 下一步 |
|---|---|---|

| [`gc`](gc.md) | 未在 `src/cli.rs::Commands` 公开 | 若要发布，补 CLI/dispatch/`COMPATIBILITY.md`/测试；否则保持为历史设计资料。 |
| [`ls-files`](ls-files.md) | 未在 `src/cli.rs::Commands` 公开 | 若要发布，补 CLI/dispatch/`COMPATIBILITY.md`/测试；否则保持为历史设计资料。 |
| [`package`](package.md) | 未在 `src/cli.rs::Commands` 公开 | 若要发布，补 CLI/dispatch/`COMPATIBILITY.md`/测试；否则保持为历史设计资料。 |
| [`prune`](prune.md) | 未在 `src/cli.rs::Commands` 公开 | 若要发布，补 CLI/dispatch/`COMPATIBILITY.md`/测试；否则保持为历史设计资料。 |
| [`stats`](stats.md) | 未在 `src/cli.rs::Commands` 公开 | 若要发布，补 CLI/dispatch/`COMPATIBILITY.md`/测试；否则保持为历史设计资料。 |

## 汇总文档

- [`_compatibility.md`](_compatibility.md)：Git 兼容治理、D1-D10 拒绝/延后决策、参数级缺口状态。
- [`_general.md`](_general.md)：跨命令实现规范、CLIG 现代化、测试和文档维护要求。
