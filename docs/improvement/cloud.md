# Cloud 命令改进详细计划

> 最后编写时间：2026-05-16

本文记录第 33 批中 `cloud` 的当前落地状态。`cloud` 同时包含本地状态查询和远端 D1/R2 写入/恢复流程；本轮在 `cloud status` / `cloud sync` 之外补齐了 `cloud restore` 的结构化成功输出入口，但 typed error 收口仍未完成。

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
- `cloud restore` 在 `--json` / `--machine` / `--quiet` 路径下接入 `run_cloud_restore()`，成功路径输出 `cloud.restore` envelope（metadata-only、对象恢复统计、metadata/agent-capture status）。
- `cloud restore` 的 structured 路径已静默 worktree/agent-capture human stdout（保留 stderr warning），避免污染 JSON stdout。
- `cloud_cli_error()` 已新增分类映射：缺失云端配置 → `LBR-AUTH-001`、repo-name not found → `LBR-CLI-003`、D1 失败 → `LBR-NET-002`、对象恢复/同步失败 → `LBR-CONFLICT-002`。

## 当前未完成

- 远端 D1/R2 错误虽然已做稳定错误码分类，但仍是字符串分类分支，尚未替换成 typed `CloudError` 执行链。

## 后续切片建议

1. 抽 `CloudError` typed enum，覆盖 env 缺失、D1、R2、repo-name 冲突、metadata 和 agent-capture 错误映射。
2. 抽 `cloud restore` typed error 分层（name 解析、D1、R2、object restore、agent-capture restore），替换字符串拼接错误链。
