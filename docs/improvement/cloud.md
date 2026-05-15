# Cloud 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 33 批中 `cloud` 的当前落地状态。`cloud` 同时包含本地状态查询和远端 D1/R2 写入/恢复流程；本轮先收口完全离线可验证的 `cloud status` 输出契约，不改变远端同步/恢复行为。

## 当前已落地

- `cloud status` 已接入 `run_cloud_status()` + `render_cloud_status_output()`。
- `cloud status --json` / `--machine` 输出 `cloud.status` command envelope。
- 结构化 status payload 包含 `repo_id`、`total_objects`、`synced`、`pending`、`synced_percent`、`by_type`。
- `cloud status --verbose` 的结构化输出补充最多 20 个 `unsynced_objects`，字段为 `oid`、`object_type`、`size`。
- status 路径的本地 object index 查询失败映射到 `LBR-IO-001`。
- CLI 回归测试覆盖空仓库的 JSON 与 machine 输出，且不依赖 Cloudflare 凭据。

## 当前未完成

- `cloud sync` / `cloud restore` 成功路径仍使用 legacy human progress 输出。
- `cloud sync` 的 object / metadata / agent-capture progress 还没有 JSON progress event 契约。
- `cloud restore` 的 D1/R2 成功摘要、metadata-only 摘要和 agent-capture restore 摘要还没有统一结构化 schema。
- 远端 D1/R2 错误目前仍多以字符串形式进入 `cloud_cli_error()`，尚未拆成 typed `CloudError`。

## 后续切片建议

1. 为 `cloud sync --json` 建立 quiet progress adapter，直接复用 `CloudSyncReport` 输出成功 schema。
2. 为 `cloud sync --progress=json` 定义 object / metadata / agent-capture progress event，并验证 JSON 模式不混入 human stdout。
3. 抽 `CloudError` typed enum，覆盖 env 缺失、D1、R2、repo-name 冲突、metadata 和 agent-capture 错误映射。
4. 为 `cloud restore --metadata-only --json` 建立最小成功 schema，再扩展 full restore 和 agent-capture restore 摘要。
