# `libra show-ref`

列出本地引用及其对象 ID，并可按类型和模式筛选。

## 概要

```
libra show-ref [OPTIONS] [PATTERN]...
```

## 说明

`libra show-ref` 枚举仓库中存储的引用（分支、标签，以及可选的 `HEAD`），并显示每个引用指向的对象哈希。默认显示分支和标签。使用 `--heads` / `--branches` 或 `--tags` 可将输出限制为某一类。

位置 `<PATTERN>` 参数按 Git 的 `show-ref` 行为，从全限定引用名末尾匹配完整路径段。例如 `main` 会匹配 `refs/heads/main` 和 `refs/remotes/origin/main`，但不会匹配 `refs/heads/main-2`。当指定 `--head` 时，`HEAD` 永远不会被模式过滤掉。

使用 `-d` / `--dereference` 可展开 annotated tag。Annotated tag 会输出两行：一行是 tag object 本身，另一行是 `refs/tags/<name>^{}`，指向 peeled target。Lightweight tag 仍只输出一行。

使用 `--abbrev[=<n>]` 可缩短显示的对象 ID；省略数值时默认 7 位十六进制，`--abbrev=0` 保留完整哈希。`--hash=<n>` 将 hash-only 输出与同一宽度控制组合；`--hash` 不带值时保留完整哈希，除非同时传入 `--abbrev`。

当脚本需要精确引用名时，使用 `--verify <ref>`，例如 `HEAD` 或 `refs/heads/main`；`main` 这类短名称会被拒绝。使用 `--exists <ref>` 可检查单个引用是否存在，成功时不输出人类可读文本。

使用 `--exclude-existing[=<pattern>]` 可获得 Git 兼容的 stdin filter。每行输入会取最后一个以空白分隔的字段作为 refname，存在性检查前会忽略末尾的 `^{}` peel 后缀；本地已存在的 ref 会被丢弃，缺失 ref 会按 stdin 原始行输出。提供 `<pattern>` 时，只处理带有该前缀的 refname。

Libra 将引用存储在 SQLite 中，而不是 loose 文件或 packed-refs 中，因此 `show-ref` 直接查询数据库。这使枚举为 O(rows)，无需扫描文件系统。

## 选项

| 标志 | 短选项 | 说明 |
|------|-------|-------------|
| `--heads` | | 只显示分支（`refs/heads/`）。 |
| `--branches` | | Git 兼容的 `--heads` alias。 |
| `--tags` | | 只显示标签（`refs/tags/`）。 |
| `--head` | | 在输出中包含 `HEAD`。 |
| `--hash[=<n>]` | `-s[<n>]` | 只显示对象哈希，可选缩短到 `n` 位十六进制。 |
| `--abbrev[=<n>]` | | 将对象 ID 缩短到 `n` 位；省略 `n` 时为 7 位。 |
| `--dereference` | `-d` | 展开 annotated tag 并包含 peeled `^{}` 条目。 |
| `--verify` | | 验证精确引用名，不使用模式过滤。 |
| `--exists` | | 检查一个精确引用是否存在，成功时不打印。 |
| `--exclude-existing[=<pattern>]` | | 过滤 stdin，只输出本地尚不存在的 refs。 |
| `<PATTERN>...` | | 按引用名路径段后缀匹配过滤引用。多个模式按 OR 组合。 |

### 示例

```bash
# 列出所有引用
libra show-ref

# 只显示分支
libra show-ref --heads

# 使用 Git alias 只显示分支
libra show-ref --branches

# 只显示标签
libra show-ref --tags

# 展开 annotated tag
libra show-ref --dereference --tags v1.0

# 包含 HEAD 且只显示哈希
libra show-ref --head --hash

# 将引用哈希缩短为 12 位
libra show-ref --abbrev=12 --heads

# 每个匹配项只打印 12 位哈希
libra show-ref --hash=12 --heads

# 验证精确引用
libra show-ref --verify refs/heads/main

# 检查引用是否存在且成功时不输出
libra show-ref --exists refs/heads/main

# 只保留 stdin 中本地缺失的 refs
printf '%s\n' 'abc123 refs/heads/new' | libra show-ref --exclude-existing

# 过滤到路径段以 "release" 结尾的引用
libra show-ref release

# 组合过滤：只显示匹配 "feat" 的分支
libra show-ref --heads feat
```

## 常用命令

```bash
libra show-ref
libra show-ref --heads
libra show-ref --branches
libra show-ref --tags
libra show-ref --dereference --tags v1.0
libra show-ref --head --hash
libra show-ref --abbrev=12 --heads
libra show-ref --hash=12 --heads
libra show-ref --verify refs/heads/main
libra show-ref --exists refs/heads/main
libra show-ref --exclude-existing
libra show-ref --json --head --heads
libra show-ref main
```

## 人类可读输出

默认：

```text
abc1234def5678901234567890abcdef12345678 refs/heads/main
def5678901234567890abcdef12345678abc1234 refs/tags/v1.0.0
```

使用 `--hash` 时，只打印对象 ID：

```text
abc1234def5678901234567890abcdef12345678
def5678901234567890abcdef12345678abc1234
```

使用 `--dereference` 时，annotated tag 会包含额外的 peeled 条目：

```text
def5678901234567890abcdef12345678abc1234 refs/tags/v1.0.0
abc1234def5678901234567890abcdef12345678 refs/tags/v1.0.0^{}
```

使用 `--abbrev=12` 时，哈希会缩短但仍显示引用名：

```text
abc1234def56 refs/heads/main
```

使用 `--hash=12` 时，只打印缩短后的哈希：

```text
abc1234def56
```

## 结构化输出（JSON 示例）

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "hash_only": false,
    "abbrev": null,
    "entries": [
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "HEAD"
      },
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "refs/heads/main"
      },
      {
        "hash": "def5678901234567890abcdef12345678abc1234",
        "refname": "refs/tags/v1.0.0"
      }
    ]
  }
}
```

当 `--hash` 激活时，`hash_only` 为 `true`。当 `--abbrev` 或 hash 宽度激活时，`abbrev` 记录宽度，`entries[].hash` 包含显示用的缩短值。无论该标志如何，`entries` 数组始终存在，以便 JSON 消费者获得统一 schema。

使用 `--exists` 时，人类输出成功时为空。JSON 输出会报告被检查的引用：

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "exists": true,
    "refname": "refs/heads/main"
  }
}
```

使用 `--exclude-existing` 时，人类可读输出会保留每条缺失 ref 的输入原文。JSON 输出会同时报告解析后的 refname：

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "exclude_existing": true,
    "pattern": "refs/heads",
    "entries": [
      {
        "line": "abc123 refs/heads/new",
        "refname": "refs/heads/new"
      }
    ]
  }
}
```

## 设计理由

### 为什么使用路径段后缀匹配？

Git 的 `show-ref` 会把 pattern 当作从全限定引用名末尾匹配的完整路径段。Libra 跟随这个行为，因此脚本可以传入 `main` 而不会误匹配 `main-2`，同时仍能匹配 `refs/heads/main` 和 `refs/remotes/origin/main`。

### 为什么使用 SQLite-backed refs？

Git 将引用存储为单独文件（`refs/heads/main`），并最终将它们打包到一个扁平的 `packed-refs` 文件中。这可行，但在大型 monorepo 中有众所周知的扩展性问题：数千个分支意味着数千次文件系统 stat 调用，任何引用更新都需要 O(N) 重写 packed-refs，并发写入者需要 lockfile。Libra 在 SQLite 中使用 `reference` 表，提供 ACID 事务、通过 B-tree 索引实现 O(log N) 查询，以及无锁争用的原子多引用更新。`show-ref` 直接受益：它是一次 `SELECT`，而不是目录遍历加 packed-refs 解析。

### 为什么 `--head` 是 opt-in？

遵循 Git 约定，默认省略 `HEAD`，因为它是符号引用，会重复某个 `refs/heads/*` 条目。使用 `--head` 显式包含它，对需要确认 `HEAD` 已附着并解析到哪个提交的脚本很有用。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 列出所有引用 | `libra show-ref` | `git show-ref` | `jj bookmark list` + `jj tag list` |
| 筛选到分支 | `--heads` / `--branches` | `--heads` / `--branches` | `jj bookmark list` |
| 筛选到标签 | `--tags` | `--tags` | `jj tag list` |
| 包含 HEAD | `--head` | `--head` | N/A（无 HEAD 概念） |
| 仅哈希输出 | `-s[<n>]` / `--hash[=<n>]` | `-s[<n>]` / `--hash[=<n>]` | N/A |
| 缩短对象 ID | `--abbrev[=<n>]` | `--abbrev[=<n>]` | N/A |
| 展开 annotated tag | `-d` / `--dereference` | `-d` / `--dereference` | N/A |
| 模式匹配 | 路径段后缀匹配 | 路径段后缀匹配 | 通过 revset 的正则 |
| `--verify`（检查精确引用） | `--verify <ref>` | 是 | N/A |
| `--exists`（存在性检查） | `--exists <ref>` | 是 | N/A |
| `--exclude-existing` stdin filter | `--exclude-existing[=<pattern>]` | 是 | N/A |
| JSON 输出 | `--json` | 无 | 无 |
| 引用存储 | SQLite `reference` 表 | Loose files + packed-refs | Operation log |
| 远程跟踪引用 | 是（`refs/remotes/`） | 是（`refs/remotes/`） | 通过 `jj git fetch` |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 没有匹配引用 | `LBR-CLI-003` | 129 |
| `--verify` 目标不是已存在的精确引用 | `LBR-CLI-003` | 128；全局 `--quiet` 时为 1 |
| `--exists` 目标不存在 | `LBR-CLI-003` | 2 |
| `--exclude-existing` 与 `--verify` / `--exists` 组合 | `LBR-CLI-002` | 129 |
| 无法读取引用 | `LBR-IO-001` | 128 |
| 存储的分支/标签数据损坏 | `LBR-REPO-002` | 128 |
