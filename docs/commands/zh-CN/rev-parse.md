# `libra rev-parse`

解析修订名，并打印规范化的提交 ID、符号引用或仓库路径。

## 概要

```bash
libra rev-parse [OPTIONS] [SPEC]
```

## 说明

`libra rev-parse` 会将类似修订的输入解析为以下三种形式之一：

- 完整提交 ID（默认）
- 使用 `--short` 得到的短提交 ID
- 使用 `--abbrev-ref` 得到的符号分支名

它还支持 `--show-toplevel`，用于打印工作树的绝对仓库根目录；支持 `--verify`，用于断言参数必须解析为单个对象。未提供 `<SPEC>` 时，命令默认为 `HEAD`，如果提供了 `--default <SPEC>` 则使用该默认修订。

## 选项

| 标志 | 说明 |
|------|-------------|
| `--short` | 打印无歧义的缩写对象 ID。 |
| `--abbrev-ref` | 打印符号分支名，而不是提交哈希。 |
| `--verify` | 要求参数解析为单个对象并打印；否则失败。可与 `--short` 组合，和 `--abbrev-ref` / `--show-toplevel` 互斥。 |
| `--default <SPEC>` | 未提供位置参数 `<SPEC>` 时使用的修订。 |
| `--show-toplevel` | 打印顶层工作树的绝对路径。 |
| `<SPEC>` | 要解析的修订。省略时默认为 `HEAD`。 |

### `--verify` 退出码

`--verify` 遵循 Git plumbing 契约：

- 成功：打印解析后的哈希，退出 0。
- 失败（无效引用、unborn HEAD 或没有 revision）：向 stderr 打印 `fatal: Needed a single revision`，退出 **128**。
- 在全局 `--quiet` / `-q` 下失败：不打印任何内容，退出 **1**（匹配 `git rev-parse --verify -q`）。

> 注意：未使用 `--verify` 时，无效 spec 退出 **129**（`LBR-CLI-003`）。这是相对 Git 的 intentional difference，用于保持 Libra invalid-target 退出码模型一致。

## 常用命令

```bash
libra rev-parse
libra rev-parse HEAD~1
libra rev-parse --short HEAD
libra rev-parse --abbrev-ref HEAD
libra rev-parse --verify HEAD
libra rev-parse --verify --default HEAD
libra rev-parse --show-toplevel
libra --json rev-parse --short HEAD
```

## 人类可读输出

默认输出为包含已解析值的单行。

```text
abc1234def5678901234567890abcdef12345678
```

使用 `--short`：

```text
abc1234
```

使用 `--abbrev-ref`：

```text
main
```

使用 `--show-toplevel`：

```text
/home/alice/project
```

## 结构化输出

```json
{
  "ok": true,
  "command": "rev-parse",
  "data": {
    "mode": "short",
    "input": "HEAD",
    "value": "abc1234"
  }
}
```

`mode` 是 `resolve`、`short`、`abbrev_ref`、`verify` 或 `show_toplevel` 之一。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 解析完整提交 ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| 缩写提交 ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| 符号分支名 | `--abbrev-ref` | `--abbrev-ref` | N/A |
| 校验单个对象 | `--verify`（退出 128，或在 `-q` 下退出 1） | `--verify` | N/A |
| 默认修订 | `--default <SPEC>` | `--default` | N/A |
| 工作树根目录 | `--show-toplevel` | `--show-toplevel` | `jj root` |
| 路径/状态查询 | 未实现（延后）：`--git-dir`、`--show-prefix`、`--show-cdup`、`--is-*` | 全部支持 | `jj root`（部分） |
| Shell 引用/范围 | 未实现（延后）：`--sq`、`--sq-quote`、`A..B`、`A...B` | 全部支持 | revsets |
| JSON 输出 | `--json` | 无 | 无 |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效目标引用（无 `--verify`） | `LBR-CLI-003` | 129 |
| `--verify` 失败（无 `--quiet`） | `LBR-REPO-003` | 128 |
| `--verify` 失败（有 `--quiet`） | （静默） | 1 |
| 无效工作树状态 | `LBR-REPO-003` | 128 |
| 无法读取仓库元数据 | `LBR-IO-001` | 128 |
| 存储的引用/配置损坏 | `LBR-REPO-002` | 128 |
