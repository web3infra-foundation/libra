# `libra usage`

Report and prune AI provider/model usage aggregates for the current repository.

## Synopsis

```bash
libra usage report [OPTIONS]
libra usage prune [--retention-days <days>]
```

## Description

`libra usage` reads usage rows recorded by Libra's AI provider runtime and
aggregates them by provider/model. Reports can be filtered by time range,
session id, thread id, and whether failed provider requests should be included.
When a provider reports an exact `cost_usd`, Libra stores and displays that
value. Otherwise it estimates `cost_estimate_micro_dollars` from the built-in
model capability pricing table or a repository override in `.libra/config.toml`.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `report` | Aggregate usage rows; currently supports `--by model` |
| `prune` | Delete usage rows older than the configured retention window |

## Report Options

| Flag | Description |
|------|-------------|
| `--by model` | Aggregation dimension; `model` is the current supported value |
| `--since <time>` | Start filter; accepts RFC3339, `YYYY-MM-DD`, or relative values like `24h` / `7d` |
| `--until <time>` | End filter; accepts RFC3339, `YYYY-MM-DD`, or relative values like `1h` |
| `--session <id>` | Restrict to one provider session id |
| `--thread <id>` | Restrict to one canonical thread id |
| `--include-failed` | Include failed provider requests in request counts and wall-clock totals |
| `--format human|json|csv` | Select report format; global `--json` also forces JSON |

## Prune Options

| Flag | Description |
|------|-------------|
| `--retention-days <days>` | Keep rows newer than this many days; overrides `[usage].retention_days`; defaults to `90` |

## Human Output

Human reports print one tab-separated row per provider/model:

```text
<provider>	<model>	requests=<n>	failed=<n>	tokens=<n>	cached=<n>	reasoning=<n>	tool_calls=<n>	wall_ms=<n> [ $<actual>| ~$<estimate>]
```

CSV mode prints a header row followed by comma-separated rows suitable for
spreadsheet import. The CSV columns include both `cost_usd` and
`cost_estimate_micro_dollars`; estimated human output is prefixed with `~$`.

## Pricing Overrides

Repository-local usage settings live in `.libra/config.toml`. Price overrides
are keyed by provider and model. Values are micro-dollars per million tokens:

```toml
[usage]
retention_days = 90

[usage.pricing.openai."gpt-4o-mini"]
input_micro_dollars_per_mtok = 150000
output_micro_dollars_per_mtok = 600000
cached_micro_dollars_per_mtok = 75000
reasoning_micro_dollars_per_mtok = 600000
```

If the config is missing or a provider/model has no built-in or overridden
price, the usage row is still written and `cost_estimate_micro_dollars` remains
empty.

`libra usage prune` uses `[usage].retention_days` when `--retention-days` is not
passed. The flag always wins over the project config.

## JSON Output

`report` uses the `usage.report` envelope:

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

`prune` uses `usage.prune` and reports the retention window, cutoff timestamp,
and deleted row count.

## Examples

```bash
# Per-model totals across all recorded rows
libra usage report

# Per-model totals for the last 24 hours
libra usage report --since 24h

# Include failed requests in counts and wall-clock totals
libra usage report --since 7d --include-failed

# Restrict the report to a single session
libra usage report --session <session-id>

# Restrict the report to a single canonical thread
libra usage report --thread <thread-uuid>

# CSV table for downstream tooling (spreadsheets, BI dashboards)
libra usage report --format csv

# Structured JSON envelope for agents
libra usage --json report --since 7d

# Use the configured retention window (.libra/libra.db config `[usage].retention_days`)
libra usage prune

# Delete rows older than 30 days
libra usage prune --retention-days 30
```

The same banner is rendered by `libra usage --help` so the doc and the
CLI surface stay in sync (cross-cutting `--help` EXAMPLES rollout, see
`docs/improvement/README.md` item B).

## Notes

- The command requires a Libra repository because usage rows live in
  `.libra/libra.db`.
- Relative time filters are evaluated at command runtime and normalized to
  RFC3339 before querying.
- Retention windows must be greater than `0`; invalid config fails closed before
  deleting rows.
