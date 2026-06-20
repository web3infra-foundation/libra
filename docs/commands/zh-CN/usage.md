# `libra usage`

报告并修剪当前仓库的 AI provider/model 和 agent 用量聚合。

## 概要

```bash
libra usage report [OPTIONS]
libra usage prune [--retention-days <days>]
```

## 说明

`libra usage` 读取由 Libra 的 AI provider runtime 记录的用量行，并按 provider/model、agent 或 agent/provider/model 聚合。报告可按时间范围、session id、thread id，以及是否包含失败的 provider 请求进行过滤。
当 provider 报告精确 `cost_usd` 时，Libra 会存储并显示该值。否则，它会从内置模型能力定价表或 `.libra/config.toml` 中的仓库覆盖项估算 `cost_estimate_micro_dollars`。

## 子命令

| 子命令 | 说明 |
|------------|-------------|
| `report` | 按 model、agent 或 agent/provider/model 聚合用量行 |
| `prune` | 删除早于已配置保留窗口的用量行 |

## Report 选项

| 标志 | 说明 |
|------|-------------|
| `--by model` | 按 provider/model 聚合；这是默认值，并保留原始输出形状 |
| `--by agent` | 只按声明式 agent 名称聚合 |
| `--by agent-provider-model` | 按 agent 名称、provider 和 model 聚合 |
| `--since <time>` | 起始过滤器；接受 RFC3339、`YYYY-MM-DD`，或 `24h` / `7d` 这样的相对值 |
| `--until <time>` | 结束过滤器；接受 RFC3339、`YYYY-MM-DD`，或 `1h` 这样的相对值 |
| `--session <id>` | 限制为一个 provider session id |
| `--thread <id>` | 限制为一个规范 thread id |
| `--include-failed` | 在请求计数和 wall-clock 总计中包含失败的 provider 请求 |
| `--format human|json|csv` | 选择报告格式；全局 `--json` 也会强制 JSON |

## Prune 选项

| 标志 | 说明 |
|------|-------------|
| `--retention-days <days>` | 保留比该天数更新的行；覆盖 `[usage].retention_days`；默认为 `90` |

## 人类可读输出

人类报告为所选分组的每一行打印一条制表符分隔记录。

默认 provider/model 分组：

```text
<provider>	<model>	requests=<n>	failed=<n>	tokens=<n>	cached=<n>	reasoning=<n>	tool_calls=<n>	wall_ms=<n> [ $<actual>| ~$<estimate>]
```

Agent 分组：

```text
<agent_name>	requests=<n>	failed=<n>	tokens=<n>	cached=<n>	reasoning=<n>	tool_calls=<n>	wall_ms=<n> [ $<actual>| ~$<estimate>]
```

Agent/provider/model 分组：

```text
<agent_name>	<provider>	<model>	requests=<n>	failed=<n>	tokens=<n>	cached=<n>	reasoning=<n>	tool_calls=<n>	wall_ms=<n> [ $<actual>| ~$<estimate>]
```

CSV 模式打印一个表头行，后面跟适合导入电子表格的逗号分隔行。CSV 前导列匹配所选分组，指标列同时包含 `cost_usd` 和 `cost_estimate_micro_dollars`；估算的人类可读输出以 `~$` 为前缀。

## 定价覆盖

仓库本地用量设置位于 `.libra/config.toml`。价格覆盖按 provider 和 model 作为 key。值为每百万 token 的微美元：

```toml
[usage]
retention_days = 90

[usage.pricing.openai."gpt-4o-mini"]
input_micro_dollars_per_mtok = 150000
output_micro_dollars_per_mtok = 600000
cached_micro_dollars_per_mtok = 75000
reasoning_micro_dollars_per_mtok = 600000
```

如果配置缺失，或某个 provider/model 没有内置或覆盖价格，用量行仍会写入，`cost_estimate_micro_dollars` 保持为空。

当未传递 `--retention-days` 时，`libra usage prune` 使用 `[usage].retention_days`。该标志始终优先于项目配置。

## JSON 输出

`report` 使用 `usage.report` 信封：

```json
{
  "ok": true,
  "command": "usage.report",
  "data": {
    "by": "model",
    "filter": {
      "since": null,
      "until": null,
      "session": null,
      "thread": null,
      "include_failed": false
    },
    "rows": []
  }
}
```

`prune` 使用 `usage.prune`，并报告保留窗口、cutoff 时间戳和删除行数。

## 示例

```bash
# 所有记录行的按模型总计
libra usage report

# 最近 24 小时的按模型总计
libra usage report --since 24h

# 按 agent 总计
libra usage report --by agent

# 按 agent 的 provider/model 明细
libra usage report --by agent-provider-model

# 在计数和 wall-clock 总计中包含失败请求
libra usage report --since 7d --include-failed

# 将报告限制到单个 session
libra usage report --session <session-id>

# 将报告限制到单个规范 thread
libra usage report --thread <thread-uuid>

# 面向下游工具（电子表格、BI 仪表板）的 CSV 表
libra usage report --format csv

# 面向代理的结构化 JSON 信封
libra usage --json report --since 7d

# 使用已配置的保留窗口（.libra/libra.db config `[usage].retention_days`）
libra usage prune

# 删除超过 30 天的行
libra usage prune --retention-days 30
```

`libra usage --help` 会渲染同一横幅，因此文档和 CLI 表面保持同步（跨命令 `--help` EXAMPLES 推出，见 `docs/development/commands/_general.md` 条目 B）。

## 说明

- 该命令需要 Libra 仓库，因为用量行位于 `.libra/libra.db` 中。
- 相对时间过滤器会在命令运行时求值，并在查询前规范化为 RFC3339。
- 保留窗口必须大于 `0`；无效配置会在删除行前失败关闭。
