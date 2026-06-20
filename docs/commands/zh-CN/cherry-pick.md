# `libra cherry-pick`

应用一些已有提交引入的更改。

**别名：** `cp`

## 概要

```
libra cherry-pick [-n | --no-commit] [-x] [--json] [--quiet] <commit>...
```

## 说明

`libra cherry-pick` 将指定提交引入的更改应用到当前分支。对于每个具名提交，Libra 会计算该提交与其父提交之间的 diff，将得到的 changeset 应用到当前索引和工作树，并且（除非给出 `--no-commit`）记录一个新提交。需要在新提交消息中包含原始提交哈希时，使用 `-x`。

这适合在不合并的情况下，将一个分支上的提交选择性应用到另一个分支。提供多个提交时，它们会按给定顺序应用，每个提交都会先成为当前分支上的新提交，然后再处理下一个。

该命令要求处于活动分支（不是 detached HEAD），并完全拒绝 merge commits。

## 选项

### `-n`, `--no-commit`

将源提交的更改应用到索引和工作树，但**不**创建新提交。这样你可以在手动运行 `libra commit` 前检查或组合更改。

**限制：** 使用 `--no-commit` 时只能指定一个提交。尝试用该标志传递多个提交会产生错误 `LBR-CLI-002`。

```bash
# 暂存 abc1234 的更改但不提交
libra cherry-pick -n abc1234

# 检查暂存更改，然后手动提交
libra status
libra commit -m "cherry-picked and adjusted abc1234"
```

### `-x`

在新提交消息中追加 `(cherry picked from commit <hash>)`。不带 `-x` 时，Libra 保留源提交消息且不添加来源行，与 Git 默认行为一致。

```bash
# 在新提交消息中记录原始提交哈希
libra cherry-pick -x abc1234
```

### `<commit>...`（位置参数，必需）

要 cherry-pick 的一个或多个提交引用。每个值可以是完整 SHA-1 哈希、缩写哈希、分支名、`HEAD`，或任何解析为提交的引用。提交从左到右应用。

```bash
# 按哈希应用单个提交
libra cherry-pick abc1234

# 按顺序应用多个提交
libra cherry-pick abc1234 def5678 ghi9012
```

### `--json`

输出机器可读 JSON，而不是人类可读文本。见下方[结构化输出](#结构化输出-json-示例)。

### `--quiet`

抑制所有人类可读输出。退出码仍表示成功或失败。

## 常用命令

```bash
# 将单个提交 cherry-pick 到当前分支
libra cherry-pick abc1234

# 按顺序 cherry-pick 多个提交
libra cherry-pick abc1234 def5678

# Cherry-pick 但不提交，用于编辑或组合更改
libra cherry-pick -n abc1234

# Cherry-pick 并在新提交消息中记录原始提交哈希
libra cherry-pick -x abc1234

# 为 AI 代理或脚本输出 JSON
libra cherry-pick --json abc1234
```

## 人类可读输出

使用自动提交（默认）进行 cherry-pick 时：

```
[def5678] cherry-picked from abc1234
```

不使用自动提交（`-n`）进行 cherry-pick 时：

```
Changes from abc1234 staged. Use 'libra commit' to finalize.
```

## 结构化输出（JSON 示例）

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234abcdef1234567890abcdef1234567890ab",
        "short_source": "abc1234",
        "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
        "short_new": "def5678"
      }
    ],
    "no_commit": false
  }
}
```

使用 `--no-commit` 时，`new_commit` 和 `short_new` 为 `null`：

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234abcdef1234567890abcdef1234567890ab",
        "short_source": "abc1234",
        "new_commit": null,
        "short_new": null
      }
    ],
    "no_commit": true
  }
}
```

## 设计理由（为什么不同于 Git/jj）

### 为什么没有 `--edit` 标志？

Git 的 `--edit` 会打开编辑器，让用户在记录前修改提交消息。Libra 省略它有两个原因：

1. **Agent-first 工作流。** Libra 面向 AI-agent-driven 开发，其中交互式编辑器提示会阻塞自动化流水线。默认消息来自源提交；需要机器可解析的来源信息时，`-x` 会追加确定的来源行。
2. **与 `--no-commit` 组合。** 想自定义消息的用户可以使用 `-n` 暂存更改但不提交，然后运行 `libra commit -m "custom message"`。这种两步方式显式、可脚本化，并避免生成编辑器子进程的复杂度。

### 为什么 merge commits 没有 `--mainline`？

Git 的 `--mainline <parent-number>` 允许通过指定要与哪个父提交做 diff 来 cherry-pick merge commits。Libra 直接拒绝 merge commits，原因是：

1. **歧义很危险。** 选错父提交会静默地产生完全不同的 changeset。在 trunk-based monorepo 工作流中，merge commits 是短暂集成点，不是工作单元。有意义的更改位于被合并的单个提交中。
2. **简单性优于边缘场景。** 支持 `--mainline` 会增加显著复杂度（父提交选择、相对任意 base 的冲突解决），而该用例在 trunk-based 开发中很少出现。用户可以 cherry-pick 单个非合并提交。

### 为什么 `--no-commit` 限制为单个提交？

当 cherry-pick 多个提交时，每个提交都建立在前一个结果之上。没有中间提交时，索引只表示所有更改的累计效果，会丢失逐提交归属。允许这样做会：

1. **破坏来源证明。** 如果 `-x` 与多个未提交的 cherry-pick 组合，`(cherry picked from commit ...)` trailer 将失去意义，因为暂存状态是多个源提交的混合。
2. **复杂化恢复。** 如果五个提交中的第三个发生冲突，没有中间提交可回滚。Git 用 `--continue`/`--abort` 状态文件处理这一点，而 Libra 有意避免（见下方）。

### 为什么没有 `--continue`、`--abort` 或 `--skip`？

Git 维护 `.git/CHERRY_PICK_HEAD` 和 sequencer 状态文件，以支持多步冲突解决。Libra 省略这套机制，因为：

1. **无状态设计。** Libra 避免可能变旧或损坏的隐藏状态文件。每次 cherry-pick 调用都是原子的：要么完全成功，要么失败且不留下部分状态。
2. **显式冲突解决。** 发生冲突时，Libra 会尽量暂存可处理内容，并告诉用户手动解决冲突，然后运行 `libra commit`。这与 `git cherry-pick --continue` 的最终结果相同，但没有隐藏 sequencer 状态。
3. **代理兼容性。** AI 代理可以检测冲突错误码（`LBR-CONFLICT-001`），以编程方式解决冲突并运行 `libra commit`，这比管理 `--continue`/`--abort`/`--skip` 状态转换更简单。

## 参数对比：Libra vs Git vs jj

| 参数 | Git | jj | Libra |
|-----------|-----|-----|-------|
| 位置提交 | `git cherry-pick <commit>...` | N/A（使用 `jj rebase`） | `libra cherry-pick <commit>...` |
| No-commit 模式 | `--no-commit` / `-n` | N/A | `--no-commit` / `-n` |
| 记录来源 | `-x` | N/A | `-x` |
| 编辑消息 | `--edit` / `-e` | N/A | 不支持（使用 `-n` 后再 `commit -m`） |
| Mainline 父提交 | `--mainline <n>` / `-m <n>` | N/A | 不支持（拒绝 merge commits） |
| 冲突后继续 | `--continue` | N/A | 不支持（解决后 `commit`） |
| 中止进行中操作 | `--abort` | N/A | 不支持（无 sequencer 状态） |
| 跳过当前提交 | `--skip` | N/A | 不支持 |
| 策略 | `--strategy <s>` | N/A | 不支持（单一 merge 策略） |
| 策略选项 | `-X <option>` | N/A | 不支持 |
| GPG 签名 | `--gpg-sign` / `-S` | N/A | 不支持（计划中） |
| 允许空提交 | `--allow-empty` | N/A | 不支持 |
| 保留冗余提交 | `--keep-redundant-commits` | N/A | 不支持 |
| JSON 输出 | N/A | N/A | `--json` |
| Quiet 模式 | `--quiet` | `--quiet` | `--quiet` |

**注意：** jj 没有直接的 cherry-pick 等价操作。最接近的是 `jj rebase -r <rev> -d <dest>`，它将提交移动或复制到新目标。

## 错误处理

| 代码 | 条件 | 提示 |
|------|-----------|------|
| `LBR-REPO-001` | 不在 libra 仓库内 | 使用 `libra init` 初始化或进入仓库 |
| `LBR-REPO-003` | HEAD detached（不在分支上） | 使用 `libra switch <branch>` 切换到分支 |
| `LBR-CLI-003` | 无法解析提交引用 | 使用 `libra log` 查找有效提交引用 |
| `LBR-CLI-002` | `--no-commit` 搭配多个提交，或遇到 merge commit | 使用 `-n` 搭配单个提交；选择非合并提交 |
| `LBR-CONFLICT-001` | Cherry-pick 期间冲突（例如未跟踪文件会被覆盖） | 手动解决冲突，然后使用 `libra commit` |
| `LBR-IO-001` | 无法加载对象（提交、树、索引） | 检查仓库完整性并重试 |
| `LBR-IO-002` | 无法保存对象、索引或更新分支引用 | 检查文件系统权限和仓库可写性 |
