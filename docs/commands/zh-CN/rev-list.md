# `libra rev-list`

列出从某个修订可达的提交对象。

## 概要

```bash
libra rev-list [SPEC]
```

## 说明

`libra rev-list` 会将修订输入解析为提交，遍历可达历史，并按从新到旧的顺序打印提交 ID。省略 `<SPEC>` 时，命令默认为 `HEAD`。

## 选项

| 标志 | 说明 |
|------|-------------|
| `<SPEC>` | 要从中枚举的修订。默认为 `HEAD`。 |

## 常用命令

```bash
libra rev-list
libra rev-list HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
libra --json rev-list HEAD
```

## 人类可读输出

输出为每行一个提交 ID。

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
    "total": 2
  }
}
```

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 默认目标 | `HEAD` | `HEAD` | 当前修订 |
| 修订导航 | `HEAD~1`、标签、远程引用 | 相同 | revsets |
| JSON 输出 | `--json` | 无 | 无 |
| 排序 | 最新优先 | 可达性顺序 | 取决于 revset |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无效目标引用 | `LBR-CLI-003` | 129 |
| 无法读取仓库元数据 | `LBR-IO-001` | 128 |
| 存储的引用/对象损坏 | `LBR-REPO-002` | 128 |
