# check-mailmap 命令开发设计

## 命令实现目标

`libra check-mailmap` 通过工作树 `.mailmap` 解析 `Name <email>` 联系人并打印规范形式。GGT-13 互操作池的第一个、最自包含的命令；纯解析+查询，不写对象。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`check-mailmap <contact>...` / `--stdin`、`--json`/`--machine`。四种 `.mailmap` 行形式；`(name,email)` 规则优先于 email-only；注释/空行忽略；email 大小写不敏感。
- 未公开（延后）：`mailmap.file`/`mailmap.blob` 配置；接入 `log`/`blame` 作者显示（acceptance「先只做解析查询，再接入 log/blame」—— 后者为后续）。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::CheckMailmap` → `command::check_mailmap::execute_safe`。
- 源码分层：`src/command/check_mailmap.rs`：`CheckMailmapArgs`（`stdin`/`contacts`）、`MailmapEntry`（`new_name`/`new_email`/`old_name?`/`old_email`）、`load_mailmap`/`parse_mailmap_line`/`resolve`/`parse_contact`/`format_contact`。
- 解析 `.mailmap` 行：按第一/第二个 `<...>` 取 new/old email；first email 前为 new_name，first `>` 与 second `<` 之间为 old_name（可空）；单 email 时 old_email=new_email（按该 email 键控）。
- 解析：`(in_name,in_email)` → 先找 `old_name==Some(in_name) && old_email==in_email`，再找 `old_name==None && old_email==in_email`（email 大小写不敏感）；命中则 name=new_name(非空否则 in_name)、email=new_email；否则原样。
- 输入：positional contacts 或 `--stdin`（逐行）；空 → 128；联系人无 `<email>` → 128。
- 底层操作对象：只读工作树 `.mailmap`。无对象库/refs/网络写入。

## 实现历史

- 2026-06-30（GGT-13 / 1，`grit-gap.md` 阶段 6）：互操作池首个命令；独立 PR。

## 当前状态

- 公开状态：已公开（`Commands::CheckMailmap`）。
- 测试：`tests/command/check_mailmap_test.rs`（解析 commit-email、未匹配透传、`--stdin`、`--json`、无联系人 128、非法联系人 128、非仓库 128）+ `check_mailmap.rs` 单测（commit-email 映射、name+email 优先、未匹配、注释跳过）。
- 用户文档：`docs/commands/check-mailmap.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 集成 | 接入 `log`/`blame` 作者显示 | 延后；当前仅独立查询（acceptance 允许分阶段）。 |
| 配置 | `mailmap.file`/`mailmap.blob` | 延后；仅读工作树 `.mailmap`。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- 接入 log/blame 时复用 `resolve`，不得另写一套 mailmap 解析。
