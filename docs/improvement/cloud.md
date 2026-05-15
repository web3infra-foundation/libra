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
- `cloud_cli_error()` 已落地 `CloudError` typed enum（`MissingEnv` / `NameAlreadyTaken` / `NameNotFound` / `PartialTransfer` / `D1` / `R2` / `Generic`），集中 String → `StableErrorCode` 的分类映射；由 `cloud_error_classifies_each_failure_shape` + `cloud_error_into_cli_error_attaches_stable_codes` 单测锁定。底层 helper 仍返回 `Result<_, String>`，但 CLI 层先转 `CloudError` 再映射 `CliError`，分类逻辑只此一处。

## 当前未完成

- 底层 D1/R2/repo-name/metadata/agent-capture helper 仍以 `Result<_, String>` 返回；后续可改为各自直接构造 `CloudError` 变体，省去 `From<String>` 的字符串再分类。

## 后续切片建议

1. 把 D1/R2/repo-name/metadata/agent-capture 各 helper 的返回类型从 `Result<_, String>` 改为 `Result<_, CloudError>`，让 `CloudError` 成为分类源头而不仅是 CLI 边界的"重新分类"层。
2. 抽 `cloud restore` typed error 分层（name 解析、D1、R2、object restore、agent-capture restore），替换字符串拼接错误链。
