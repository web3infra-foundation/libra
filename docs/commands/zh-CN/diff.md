# `libra diff`

比较 HEAD、索引、工作树或两个修订之间的差异。

## 概要

```
libra diff [<pathspec>...]
libra diff --staged [<pathspec>...]
libra diff --old <commit> --new <commit> [<pathspec>...]
libra diff [--name-only | --name-status | --numstat | --stat | --shortstat | --summary]
           [-s | --no-patch] [--exit-code] [--check] [-R] [-z]
libra diff [--algorithm <name>] [--output <file>]
```

## 说明

`libra diff` 显示仓库不同状态之间的更改。默认情况下，它比较索引和工作树（未暂存更改）。使用 `--staged` 时，它比较 HEAD 和索引（已暂存更改）。使用 `--old` 和 `--new` 时，它比较两个任意提交。

Diff 引擎支持多种算法（默认 histogram，myers 和 myersMinimal 作为替代）。输出可以通过 `--output` 写入文件，并提供若干摘要格式（`--name-only`、`--name-status`、`--numstat`、`--stat`、`--shortstat`、`--summary`）。可用 `-s`/`--no-patch` 配合 `--exit-code` 做仅状态检查；`-z`/`--null` 让 name/numstat 输出以 NUL 终止，便于安全脚本解析。

Pathspec 参数会将 diff 过滤为只显示匹配文件或目录中的更改。

## 选项

| 选项 | 短选项 | 长选项 | 说明 |
|--------|-------|------|-------------|
| Old commit | | `--old <COMMIT>` | 指定比较的“旧”侧。使用 `--staged` 时默认为 HEAD，否则默认为索引。 |
| New commit | | `--new <COMMIT>` | 指定“新”侧。需要 `--old`。与 `--staged` 冲突。 |
| Staged | | `--staged` | 比较 HEAD 和索引（已暂存更改）。与 `--new` 冲突。 |
| Pathspec | | 位置参数 | 一个或多个文件或目录，用于限制 diff。 |
| Algorithm | | `--algorithm <name>` | Diff 算法：`histogram`（默认）、`myers` 或 `myersMinimal`。 |
| Output file | | `--output <FILENAME>` | 将人类可读输出写入文件而不是 stdout。在 `--json` 模式中忽略。 |
| Name only | | `--name-only` | 只显示已更改文件名。 |
| Name status | | `--name-status` | 显示已更改文件名和状态字母（A/D/M）。 |
| Numstat | | `--numstat` | 以机器友好的制表符分隔格式显示插入/删除数量。 |
| Stat | | `--stat` | 显示带 +/- 条形图的 diffstat 摘要。 |
| 上下文行数 | `-U<n>` | `--unified=<n>` | patch 中每处变更周围的上下文行数（默认 3）。只改变周围上下文、不改变 `+`/`-` 行，故 `--stat`/`--name-only`/`--numstat` 计数不受影响；`--json` 的 hunk 范围与行数组随 `<n>` 变化。 |
| 忽略空白 | `-w` | `--ignore-all-space` | 比较行时忽略所有空白。仅空白的变更不再报告（若这是文件唯一的变更则该文件不出现）；上下文行取新一侧。受影响文件会重新 diff，故 `--stat`/`--name-only`/`--numstat`/JSON 都反映忽略空白后的结果。遵循 `-U<n>`。 |
| 忽略空白数量 | `-b` | `--ignore-space-change` | 只忽略空白*数量*的变化：连续空白视为单个空格、忽略行尾空白，但空白的有无仍然重要（`a  b` 等于 `a b`；`a b` 仍不同于 `ab`）。重新 diff/丢弃行为同 `-w`。二者同时给出时 `-w` 优先。 |
| 忽略行尾空白 | | `--ignore-space-at-eol` | 只忽略行尾空白变化；前导与内部空白精确比较。重新 diff/丢弃行为同 `-w`。与 `-w`/`-b` 组合时后者优先。 |
| 忽略空白行 | | `--ignore-blank-lines` | 忽略全为空白（真正空）行的变更：仅由增删空行构成的变更不报告（若新增/删除的文件内容全为空行，仍以零计数列出该文件），而紧邻真实编辑的空行则完整显示。重新 diff 受影响文件（故 `--stat`/`--name-only`/`--numstat`/JSON 反映结果）；遵循 `-U<n>`。与空白标志（`-w`/`-b`/`--ignore-space-at-eol`）复合：经空白归一化后为空的行即视为空行。 |
| Shortstat | | `--shortstat` | 只显示 `--stat` 的汇总行（文件数/插入/删除），零项省略对应子句。 |
| Summary | | `--summary` | 显示创建/删除文件的精简摘要（纯内容修改不产生行）。Libra 的 diff 不检测重命名（显示为 delete+create），也不暴露纯 mode 变更。 |
| No patch | `-s` | `--no-patch` | 抑制 patch（diff 主体）。与 `--exit-code` 组合做状态检查。 |
| 空白检查 | | `--check` | 不输出 diff，而是对新增行的空白错误（尾随空白、indent 中 space-before-tab）告警，打印 `<path>:<line>: <message>`，发现即退出码 2。不检测 Git 的 blank-at-eof；优先于其他输出模式。 |
| 反向 | `-R` | `--reverse` | 交换两侧，使新增变删除、删除变新增（即可撤销该变更的 patch）。 |
| 文本 | `-a` | `--text` | 把所有文件按文本处理。接受式 no-op：Libra 的 diff 从不检测二进制文件，始终输出内容 diff（从不打印 “Binary files differ”）。与 `--binary`（二进制 patch 格式，尚未支持）不同。 |
| 禁用外部 diff | | `--no-ext-diff` | 禁止外部 diff 驱动。接受式 no-op：Libra 无外部 diff 驱动，始终使用内建引擎。（外部 diff 工具本身 `--ext-diff` / `diff.external` 不支持。） |
| 不对移动行着色 | | `--no-color-moved` | 不对移动行单独着色。接受式 no-op：Libra 的 diff 从不检测/着色移动行。（Git 的 `--color-moved` 不支持。） |
| 不检测重命名 | | `--no-renames` | 关闭重命名检测。接受式 no-op：Libra 的 diff 从不检测重命名（重命名显示为 delete+create）。（Git 的 `--renames`/`-M` 不支持。） |
| 不用相对路径 | | `--no-relative` | 显示仓库根相对路径而非 cwd 相对。接受式 no-op：Libra 的 diff 始终用仓库根相对路径。（Git 的 `--relative` 不支持。） |
| 不用 indent 启发式 | | `--no-indent-heuristic` | 禁用 hunk 边界的 indent 启发式。接受式 no-op：Libra 的 diff 不使用 Git 的 indent 启发式。（Git 的 `--indent-heuristic` 不支持。） |
| 不用 textconv | | `--no-textconv` | 不运行 textconv 过滤器把二进制文件转为可 diff。接受式 no-op：Libra 的 diff 无 textconv 过滤器，始终 diff 原始内容。（Git 的 `--textconv` 不支持。） |
| Exit code | | `--exit-code` | 仍打印 diff，但存在差异时退出码为 1（否则 0）。区别于 `--quiet`，不抑制 diff。 |
| NUL 输出 | `-z` | `--null` | 对 `--name-only`/`--name-status`/`--numstat` 用 NUL 终止每条记录（`--name-status` 的状态与路径以 NUL 分隔）；其他模式不受影响。 |
| JSON | | `--json` | 输出结构化 JSON。 |
| Quiet | | `--quiet` | 抑制 stdout；存在差异时退出码为 1，否则为 0。与 `--output` 组合时，文件仍会被写入。 |

### 选项细节

**`--old` / `--new`**

比较两个特定提交。指定 `--new` 时也必须指定 `--old`：

```bash
# 比较两个提交
libra diff --old HEAD~3 --new HEAD

# 比较标签和 HEAD
libra diff --old v1.0 --new HEAD
```

**`--staged`**

显示已为下一次提交暂存的内容：

```bash
libra diff --staged
libra diff --staged src/
```

**`--algorithm`**

选择 diff 算法。Histogram（默认）通常为代码生成更可读的 diff：

```bash
libra diff --algorithm myers
libra diff --algorithm myersMinimal
```

**`--output`**

将 diff 输出写入文件。适合保存 diff 以供评审：

```bash
libra diff --output changes.patch
libra diff --staged --output staged.diff
```

**摘要格式：**

```bash
# 仅文件名
libra diff --name-only

# 文件名和状态字母
libra diff --name-status
# Output: M	src/main.rs
#         A	src/new_file.rs

# 机器友好的数量
libra diff --numstat
# Output: 5	2	src/main.rs

# 可视条形图
libra diff --stat
# Output:  src/main.rs | 7 +++++--
```

## 常用命令

```bash
# 显示未暂存更改
libra diff

# 显示已暂存更改
libra diff --staged

# 比较两个提交
libra diff --old HEAD~1 --new HEAD

# 显示子目录的 diff 统计
libra diff --stat src/

# 使用不同的上下文行数（0，或多于默认的 3）
libra diff -U0
libra diff --unified=5 src/main.rs

# 忽略仅空白的变更（重新缩进不会显示）
libra diff -w

# 只忽略空白数量的变化（a  b == a b）
libra diff -b

# 忽略仅由空白行构成的变更
libra diff --ignore-blank-lines

# 将 diff 保存到文件
libra diff --output my.patch

# 面向代理的 JSON 输出
libra --json diff --staged
```

## 人类可读输出

支持的输出模式：

- 默认 unified diff（检测到终端时带 ANSI 颜色）
- `--name-only`
- `--name-status`
- `--numstat`
- `--stat`
- `--shortstat`（只有 `--stat` 的汇总行，零项子句省略）
- `--summary`（精简的 create/delete 摘要；重命名显示为 delete+create，不暴露纯 mode 变更）
- `-s` / `--no-patch` 抑制 patch 主体（用于仅状态检查）
- `-z` / `--null` 对 `--name-only`/`--name-status`/`--numstat` 用 NUL 终止记录（`--name-status` 的状态与路径分为独立 NUL 字段）
- `--check` 对新增行检测空白错误（尾随空白、indent 中 space-before-tab），打印 `<path>:<line>: <message>`，发现即退出码 2（不检测 Git 的 blank-at-eof）
- `-R` / `--reverse` 交换两侧得到反向 diff（新增↔删除）
- `-a` / `--text` 把所有文件按文本处理；接受式 no-op，因为 Libra 从不做二进制检测，始终输出内容 diff（与输出二进制 patch 的 `--binary` 不同）
- `--no-ext-diff` 禁止外部 diff 驱动；接受式 no-op，因为 Libra 无外部 diff 驱动、始终用内建引擎（外部工具 `--ext-diff` / `diff.external` 不支持）
- `--exit-code` 仍打印 diff，但存在差异时退出码为 `1`
- `--quiet` 抑制 stdout，并用退出码 `1` 表示存在差异

`--output <file>` 将人类可读输出写入文件。在 `--quiet` 模式下仍会写入文件，但存在差异仍返回退出码 `1`。在 `--json` 模式下，该标志会被忽略，输出始终发送到 stdout。

连接到终端时，输出会自动分页。

## 结构化输出（JSON）

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "index",
    "new_ref": "working tree",
    "files": [
      {
        "path": "tracked.txt",
        "status": "modified",
        "insertions": 1,
        "deletions": 0,
        "hunks": [
          {
            "old_start": 1,
            "old_lines": 1,
            "new_start": 1,
            "new_lines": 2,
            "lines": [" tracked", "+updated"]
          }
        ]
      }
    ],
    "total_insertions": 1,
    "total_deletions": 0,
    "files_changed": 1
  }
}
```

`status` 字段是 `added`、`deleted`、`modified` 之一。

`old_ref` 和 `new_ref` 字段表示比较了什么（例如 `"index"`、`"working tree"`、`"HEAD"` 或提交引用）。

## 设计理由

### 为什么用 `--old` / `--new` 而不是位置提交参数？

Git 使用位置参数进行提交比较（`git diff HEAD~1 HEAD`），但这会与 pathspec 参数产生歧义。`git diff main src/` 是将 `main` 分支与 `src/` 比较，还是显示 `src/` 自 `main` 以来的更改？Git 用 `--` 分隔符解决此问题，但歧义仍是困惑来源。

Libra 使用显式具名标志（`--old`、`--new`）来消除所有歧义。任何位置参数始终都是 pathspec。对以编程方式构造命令的 AI 代理来说，这尤其有价值；每种意图只有一种表达方式。

### 为什么 histogram 是默认算法？

Git 出于历史原因默认使用 Myers 算法。Histogram 算法（在 Git 2.0 中作为选项引入）通常为源代码生成更可读的 diff，因为它更擅长识别移动块，并避免重复行带来的病态情况。Libra 默认使用 histogram，以获得更好的开箱质量。Myers 和 myersMinimal 仍可用于兼容性和边缘场景。

### 为什么没有 `--cached` 别名？

Git 同时支持 `--staged` 和 `--cached` 作为同义词。这种重复没有实际用途，并让文档更难搜索。Libra 将 `--staged` 标准化为唯一规范名称，匹配 `libra status` 和 `libra restore --staged` 中使用的术语。

### 为什么 `--new` 要求 `--old`？

允许只有 `--new` 而没有 `--old` 会产生模糊比较（new 与什么比较？）。当指定 `--new` 时要求 `--old`，让比较显式且可预测。对于与 HEAD 比较的常见场景，请使用 `--staged`。

### 为什么没有 `--word-diff` 或 `--color-words`？

这些 Git 选项提供替代 diff 呈现，对散文很有用，但对代码很少需要。Libra 专注于工具和 AI 代理普遍理解的 unified diff 格式。如果需求足够，词级 diff 可以作为未来增强加入。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| 未暂存更改 | `diff`（默认） | `diff`（默认） | `jj diff`（显示所有未提交更改） |
| 已暂存更改 | `--staged` | `--staged` / `--cached` | N/A（无暂存区） |
| 两个提交 | `--old <A> --new <B>` | `<A> <B>` 或 `<A>..<B>` | `--from <A> --to <B>` |
| Pathspec 过滤 | `<pathspec>...` | `-- <pathspec>...` | `<paths>...` |
| 算法 | `--algorithm`（histogram/myers/myersMinimal） | `--diff-algorithm`（patience/histogram/myers/minimal） | N/A（使用内部算法） |
| 输出到文件 | `--output <file>` | `--output <file>` | N/A（使用 shell redirect） |
| 仅名称 | `--name-only` | `--name-only` | `--name-only` |
| 名称和状态 | `--name-status` | `--name-status` | N/A |
| 数字统计 | `--numstat` | `--numstat` | `--stat`（组合） |
| Stat 摘要 | `--stat` | `--stat` | `--stat` |
| 短统计 | `--shortstat` | `--shortstat` | N/A |
| Summary | `--summary` | `--summary` | `--summary` |
| 抑制 patch | `-s` / `--no-patch` | `-s` / `--no-patch` | N/A |
| 退出码 | `--exit-code` | `--exit-code` | N/A |
| NUL 终止输出 | `-z` / `--null` | `-z` | N/A |
| 空白检查 | `--check`（尾随空白 / space-before-tab） | `--check` | N/A |
| 反向 diff | `-R` / `--reverse` | `-R` | N/A |
| 按文本处理 | `-a` / `--text`（no-op；始终显示） | `-a` / `--text` | N/A |
| Word diff | 不支持 | `--word-diff` / `--color-words` | N/A |
| Binary diff（二进制 patch） | 不支持 | `--binary` | N/A |
| 上下文行数 | `-U<n>` / `--unified=<n>`（默认 3） | `-U<n>` / `--unified=<n>` | `--context <n>` |
| 忽略空白 | `-w` / `--ignore-all-space` | `-w` / `--ignore-all-space` | N/A |
| 忽略空白数量 | `-b` / `--ignore-space-change` | `-b` / `--ignore-space-change` | N/A |
| 忽略行尾空白 | `--ignore-space-at-eol` | `--ignore-space-at-eol` | N/A |
| 忽略空白行 | `--ignore-blank-lines` | `--ignore-blank-lines` | N/A |
| 颜色 | 自动（终端检测） | `--color` / `--no-color` | `--color` / `--no-color` |
| 禁用外部 diff | `--no-ext-diff`（no-op；始终内建） | `--no-ext-diff` | N/A |
| 外部 diff 工具 | 不支持 | `--ext-diff` / `diff.external` | `--tool <name>` |
| Quiet（仅退出码） | `--quiet` | `--quiet` | N/A |
| JSON 输出 | `--json` | 不支持 | N/A |
| Rename 检测 | 不支持 | `-M` / `--find-renames` | 自动 |
| Copy 检测 | 不支持 | `-C` / `--find-copies` | N/A |
| Three-dot diff | 不支持 | `<A>...<B>`（merge base） | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 仓库外部 | `LBR-REPO-001` | 128 |
| 无效修订 | `LBR-CLI-003` | 129 |
| 无法读取索引或对象存储 | `LBR-REPO-002` | 128 |
| 无法读取文件 | `LBR-IO-001` | 128 |
| 无法写入输出文件 | `LBR-IO-002` | 128 |
