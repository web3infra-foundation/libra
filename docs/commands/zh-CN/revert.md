# `libra revert`

通过创建新的反向提交来回滚一个或多个已有提交。

## 概要

```bash
libra revert [options] <commit>...
libra revert --continue
libra revert --skip
libra revert --abort
libra revert --quit
```

## 说明

`libra revert` 会应用已有提交的反向变更，但不会重写历史。干净回滚会在当前 `HEAD` 之上创建新的 revert 提交；`-n` / `--no-commit` 只对单个提交应用反向变更到索引和工作树，不自动提交。

位置参数可以是单个提交引用，也可以是 `HEAD~3..HEAD` 这样的双点范围。范围按 newest-first 顺序回滚，与 Git revert 的顺序一致。重复提交只处理第一次出现的条目。

如果发生冲突，Libra 会写入冲突标记、记录非零 index stage，并把进行中的操作持久化到 `.libra/libra.db` 的 `revert_sequence`。解决文件冲突后运行 `libra add <path>`，再运行 `libra revert --continue`。也可以使用 `--abort`、`--skip` 或 `--quit` 控制序列。

支持 detached `HEAD`。在 detached 模式下，生成的 revert 提交会直接前推 detached `HEAD`，不会更新分支。

## 选项

### `<commit>...`

一个或多个要回滚的提交或 `A..B` 范围。

```bash
libra revert HEAD
libra revert abc1234 def5678
libra revert HEAD~3..HEAD
```

### `-n`, `--no-commit`

只把单个提交的反向变更应用到索引和工作树，不创建提交。

`--no-commit` 有意限制为单提交模式。与多个提交或范围组合会以 128 退出。

### `-m`, `--mainline <parent-number>`

回滚 merge commit 时，选择 1-based 父提交作为 mainline。

```bash
libra revert -m 1 <merge-commit>
```

merge commit 必须传 `-m`。对非 merge commit 传 `-m`，或选择超出父提交数量的编号，都会以 128 退出。

### `-s`, `--signoff`

为生成的 revert 提交追加 `Signed-off-by: <name> <email>` trailer。

### `-e`, `--edit`; `--no-edit`

为保持 Git 兼容的命令形状而接受。Libra 当前直接使用生成的提交消息；编辑器集成后续再补。

### `--continue`

冲突解决并暂存后，继续进行中的 revert 序列。

### `--skip`

跳过当前冲突提交，把工作树重置到该步骤开始时的状态，然后继续处理剩余提交。

### `--abort`

取消进行中的序列，并把已追踪文件和索引重置到序列开始时记录的原始 `HEAD`。未追踪文件会保留。

### `--quit`

只清除持久化的 revert 序列，保留当前工作树和索引不变。

### `--json`

输出标准 Libra JSON envelope。

## 常用命令

```bash
# 回滚最新提交
libra revert HEAD

# 按 newest-first 回滚一个范围
libra revert HEAD~3..HEAD

# 以第一个父提交为 mainline 回滚 merge commit
libra revert -m 1 <merge-commit>

# 解决冲突、暂存并继续
libra add conflicted.txt
libra revert --continue

# 取消进行中的 revert 序列
libra revert --abort
```

## 人类可读输出

干净自动提交：

```text
[def5678] Revert commit abc1234
```

No-commit 模式：

```text
Changes staged for revert. Use 'libra commit' to finalize.
```

序列控制：

```text
revert sequence continued
revert skipped current commit
revert aborted; HEAD reset to abc1234
revert state cleared; working tree left unchanged
```

## 结构化输出

单个干净回滚：

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
    "short_new": "def5678",
    "no_commit": false,
    "files_changed": 1,
    "reverted_commits": [
      "abc1234abcdef1234567890abcdef1234567890ab"
    ],
    "new_commits": [
      "def5678abcdef1234567890abcdef1234567890ab"
    ]
  }
}
```

使用 `--no-commit` 时，`new_commit` 和 `short_new` 为 `null`。

序列控制输出会额外包含 `action`；`--abort` 还会包含 `restored_head`。

## 兼容性

`libra revert` 是 partial Git 兼容。已支持：多个提交、`A..B` 范围、detached `HEAD`、单提交 `-n`、merge revert 的 `-m`、`--signoff`、冲突 sequencer 控制、JSON 输出和 quiet 输出。

暂缓支持：策略选择（`--strategy`、`-X`）、外部 GPG 签名（`-S` / `--gpg-sign`）、`--cleanup`、`--commit`、`--rerere-autoupdate`、`--reference`、`--edit` 的编辑器启动，以及 Git 的完整 `--no-*` 别名集合。

## 错误处理

| 代码 | 条件 | 提示 |
|------|------|------|
| `LBR-REPO-001` | 不在 Libra 仓库内 | 使用 `libra init` 初始化或进入仓库 |
| `LBR-REPO-003` | revert 状态冲突、无进行中序列，或已有活动序列 | 使用 `--continue`、`--skip`、`--abort` 或 `--quit` 完成/清理 |
| `LBR-CLI-003` | 无法解析提交引用 | 使用 `libra log` 查找有效提交 |
| `LBR-CLI-002` | 参数无效、mainline 无效，或 `--no-commit` 与多个提交组合 | 调整参数 |
| `LBR-CONFLICT-001` | 存在 revert 冲突 | 解决冲突，运行 `libra add`，再运行 `libra revert --continue` |
| `LBR-IO-001` | 无法加载对象或序列 | 检查仓库完整性 |
| `LBR-IO-002` | 无法保存对象、索引、序列或更新 `HEAD` | 检查文件系统和数据库可写性 |
