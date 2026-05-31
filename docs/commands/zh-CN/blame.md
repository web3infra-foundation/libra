# `libra blame`

追踪文件每一行最后由哪个提交引入。

## 概要

```
libra blame <file> [<commit>] [-L <range>]
```

## 说明

`libra blame` 为文件每一行标注最后修改该行的提交哈希、作者名、日期和行号。它从指定修订（默认 HEAD）开始，沿父提交向后遍历提交历史，并使用 diff 操作将行归因到最早引入它们的提交。

输出格式匹配 Git 的 blame 格式以保持熟悉度：每行包含短哈希、作者名（截断为 15 个字符）、日期、行号和行内容。

对于大文件，`-L` 选项可将输出限制到特定行范围，减少计算时间和输出量。

## 选项

| 选项 | 短选项 | 长选项 | 说明 |
|--------|-------|------|-------------|
| File | | 位置参数（必需） | 要 blame 的文件。必须存在于指定修订中。 |
| Commit | | 位置参数（可选） | blame 的起始修订。默认为 `HEAD`。 |
| Line range | `-L` | `-L <RANGE>` | 将 blame 限制到一个行范围。格式见下方。 |
| JSON | | `--json` | 输出结构化 JSON。 |
| Quiet | | `--quiet` | 验证输入但抑制所有 blame 输出。 |

### 行范围格式（`-L`）

`-L` 标志支持三种格式：

| 格式 | 含义 | 示例 |
|--------|---------|---------|
| `N` | 单行 N | `-L 10` |
| `N,M` | 第 N 到第 M 行（包含两端） | `-L 10,20` |
| `N,+C` | 从第 N 行开始的 C 行 | `-L 10,+5`（第 10-14 行） |

行号从 1 开始。越界值会产生错误。

```bash
# Blame 单行
libra blame -L 42 src/main.rs

# Blame 一个范围
libra blame -L 10,20 src/main.rs

# 从第 100 行开始 blame 5 行
libra blame -L 100,+5 src/main.rs
```

## 常用命令

```bash
# Blame HEAD 中的文件
libra blame src/main.rs

# 在特定提交上 blame
libra blame src/main.rs abc1234

# Blame 第 10-20 行
libra blame -L 10,20 src/main.rs

# 从第 10 行开始 blame 5 行
libra blame -L 10,+5 src/main.rs

# 面向代理的 JSON 输出
libra --json blame src/main.rs
```

## 人类可读输出

```text
abc12345 (Author Name     2026-03-30 10:00:00 +0800 1) line content
def67890 (Other Author    2026-03-28 14:30:00 +0800 2) another line
abc12345 (Author Name     2026-03-30 10:00:00 +0800 3) third line
```

每一行显示：
- **短哈希**（8 个字符）：最后更改此行的提交。
- **作者名**（填充到 15 个字符，过长时用 `...` 截断）。
- **日期**：以本地时区格式化为 `YYYY-MM-DD HH:MM:SS +ZZZZ`。
- **行号**：文件中的 1-based 行号。
- **内容**：实际行内容。

`--quiet` 会验证修订、文件和行范围，但抑制所有输出。这适合脚本检查（“此文件在此修订中是否存在？”）。

连接到终端时，输出会自动分页。

## 结构化输出（JSON）

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "tracked.txt",
    "revision": "abc123...",
    "lines": [
      {
        "line_number": 1,
        "short_hash": "abc12345",
        "hash": "abc123...",
        "author": "Test User",
        "date": "2026-03-30T10:00:00+00:00",
        "content": "tracked"
      }
    ]
  }
}
```

`revision` 字段包含作为 blame 起点的完整提交哈希。每个行条目同时包含 `short_hash`（8 个字符）和完整 `hash`，便于程序使用。

当文件为空时，`lines` 数组为空，人类输出显示 "File is empty"。

## 设计理由

### 为什么没有 `--reverse`？

Git 的 `blame --reverse` 会显示一行最后存在于哪个修订中，即向前遍历历史而不是向后。这对寻找一行何时被*删除*很有用，但它需要向前历史遍历，计算代价高，并且在架构上不同于普通 blame。Libra 省略此功能，以保持 blame 实现简单快速。要查找一行何时被删除，请使用 `libra log -p -- <file>` 并搜索删除。

### 为什么使用简化的行范围格式？

Git 的 `-L` 支持复杂格式，包括基于正则的函数匹配（`-L :<funcname>`）和 `/regex/` 行选择。这些功能很强大，但依赖语言特定配置（`.gitattributes` `diff` driver），且很少被正确使用。Libra 仅支持数字范围（`N`、`N,M`、`N,+C`），这些格式无歧义，并足以覆盖绝大多数 blame 操作。AI 代理可以很容易地从文件内容确定行号，而无需基于正则的函数匹配。

### 为什么默认 HEAD 而不是工作树？

Git 的 blame 默认使用 HEAD，并要求 `git blame --contents <file>` 才能 blame 工作树版本。Libra 遵循相同约定：blame 始终作用于已提交内容。这保证结果可复现，同一提交上的同一命令总是产生相同输出，而不受工作树状态影响。

### 为什么提交参数是位置参数而不是标志？

提交参数是位置参数（文件路径后的第二个参数），而不是 `--commit` 或 `--rev` 这样的标志。这匹配 Git 语法，保持熟悉度，并使常见场景（`libra blame file.rs`）简洁。由于文件路径始终是第一个位置参数，因此没有歧义。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| 文件 | `<file>`（位置参数，必需） | `<file>`（位置参数，必需） | N/A（jj 没有 blame；使用 `jj annotate`） |
| 修订 | `<commit>`（位置参数，默认 HEAD） | `<rev>`（位置参数，默认 HEAD） | `-r <revision>`（在 `jj annotate` 中） |
| 行范围（数字） | `-L N,M` / `-L N,+C` / `-L N` | `-L <start>,<end>` | N/A |
| 行范围（正则） | 不支持 | `-L :<funcname>` / `-L /regex/` | N/A |
| Reverse blame | 不支持 | `--reverse` | N/A |
| 显示 email | 不支持 | `-e` / `--show-email` | N/A |
| 显示时间戳 | 默认包含 | `-t`（原始时间戳） | N/A |
| Porcelain 格式 | 不支持 | `--porcelain` / `--line-porcelain` | N/A |
| 增量输出 | 不支持 | `--incremental` | N/A |
| 评分阈值 | 不支持 | `-M` / `-C`（移动/复制检测） | N/A |
| 忽略修订 | 不支持 | `--ignore-rev` / `--ignore-revs-file` | N/A |
| 工作树内容 | 不支持 | `--contents <file>` | N/A |
| 日期格式 | 不支持（固定） | `--date <format>` | N/A |
| 编码 | 不支持 | `--encoding <encoding>` | N/A |
| JSON 输出 | `--json` | 不支持 | 不支持 |
| Quiet 模式 | `--quiet` | 不支持 | N/A |
| 分页器 | 自动 | 可配置 | 可配置 |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 仓库外部 | `LBR-REPO-001` | 128 |
| 无效修订或缺失文件 | `LBR-CLI-003` | 129 |
| 无效 `-L` 范围 | `LBR-CLI-002` | 129 |
| 无法读取提交或对象 | `LBR-REPO-002` | 128 |
