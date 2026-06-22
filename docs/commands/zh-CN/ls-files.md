# `libra ls-files`

列出索引中的已跟踪条目以及工作区中的未跟踪路径。

## 用法

```bash
libra ls-files [OPTIONS] [pathspec]...
```

## 说明

`libra ls-files` 会读取 Libra 的索引和工作区，并输出仓库路径；它不会修改
refs、索引、工作区或对象存储。未提供状态筛选时，默认使用 cached 视图，
因此即使工作区副本已经修改或删除，已跟踪路径仍会被列出。

当前公开兼容面支持 cached 列表、modified / deleted 过滤、stage 样式输出、
未跟踪文件列表、通过 `--others --exclude-standard` 基于 `.libraignore`
的忽略过滤、仓库内 pathspec 过滤、`--error-unmatch`、通过 `-z`
输出 NUL 分隔文本记录、通过 `-t` 输出状态标签，以及通过 `-u` / `--unmerged`
只列出冲突条目。`--full-name` 作为 no-op 接受（Libra 始终输出仓库根相对路径）。

Pathspec 会相对于调用命令时的当前工作目录解析，而不是强制相对于仓库根目录。
它既支持精确文件匹配，也支持目录前缀过滤；若 pathspec 解析到仓库外，则会报错。
仅 ignored / 显式 exclude-source 模式、resolve-undo 和 sparse-checkout 集成仍未公开。

## 选项

| 选项 | 说明 |
|------|------|
| `--cached` | 显示 cached 索引条目；在没有状态筛选时这是默认行为。 |
| `--deleted` | 显示工作区文件已缺失的已跟踪路径。 |
| `--modified` | 显示工作区内容哈希与索引不同的已跟踪路径。 |
| `--stage` | 以 stage 样式输出记录；若存在冲突阶段也会显示。 |
| `-s` | stage 样式输出的短别名：`<mode> <object> <stage>\t<path>`。 |
| `-t` | 在每行路径前加状态标签：`H`（cached）、`R`（removed/deleted）、`C`（modified/changed）、`?`（other/untracked）、`M`（unmerged）。 |
| `-u`, `--unmerged` | 只列出未合并（冲突）条目——索引 stage 1/2/3——以 stage 样式输出。 |
| `--full-name` | 为 Git 兼容而接受。Libra 始终输出仓库根相对路径（即 `git --full-name` 形式），因此该标志为 no-op。 |
| `--others` | 显示未跟踪的工作区文件。 |
| `--exclude-standard` | 与 `--others` 一起使用时，遵循 `.libraignore` 规则。 |
| `--error-unmatch` | 只要任一显式 pathspec 在当前筛选结果中没有命中，就以 `LBR-CLI-003` 退出。 |
| `-z` | 输出 NUL 分隔的文本记录而不是换行分隔；仅限文本模式，不能与 `--json` / `--machine` 组合。 |
| `<pathspec>...` | 将输出限制为精确文件或目录前缀；相对于当前工作目录解析。 |
| `--json` | 输出标准 Libra JSON 信封。 |
| `--machine` | 以一行紧凑 JSON 输出同一信封。 |

## 示例

```bash
libra ls-files
libra ls-files --modified
libra ls-files --deleted
libra ls-files --others
libra ls-files --others --exclude-standard
libra ls-files tracked-dir
libra ls-files --others --exclude-standard others-dir
libra ls-files --error-unmatch src/lib.rs
libra ls-files -z tracked-dir
libra ls-files --stage
libra ls-files -t
libra ls-files -t --others --exclude-standard
libra ls-files -u
libra --json ls-files --modified
```

## 人类可读输出

默认输出每行一个仓库路径：

```text
.libraignore
tracked.txt
```

`--stage` 和 `-s` 会输出 Git 风格的 stage 记录：

```text
100644 4f3c2d1a7b8c9d0e1234567890abcdef12345678 0	tracked.txt
```

`-z` 保持相同的记录内容，但用 NUL 而不是换行结尾，适合脚本安全消费：

```text
tracked-dir/alpha.txt\0tracked-dir/bravo.txt\0
```

## 结构化输出

`--json` 和 `--machine` 使用标准 Libra 命令信封。`data` 中的每个条目都包含
`path`、`hash`、`mode`、`stage` 与 `status`。未跟踪条目在不适用的字段上使用
`null`：

```json
{
  "ok": true,
  "command": "ls-files",
  "data": [
    {
      "path": "tracked.txt",
      "hash": "4f3c2d1a7b8c9d0e1234567890abcdef12345678",
      "mode": "100644",
      "stage": 0,
      "status": "modified"
    },
    {
      "path": "untracked.txt",
      "hash": null,
      "mode": null,
      "stage": null,
      "status": "other"
    }
  ]
}
```

## 参数对比：Libra vs Git vs Jujutsu

| 功能 | Libra | Git | Jujutsu |
|------|-------|-----|---------|
| Cached 索引列表 | 默认 / `--cached` | 默认 / `--cached` | 使用 status / file 命令 |
| 已跟踪且已修改文件 | `--modified` | `--modified` | 使用 status / diff 命令 |
| 已跟踪且已删除文件 | `--deleted` | `--deleted` | 使用 status 命令 |
| Stage 样式输出 | `--stage` / `-s` | `--stage` / `-s` | 模型不同 |
| 未跟踪文件 | `--others` | `--others` | 使用 status / file 命令 |
| 带忽略规则的未跟踪列表 | `--others --exclude-standard` | 相同 | 模型不同 |
| Pathspec 过滤 | `<pathspec>...` | 支持 | 模型不同 |
| 未命中 pathspec 报错 | `--error-unmatch` | `--error-unmatch` | 模型不同 |
| NUL 输出 | `-z`（仅文本模式） | `-z` | 模型不同 |
| 状态标签 | `-t`（H/R/C/?/M） | `-t`（H/S/M/R/C/K/?） | 模型不同 |
| 未合并条目 | `-u` / `--unmerged` | `-u` / `--unmerged` | 模型不同 |
| 根相对路径 | `--full-name`（始终；no-op 标志） | `--full-name`（按需） | 模型不同 |
