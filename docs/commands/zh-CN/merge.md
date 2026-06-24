# `libra merge`

将一个目标合并到当前分支。

## 概要

```text
libra merge [--ff-only | --no-ff | --squash | --no-commit] [-m <msg>] [--no-edit] [-n | --no-stat] <branch>
libra merge --continue
libra merge --abort
```

## 说明

`libra merge <branch>` 会解析本地分支、提交哈希，或 `refs/remotes/origin/main` 这样的远程跟踪引用。

如果当前分支可以快进，Libra 会将分支指针移动到目标提交，并恢复索引和工作树。如果分支已经分叉，Libra 会使用 merge base 执行单头三方合并。

干净的三方合并会创建双父合并提交、更新 HEAD、重建索引、恢复工作树，并写入 merge reflog 条目。有冲突的三方合并会向工作树写入冲突标记，写入未合并的索引 stage，保存 Libra merge 状态，并返回 `LBR-CONFLICT-002`，同时给出 `libra merge --continue` 和 `libra merge --abort` 的提示。

Libra 仍未实现 octopus merge、自定义策略、策略选项、交互式消息编辑（`--edit`/启动编辑器）或签名验证。

## 选项

| 选项 | 说明 |
|--------|-------------|
| `<branch>` | 要合并的目标分支、提交或远程跟踪引用。 |
| `-m, --message <MSG>` | 覆盖合并提交消息（默认 `Merge <branch> into <head>`）。 |
| `--ff-only` | 仅当当前分支可快进时才合并，否则失败。 |
| `--no-ff` | 即使可以快进也强制生成双父合并提交。 |
| `--squash` | 生成合并后的索引/工作树但不创建提交、不移动 HEAD；随后用普通 `libra commit` 收尾。 |
| `--no-commit` | 执行合并并暂存结果但停在提交之前；随后用 `libra merge --continue` 收尾。 |
| `--no-edit` | 接受自动生成的合并消息而不启动编辑器。Libra 从不为 merge 打开编辑器，故此为对齐 Git 而接受的 no-op。 |
| `-n`, `--no-stat` | 合并结束时不显示 diffstat。为对齐 Git 而接受的 no-op：Libra 的 merge 从不打印 diffstat。（Git 默认的 `--stat` diffstat 未实现。） |
| `--no-progress` | 不显示进度条。为对齐 Git 而接受的 no-op：Libra 的 merge 从不渲染进度条。 |
| `--continue` | 在冲突已解决并暂存后完成进行中的合并。 |
| `--abort` | 恢复合并前的 HEAD、索引和工作树。 |
| `--json` | 输出结构化成功信封。 |
| `--machine` | 以一行紧凑 JSON 输出同一结构化信封。 |
| `--quiet` | 抑制人类可读的成功输出。 |

## 常用命令

```bash
libra merge feature-x
libra merge refs/remotes/origin/main
libra merge --continue
libra merge --abort
libra merge --json feature-x
```

## 冲突生命周期

当合并发生冲突时：

1. 编辑包含冲突标记的文件。
2. 使用 `libra add <path>` 暂存每个已解决路径。
3. 运行 `libra merge --continue` 创建双父合并提交。

在继续之前运行 `libra merge --abort` 可将分支、索引和工作树恢复到合并前提交。当存在 merge 状态时，`libra status` 会显示进行中的合并目标，以及 continue/abort 命令。

## 人类可读输出

快进：

```text
Fast-forward
```

干净三方合并：

```text
Merge made by the 'three-way' strategy.
```

已经是最新：

```text
Already up to date.
```

`--continue` 后：

```text
Merge completed.
```

`--abort` 后：

```text
Merge aborted.
```

冲突错误会通过 Libra 的标准结构化错误信封打印到 stderr，并包含恢复提示。

## JSON / Machine 输出

成功输出保留历史上的 `files_changed` 数值字段，并仅在相关时添加 merge 生命周期字段。

```json
{
  "ok": true,
  "command": "merge",
  "data": {
    "strategy": "three-way",
    "old_commit": "abc1234...",
    "commit": "def5678...",
    "files_changed": 2,
    "up_to_date": false,
    "parents": ["abc1234...", "fedcba9..."]
  }
}
```

已经最新的合并使用 `strategy: "already-up-to-date"`、`commit: null`、`files_changed: 0` 和 `up_to_date: true`。

`--abort` 设置 `aborted: true`；`--continue` 设置 `continued: true`。冲突失败会在 stderr 上返回带有 `LBR-CONFLICT-002` 的错误信封。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| 分支目标 | `<branch>`（单个目标） | `<commit>...`（一个或多个） | N/A（使用 `jj new`） |
| 快进 | 支持 | 支持 | N/A |
| 单头三方合并 | 支持 | 支持 | N/A |
| Continue / abort | `--continue`, `--abort` | `--continue`, `--abort` | N/A |
| Octopus merge | 不支持 | 支持 | N/A |
| 仅快进 | `--ff-only` | `--ff-only` | N/A |
| 强制合并提交 | `--no-ff` | `--no-ff` | N/A |
| Squash | `--squash` | `--squash` | N/A |
| 不提交 | `--no-commit` | `--no-commit` | N/A |
| 提交消息 | `-m <msg>` | `-m <msg>` | N/A |
| 不编辑 | `--no-edit`（no-op；从不编辑） | `--no-edit` | N/A |
| 不显示 diffstat | `-n` / `--no-stat`（no-op；从不打印） | `-n` / `--no-stat` | N/A |
| 不显示进度条 | `--no-progress`（no-op；从不渲染） | `--no-progress` | N/A |
| 自定义策略 | 不支持 | `--strategy`, `-X` | N/A |
| 验证签名 | 不支持 | `--verify-signatures` | N/A |
| JSON 输出 | `--json` / `--machine` | 不支持 | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 缺少分支 / 动作 | `LBR-CLI-001` | 129 |
| 无法解析目标引用 | `LBR-CLI-003` | 129 |
| 无法加载合并目标/当前提交/树 | `LBR-REPO-002` | 128 |
| 无关历史 | `LBR-REPO-003` | 128 |
| 合并冲突 | `LBR-CONFLICT-002` | 128 |
| 脏工作树或暂存更改 | `LBR-CONFLICT-002` | 128 |
| 未跟踪文件会被覆盖 | `LBR-CONFLICT-002` | 128 |
| 合并已在进行中 | `LBR-CONFLICT-002` | 128 |
| 对 `--continue` / `--abort` 没有进行中的合并 | `LBR-REPO-003` | 128 |
| `--continue` 仍有未解决的冲突 stage | `LBR-CONFLICT-002` | 128 |
| 无法读取 merge 状态或索引 | `LBR-IO-001` | 128 |
| 无法保存状态、索引、树、提交、HEAD 或工作树 | `LBR-IO-002` | 128 |
