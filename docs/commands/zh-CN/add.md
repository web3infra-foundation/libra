# `libra add`

为下一次提交暂存文件内容。

## 概要

```
libra add [OPTIONS] [PATHSPEC...]
libra add -A
libra add -u [PATHSPEC...]
libra add --refresh [PATHSPEC...]
```

## 说明

`libra add` 将工作树中的文件更改暂存到索引中，为下一次 `libra commit` 做准备。它支持 pathspec、glob 模式、`--dry-run` 预览，以及用 `--refresh` 对已跟踪条目重新 stat 而不暂存新内容。

该命令相对于当前工作目录解析 pathspec，验证它们位于仓库根内，并遵守 `.libraignore` 规则。由 LFS 跟踪的文件会自动作为指针文件暂存。`-A` 标志会暂存整个工作树中的所有更改（新增、修改、删除），而 `-u` 只更新已跟踪文件，不添加新文件。

## 选项

### `[PATHSPEC...]`

要暂存的一个或多个文件或目录。路径相对于当前目录解析。除非指定 `-A`、`-u` 或 `--refresh`，否则必需。

```bash
libra add file.txt
libra add src/ tests/
libra add .
```

### `-A, --all`

更新索引以匹配整个工作树。暂存新文件、修改和删除。不带 pathspec 时，会更新工作树中的所有文件。与 `-u` 和 `--refresh` 互斥。

```bash
libra add -A
```

### `-u, --update`

只更新索引中已有并匹配 pathspec 的条目。暂存已跟踪文件的修改和删除，但不添加新（未跟踪）文件。与 `-A` 和 `--refresh` 互斥。

```bash
libra add -u
libra add -u src/
```

### `--refresh`

刷新索引中当前所有文件的条目。只更新已有索引条目的元数据（时间戳、文件大小）以匹配工作树，不添加新文件，也不移除条目。与 `-A` 和 `-u` 互斥。

```bash
libra add --refresh
```

### `-f, --force`

允许添加本来会被 `.libraignore` 忽略的文件。

```bash
libra add -f ignored_file.log
```

### `-n, --dry-run`

预览会暂存什么，但不实际修改索引。输出显示哪些文件会被添加、修改或移除。

```bash
libra add -n file.txt
libra add --dry-run .
```

### `-v, --verbose`

产生更详细输出，显示暂存期间的逐文件动作。

```bash
libra add -v src/
```

### `--ignore-errors`

当单个路径失败时继续暂存剩余文件。失败路径会在输出中报告，但不会导致命令以错误退出。

```bash
libra add --ignore-errors src/
```

## 常用命令

```bash
libra add file.txt
libra add src/
libra add .
libra add -n file.txt
libra add --refresh
libra add --ignore-errors src/
```

## 人类可读输出

默认人类模式将暂存摘要写到 `stdout`。

单个文件：

```text
add 'src/main.rs' (new file)
```

多个文件：

```text
add 'src/main.rs' (new file)
add 'src/lib.rs' (modified)
add 'old.txt' (deleted)
```

Dry-run：

```text
add 'src/main.rs' (new file)
add 'src/lib.rs' (modified)
(dry run, no files were staged)
```

被忽略文件会在 `stderr` 上产生 warning：

```text
warning: all specified paths are ignored by .libraignore
Hint: use '-f' to force staging of ignored files
```

`--quiet` 会抑制所有 `stdout` 输出，但保留 `stderr` warnings。

## 结构化输出

`libra add` 支持全局 `--json` 和 `--machine` 标志。

- `--json` 向 `stdout` 写入一个成功信封
- `--machine` 以紧凑单行 JSON 写入相同 schema
- 成功时 `stderr` 保持干净

示例：

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": ["src/lib.rs"],
    "removed": ["old.txt"],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

Dry-run：

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": true
  }
}
```

使用 `--ignore-errors` 的部分失败：

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["good.txt"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [
      {"path": "bad.bin", "message": "file too large"}
    ],
    "dry_run": false
  }
}
```

### Schema 说明

- `added` / `modified` / `removed` 对应已暂存的新文件、变更文件和删除文件
- `refreshed` 仅在使用 `--refresh` 时填充
- `ignored` 列出被 `.libraignore` 跳过的路径
- `failed` 列出暂存失败的路径，每个包含 `path` 和 `message`
- 传递 `-n` / `--dry-run` 时 `dry_run` 为 `true`；不会实际暂存文件

## 设计理由

### 没有 `--intent-to-add` / `-N`

Git 的 `--intent-to-add`（`-N`）会为未跟踪文件记录空 blob，使它们出现在 `git diff` 输出中，但不真正暂存其内容。这是为了在暂存前审查新文件的工作流便利。Libra 省略该标志，因为 `libra status` 已经清楚显示未跟踪文件，且 `libra diff` 设计为配合完整工作树状态工作。“intent 然后 stage”的两步工作流增加认知负担，却没有显著改善审查体验。想在提交前审查新文件的用户可以使用 `libra add --dry-run`，暂存后再使用 `libra diff --staged`。

### 没有 `--patch` / `-p` 交互式暂存

Git 的 `--patch` 模式在终端内提供逐 hunk 的交互式暂存接口。Libra 有意从 CLI `add` 命令中省略交互式暂存，因为 `libra code` TUI 提供更丰富的可视暂存体验，支持完整文件和 hunk 选择。交互式终端提示也不兼容 AI 代理工作流（MCP/stdio 模式），这是 Libra 的主要设计目标。保持 `libra add` 非交互，确保它在人类、脚本和代理上下文中行为一致。

### `--refresh` 作为显式标志

在 Git 中，`git add --refresh` 会静默更新已跟踪文件的 stat 信息。Libra 将其作为一等模式暴露，并与 `-A` 和 `-u` 互斥（由 clap 参数组强制）。这让意图明确：`--refresh` 永远不暂存新内容，只更新元数据。互斥性避免 `-A --refresh` 这种意图模糊的组合。

### `.libraignore` 而不是 `.gitignore`

Libra 使用 `.libraignore` 文件作为 ignore 策略，而不是 `.gitignore`。这避免 Libra 仓库与 Git 仓库共存或从 Git 仓库转换时发生冲突，并清楚表明哪个 VCS 拥有 ignore 规则。Ignore 文件格式与 Git 的模式语法兼容（glob、用 `!` 取反、以 `/` 结尾的目录专用模式）。`libra init` 会在非 bare 仓库中创建根 `.libraignore`，Git 导入或非 bare clone 会将已有 `.gitignore` 文件复制为匹配的 `.libraignore` 文件。

## 参数对比：Libra vs Git vs jj

| 参数 / 标志 | Git | jj | Libra |
|---|---|---|---|
| 暂存文件 | `git add file.txt` | N/A（jj 自动跟踪） | `libra add file.txt` |
| 暂存所有内容 | `git add .` 或 `git add -A` | N/A（自动） | `libra add .` 或 `libra add -A` |
| 只更新已跟踪 | `git add -u` | N/A | `libra add -u` |
| Dry-run 预览 | `git add -n` / `--dry-run` | N/A | `libra add -n` / `--dry-run` |
| 强制添加被忽略文件 | `git add -f` | N/A | `libra add -f` |
| 刷新 stat 信息 | `git add --refresh` | N/A | `libra add --refresh` |
| Verbose 输出 | `git add -v` | N/A | `libra add -v` |
| 忽略错误 | `git add --ignore-errors` | N/A | `libra add --ignore-errors` |
| Intent to add | `git add -N` / `--intent-to-add` | N/A | N/A（未实现） |
| 交互式 patch | `git add -p` / `--patch` | N/A | N/A（使用 `libra code` TUI） |
| 交互式选择 | `git add -i` / `--interactive` | N/A | N/A（使用 `libra code` TUI） |
| 暂存前编辑 diff | `git add -e` / `--edit` | N/A | N/A |
| 仅 chmod | `git add --chmod=+x` | N/A | N/A |
| Sparse checkout 路径 | `git add --sparse` | N/A | N/A |
| Ignore 文件 | `.gitignore` | N/A（jj 使用 `.gitignore`） | `.libraignore` |
| 结构化 JSON 输出 | N/A | N/A | `--json` / `--machine` |
| 错误提示 | 最少 | N/A | 每种错误类型都有可操作提示 |

## 错误处理

每个 `AddError` 变体都会映射到显式 `StableErrorCode`。

| 场景 | 错误码 | 退出码 | 提示 |
|----------|-----------|------|------|
| 不在仓库内 | `LBR-REPO-001` | 128 | "run 'libra init' to create a repository" |
| Pathspec 没有匹配 | `LBR-CLI-003` | 129 | "check the spelling and use 'libra status' to see what changed" |
| 路径在仓库根外 | `LBR-CLI-003` | 129 | "only files within the repository root can be staged" |
| 无效路径编码 | `LBR-CLI-003` | 129 | "path contains invalid UTF-8 characters" |
| 索引文件损坏 | `LBR-REPO-002` | 128 | "the index file may be corrupted; try 'libra status' to verify" |
| 无法保存索引 | `LBR-IO-002` | 128 | "check disk space and file permissions" |
| Refresh 失败 | `LBR-IO-001` | 128 | -- |
| 条目创建失败 | `LBR-IO-002` | 128 | -- |
| 工作目录错误 | `LBR-REPO-001` | 128 | "cannot determine the working tree" |
| 状态计算失败 | `LBR-REPO-002` | 128 | -- |
| 所有路径都被忽略（未暂存任何内容） | `LBR-ADD-001` | 128 | "use -f if you really want to add them" |
| 无 pathspec 且无模式标志 | `LBR-CLI-001` | 129 | "maybe you wanted to say 'libra add .'?" |

## 兼容性说明

- jj 没有 `add` 命令；它自动跟踪所有工作树更改
- Libra 的 `add` 是 `commit` 前必需步骤，匹配 Git 的显式暂存模型
- `.libraignore` 使用与 `.gitignore` 相同的模式语法，但它是单独文件；导入和非 bare clone 会复制 `.gitignore` 规则，而不是删除或重命名原文件
- LFS 跟踪文件会在暂存期间自动转换为指针文件
