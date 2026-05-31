# `libra log`

显示提交历史。

**别名：** `hist`, `history`

## 概要

```
libra log [OPTIONS] [-- PATHS...]
```

## 说明

`libra log` 从当前 HEAD 开始显示提交历史。它支持多种输出格式，包括 oneline、自定义 pretty-print、图形可视化和结构化 JSON。提交可按作者、日期范围和文件路径过滤。Diff 输出（`--patch`、`--stat`、`--name-only`、`--name-status`）可以限制到特定路径。

人类模式保留当前 `--oneline`、`--graph`、`--pretty`、`--stat`、`--patch` 和相关输出样式。`--quiet` 抑制人类输出，但仍验证请求的历史范围。

## 选项

### `-n, --number <N>`

限制显示的提交数量。

```bash
libra log -n 5
libra log --number 10
```

### `--oneline`

`--pretty=oneline --abbrev-commit` 的简写。以缩写哈希和主题在单行显示每个提交。

```bash
libra log --oneline
```

### `--abbrev-commit`

显示缩写提交哈希，而不是完整 40 字符哈希。

```bash
libra log --abbrev-commit
```

### `--abbrev <LENGTH>`

设置缩写提交哈希长度。

```bash
libra log --abbrev 8
```

### `--no-abbrev-commit`

显示完整提交哈希。覆盖 `--abbrev-commit`。

```bash
libra log --no-abbrev-commit
```

### `-p, --patch`

显示每个提交的 diff（patch）。可与路径参数组合，将 diff 限制到特定文件。

```bash
libra log -p
libra log -p -- src/main.rs
```

### `--name-only`

只显示每个提交中已更改文件的名称。

```bash
libra log --name-only
```

### `--name-status`

显示每个提交中已更改文件的名称和状态（added/modified/deleted）。

```bash
libra log --name-status
libra log --name-status -- src/
```

### `--stat`

显示每个提交的 diffstat（文件变更统计），展示每个文件的插入和删除。

```bash
libra log --stat
```

### `--author <PATTERN>`

只显示作者姓名或 email 匹配给定模式的提交。

```bash
libra log --author alice
libra log --author "alice@example.com"
```

### `--since <DATE>`

显示晚于指定日期的提交。

```bash
libra log --since 2026-01-01
libra log --since "2 weeks ago"
```

### `--until <DATE>`

显示早于指定日期的提交。

```bash
libra log --until 2026-03-01
```

### `--pretty <FORMAT>`

自定义 pretty-print 格式字符串。支持 `%h`（短哈希）、`%s`（主题）、`%an`（作者名）、`%ae`（作者 email）、`%ad`（作者日期）等占位符。

```bash
libra log --pretty="%h - %s (%an)"
libra log --pretty="format:%H %s"
```

### `--decorate[=<style>]`

在提交旁打印 ref 名称（分支、标签）。样式：`short`（默认）、`full`、`no`。

```bash
libra log --decorate
libra log --decorate=full
```

### `--no-decorate`

不打印 ref 名称。覆盖 `--decorate`。

```bash
libra log --no-decorate
```

### `--graph`

绘制基于文本的提交历史图形表示，直观显示分支和合并。

```bash
libra log --graph
libra log --oneline --graph
```

### `[PATHS...]`

将 diff 输出限制到指定路径。与 `-p`、`--name-only`、`--name-status` 或 `--stat` 一起使用。

```bash
libra log -- src/
libra log -p -- src/main.rs tests/
```

## 常用命令

```bash
libra log
libra log -n 5
libra log --oneline --graph
libra log --author alice --since 2026-01-01
libra log --name-status src/
libra --json log -n 1
```

## 人类可读输出

默认人类模式以详细多行格式显示提交：

```text
commit abc1234def5678901234567890abcdef12345678 (HEAD -> main, origin/main)
Author: Test User <test@example.com>
Date:   Sat Mar 30 10:00:00 2026 +0800

    Add new feature
```

Oneline 格式：

```text
abc1234 (HEAD -> main) Add new feature
def5678 Fix bug in parser
```

Graph 格式：

```text
* abc1234 (HEAD -> main) Add new feature
* def5678 Fix bug in parser
|\ 
| * 1234567 Feature branch commit
|/
* 7890abc Initial commit
```

`--quiet` 会抑制所有人类输出。

## 结构化输出

`--json` / `--machine` 返回经过过滤的结构化提交列表：

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [
      {
        "hash": "abc123...",
        "short_hash": "abc1234",
        "author_name": "Test User",
        "author_email": "test@example.com",
        "author_date": "2026-03-30T10:00:00+08:00",
        "committer_name": "Test User",
        "committer_email": "test@example.com",
        "committer_date": "2026-03-30T10:00:00+08:00",
        "subject": "base",
        "body": "",
        "parents": [],
        "refs": ["HEAD -> main"],
        "files": [
          { "path": "tracked.txt", "status": "added" }
        ]
      }
    ],
    "total": 1
  }
}
```

### Schema 说明

- `-n` 也适用于 JSON 模式
- 仅在未提供 `-n` 时，`total` 反映过滤后的提交数量；使用 `-n` 时始终为 `null`
- `--graph`、`--pretty` 和 `--oneline` 不改变 JSON schema
- `--decorate` 只影响人类渲染；JSON 始终返回 `refs` 数组，辅助 ref 元数据以 best-effort 收集
- `files` 始终是结构化变更摘要，永远不包含 patch 文本

## 设计理由

### 暂无 `--all` / `--branches` / `--remotes` 标志

Git 的 `--all` 显示从所有 refs（分支、标签、stash）可达的提交，而 `--branches` 和 `--remotes` 分别过滤为本地或远程分支。Libra 当前只从 HEAD 遍历提交图。实现 `--all` 需要先枚举 SQLite `reference` 表，收集所有分支 tip 和标签目标，然后将多个提交遍历合并为单个时间线。这已计划但尚未实现。当前单 HEAD 遍历覆盖最常见用例（检查当前分支历史），并避免多根图合并的复杂度。

### 暂无修订范围（`A..B`）语法

Git 的修订范围语法（`main..feature`、`main...feature`、`HEAD~3..HEAD`）很强大但复杂，需要完整修订解析器，支持符号引用、祖先操作符（`~`、`^`）和集合操作（差集、对称差）。Libra 尚未实现修订解析器。`-n` 标志和 `--since`/`--until` 日期过滤提供基础历史作用域。完整修订范围解析器在路线图中，将同时支持 Git 兼容语法和额外 Libra 特定扩展。

### 文本渲染的 `--graph`

Libra 将 `--graph` 实现为基于文本的 ASCII/Unicode 图渲染器，类似 Git 内置 graph 输出。与 GUI 工具（GitKraken、SourceTree）或带外部 graph renderer 的 Git `--format` 不同，Libra 的图直接在终端内渲染。这让 CLI 自包含，并确保跨平台输出一致。Graph renderer 处理分支、合并和 octopus merges，绘制父子提交之间的连接线。

### JSON 始终返回 `refs` 数组，不受 `--decorate` 影响

在人类输出中，`--decorate` 控制是否在提交哈希旁显示 ref 名称（分支、标签）。在 JSON 模式中，无论 `--decorate` 标志如何，`refs` 数组总是填充。这一设计选择体现了 JSON 输出应为程序消费者提供最大信息量的原则。解析 JSON 输出的 AI 代理或 CI 工具不应需要记得传 `--decorate` 才能获得 ref 信息。`--decorate` 标志只影响人类渲染层。

## 参数对比：Libra vs Git vs jj

| 参数 / 标志 | Git | jj | Libra |
|---|---|---|---|
| 显示 log | `git log` | `jj log` | `libra log` |
| 限制数量 | `git log -n <N>` | `jj log -n <N>` | `libra log -n <N>` |
| Oneline 格式 | `git log --oneline` | 默认格式为 oneline | `libra log --oneline` |
| 缩写哈希 | `git log --abbrev-commit` | 默认 | `libra log --abbrev-commit` |
| 缩写长度 | `git log --abbrev=<N>` | N/A | `libra log --abbrev <N>` |
| 完整哈希 | `git log --no-abbrev-commit` | `jj log --no-short-hash` | `libra log --no-abbrev-commit` |
| 显示 patch | `git log -p` | `jj diff -r <rev>`（单独命令） | `libra log -p` / `--patch` |
| 仅名称 | `git log --name-only` | N/A | `libra log --name-only` |
| 名称和状态 | `git log --name-status` | N/A | `libra log --name-status` |
| Diffstat | `git log --stat` | `jj diff --stat -r <rev>` | `libra log --stat` |
| 按作者过滤 | `git log --author=<pat>` | `jj log --author <pat>`（revset） | `libra log --author <pat>` |
| Since 日期 | `git log --since=<date>` | Revset 表达式 | `libra log --since <date>` |
| Until 日期 | `git log --until=<date>` | Revset 表达式 | `libra log --until <date>` |
| 自定义格式 | `git log --pretty=<fmt>` | `jj log -T <template>` | `libra log --pretty <fmt>` |
| Decorate refs | `git log --decorate` | 始终显示 | `libra log --decorate` |
| 不 decorate | `git log --no-decorate` | N/A | `libra log --no-decorate` |
| Graph 视图 | `git log --graph` | `jj log`（默认有 graph） | `libra log --graph` |
| 所有 refs | `git log --all` | `jj log -r 'all()'` | N/A（尚未实现） |
| 仅分支 | `git log --branches` | `jj log -r 'branches()'` | N/A |
| 仅远程 | `git log --remotes` | `jj log -r 'remote_branches()'` | N/A |
| 修订范围 | `git log A..B` | `jj log -r 'A..B'` | N/A（尚未实现） |
| Grep 消息 | `git log --grep=<pat>` | Revset `description()` | N/A |
| 路径过滤 | `git log -- <paths>` | N/A（使用 revset） | `libra log -- <paths>` |
| 反向顺序 | `git log --reverse` | `jj log --reversed` | N/A |
| 仅 merge commits | `git log --merges` | N/A | N/A |
| 仅 first parent | `git log --first-parent` | N/A | N/A |
| 结构化 JSON 输出 | N/A | N/A | `--json` / `--machine` |
| 错误提示 | 最少 | 最少 | 每种错误类型都有可操作提示 |

## 错误处理

| 场景 | 错误码 | 退出码 | 提示 |
|----------|-----------|------|------|
| 仓库外部 | `LBR-REPO-001` | 128 | -- |
| 空分支或空 HEAD | `LBR-REPO-003` | 128 | "create a commit first before running 'libra log'" |
| 无效日期参数 | `LBR-CLI-002` | 129 | -- |
| 无效 `--decorate` 选项 | `LBR-CLI-002` | 129 | -- |
| 无效对象名 | `LBR-CLI-003` | 129 | "check the revision name and try again" |
| 损坏的 commit/tree/blob | `LBR-REPO-002` | 128 | -- |
| 无法读取历史对象 | `LBR-REPO-002` | 128 | -- |

## 兼容性说明

- `--all`、`--branches` 和 `--remotes` 尚未实现；log 从 HEAD 遍历
- 修订范围语法（`A..B`、`A...B`）尚不支持；使用 `-n` 和 `--since`/`--until` 限定范围
- jj 的 log 使用模板语言（`-T`）进行格式化；Libra 使用 Git 兼容的 `--pretty` 格式字符串
- 尚未实现用于消息过滤的 `--grep`
- 尚未实现用于时间顺序的 `--reverse`
- 在 JSON 模式中，`files` 包含结构化变更摘要；JSON 输出永远不包含 patch 文本
