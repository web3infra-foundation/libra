# `libra shortlog`

按作者汇总可达提交。

**别名：** `slog`

## 概要

```
libra shortlog [<revision>] [-n] [-s] [-e] [--since <date>] [--until <date>]
```

## 说明

`libra shortlog` 汇总按作者分组的可达提交，主要用于发布公告和贡献者概览。它从指定修订（默认 HEAD）开始遍历提交图，并按作者聚合提交，显示每个作者的提交数量，以及可选的提交主题。

默认情况下，作者按姓名字母顺序排序。使用 `-n` 时，按提交数量降序排序。`-s` 标志生成只包含数量的摘要，抑制单个提交主题。`-e` 标志会在输出中包含作者 email 地址。

通过 `--since` 和 `--until` 的日期过滤会基于 committer 时间戳限制包含哪些提交，支持 `YYYY-MM-DD`、`"N days ago"` 和 Unix 时间戳等格式。

## 选项

| 选项 | 短选项 | 长选项 | 说明 |
|--------|-------|------|-------------|
| Numbered | `-n` | `--numbered` | 按每个作者的提交数量降序排序，而不是按字母顺序。 |
| Summary | `-s` | `--summary` | 抑制提交描述；只显示每个作者的提交数量。 |
| Email | `-e` | `--email` | 在作者名旁显示 email 地址。启用后，作者按 `name <email>` 对分组。 |
| Since | | `--since <date>` | 只包含比指定日期更新的提交。 |
| Until | | `--until <date>` | 只包含比指定日期更旧的提交。 |
| Revision | | 位置参数（可选） | 要从中汇总的修订。默认为 `HEAD`。 |
| JSON | | `--json` | 输出结构化 JSON。 |
| Quiet | | `--quiet` | 抑制人类可读输出。 |

### 选项细节

**`-n` / `--numbered`**

按提交数量降序排序作者。当两个作者数量相同时，按字母顺序排序：

```bash
$ libra shortlog -n
   5  Alice
   3  Bob
   1  Charlie
```

**`-s` / `--summary`**

产生只包含数量的紧凑输出，省略单个提交主题：

```bash
$ libra shortlog -s
   2  Test User
```

不使用 `-s` 时，提交主题列在每个作者下方：

```bash
$ libra shortlog
   2  Test User
      initial
      follow-up
```

**`-e` / `--email`**

将 email 地址追加到每个作者。启用后，同名但不同 email 的作者会分开列出：

```bash
$ libra shortlog -e
   2  Test User <test@example.com>
      initial
      follow-up
```

**`--since` / `--until`**

按 committer 时间戳过滤提交。支持的日期格式包括：

- `YYYY-MM-DD`（例如 `2026-01-01`）
- 相对日期（例如 `"7 days ago"`、`"2 weeks ago"`）
- Unix 时间戳

```bash
# 最近一个月的提交
libra shortlog --since "30 days ago"

# 某个日期范围内的提交
libra shortlog --since 2026-01-01 --until 2026-03-31
```

**Revision 参数**

指定 HEAD 以外的起点：

```bash
# 汇总最近 5 个提交
libra shortlog HEAD~5

# 从标签汇总
libra shortlog v1.0
```

## 常用命令

```bash
# 从 HEAD 生成默认 shortlog
libra shortlog

# 只显示数量摘要，并按数量排序
libra shortlog -n -s

# 包含 email 地址
libra shortlog -e

# 最近 5 个提交摘要
libra shortlog HEAD~5

# 日期范围内的提交
libra shortlog --since 2026-01-01 --until 2026-03-31

# 面向脚本的 JSON 输出
libra shortlog --json
```

## 人类可读输出

默认（按字母顺序，包含主题）：

```text
   2  Test User
      initial
      follow-up
```

摘要模式（`-s`）抑制主题。`-e` 会追加 `<email>`。

主题提取会跳过嵌入的签名头，并使用第一条有意义的提交消息行。

数量列会基于所有作者中的最大数量使用一致宽度右对齐。

## 结构化输出（JSON）

```json
{
  "ok": true,
  "command": "shortlog",
  "data": {
    "revision": "HEAD",
    "numbered": false,
    "summary": false,
    "email": false,
    "total_authors": 1,
    "total_commits": 2,
    "authors": [
      {
        "name": "Test User",
        "email": null,
        "count": 2,
        "subjects": ["initial", "follow-up"]
      }
    ]
  }
}
```

摘要模式下，`subjects` 是空数组。启用 `-e` 时，`email` 字段包含作者的 email 字符串；否则为 `null`。

`total_authors` 和 `total_commits` 字段为脚本和代理提供便捷聚合数量。

## 设计理由

### 为什么没有 `--group`？

Git 的 `shortlog --group=trailer:<key>` 和 `--group=author`/`--group=committer` 允许按不同提交元数据字段或 trailer 值分组。这是一个小众功能，主要用于分析 co-authored 提交或通过 `Signed-off-by` trailer 归属的提交。Libra 省略 `--group`，以保持命令专注于主要用例：按作者汇总贡献。shortlog 的压倒性常见用法是基于作者分组，支持任意分组需要通用聚合框架，会增加复杂度但价值不成比例。

### 为什么使用位置修订而不是管道输入？

Git 的 `shortlog` 可在两种模式下运行：从 stdin 读取经管道传入的 `git log` 输出，或直接遍历提交历史。管道模式（`git log | git shortlog`）是 Unix 哲学下的组合性功能，但它需要解析序列化提交数据，这既脆弱又依赖格式。

Libra 将修订作为位置参数，并始终直接从提交图读取。这更简单、更快（无需序列化/反序列化），并且与 `--json` 输出模式自然配合。对于超出 `--since`/`--until` 提供能力的过滤，请使用 `libra log --json` 加外部工具。

### 为什么使用 `--since`/`--until` 而不是完整 log 选项？

Git 的 `shortlog` 在直接使用时（非管道）继承完整 `git log` 选项集。这包括 `--author`、`--grep`、`--no-merges` 等几十个选项。Libra 只提供基于日期的过滤，因为它覆盖最常见的 shortlog 过滤需求（发布周期摘要），同时避免继承 log 命令选项空间的全部复杂性。

### 为什么使用 committer 时间戳进行过滤？

`--since`/`--until` 过滤器使用 committer 时间戳（不是 author 时间戳），匹配 Git 行为。Committer 时间戳反映提交实际应用到当前分支的时间（例如 rebase 后），这对发布周期摘要比原始作者时间更相关。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| 按数量排序 | `-n` / `--numbered` | `-n` / `--numbered` | N/A（无 shortlog 命令） |
| 仅摘要 | `-s` / `--summary` | `-s` / `--summary` | N/A |
| 显示 email | `-e` / `--email` | `-e` / `--email` | N/A |
| Since 日期 | `--since <date>` | `--since <date>` / `--after <date>` | N/A |
| Until 日期 | `--until <date>` | `--until <date>` / `--before <date>` | N/A |
| 修订 | `<revision>`（位置参数） | `<revision range>...` | N/A |
| Group by | 不支持 | `--group=author\|committer\|trailer:<key>` | N/A |
| 格式 | 不支持 | `--format=<format>` | N/A |
| Committer 分组 | 不支持 | `--committer`（已弃用，使用 `--group=committer`） | N/A |
| 管道输入 | 不支持 | 通过管道时从 stdin 读取 | N/A |
| No merges | 不支持 | `--no-merges` | N/A |
| Author 过滤 | 不支持 | `--author=<pattern>` | N/A |
| Grep 过滤 | 不支持 | `--grep=<pattern>` | N/A |
| 宽度限制 | 不支持 | `-w[<width>[,<indent1>[,<indent2>]]]` | N/A |
| JSON 输出 | `--json` | 不支持 | N/A |
| Quiet 模式 | `--quiet` | 不支持 | N/A |

注意：jj 没有 shortlog 命令。类似信息可通过过滤 `jj log` 输出获得，但没有内置作者聚合。

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效 `--since` / `--until` 日期 | `LBR-CLI-002` | 129 |
| 无效修订 | `LBR-CLI-003` | 129 |
| HEAD 没有提交 | `LBR-REPO-003` | 128 |
| 无法读取引用或提交图 | `LBR-IO-001` / `LBR-REPO-002` | 128 |
