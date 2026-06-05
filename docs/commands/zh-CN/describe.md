# `libra describe`

查找某个提交可达的最近标签，并将其格式化为人类可读的版本描述。

**别名：** `desc`

## 概要

```
libra describe [OPTIONS] [COMMIT]
```

## 说明

`libra describe` 从给定提交（默认 `HEAD`）开始遍历提交祖先图（BFS），查找最近的标签。输出遵循 Git 的 describe 格式：

- 精确匹配：`v1.2.3`
- 带距离的可达标签：`v1.2.3-4-gabc1234`
- 回退（`--always`）：`abc1234`

默认只考虑附注标签。传递 `--tags` 可同时匹配轻量标签。当多个标签以相同距离可达时，优先选择附注标签；仍相同时按字典序打破平局。

当找不到标签且未使用 `--always` 时，命令会失败，并给出建议使用 `--tags` 或 `--always` 的可操作提示。

## 选项

| 标志 | 说明 | 默认值 |
|------|-------------|---------|
| `<COMMIT>` | 要描述的 commit-ish。接受 `HEAD`、分支名、标签名、原始 SHA-1、`HEAD~N`。 | `HEAD` |
| `--tags` | 在搜索中包含轻量标签（而不只是附注标签）。 | 关闭 |
| `--abbrev <N>` | 输出中缩写提交哈希的十六进制位数。 | `7` |
| `--always` | 当没有标签可描述目标时，回退到缩写提交哈希，而不是失败。 | 关闭 |
| `--exact-match` | 仅当标签恰好指向该提交（距离 0）时成功，否则失败。 | 关闭 |
| `--first-parent` | 遍历历史时仅跟随合并提交的第一个父节点。 | 关闭 |
| `--match <PATTERN>` | 仅考虑名称匹配该 glob 的标签（可重复；按任一匹配）。 | — |
| `--exclude <PATTERN>` | 排除名称匹配该 glob 的标签（可重复；优先级高于 `--match`）。 | — |
| `--dirty[=<MARK>]` | 当工作区存在已跟踪改动时追加标记（默认 `-dirty`）。仅有未跟踪文件不计入。 | 关闭 |
| `--contains` | 反向搜索：以 `<refname>~<offset>` 形式打印哪个引用的历史包含该提交。默认含轻量标签。 | 关闭 |
| `--candidates <N>` | 最多考虑 N 个候选标签（拒绝 `0`；可由 `describe.maxCandidates` 覆盖）。 | `describe.maxCandidates` |
| `--all` | 与 `--contains` 配合时，同时搜索本地分支头与远程跟踪分支（不只是标签）。 | 关闭 |

Glob 模式使用 [`wax`](https://docs.rs/wax) 语法，长度上限 256 字符；超长或非法模式按用法错误拒绝（`LBR-CLI-002`，退出码 129）。

### 示例

```bash
# 仅使用附注标签描述 HEAD
libra describe

# 包含轻量标签
libra describe --tags

# 即使没有标签也始终产生输出
libra describe --always

# 描述特定提交
libra describe HEAD~5

# 使用更长的缩写哈希
libra describe --abbrev 12

# 仅在精确标签处成功
libra describe --exact-match

# 遍历合并时仅跟随第一个父节点
libra describe --first-parent

# 按 glob 过滤标签（排除优先于匹配）
libra describe --match 'v1.*' --exclude '*-rc*'

# 工作区有已跟踪改动时追加 -dirty（或自定义标记）
libra describe --dirty
libra describe --dirty=-wip

# 反向查找：哪个引用包含该提交？
libra describe --contains HEAD~3
libra describe --contains --all HEAD~3

# 限制候选标签搜索数量
libra describe --candidates 5

# 面向自动化的 JSON 输出
libra describe --json
```

## 常用命令

```bash
libra describe
libra describe --tags
libra describe --always
libra describe HEAD~1
libra describe --json
libra describe --tags --abbrev 10
libra describe --match 'v*' --exclude '*-rc*'
libra describe --dirty
libra describe --contains HEAD~1
libra describe --candidates 5
```

## 人类可读输出

- 精确标签匹配：`v1.2.3`
- 可达标签：`v1.2.3-4-gabc1234`
- `--always` 回退：`abc1234`

`--quiet` 会抑制 `stdout`。

## 结构化输出（JSON 示例）

`--json` / `--machine` 返回：

### 标签匹配（精确）

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3",
    "tag": "v1.2.3",
    "distance": 0,
    "abbreviated_commit": null,
    "exact_match": true,
    "used_always": false
  }
}
```

### 标签匹配（带距离）

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3-4-gabc1234",
    "tag": "v1.2.3",
    "distance": 4,
    "abbreviated_commit": "abc1234",
    "exact_match": false,
    "used_always": false
  }
}
```

### 回退（`--always`，未找到标签）

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "abc1234",
    "tag": null,
    "distance": null,
    "abbreviated_commit": "abc1234",
    "exact_match": false,
    "used_always": true
  }
}
```

当使用 `--always` 且没有标签匹配时，`tag` 和 `distance` 为 `null`，`abbreviated_commit` 包含输出的哈希。

### `--contains`（反向查找）

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD~3",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3~3",
    "tag": "v1.2.3",
    "distance": null,
    "abbreviated_commit": null,
    "exact_match": false,
    "used_always": false,
    "dirty": false,
    "dirty_suffix": null,
    "contains_offset": 3,
    "ref_kind": "tag",
    "ref_name": "v1.2.3"
  }
}
```

输出对象始终携带以下追加字段：

- `dirty` / `dirty_suffix`——当 `--dirty` 检测到已跟踪改动时设置（追加到 `result` 的后缀）。
- `contains_offset`——`--contains` 的 `~N` 偏移。
- `ref_kind`（`"tag"` / `"head"` / `"remote"`）与 `ref_name`（如 `heads/main`）——在 `--contains`（及 `--all`）下填充；命中分支/远程时 `tag` 为 `null`。

## 设计理由

### match/exclude、dirty、contains、candidates 现已支持

早期版本曾有意提供 `git describe` 的最小子集，并把 `--match`、`--exclude`、`--candidates`、`--first-parent`、`--dirty` 标注为「刻意不实现」。**该决策已被推翻。** 版本生成脚本与发布工具链广泛依赖这些标志（`vX.Y.Z-N-gHASH`、`-dirty` 后缀、`--contains` 偏移），缺失会破坏互操作。Libra 现已实现 `--first-parent`、`--exact-match`、`--match`、`--exclude`、`--dirty`、`--contains`、`--candidates`、`--all`，同时保留若干有意差异（见下文）。

### 仍然简化：没有 `--long`

Libra 始终产生标准 `tag-N-gHASH` 格式（精确匹配时仅标签名）。没有 `--long` 标志来强制精确匹配使用长格式。JSON 输出已经包含独立的 `tag`、`distance`、`abbreviated_commit` 和 `exact_match` 字段，任何需要区分精确/非精确匹配的消费者都可直接检查 `exact_match`——这比仅改变字符串格式的 Git `--long` 信息更丰富。

### BFS 最短路径 vs Git 的候选启发式

Git 的 `describe` 使用带剪枝的启发式以避免遍历整个图。Libra 从目标提交开始使用有界 BFS，保证找到最近标签（DAG 中的最短路径）。`--candidates` 与 `describe.maxCandidates` 限定收集的候选标签数量，但由于 BFS 已返回拓扑最近的标签，*结果*与 `N` 无关、确定可复现。遍历上限为 10,000 个提交；更深的历史会以 `LBR-REPO-003`（退出码 128）失败，除非指定 `--always`。

### `--abbrev` 固定默认 7

Libra 的 `--abbrev` 固定默认 7 位；Git 动态选择足以唯一的最短长度。需要确定性的脚本应显式传 `--abbrev=<N>`。

### `--all` 范围（部分）

`--all` 把 `--contains` 的候选引用集扩展到本地分支头与远程跟踪分支（标签之外）。它不枚举 `refs/notes` 或 `refs/stash`，也不改变默认（正向）describe（始终解析为最近标签）。`--json` / `--machine` 输出为 Libra 扩展，Git 无对应项。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 默认目标 | `HEAD` | `HEAD` | N/A（无内置 describe） |
| 仅附注标签 | 默认行为 | 默认行为 | N/A |
| 包含轻量标签 | `--tags` | `--tags` | N/A |
| 缩写哈希长度 | `--abbrev <N>`（默认 7） | `--abbrev=<N>`（默认动态选择） | N/A |
| 回退到哈希 | `--always` | `--always` | N/A |
| 仅精确匹配 | `--exact-match` | `--exact-match` | N/A |
| 强制长格式 | 未实现（使用 JSON `exact_match`） | `--long` | N/A |
| 匹配标签模式 | `--match <glob>`（wax，可重复） | `--match <glob>` | N/A |
| 排除标签模式 | `--exclude <glob>`（wax，可重复） | `--exclude <glob>` | N/A |
| 候选数量 | `--candidates=<N>` / `describe.maxCandidates`（结果仍为最近） | `--candidates=<N>`（默认 10） | N/A |
| 仅 first-parent | `--first-parent` | `--first-parent` | N/A |
| Dirty 后缀 | `--dirty[=<mark>]`（仅已跟踪改动） | `--dirty[=<mark>]` | N/A |
| Contains 查找 | `--contains` → `<refname>~N` | `--contains` | N/A |
| 所有引用 | `--all`（标签 + 分支头 + 远程，配合 `--contains`；部分） | `--all` | N/A |
| JSON 输出 | `--json`，带类型字段 | 无 | 无 |
| 算法 | 有界 BFS（最短路径，≤10,000 提交） | 启发式多候选 | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效修订 / commit-ish | `LBR-CLI-003` | 129 |
| 无效参数（`--match`/`--exclude` glob 超长或非法；`--candidates=0`） | `LBR-CLI-002` | 129 |
| `--abbrev=-1` 等 clap 解析错误 | （clap） | 2 |
| `HEAD` 没有提交 | `LBR-REPO-003` | 128 |
| 无标签/引用可描述目标且未使用 `--always` | `LBR-REPO-003` | 128 |
| `--exact-match` 但无精确标签 | `LBR-REPO-003` | 128 |
| `--contains` 但无包含目标的引用且未使用 `--always` | `LBR-REPO-003` | 128 |
| 历史遍历超过 10,000 个提交且未使用 `--always` | `LBR-REPO-003` | 128 |
| 无法读取引用或对象 | `LBR-IO-001` / `LBR-REPO-002` | 128 |
