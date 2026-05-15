# Cloud 命令改进详细计划

> 最后编写时间：2026-05-16

本文记录第 33 批中 `cloud` 的当前落地状态。`cloud` 同时包含本地状态查询和远端 D1/R2 写入/恢复流程；本轮在 `cloud status` 之外补齐了 `cloud sync` 的结构化成功输出与 JSON progress 事件入口，但 `restore` 结构化 schema 与 typed error 收口仍未完成。

## 当前已落地

- `cloud status` 已接入 `run_cloud_status()` + `render_cloud_status_output()`。
- `cloud status --json` / `--machine` 输出 `cloud.status` command envelope。
- 结构化 status payload 包含 `repo_id`、`total_objects`、`synced`、`pending`、`synced_percent`、`by_type`。
- `cloud status --verbose` 的结构化输出补充最多 20 个 `unsynced_objects`，字段为 `oid`、`object_type`、`size`。
- status 路径的本地 object index 查询失败映射到 `LBR-IO-001`。
- CLI 回归测试覆盖空仓库的 JSON 与 machine 输出，且不依赖 Cloudflare 凭据。
- `cloud sync` 在 `--json` / `--machine` / `--quiet` 路径下改为 silent runner（`run_cloud_sync()` + `SilentCloudSyncProgress`），不再泄露 legacy human progress。
- `cloud sync --json` / `--machine` 成功路径新增 `cloud.sync` envelope，payload 包含 `repo_id`、`project_name`、`total_unsynced`、`synced_count`、`failed_count`、`metadata`、`agent_capture`。
- `cloud sync` 在 structured/quiet 路径仍保留原有失败语义：有 failed objects 时返回错误退出码，不输出成功 envelope。
- `cloud sync --progress=json` 新增 `cloud_sync.*` 事件流（objects / metadata / agent_capture 三阶段），事件写入 stderr，且不再混入 legacy stdout progress。
- CLI 回归测试覆盖 `cloud sync --json --progress=json` 与 `cloud sync --progress=json` 失败前置校验路径，验证存在 `cloud_sync.start` 事件并且无 `Starting cloud sync...` human 文本。

## 当前未完成

- `cloud restore` 的 D1/R2 成功摘要、metadata-only 摘要和 agent-capture restore 摘要还没有统一结构化 schema。
- 远端 D1/R2 错误目前仍多以字符串形式进入 `cloud_cli_error()`，尚未拆成 typed `CloudError`。

## 后续切片建议

1. 抽 `CloudError` typed enum，覆盖 env 缺失、D1、R2、repo-name 冲突、metadata 和 agent-capture 错误映射。
2. 为 `cloud restore --metadata-only --json` 建立最小成功 schema，再扩展 full restore 和 agent-capture restore 摘要。
