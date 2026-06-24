# `libra prune`

从仓库中删除不可达的 loose object。

## 概要

```bash
libra prune [OPTIONS] [HEAD]...
```

## 说明

`libra prune` 会从仓库 refs 和可选的额外 `HEAD` 参数出发计算可达对象，然后删除不可达且符合过期策略的 loose object。日常维护优先使用 [`libra gc`](./gc.md)，因为 `gc` 还会处理 reflog 过期和 pack 辅助文件清理。

## 选项

| 标志 | 短选项 | 说明 |
|------|--------|------|
| `--dry-run` | `-n` | 只报告将删除的对象，不实际删除 |
| `--verbose` | `-v` | 输出删除的对象 |
| `--expire <TIME>` | | 只删除早于指定时间的 loose object |
| `[HEAD]...` | | 额外保留从这些对象可达的对象 |

## 示例

```bash
libra prune
libra prune -n
libra prune -v --expire "2 weeks ago"
libra prune HEAD~2
```

## 结构化输出

支持全局 `--json` 和 `--machine`。成功时输出 `prune` 信封，包含 `objects`、`expire`、`heads`、`dry_run` 和 `verbose`。

## 注意事项

直接运行 prune 与 Git 一样有并发写入风险：如果另一个进程刚写入对象但尚未创建引用，prune 可能删除它仍要使用的 loose object。建议先用 `--dry-run` 预览，并在直接运行时搭配 `--expire`。
