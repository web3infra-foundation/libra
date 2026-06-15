# `libra rev-list`

列出从某个修订可达的提交对象。

## 概要

```bash
libra rev-list [OPTIONS] [SPEC]
```

## 说明

`libra rev-list` 会将修订输入解析为提交，遍历可达历史，应用可选的计数/限制过滤，并按从新到旧的顺序打印提交 ID。省略 `<SPEC>` 时，命令默认为 `HEAD`。

## 选项

| 标志 | 说明 |
|------|-------------|
| `-n <N>`, `--max-count <N>` | 排序后最多输出 `N` 个提交。 |
| `--skip <N>` | 输出或计数前跳过前 `N` 个提交。 |
| `--count` | 只打印过滤后的提交数量。 |
| `<SPEC>` | 要从中枚举的修订。默认为 `HEAD`。 |

## 常用命令

```bash
libra rev-list
libra rev-list HEAD
libra rev-list --count HEAD
libra rev-list -n 5 HEAD
libra rev-list --skip 5 --max-count 10 HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
libra --json rev-list HEAD
```

## 人类可读输出

输出为每行一个提交 ID。使用 `--count` 时，输出为单行十进制数量。

```text
abc1234def5678901234567890abcdef12345678
def5678901234567890abcdef12345678abc1234
```

## 结构化输出

```json
{
  "ok": true,
  "command": "rev-list",
  "data": {
    "input": "HEAD",
    "commits": [
      "abc1234def5678901234567890abcdef12345678",
      "def5678901234567890abcdef12345678abc1234"
    ],
    "total": 2,
    "count_only": false,
    "max_count": null,
    "skip": 0
  }
}
```

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 默认目标 | `HEAD` | `HEAD` | 当前修订 |
| 修订导航 | `HEAD~1`、标签、远程引用 | 相同 | revsets |
| 计数与限制 | `--count`、`-n` / `--max-count`、`--skip` | 相同 | revset 函数 |
| JSON 输出 | `--json` | 无 | 无 |
| 排序 | 最新优先 | 可达性顺序 | 取决于 revset |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效目标引用 | `LBR-CLI-003` | 129 |
| 无法读取仓库元数据 | `LBR-IO-001` | 128 |
| 存储的引用/对象损坏 | `LBR-REPO-002` | 128 |
