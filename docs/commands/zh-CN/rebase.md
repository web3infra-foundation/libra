# `libra rebase`

在另一个 base tip 之上重新应用提交。

**别名：** `rb`

## 概要

```
libra rebase [--autosquash] [--reapply-cherry-picks] [--no-autostash] [--no-rerere-autoupdate] [--keep-empty] <upstream>
libra rebase --continue
libra rebase --abort
libra rebase --skip
```

## 说明

`libra rebase` 将当前分支上的一系列提交移动到新的 base 提交之上。它会找到当前分支与指定 upstream 之间的共同祖先，收集从该祖先到当前 HEAD 的所有提交，并在 upstream 分支之上重放每个提交。所有提交重放后，当前分支引用会更新为指向最终 rebased 提交。

如果重放期间发生冲突，rebase 会停止并报告冲突文件。用户手动解决冲突、暂存已解决文件，然后运行 `libra rebase --continue` 继续。或者，`--abort` 会恢复原始分支状态，`--skip` 会丢弃当前提交并继续下一个。

Rebase 状态（剩余和已完成提交列表、原始 HEAD 和目标 base）持久化在 SQLite 数据库中。这让 rebase 状态能跨进程重启存活，并避免 Git 使用的脆弱文件式状态。旧 Libra 版本的 legacy file-based 状态会在首次访问时自动迁移到数据库。

## 选项

| 选项 | 长选项 | 说明 |
|--------|------|-------------|
| `<upstream>` | | 要 rebase 到的 upstream 分支或提交。除非指定 `--continue`、`--abort` 或 `--skip`，否则必需。可以是分支名、提交哈希或任何 Git 引用。 |
| | `--continue` | 在解决冲突后继续 rebase。与 `--abort`、`--skip` 和 `<upstream>` 互斥。 |
| | `--abort` | 中止当前 rebase，并将原始分支恢复到 rebase 前状态。与 `--continue`、`--skip` 和 `<upstream>` 互斥。 |
| | `--skip` | 跳过当前提交，并继续 rebase 序列中的下一个提交。与 `--continue`、`--abort` 和 `<upstream>` 互斥。 |
| | `--keep-empty` | 保留 start-empty（重放前就为空）的提交而非丢弃。为 Git 兼容性接受的 no-op：Libra 的 rebase 默认就保留空提交。（反向 `--no-keep-empty`（丢弃 start-empty 提交）未实现；丢弃 replay 后变空提交的独立 `--empty=drop` 亦未实现。） |

### 选项细节

**`<upstream>`**

开始新的 rebase，将当前分支提交重放到指定 upstream 之上：

```bash
$ libra rebase main
Found common ancestor: abc1234
Rebasing 3 commits from `feature` onto `main`...
Applied: def5678 feat: add parser
Applied: 987abcd feat: add lexer
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

**`--continue`**

解决冲突并暂存已解决文件后，继续 rebase：

```bash
$ libra rebase --continue
Applied: 987abcd feat: add lexer
Rebasing 1 commits from `feature` onto `1234567`...
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

**`--abort`**

中止 rebase 并恢复原始分支状态：

```bash
$ libra rebase --abort
Rebase aborted. Restored branch 'feature'.
```

**`--skip`**

跳过当前冲突提交并移动到下一个：

```bash
$ libra rebase --skip
Skipped: 987abcd feat: add lexer
Rebasing 1 commits from `feature` onto `1234567`...
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

## 常用命令

```bash
# 将当前分支 rebase 到 main
libra rebase main

# Rebase 到特定提交
libra rebase abc1234

# 解决冲突后继续
libra rebase --continue

# 中止 rebase
libra rebase --abort

# 跳过有问题的提交
libra rebase --skip

# 使用别名
libra rb main
```

## 人类可读输出

正常 rebase 进度：

```text
Found common ancestor: abc1234
Rebasing 3 commits from `feature` onto `main`...
Applied: def5678 feat: add parser
Applied: 987abcd feat: add lexer
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

Rebase 期间冲突：

```text
fatal: rebase stopped while applying 987abcd: feat: add lexer

Hint: conflicted files:
Hint:   src/lexer.rs
Hint: resolve conflicts, stage them, then run 'libra rebase --continue'.
Hint: or run 'libra rebase --skip' / 'libra rebase --abort'.
```

已经最新：

```text
Current branch is ahead of upstream. No rebase needed.
```

仅快进场景：

```text
Fast-forwarded branch 'feature' to 'main'.
```

Abort：

```text
Rebase aborted. Restored branch 'feature'.
```

## JSON / Machine 输出

当前，成功的 `rebase <upstream>`、`--abort`、`--continue` 和 `--skip` 输出支持 `--json` 和 `--machine`。CLI/preflight 失败、未解决冲突的 `--continue` 失败，以及结构化 `rebase <upstream>` 冲突停止，都会通过 Libra 标准结构化错误信封渲染。更深层的 replay/conflict-stop 错误分类仍在命令改进计划中作为后续工作跟踪。

开始并完成重放：

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "start",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "upstream": "main",
    "onto": "fedcba9...",
    "common_ancestor": "0123456...",
    "replay_count": 1,
    "previous_commit": "def5678...",
    "applied_commits": [
      {
        "original_commit": "0123456...",
        "commit": "abc1234...",
        "subject": "Feature adds file"
      }
    ],
    "remaining": 0
  }
}
```

Fast-forward start 结果使用相同信封，`status: "fast-forwarded"`，`commit` 等于 `onto`，并且没有 `applied_commits`。已经领先 upstream 的分支返回 `status: "already-up-to-date"`。

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "abort",
    "status": "aborted",
    "branch": "feature",
    "commit": "abc1234...",
    "previous_commit": "def5678...",
    "restored": true
  }
}
```

解决冲突后 continue：

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "continue",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "onto": "fedcba9...",
    "previous_commit": "def5678...",
    "applied_commits": [
      {
        "original_commit": "0123456...",
        "commit": "abc1234...",
        "subject": "Feature modifies conflict.txt"
      }
    ],
    "remaining": 0
  }
}
```

跳过已停止提交：

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "skip",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "onto": "fedcba9...",
    "previous_commit": "def5678...",
    "skipped_commit": "0123456...",
    "skipped_subject": "Feature modifies conflict.txt",
    "remaining": 0
  }
}
```

## Rebase 状态持久化

Rebase 状态存储在 `rebase_state` SQLite 表中，包含以下字段：

| 字段 | 类型 | 说明 |
|-------|------|-------------|
| `head_name` | TEXT | 正在 rebase 的原始分支名 |
| `onto` | TEXT | 正在 rebase 到其上的提交哈希 |
| `orig_head` | TEXT | Rebase 开始前的原始 HEAD 提交 |
| `current_head` | TEXT | 当前新 base（目前已 rebased 提交的 HEAD） |
| `todo` | TEXT | 剩余待重放提交（换行分隔哈希） |
| `done` | TEXT | 已重放提交（换行分隔哈希） |
| `stopped_sha` | TEXT（nullable） | 导致冲突的当前提交 |

## 设计理由

### 为什么没有 `--interactive` / `-i`？

Git 的交互式 rebase 会打开编辑器，包含一份可以重排、squash、edit 或 drop 的提交列表。这是 Git 最强大的功能之一，但本质上是交互式的：它需要编辑器会话，并在启动时由人类决策。

Libra 面向 AI 代理和自动化工作流，在这些场景中交互式编辑器会话不可行。Libra 不提供交互式 rebase，而是鼓励将复杂历史重写拆成离散操作：使用 `rebase` 进行线性重放，并在未来使用专用命令进行 squash 或重排。

### `--onto`

Git 的 `--onto` 标志允许将提交子集 rebase 到任意 base 上，独立于 upstream 引用。Libra **已支持** `--onto <newbase> [<upstream>] [<branch>]`：把 `<upstream>..HEAD` 区间重放到 `<newbase>` 上，第三个位置参数 `<branch>` 会在 rebase 前先被检出。不带 `--onto` 时，Libra 将从共同祖先到 HEAD 的所有提交 rebase 到指定 upstream 上，覆盖绝大多数 rebase 用例。

### 为什么在 SQLite 中持久化状态？

Git 将 rebase 状态持久化在 `.git/rebase-merge/` 目录中，每个字段一个文件（head-name、onto、orig-head 等）。这种方式脆弱：部分写入可能损坏状态，并发访问没有保护。

Libra 使用 SQLite 持久化 rebase 状态，提供：
- **原子写入**：状态更新是事务性的，防止部分损坏。
- **一致读取**：不会从部分写入文件中产生 torn reads。
- **Schema 演进**：可以通过迁移添加新字段，而不是添加新文件。
- **单一事实来源**：所有元数据位于一个数据库中，简化备份和恢复。

### 这与 Git 和 jj 如何比较？

Git 的 rebase 功能丰富，包含交互模式、autosquash、`--onto`、`--exec`、`--rebase-merges` 等。它是 Git 中最复杂的命令之一，在冲突解决和状态管理方面有大量边缘场景。

jj 采取根本不同的方法：历史默认不可变，没有传统 rebase 命令。虽然存在 `jj rebase`，但它直接作用于修订 DAG，将修订及其后代移动到新父级。冲突记录在提交自身中，而不是停止流程，因此没有 `--continue`/`--abort` 流程。

Libra 提供折中方案：带 conflict-stop 语义的线性 rebase（Git 用户熟悉），同时使用 SQLite-backed 状态持久化以提高可靠性。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| Upstream | `<upstream>`（位置参数） | `<upstream>`（位置参数） | `-d` / `--destination` |
| Continue | `--continue` | `--continue` | N/A（冲突存储在提交中） |
| Abort | `--abort` | `--abort` | `jj op undo` |
| Skip | `--skip` | `--skip` | N/A |
| Interactive | 不支持 | `-i` / `--interactive` | N/A |
| Onto | `--onto <newbase>` | `--onto <newbase>` | 带 `-s` / `--source` 的 `-d` |
| Exec | 不支持 | `--exec <cmd>` | N/A |
| Autosquash | 支持（`--autosquash`） | `--autosquash` | N/A |
| Autostash | `--no-autostash`（no-op；从不 autostash）；`--autostash` 不支持 | `--autostash` / `--no-autostash` | N/A |
| Rerere autoupdate | `--no-rerere-autoupdate`（no-op；无 rerere）；`--rerere-autoupdate` 不支持 | `--rerere-autoupdate` / `--no-rerere-autoupdate` | N/A |
| Rebase merges | 不支持 | `--rebase-merges` | 默认行为 |
| Keep empty | `--keep-empty`（no-op；默认已保留空提交）；`--no-keep-empty` 不支持 | `--keep-empty` / `--no-keep-empty` | 默认保留空提交 |
| Force rebase | 不支持 | `--force-rebase` | N/A |
| Branch | `<branch>`（第三个位置参数） | `<branch>`（第三个位置参数） | `-s` / `--source` |
| Revision set | 不支持 | N/A | `-r` / `--revisions` |
| 状态持久化 | SQLite 数据库 | `.git/rebase-merge/` 目录 | 不适用 |

注意：jj 在 rebase 期间不会因冲突停止。相反，冲突会 materialize 到提交内容中，并可稍后解决，因此不需要 `--continue`/`--abort`/`--skip`。

## 错误处理

`execute_safe` 当前对 CLI/preflight 失败返回标准结构化 `CliError` 信封。更深层 replay 引擎仍是 legacy text 路径，并作为待完成结构化输出工作跟踪。

| 场景 | StableErrorCode | 退出码 | 行为 |
|----------|-----------------|------|----------|
| 不是 libra 仓库 | `LBR-REPO-001`（RepoNotFound） | 128 | 以 repo-not-found 消息报错 |
| 缺少 upstream | `LBR-CLI-002`（CliInvalidArgument） | 129 | 来自 clap 的用法错误 |
| Upstream ref 无法解析 | `LBR-CLI-003`（CliInvalidTarget） | 129 | 报告 ref 无效的错误 |
| 没有进行中 rebase 却 `--continue` | `LBR-REPO-003`（RepoStateInvalid） | 128 | 报告没有进行中 rebase |
| `--continue` 仍有未解决冲突 | `LBR-CONFLICT-001`（ConflictUnresolved） | 128 | 报告冲突必须用 `libra add <file>` 暂存 |
| 没有进行中 rebase 却 `--abort` | `LBR-REPO-003`（RepoStateInvalid） | 128 | 报告没有进行中 rebase |
| 没有进行中 rebase 却 `--skip` | `LBR-REPO-003`（RepoStateInvalid） | 128 | 报告没有进行中 rebase |
| `--skip` 但没有已停止或待处理提交 | `LBR-REPO-003`（RepoStateInvalid） | 128 | 报告没有可跳过提交 |
| 找不到共同祖先 | 待定类型映射 | 128 | Legacy text 错误，拒绝 rebase 无关历史 |
| 提交重放期间冲突 | 待定类型映射 | 128 | Rebase 停止，状态已保存，提示用户解决 |
| 无法创建 rebased 提交 | 待定类型映射 | 128 | 带提交详情的 legacy text 错误 |
| 无法更新分支引用 | 待定类型映射 | 128 | 带 ref 更新详情的 legacy text 错误 |
