# `libra show-ref`

列出本地引用及其对象 ID，并可按类型和模式筛选。

## 概要

```
libra show-ref [OPTIONS] [PATTERN]...
```

## 说明

`libra show-ref` 枚举仓库中存储的引用（分支、标签，以及可选的 `HEAD`），并显示每个引用指向的对象哈希。默认显示分支和标签。使用 `--heads` 或 `--tags` 可将输出限制为某一类。

位置 `<PATTERN>` 参数会作为全限定引用名（例如 `refs/heads/main`）上的子字符串过滤器。只有名称包含至少一个给定模式的引用会被包含。当指定 `--head` 时，`HEAD` 永远不会被模式过滤掉。

当脚本需要精确引用名时，使用 `--verify <ref>`，例如 `HEAD` 或 `refs/heads/main`；`main` 这类短名称会被拒绝。使用 `--exists <ref>` 可检查单个引用是否存在，成功时不输出人类可读文本。

Libra 将引用存储在 SQLite 中，而不是 loose 文件或 packed-refs 中，因此 `show-ref` 直接查询数据库。这使枚举为 O(rows)，无需扫描文件系统。

## 选项

| 标志 | 短选项 | 说明 |
|------|-------|-------------|
| `--heads` | | 只显示分支（`refs/heads/`）。 |
| `--tags` | | 只显示标签（`refs/tags/`）。 |
| `--head` | | 在输出中包含 `HEAD`。 |
| `--hash` | `-s` | 只显示对象哈希，不显示引用名。 |
| `--verify` | | 验证精确引用名，不使用子字符串过滤。 |
| `--exists` | | 检查一个精确引用是否存在，成功时不打印。 |
| `<PATTERN>...` | | 按引用名上的子字符串匹配过滤引用。多个模式按 OR 组合。 |

### 示例

```bash
# 列出所有引用
libra show-ref

# 只显示分支
libra show-ref --heads

# 只显示标签
libra show-ref --tags

# 包含 HEAD 且只显示哈希
libra show-ref --head --hash

# 验证精确引用
libra show-ref --verify refs/heads/main

# 检查引用是否存在且成功时不输出
libra show-ref --exists refs/heads/main

# 过滤到包含 "release" 的引用
libra show-ref release

# 组合过滤：只显示匹配 "feat" 的分支
libra show-ref --heads feat
```

## 常用命令

```bash
libra show-ref
libra show-ref --heads
libra show-ref --tags
libra show-ref --head --hash
libra show-ref --verify refs/heads/main
libra show-ref --exists refs/heads/main
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

## 结构化输出（JSON 示例）

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "hash_only": false,
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

当 `--hash` 激活时，`hash_only` 为 `true`。无论该标志如何，`entries` 数组始终存在，以便 JSON 消费者获得统一 schema。

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

## 设计理由

### 为什么用子字符串匹配而不是 glob？

Git 的 `show-ref` 使用全限定引用名的前缀匹配，但实践中用户最常想问的是“显示任何与 `release` 有关的东西”或“名称里带 `main` 的东西”。子字符串匹配更容易实现、更容易解释，并覆盖常见场景。它避免了记忆到底需要 `refs/heads/main*` 还是 `main*` 的认知负担。对于需要精确控制的少数情况，JSON 输出会给出完整引用名数组，你可以在客户端过滤。将来可以把 glob 支持作为超集添加。

### 为什么使用 SQLite-backed refs？

Git 将引用存储为单独文件（`refs/heads/main`），并最终将它们打包到一个扁平的 `packed-refs` 文件中。这可行，但在大型 monorepo 中有众所周知的扩展性问题：数千个分支意味着数千次文件系统 stat 调用，任何引用更新都需要 O(N) 重写 packed-refs，并发写入者需要 lockfile。Libra 在 SQLite 中使用 `reference` 表，提供 ACID 事务、通过 B-tree 索引实现 O(log N) 查询，以及无锁争用的原子多引用更新。`show-ref` 直接受益：它是一次 `SELECT`，而不是目录遍历加 packed-refs 解析。

### 为什么 `--head` 是 opt-in？

遵循 Git 约定，默认省略 `HEAD`，因为它是符号引用，会重复某个 `refs/heads/*` 条目。使用 `--head` 显式包含它，对需要确认 `HEAD` 已附着并解析到哪个提交的脚本很有用。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 列出所有引用 | `libra show-ref` | `git show-ref` | `jj bookmark list` + `jj tag list` |
| 筛选到分支 | `--heads` | `--heads` | `jj bookmark list` |
| 筛选到标签 | `--tags` | `--tags` | `jj tag list` |
| 包含 HEAD | `--head` | `--head` | N/A（无 HEAD 概念） |
| 仅哈希输出 | `-s` / `--hash` | `-s` / `--hash` | N/A |
| 模式匹配 | 子字符串匹配 | 前缀/glob 匹配 | 通过 revset 的正则 |
| `--verify`（检查精确引用） | `--verify <ref>` | 是 | N/A |
| `--exists`（存在性检查） | `--exists <ref>` | 是 | N/A |
| `-d` / `--dereference` | 尚未实现 | 是 | N/A |
| JSON 输出 | `--json` | 无 | 无 |
| 引用存储 | SQLite `reference` 表 | Loose files + packed-refs | Operation log |
| 远程跟踪引用 | 是（`refs/remotes/`） | 是（`refs/remotes/`） | 通过 `jj git fetch` |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 没有匹配引用 | `LBR-CLI-003` | 129 |
| `--verify` 目标不是已存在的精确引用 | `LBR-CLI-003` | 128；全局 `--quiet` 时为 1 |
| `--exists` 目标不存在 | `LBR-CLI-003` | 2 |
| 无法读取引用 | `LBR-IO-001` | 128 |
| 存储的分支/标签数据损坏 | `LBR-REPO-002` | 128 |
