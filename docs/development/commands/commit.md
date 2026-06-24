# `libra commit` 开发设计

## 命令实现目标

`libra commit` 的目标是把索引快照记录为新的提交，并处理消息来源、作者、签名、hooks、结构化输出和兼容拒绝。实现已支持 `--all`、`--author`、`--cleanup`、`--dry-run`、`--fixup`、`--squash`、`-C/-c`（复用提交消息）、`--trailer`、`--reset-author`、`-e/--edit`（始终开编辑器）、`-v/--verbose`（编辑器模板含 staged diff，经 scissors 剥离）、bare `commit` 在可用编辑器时开编辑器、autosquash、dry-run porcelain、commit trailers 和稳定错误码，`--porcelain`（would-be-committed 状态的 porcelain v1 机器输出，隐含 `--dry-run`，不创建提交）、`--no-status`（接受式 no-op：Libra 的编辑器模板从不含 status 段）也已支持，同时对 `--status`（模板含状态段）、`-t/--template`、`commit.cleanup`/`commit.verbose` 配置等未完成行为明确说明。

## 对比 Git 与兼容性

- 兼容级别：`partial`。

- 当前矩阵承诺常用 Git commit 子集已支持；`--cleanup`、`--dry-run`、`--fixup`、`--squash`、`-C/-c`、`--trailer`、`--reset-author`、`-e/--edit`、`-v/--verbose`、`--porcelain`、`--no-status`（接受式 no-op）已补齐，`--status`（模板含状态段）/`-t/--template`/`commit.cleanup`/`commit.verbose` 仍为缺口。新增语义必须同步矩阵、用户文档和测试。
- `--amend` 作者归属与 Git 对齐：默认**保留**被修订提交的原作者（name/email/authored date），只有显式 `--reset-author` 或 `--author <AUTHOR>` 才会改写为当前身份；committer 始终取当前身份。此前 `--amend` 会静默把作者改成当前身份、使 `--reset-author` 沦为空操作，已修正（见 `src/command/commit.rs` amend 分支）。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/commit.rs`。参数/子命令类型包括：`CommitArgs`；输出、错误或状态类型包括：`CommitError`、`CommitOutput`；主要执行函数包括：`run_commit`、`execute`、`execute_safe`。
- 源码意图：源码模块注释说明该命令会收集暂存变更、构建 tree/commit 对象、校验提交消息和签名，并更新 HEAD/refs。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；核心领域逻辑集中在 `run_commit`；索引路径会加载、比较、刷新或保存 `.libra/index`；对象路径会解析 revision 并读写 blob/tree/commit/tag 等对象；引用路径会读取或更新 SQLite refs、HEAD 与 reflog；数据库路径会通过 SeaORM/SQLite 或 D1 客户端持久化元数据。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/commit.rs"]
    B --> C["参数模型<br/>CommitArgs"]
    C --> D["执行路径<br/>run_commit / execute / execute_safe"]
    D --> E["底层对象<br/>IndexEntry / Index / .libra/index / Blob"]
    D --> F["输出与错误<br/>CommitError / CommitOutput"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`IndexEntry`（索引条目，承载路径、mode、object id 和 stat 元数据）；`Index` / `.libra/index`（暂存区状态、路径条目和刷新/保存边界）；`Blob`（文件内容或 LFS pointer 写入对象库后的 blob 对象）；`Commit`（提交对象、父提交关系和提交消息载荷）；`TreeItem` / `TreeItemMode`（tree 中的路径项和 mode）；`Tree`（由索引或对象遍历生成的目录树对象）；`Branch` / branch store（SQLite refs 上的分支读写、过滤和上游关系）；`Head`（SQLite 中的 HEAD 指向、当前分支和 detached 状态）；`ReflogContext` / `with_reflog`（SQLite reflog 写入和动作记录）；`ClientStorage`（本地/分层对象存储读写入口）；SeaORM / `.libra/libra.db`（配置、refs、reflog、AI/发布元数据等 SQLite 表）；`ObjectHash`（SHA-1/SHA-256 对象 ID 和 revision 解析结果）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2025-10-02 `e45fc0f7`（`feat: add option -F/--file for commit command (#10)`）：基础实现节点：add option -F/--file for commit command (#10)；当前实现的主要轮廓可追溯到该提交。
- 2026-06-07 `d399c043`（`feat: support show-ref dereference and commit trailers`）：功能演进：support show-ref dereference and commit trailers；该节点扩展了当前命令可用的参数或行为。
- 2026-06-05 `d68e5d66`（`feat(commit): support autosquash and dry-run porcelain modes`）：功能演进：support autosquash and dry-run porcelain modes；当前 `CommitArgs` 含 `--dry-run` 与 `--porcelain`（would-be-committed 状态的 porcelain v1 机器输出，隐含 dry-run，复用 `status::output_porcelain`）；autosquash 也在实现中。
- 2026-06-07 `f2c67a80`（`fix(commit): close compatibility plan gaps`）：实现修正：close compatibility plan gaps；该节点把边界行为、错误处理或兼容差异纳入当前实现约束。
- 2026-06-19（PR-15）：实现 `-e/--edit` 与 `-v/--verbose`。新增共享编辑器模块 `src/command/editor.rs`（`resolve_editor` 返回 Option，解析序 `$GIT_EDITOR`→`core.editor`→`$VISUAL`→`$EDITOR`；`edit_message` 接收已解析 editor 串 + `abort_on_failure`），cherry-pick 复用其启动逻辑（保留自身 precedence）。`resolve_commit_message` 重构为 `resolve_final_message(args, output, parent_ids)`：拼装 base（fixup/squash/-C/-c/-m/-F），`needs_editor = edit || reedit || (无 base && !no_edit)`，显式 editor 即使非 TTY 也运行、`vi` 兜底需 TTY；`-v` 经 `build_verbose_template` 注入 staged diff（`diff::staged_diff_text`，`DiffError` 升 `pub(crate)`）并强制 `Scissors` cleanup。`cleanup_commit_message` 的 `Scissors` 谓词扩展为接受可选 `#` 前缀。CLI：`message`/`file` 改为可选、`no_edit` 去掉 `requires=amend`/`conflicts message`、`edit` 与 `no_edit` 互斥。新增 `CommitError::EditorFailed`（复用 `IoReadFailed`/128）。`-t/--template`、`commit.cleanup`/`commit.verbose` 配置仍延后（对应孤儿测试已 `#[ignore]`）。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/commit.md`。
- Synopsis：`libra commit [OPTIONS] (-m <MESSAGE> | -F <FILE> | -C <COMMIT> | -c <COMMIT> | --fixup <COMMIT> | --squash <COMMIT> | --amend --no-edit)`。
- 公开参数/子命令包括：`-m, --message <MESSAGE>`、`-F, --file <FILE>`、`--amend`、`--no-edit`、`--conventional`、`-a, --all`、`-s, --signoff`、`--author <AUTHOR>`、`--allow-empty`、`--disable-pre`、`--no-verify`、`--cleanup <MODE>`、`--dry-run`、`--fixup <COMMIT>`、`--squash <COMMIT>`、`-C/--reuse-message <COMMIT>`、`-c/--reedit-message <COMMIT>`、`--trailer <TRAILER>`、`--reset-author` 等。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容矩阵说明 | common Git commit surface plus `--cleanup`, `--dry-run`, `--fixup`, `--squash`, `-C/-c`, `--trailer`, and `--reset-author` supported | 按当前兼容矩阵保留；实现状态变化时同步 `_compatibility.md` 和测试证据。 |
| ✅ 已实现 | `--porcelain` 机器输出 | 输出 would-be-committed 状态的 porcelain v1（复用 `status::output_porcelain` + 折叠 untracked 目录），替代人类摘要；与 Git 一致 **隐含 `--dry-run`**（不创建提交）；`-a` 仅为预览自动暂存，dry-run 后通过 index 快照还原（不改动 index），`--json` 模式下惰性。带集成测试（`test_commit_porcelain_outputs_status_format`、`test_commit_all_porcelain_shows_autostaged_as_staged`）。 |
| 部分实现 | `--no-status` / `--status` | `--no-status` 作为接受式 no-op 已公开（Libra 的编辑器模板从不包含 status 段，故已是默认行为；字段 `no_status` 解析后不被读取）；Git 默认的 `--status`（在模板中包含状态段）仍未实现。 |
| 功能缺口 | `-t/--template` 初始模板、`commit.cleanup`/`commit.verbose` 配置默认 | 后续实现时需要同步源码、测试和兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
