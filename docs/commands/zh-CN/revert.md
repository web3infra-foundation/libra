# `libra revert`

回滚一些已有提交。

## 概要

```
libra revert [-n | --no-commit] [--json] [--quiet] <commit>
```

## 说明

`libra revert` 会创建一个新提交，用于撤销指定提交引入的更改。与会重写历史的 `reset` 不同，`revert` 对共享分支是安全的，因为它保留原始提交，并在其上方添加一个新提交。

该命令通过计算目标提交与其父提交之间的 diff，然后将该 diff 的逆应用到当前工作树和索引来工作。如果结果状态干净，会记录一个新提交，消息格式为 `Revert "<original subject>"`。

回滚 root 提交（没有父提交的提交）会产生空树，实际效果是撤销初始提交的更改。

该命令要求处于活动分支（不是 detached HEAD），并且只接受一个提交引用。

## 选项

### `-n`, `--no-commit`

将逆向更改应用到索引和工作树，但**不**创建新提交。当你想检查结果、组合多个 revert，或在提交前调整更改时，这很有用。

```bash
# 暂存 revert 但不提交
libra revert -n abc1234

# 查看发生了什么变化
libra diff --cached

# 使用自定义消息提交
libra commit -m "revert abc1234 with adjustments"
```

### `<commit>`（位置参数，必需）

要回滚的单个提交引用。可以是完整 SHA-1 哈希、缩写哈希、分支名、`HEAD`，或任何解析为提交的引用。

```bash
# 回滚最近一次提交
libra revert HEAD

# 按哈希回滚
libra revert abc1234

# 回滚某个分支指向的提交
libra revert feature-branch
```

### `--json`

输出机器可读 JSON，而不是人类可读文本。见下方[结构化输出](#结构化输出-json-示例)。

### `--quiet`

抑制所有人类可读输出。退出码仍然表示成功或失败。

## 常用命令

```bash
# 回滚最近一次提交
libra revert HEAD

# 按哈希回滚特定提交
libra revert abc1234

# 回滚但不自动提交（用于编辑或组合）
libra revert -n HEAD

# 为 AI 代理或脚本输出 JSON
libra revert --json HEAD
```

## 人类可读输出

使用自动提交（默认）进行 revert 时：

```
[def5678] Revert commit abc1234
```

不使用自动提交（`-n`）进行 revert 时：

```
Changes staged for revert. Use 'libra commit' to finalize.
```

## 结构化输出（JSON 示例）

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
    "short_new": "def5678",
    "no_commit": false,
    "files_changed": 3
  }
}
```

使用 `--no-commit` 时，`new_commit` 和 `short_new` 为 `null`：

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": null,
    "short_new": null,
    "no_commit": true,
    "files_changed": 3
  }
}
```

## 设计理由（为什么不同于 Git/jj）

### 为什么只支持单个提交（没有 `<commit>...`）？

Git 允许 `git revert <commit1> <commit2> ...` 回滚一系列提交。Libra 将 `revert` 限制为单个提交，原因是：

1. **原子操作。** 每次 revert 都是自包含的：要么成功，要么失败，不会留下部分状态。Git 的多提交 revert 需要 sequencer 状态（`REVERT_HEAD`、`sequencer/`），如果用户放弃操作，这些状态可能变旧或损坏。
2. **显式更好。** 在 trunk-based monorepo 工作流中，回滚多个提交是重要动作，值得逐提交地有意处理。运行 `libra revert A && libra revert B` 会在 reflog 中明确表达意图，并且非常容易脚本化。
3. **代理简单性。** AI 代理可以循环处理提交，并独立处理每个 revert 结果，这比管理 sequencer 状态转换更简单。

### 为什么不支持合并提交（`--mainline`）？

Git 的 `--mainline <parent-number>` 会选择合并提交的某个父提交，用于计算逆向 diff。Libra 拒绝合并提交，原因是：

1. **父提交歧义很危险。** 选错父提交会静默地产生截然不同的 changeset。在 trunk-based 开发中，合并内部的单个提交才是有意义的单元；应回滚那些提交。
2. **复杂度成本。** 支持 `--mainline` 要求用户了解合并的父提交顺序，而这很少直观。该功能为 trunk-based 工作流自然避开的边缘场景增加了显著代码复杂度。

### 为什么没有 `--continue`、`--abort`？

与 cherry-pick 一样，Libra 的 revert 是无状态的：

1. **没有隐藏状态文件。** Git 的 `REVERT_HEAD` 和 `sequencer/` 目录是隐式状态，可能让用户和代理困惑。Libra 完全避免这种状态。
2. **冲突解决是显式的。** 当检测到冲突（文件已被后续提交修改）时，Libra 会报告具体路径和错误码（`LBR-CONFLICT-001`）。用户解决冲突后运行 `libra commit`。这在功能上等价于 `git revert --continue`，但没有隐藏状态。
3. **便于自动化预测。** 代理检测错误码、以编程方式解决冲突并提交，不需要管理状态机。

### 为什么使用冲突检测而不是三方合并？

Libra 的 revert 使用比 Git 三方合并更简单的冲突模型：如果目标路径上的文件自待回滚提交以来已被修改，Libra 会引发冲突，而不是尝试自动解决。这是有意保守的，因为：

1. **安全优先于便利。** 当更改的语义上下文已改变时，自动合并可能静默产生错误结果。大声失败能确保用户审查交互。
2. **确定性行为。** 相同输入始终产生相同输出：要么干净 revert，要么冲突错误，绝不会是引入细微 bug 的“成功”合并。

## 参数对比：Libra vs Git vs jj

| 参数 | Git | jj | Libra |
|-----------|-----|-----|-------|
| 位置提交 | `git revert <commit>...` | N/A（使用 `jj backout`） | `libra revert <commit>`（单个） |
| No-commit 模式 | `--no-commit` / `-n` | N/A | `--no-commit` / `-n` |
| 编辑消息 | `--edit` / `--no-edit` | N/A | 不支持（使用 `-n` 后再 `commit -m`） |
| Mainline 父提交 | `--mainline <n>` / `-m <n>` | N/A | 不支持（拒绝合并提交） |
| 冲突后继续 | `--continue` | N/A | 不支持（解决后 `commit`） |
| 中止进行中操作 | `--abort` | N/A | 不支持（无 sequencer 状态） |
| 跳过当前提交 | `--skip` | N/A | 不支持 |
| 策略 | `--strategy <s>` | N/A | 不支持 |
| 策略选项 | `-X <option>` | N/A | 不支持 |
| GPG 签名 | `--gpg-sign` / `-S` | N/A | 不支持（计划中） |
| JSON 输出 | N/A | N/A | `--json` |
| Quiet 模式 | `--quiet` | N/A | `--quiet` |
| 变更文件数量 | N/A | N/A | 包含在 JSON 输出中 |

**注意：** jj 使用 `jj backout -r <rev>` 作为 `git revert` 的等价操作。它会创建一个新提交，该提交是目标修订的逆。

## 错误处理

| 代码 | 条件 | 提示 |
|------|-----------|------|
| `LBR-REPO-001` | 不在 libra 仓库内 | 使用 `libra init` 初始化或进入仓库 |
| `LBR-REPO-003` | HEAD detached（不在分支上） | 使用 `libra switch <branch>` 切换到分支 |
| `LBR-CLI-003` | 无法解析提交引用 | 使用 `libra log` 查找有效提交引用 |
| `LBR-CLI-002` | 不支持回滚合并提交 | 选择非合并提交；合并提交支持已规划 |
| `LBR-CONFLICT-001` | 文件已被后续提交修改，产生冲突 | 手动解决冲突，然后使用 `libra commit` |
| `LBR-IO-001` | 无法加载对象（提交、树、blob） | 检查仓库完整性 |
| `LBR-IO-002` | 无法保存对象、索引或更新 HEAD | 检查文件系统权限和仓库可写性 |
