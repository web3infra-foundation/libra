# `libra rev-parse`

解析修订名，并打印规范化的提交 ID、符号引用或仓库路径。

## 概要

```bash
libra rev-parse [OPTIONS] [SPEC]
```

## 说明

`libra rev-parse` 会将类似修订的输入解析为脚本友好的值：

- 完整提交 ID（默认）
- 使用 `--short` 得到的短提交 ID
- 使用 `--abbrev-ref` 得到的符号分支名
- 使用 `--symbolic-full-name` 得到的完整符号引用名
- `--git-dir`、`--show-prefix`、`--is-inside-work-tree` 等仓库路径/状态值
- `A..B`、`A...B`、`^A` 等范围端点流

它还支持 `--show-toplevel`，用于打印工作树的绝对仓库根目录；支持 `--verify`，用于断言参数必须解析为单个对象。未提供 `<SPEC>` 时，命令默认为 `HEAD`，如果提供了 `--default <SPEC>` 则使用该默认修订。

## 选项

| 标志 | 说明 |
|------|-------------|
| `--short` | 打印无歧义的缩写对象 ID。 |
| `--abbrev-ref` | 打印符号分支名，而不是提交哈希。 |
| `--verify` | 要求参数解析为单个对象并打印；否则失败。可与 `--short` 组合，和 `--abbrev-ref`、路径/状态参数、shell 引用模式互斥。 |
| `--default <SPEC>` | 未提供位置参数 `<SPEC>` 时使用的修订。 |
| `--show-toplevel` | 打印顶层工作树的绝对路径。 |
| `--git-dir` | 打印 Libra 存储目录（`.libra`，不是 `.git`）。 |
| `--show-prefix` | 打印当前目录相对工作树根目录的路径，使用 `/`，非空时带尾随 `/`。 |
| `--show-cdup` | 打印从当前目录回到工作树根目录所需的 `../` 路径。 |
| `--is-inside-git-dir` | 当前目录位于 `.libra` 内时打印 `true`，否则打印 `false`。 |
| `--is-inside-work-tree` | 位于工作树内打印 `true`，位于 `.libra` 内打印 `false`；仓库外执行为 fatal。 |
| `--is-bare-repository` | 打印解析后的 `core.bare` 值。 |
| `--sq` | 先解析每个修订，再在一行中输出 shell 引用后的结果。 |
| `--sq-quote` | 按字面 shell 引用位置参数，不需要仓库。 |
| `--symbolic` | 在 Libra 能保留时优先输出符号输入形式。 |
| `--symbolic-full-name` | 将分支、远程跟踪分支、tag 名解析为完整 `refs/...` 名称。 |
| `<SPEC>...` | 要解析的修订或范围表达式。除 `--verify` 和 `--sq-quote` 外，省略时默认为 `HEAD`。 |

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
libra rev-parse --show-prefix
libra rev-parse --git-dir
libra rev-parse HEAD~1..HEAD
libra rev-parse --sq HEAD main
libra rev-parse --sq-quote -x "a b"
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

在 `src/command` 下使用 `--show-prefix`：

```text
src/command/
```

使用两点范围：

```text
<HEAD hash>
^<HEAD~1 hash>
```

使用 `--sq-quote`：

```text
 '-x' 'a b'
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

`mode` 是 `resolve`、`short`、`abbrev_ref`、`verify`、`show_toplevel`、`git_dir`、`show_prefix`、`show_cdup`、`is_inside_git_dir`、`is_inside_work_tree`、`is_bare_repository`、`range`、`symbolic` 或 `symbolic_full_name` 之一。

范围和多 spec JSON 输出保持 `value` 为文本输出的换行拼接，并额外提供有序 `values`：

```json
{
  "ok": true,
  "command": "rev-parse",
  "data": {
    "mode": "range",
    "input": "HEAD~1..HEAD",
    "value": "<HEAD hash>\n^<HEAD~1 hash>",
    "values": ["<HEAD hash>", "^<HEAD~1 hash>"]
  }
}
```

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 解析完整提交 ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| 缩写提交 ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| 符号分支名 | `--abbrev-ref` | `--abbrev-ref` | N/A |
| 校验单个对象 | `--verify`（退出 128，或在 `-q` 下退出 1） | `--verify` | N/A |
| 默认修订 | `--default <SPEC>` | `--default` | N/A |
| 工作树根目录 | `--show-toplevel` | `--show-toplevel` | `jj root` |
| 路径/状态查询 | `--git-dir`、`--show-prefix`、`--show-cdup`、`--is-inside-git-dir`、`--is-inside-work-tree`、`--is-bare-repository` | 全部支持 | `jj root`（部分） |
| Shell 引用/范围 | `--sq`、`--sq-quote`、`A..B`、`A...B`、`^A` | 全部支持 | revsets |
| JSON 输出 | `--json` | 无 | 无 |

## 有意差异

- `--git-dir` 返回 Libra 的 `.libra` 存储目录，而不是 `.git`。
- 未使用 `--verify` 时，无效目标返回 Libra 的 invalid-target 退出码 129；Git 的部分同类场景返回 128。
- `--verify` 是单对象模式。Libra 会拒绝它与路径/状态参数组合，而不是输出混合流。
- `--sq-quote -- -x` 中的前导 `--` 会被 clap 当作选项终止符消费，不进入引用输出。要传前导连字符字面值，请使用 `--sq-quote -x "a b"`。
- `--symbolic-full-name` 覆盖本地分支、远程跟踪分支和 tag；更冷门的 Git symbolic 形式仍为 partial。

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效目标引用（无 `--verify`） | `LBR-CLI-003` | 129 |
| `--verify` 失败（无 `--quiet`） | `LBR-REPO-003` | 128 |
| `--verify` 失败（有 `--quiet`） | （静默） | 1 |
| 无效工作树状态 | `LBR-REPO-003` | 128 |
| 无法读取仓库元数据 | `LBR-IO-001` | 128 |
| 存储的引用/配置损坏 | `LBR-REPO-002` | 128 |
| shell 引用模式与 `--json`/`--machine` 组合 | `LBR-CLI-002` | 129 |
