# `libra switch`

切换分支、创建并切换到新分支，或在特定提交上 detach HEAD。

**别名：** `sw`

## 概要

```
libra switch <branch>
libra switch -c <name> [<start-point>]
libra switch -d <commit|tag|branch>
libra switch --track <remote/branch>
```

## 说明

`libra switch` 是更改分支的主要命令。它会在切换前验证工作树干净，更新 HEAD 和索引，并恢复工作树以匹配目标提交。与作为 Git 兼容表面存在的 `libra checkout` 不同，`switch` 是分支操作的推荐命令。

该命令支持四种模式：切换到已有本地分支（默认）、用 `-c` 创建新分支、用 `-d` detach HEAD，以及用 `--track` 跟踪远程分支。当目标分支已经是当前分支时，该命令是 no-op，并完全跳过干净性检查。

当找不到分支时，会通过 Levenshtein 距离提供模糊分支名建议，帮助捕获拼写错误，而无需精确匹配。

## 选项

| 标志 | 长选项 | 值 | 说明 |
|------|------|-------|-------------|
| | `<branch>` | 位置参数（可选） | 要切换到的目标分支、提交或远程引用 |
| `-c` | `--create` | `<name>` | 创建新分支并切换到它 |
| `-d` | `--detach` | | 在给定提交、标签或分支上 detach HEAD |
| | `--track` | | 创建跟踪给定远程分支的本地分支，并切换到它 |

### 标志细节

**`-c / --create <name> [start-point]`**：从 `<start-point>`（省略时为 HEAD）创建名为 `<name>` 的新分支，然后切换到它。会验证名称，检查不存在同名分支，并拒绝保留的内部分支名。

```bash
libra switch -c feature-x              # 从 HEAD 创建新分支
libra switch -c fix-123 abc1234        # 从特定提交创建新分支
libra switch -c release-2.0 main       # 从另一个分支创建新分支
```

**`-d / --detach`**：让 HEAD 直接指向某个提交，而不是分支。适合检查历史状态或从标签构建。

```bash
libra switch --detach v1.0             # 在标签处 detach
libra switch --detach abc1234          # 在提交处 detach
```

**`--track`**：查找远程跟踪引用，创建同名本地分支，设置 upstream tracking，并切换到它。与 `--create` 和 `--detach` 冲突。

```bash
libra switch --track origin/main       # 跟踪并切换到远程分支
libra switch --track feature            # 假设 origin/feature
```

## 常用命令

```bash
libra switch main                      # 切换到已有分支
libra switch -c feature-x              # 创建并切换到新分支
libra switch -c fix-123 abc1234        # 从特定提交创建分支
libra switch --detach v1.0             # 在标签上 detach HEAD
libra switch --track origin/main       # 跟踪并切换到远程分支
libra switch --json main               # 面向代理的结构化 JSON 输出
```

## 人类可读输出

默认人类模式将结果写到 `stdout`。

切换到已有分支：

```text
Switched to branch 'main'
```

创建并切换到新分支：

```text
Switched to a new branch 'feature'
```

在提交上 detach HEAD：

```text
HEAD is now at abc1234
```

已经在目标分支上（no-op）：

```text
Already on 'main'
```

`--quiet` 会抑制所有 `stdout` 输出。

## 结构化输出（JSON 示例）

`libra switch` 支持全局 `--json` 和 `--machine` 标志。

- `--json` 向 `stdout` 写入一个成功信封
- `--machine` 以紧凑单行 JSON 写入相同 schema
- 成功时 `stderr` 保持干净

切换到已有分支：

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": false,
    "detached": false,
    "already_on": false,
    "tracking": null
  }
}
```

创建并切换到新分支：

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature-x",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "created": true,
    "detached": false,
    "already_on": false,
    "tracking": null
  }
}
```

在标签或提交上 detach HEAD：

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": null,
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": false,
    "detached": true,
    "already_on": false,
    "tracking": null
  }
}
```

跟踪并切换到远程分支：

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": true,
    "detached": false,
    "already_on": false,
    "tracking": {
      "remote": "origin",
      "remote_branch": "feature"
    }
  }
}
```

### Schema 说明

- `previous_branch` 在切换前 HEAD detached 时为 `null`
- `branch` 在 HEAD 当前 detached（`--detach`）时为 `null`
- `already_on` 在目标分支等于当前分支（no-op）时为 `true`
- `tracking` 仅在 `--track` 时存在，包含 `remote` 和 `remote_branch`
- `created` 在 `--create` 或 `--track` 创建新本地分支时为 `true`

## 设计理由

### 为什么与 checkout 分离？

Git 的 `checkout` 被过度重载：它切换分支、恢复文件、detach HEAD、创建分支，这些都通过同一命令的不同标志组合完成。这让人类和 AI 代理都难以预测行为。Libra 遵循 Git 自身的现代化路径（Git 2.23 引入），将 `checkout` 拆分为 `switch`（分支操作）和 `restore`（文件操作）。`libra switch` 只处理分支相关状态变更，使行为可预测，错误消息精确。

保持 `switch` 聚焦也简化了结构化输出：每个 `SwitchOutput` 无论操作模式如何都包含相同字段，因此代理无需猜测适用哪个 schema 变体就能解析结果。

### 为什么自动跟踪远程分支？

使用 `--track origin/feature` 时，Libra 会在单个原子操作中自动创建本地分支、设置 upstream tracking 并切换到它。这消除了 `git fetch && git branch feature origin/feature && git branch -u origin/feature feature && git switch feature` 这种多步仪式。对于在 trunk-based 工作流中运行的 AI 代理，将四个命令减少为一个命令意味着更少失败点和更简单的工具编排。

当只提供分支名时（例如 `libra switch --track feature`），`--track` 标志默认使用 `origin` 远程，这匹配最常见的远程设置。

### 为什么有模糊建议？

当找不到分支名时，Libra 会对所有已知分支计算 Levenshtein 距离，并建议编辑距离 2 以内的匹配。这可以捕获常见拼写错误（`faeture` 而不是 `feature`），无需 glob 模式或正则。建议会作为错误输出中的可操作提示出现，减少人类用户和可解析提示文本的 AI 代理的往返。

## 参数对比：Libra vs Git vs jj

| 功能 | Git | Libra | jj |
|---------|-----|-------|----|
| 切换分支 | `git switch main` | `libra switch main` | `jj edit <rev>` |
| 创建并切换 | `git switch -c feature` | `libra switch -c feature` | `jj new -m "feature"` + `jj branch create feature` |
| 从提交创建 | `git switch -c fix abc1234` | `libra switch -c fix abc1234` | `jj new abc1234` + `jj branch create fix` |
| Detach HEAD | `git switch --detach v1.0` | `libra switch --detach v1.0` | `jj edit <rev>`（始终类似 detached） |
| 跟踪远程 | `git switch --track origin/main` | `libra switch --track origin/main` | N/A（jj 跟踪所有远程） |
| 强制创建 | `git switch -C feature` | 不支持（先删除） | N/A |
| Orphan 分支 | `git switch --orphan <name>` | 不支持 | `jj new root()` |
| 结构化输出 | 无 | `--json` / `--machine` | `--template` |
| 模糊建议 | 无 | 基于 Levenshtein 的 "did you mean" 提示 | 无 |
| 干净状态验证 | 警告但有时继续 | 以可操作错误阻止切换 | 无 dirty state 概念 |

## 错误处理

每个 `SwitchError` 变体都会映射到显式 `StableErrorCode`。

| 场景 | 错误码 | 退出码 | 提示 |
|----------|-----------|------|------|
| 缺少 track 目标 | `LBR-CLI-002` | 129 | "provide a remote branch name, for example 'origin/main'." |
| 缺少 detach 目标 | `LBR-CLI-002` | 129 | "provide a commit, tag, or branch to detach at." |
| 缺少分支名 | `LBR-CLI-002` | 129 | "provide a branch name." |
| 找不到分支 | `LBR-CLI-003` | 129 | "create it with 'libra switch -c {name}'." + 模糊建议 |
| 得到远程分支 | `LBR-CLI-003` | 129 | "use 'libra switch --track ...' to create a local tracking branch." |
| 找不到远程分支 | `LBR-CLI-003` | 129 | "Run 'libra fetch {remote}' to update remote-tracking branches." |
| 无效远程分支 | `LBR-CLI-003` | 129 | "expected format: 'remote/branch'." |
| 分支已存在 | `LBR-CONFLICT-002` | 128 | "use 'libra switch {name}' if you meant the existing local branch." |
| 内部分支被阻止 | `LBR-CLI-003` | 129 | -- |
| 未暂存更改 | `LBR-REPO-003` | 128 | "commit or stash your changes before switching." |
| 未提交更改 | `LBR-REPO-003` | 128 | "commit or stash your changes before switching." |
| 未跟踪文件会被覆盖 | `LBR-CONFLICT-002` | 128 | "move or remove it before switching." |
| 状态检查失败 | `LBR-IO-001` | 128 | -- |
| 提交解析失败 | `LBR-CLI-003` | 129 | "check the revision name and try again." |
| 分支创建失败 | `LBR-IO-002` | 128 | -- |
| HEAD 更新失败 | `LBR-IO-002` | 128 | -- |
| 委托（branch/restore） | 原始代码 | 原始 | 原始提示 |

`switch -c <existing-branch>` 当前通过 `DelegatedCli` 保留原始 `branch` 命令冲突契约，因此该路径保持 branch 命令现有错误形状，而不是添加 `SwitchError::BranchAlreadyExists` 提示。
