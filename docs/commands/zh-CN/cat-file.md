# `libra cat-file`

检查仓库中存储的 Git 对象和 Libra AI 历史对象。

## 概要

```
libra cat-file [OPTIONS] [OBJECT]
```

## 说明

`libra cat-file` 是类似 `git cat-file` 的低层调试工具。它可以打印任何 Git 对象（commit、tree、blob、tag）的类型、大小或美化内容，也可以检查对象是否存在。

Libra 用 `--ai*` 标志扩展了经典命令，用于检查存储在 `libra/intent` orphan 分支上的 AI 工作流对象（Intent、Task、Run、Plan、PatchSet、Evidence、Session 等）。这为开发者和代理提供了一个单一入口，用于同时内省版本控制对象和 AI 流程产物。

必须且只能指定一个模式标志。Git 模式（`-t`、`-s`、`-p`、`-e`）需要位置 `OBJECT` 参数。AI 模式（`--ai`、`--ai-type`、`--ai-list`、`--ai-list-types`）忽略 `OBJECT`，并在 AI history 分支上操作。

## 选项

| 标志 | 短选项 | 说明 |
|------|-------|-------------|
| `-t` | | 打印对象类型（`commit`、`tree`、`blob`、`tag`）。 |
| `-s` | | 打印对象大小（字节）。 |
| `-p` | | 美化打印对象内容。 |
| `-e` | | 检查对象是否存在（仅退出状态，无 stdout）。不支持 `--json`。 |
| `--ai <ID>` | | 按 ID 美化打印 AI 对象。接受 `TYPE:ID` 以消除歧义。 |
| `--ai-type <ID>` | | 打印给定 ID 的 AI 对象类型。 |
| `--ai-list <TYPE>` | | 列出给定类型的所有 AI 对象（例如 `intent`、`patchset`、`event`）。 |
| `--ai-list-types` | | 列出 history 分支中存在的所有 AI 对象类型。 |
| `<OBJECT>` | | Git 对象哈希或引用。`-t`/`-s`/`-p`/`-e` 必需；`--ai*` 模式忽略。 |

### 示例

```bash
# 打印 HEAD 的类型
libra cat-file -t HEAD

# 打印特定对象的大小
libra cat-file -s 40d352ee7190f92dcf7883b8a81f2c730fd8a860

# 美化打印 HEAD 提交
libra cat-file -p HEAD

# 检查存在性（退出码 0 = 存在）
libra cat-file -e abc1234

# 结构化 JSON 类型查询
libra cat-file -t HEAD --json

# 列出所有 AI intent 对象
libra cat-file --ai-list intent

# 美化打印 AI 对象（用 TYPE:ID 消除歧义）
libra cat-file --ai patchset:call_KjR3NB4cQaT5Rm1c7zXjsskQ

# 打印 AI 对象类型
libra cat-file --ai-type debug-local-1772707227

# 列出仓库中的所有 AI 对象类型
libra cat-file --ai-list-types --json
```

## 常用命令

```bash
libra cat-file -t HEAD
libra cat-file -s HEAD
libra cat-file -p HEAD
libra cat-file -t HEAD --json
libra cat-file --ai-list-types --json
libra cat-file --ai-list intent
libra cat-file --ai <session-id>
```

## 人类可读输出

- `-t`：在单行打印对象类型（例如 `commit`）
- `-s`：在单行打印字节大小（例如 `342`）
- `-p`：根据类型美化打印内容：
  - Commit：头字段和消息
  - Tree：每个条目为 `<mode> <type> <hash>\t<name>`
  - Blob：原始文本内容
  - Tag：tag 头和消息
- `-e`：无输出；对象存在时退出码为 0，否则非零
- `--ai <ID>`：打印格式化摘要（`ai_session` 对象为 session 摘要，其他对象为完整 JSON）
- `--ai-list <TYPE>`：每行一个对象 ID
- `--ai-list-types`：每行一个类型名

## 结构化输出（JSON 示例）

### Type 模式（`-t`）

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "type",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "object_type": "commit"
  }
}
```

### Size 模式（`-s`）

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "size",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "size": 342
  }
}
```

### Pretty-print 模式（`-p`）-- commit

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "pretty",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "object_type": "commit",
    "content": {
      "tree": "def456...",
      "parents": ["abc123..."],
      "author": "Alice <alice@example.com> 1711929600 +0000",
      "committer": "Alice <alice@example.com> 1711929600 +0000",
      "message": "feat: add new feature"
    }
  }
}
```

### AI list types

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "ai-list-types",
    "types": ["intent", "patchset", "plan", "run", "task"]
  }
}
```

说明：

- `cat-file -e` 不支持 `--json` / `--machine`（它纯粹是退出码检查）
- Blob/tag 美化打印 JSON 要求 UTF-8 内容；非文本 payload 会显式失败，而不是返回有损数据

## 设计理由

### 为什么添加 `--ai*` 标志？

Libra 的 AI 代理基础设施将流程产物（intents、plans、tasks、runs、patch sets、evidence、sessions）作为 Git 对象存储在 orphan 分支上。无需单独检查工具，`cat-file` 是自然归宿，因为它已经处理“按 ID 显示对象原始内容”的需求。`--ai*` 标志将这一能力扩展到 AI 对象命名空间，同时保持熟悉接口。这意味着单个命令既能回答“这个提交是什么类型？”，也能回答“这个 AI 计划包含什么？”，这在调试代理工作流时尤其有用。

### 为什么没有 batch 模式？

Git 的 `cat-file --batch` 以流式方式从 stdin 读取对象 ID，这对脚本处理数百万对象很重要。Libra 面向不同工作流：结构化 JSON 输出让代理能在一次调用中检索对象数据，AI 检查模式也已经支持列出某类型的所有对象。Batch 模式会增加复杂度，却没有明确的 agent-native 工作流用例。如果批量检查变得必要，可以之后添加 `--batch`，且不破坏现有接口。

### 为什么 `-e` 保持 human-only？

`-e`（存在性检查）模式通过退出码传达结果：0 表示对象存在，非零表示不存在。这是布尔谓词的 Unix 约定。将其包装为 JSON 不会增加信息（`{"exists": true}`），还会破坏 `-e` 是静默探测的预期。需要结构化存在性检查的脚本和代理可以改用带 `--json` 的 `-t`；如果对象不存在，JSON 响应会包含错误。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 打印对象类型 | `-t` | `-t` | N/A（无直接等价） |
| 打印对象大小 | `-s` | `-s` | N/A |
| 美化打印内容 | `-p` | `-p` | N/A（blob 使用 `jj file show`） |
| 检查存在性 | `-e` | `-e` | N/A |
| Batch 模式 | 未实现 | `--batch`, `--batch-check` | N/A |
| AI 对象检查 | `--ai`, `--ai-type` | N/A | N/A |
| AI 对象列出 | `--ai-list`, `--ai-list-types` | N/A | N/A |
| JSON 输出 | `--json` | 无 | 无 |
| 对象解析 | SHA-1、refs、`HEAD~N` | SHA-1、refs、所有 rev-parse 语法 | Change IDs、revsets |
| `--filters` | 否 | `--filters`（与外部格式互转） | N/A |
| `--textconv` | 否 | `--textconv` | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效对象 / 修订 | `LBR-CLI-003` | 129 |
| 不支持的参数组合 | `LBR-CLI-002` | 129 |
| 无法读取对象数据 | `LBR-IO-001` / `LBR-REPO-002` | 128 |
