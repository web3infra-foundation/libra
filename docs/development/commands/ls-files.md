# `libra ls-files`

## 命令实现目标

`libra ls-files` 提供公开的 Git 兼容索引/工作树路径列举入口。当前目标是覆盖常用脚本和 AI 安全只读场景：缓存索引列表、已修改/已删除筛选、stage 样式输出、未跟踪文件列举、`.libraignore` 感知过滤、pathspec、`--error-unmatch`、`-z` 文本输出、`-t` 状态标签、`-u`/`--unmerged` 冲突条目筛选，以及标准 JSON / machine envelope。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：默认 cached listing、`--cached` / `-c`、`--deleted`、`--modified`、`--stage` / `-s`、`--abbrev[=<n>]`（在 `-s`/`--stage` 输出里把对象名截断为 n 位 hex，bare 即 7；取值用 `=` 形式 `require_equals`，故 bare 不会吞掉 pathspec；定长截断而非最短唯一前缀）、`--others` / `-o`、`--others --exclude-standard`、`-i` / `--ignored`（只列出被忽略的集合：`-i -o` 列出被忽略的未跟踪文件——`-o` 的反集，复用 `IgnorePolicy::OnlyIgnored`；`-i -c` 列出匹配 exclude 模式的已跟踪文件——复用 `ignore::path_matches_ignore_pattern`；要求配 `-o`/`-c` 且需 `--exclude-standard`，否则退出码 128，与 git 一致）、`<pathspec>...`、`--error-unmatch`、`-z`、`-t`（状态标签 H/R/C/?/M）、`-u` / `--unmerged`（仅冲突条目）、`--full-name`（接受为 no-op；Libra 始终输出仓库根相对路径）、`--json` 和 `--machine`。
- 语义说明：pathspec 从调用者当前工作目录解析；精确文件和目录前缀都可匹配；解析到仓库外的 pathspec 会被拒绝。
- 暂未公开：`--eol`、resolve-undo、killed/debug output、sparse-checkout integration。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::LsFiles` 公开顶层命令，dispatch 到 `src/command/ls_files.rs::execute_safe`。
- 源码分层：参数模型为 `LsFilesArgs`；结果条目由 `FileEntry` 表示；输出统一走 `OutputConfig`、human text、`--json` 和 `--machine` 路径。
- 执行路径：命令只读加载 `.libra/index`，按 state filter 和 pathspec 收集索引/工作树条目。`--modified` 对工作树文件计算 blob hash 并与索引 hash 比较；`--others` 扫描工作树并可通过 `--exclude-standard` 套用 `.libraignore`。
- 副作用边界：该命令不得写入索引、对象库、refs、reflog、SQLite/D1、工作树或远端；AI/MCP `run_libra_vcs ls-files` 也按只读命令分类。

## 实现历史

- 2026-06-13 `8d4fb969`：引入基础索引列举实现轮廓。
- 2026-06-20 PR #415：公开 `ls-files` 顶层命令，补齐 pathspec、`--error-unmatch`、`-z`、AI/MCP 只读安全覆盖、用户文档和兼容矩阵。

## 当前状态

- 公开状态：已公开。
- 用户文档：`docs/commands/ls-files.md` 和 `docs/commands/zh-CN/ls-files.md`。
- 兼容矩阵：`COMPATIBILITY.md` 顶层命令表登记为 `partial`。
- 回归测试：`tests/command_test.rs` 的 `command::ls_files_test::` 覆盖 CLI 行为；`tests/ai_libra_vcs_safety_test.rs` 覆盖 AI/MCP 只读安全；compat 文档测试覆盖 help、用户文档和命令索引同步。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| ✅ 已实现 | 状态标签 `-t` | 在每行路径前加状态标签（`H`=cached、`R`=removed/deleted、`C`=modified/changed、`?`=other/untracked、`M`=unmerged），由 `status_tag(&FileEntry.status)` 映射，格式与 `git ls-files -t` 一致。Libra 不建模 skip-worktree/killed，故不产出 `S`/`K`。带集成测试（`ls_files_t_prefixes_status_tags`）。 |
| ✅ 已实现 | `-u` / `--unmerged` | 仅列出冲突（stage 1/2/3）条目，输出 stage 样式（`<mode> <hash> <stage>\t<path>`），与 `git ls-files -u` 一致；冲突条目 `status` 现统一为 `unmerged`（stage>0），`-t` 下标为 `M`。带集成测试（`ls_files_u_shows_unmerged_conflict_entries`，经真实 merge 冲突构造）。 |
| ✅ 已实现（intentionally-different） | `--full-name` | 接受 Git 的 `--full-name` 标志为 no-op：Libra 的 ls-files 始终输出仓库根相对路径（即 Git `--full-name` 的形式），不按 cwd 子目录裁剪，因此该标志不改变行为，仅为脚本兼容而接受。带集成测试（`ls_files_full_name_accepted_as_noop`）。 |
| ✅ Ignored listing | `-i`/`--ignored`（`-i -o` 被忽略未跟踪、`-i -c` 匹配 exclude 的已跟踪；要求 `-o`/`-c` + `--exclude-standard`）已实现，带集成测试 `test_ls_files_ignored`。 | 与 git 一致；其余 explicit exclude-source（`-x`/`--exclude-from`）仍未公开。 |
| EOL / resolve metadata | `--eol`、resolve-undo、killed/debug output 未公开。 | 继续列为兼容缺口。 |
| Sparse checkout | 未接入 Git sparse-checkout 语义。 | Libra 当前不维护对应状态；需要单独设计后再公开。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)。
- 行为变更必须同步 `COMPATIBILITY.md`、`docs/commands/ls-files.md`、`docs/commands/zh-CN/ls-files.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON / machine 输出契约、AI/MCP 安全分类和回归测试。
