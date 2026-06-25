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

它还支持 `--show-toplevel`，用于打印工作树的绝对仓库根目录。未提供 `<SPEC>` 时，命令默认为 `HEAD`。

## 选项

| 标志 | 说明 |
|------|-------------|
| `--short` | 打印无歧义的缩写对象 ID。 |
| `--sq` | 对解析出的对象名做单引号 shell 引用，便于安全地交给 shell 消费。仅影响解析出的修订输出，不影响 `--show-toplevel` 等查询模式。 |
| `--abbrev-ref` | 打印符号分支名，而不是提交哈希。 |
| `--symbolic-full-name` | 将 spec 解析为完整 ref 名（`refs/heads/<分支>`、`refs/tags/<标签>`、`refs/remotes/<远程>/<分支>`，分离 HEAD 时为 `HEAD`）。有效但非 ref 的对象不输出（退出码 0）；不可解析名以退出码 128 失败。 |
| `--show-toplevel` | 打印顶层工作树的绝对路径。 |
| `--git-dir` | 打印 `.libra` 目录路径（Libra 的 `$GIT_DIR`）；在 Libra 中始终为绝对路径。 |
| `--absolute-git-dir` | 同 `--git-dir`，但始终为规范化后的绝对路径。（Libra 中 `--git-dir` 已是绝对路径，故两者一致。） |
| `<SPEC>` | 要解析的修订。省略时默认为 `HEAD`。 |

## 常用命令

```bash
libra rev-parse
libra rev-parse HEAD~1
libra rev-parse --short HEAD
libra rev-parse --abbrev-ref HEAD
libra rev-parse --show-toplevel
libra rev-parse --absolute-git-dir
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

`mode` 是 `resolve`、`short`、`abbrev_ref`、`symbolic_full_name`、`show_toplevel`、`show_prefix`、`show_cdup`、`is_inside_work_tree`、`is_inside_git_dir`、`is_bare_repository`、`git_dir` 或 `absolute_git_dir` 之一。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 解析完整提交 ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| 缩写提交 ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| 符号分支名 | `--abbrev-ref` | `--abbrev-ref` | N/A |
| 完整 ref 名 | `--symbolic-full-name` | `--symbolic-full-name` | N/A |
| Shell 引用输出 | `--sq` | `--sq` | N/A |
| 工作树根目录 | `--show-toplevel` | `--show-toplevel` | `jj root` |
| JSON 输出 | `--json` | 无 | 无 |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效目标引用 | `LBR-CLI-003` | 129 |
| 无效工作树状态 | `LBR-REPO-003` | 128 |
| 无法读取仓库元数据 | `LBR-IO-001` | 128 |
| 存储的引用/配置损坏 | `LBR-REPO-002` | 128 |
