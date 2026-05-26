# LFS 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 33 批中 `lfs` 的当前落地状态。`lfs` 同时包含本地 attributes / index 操作和远端 lock API；本轮先收口成功输出层，不改变网络协议行为。

## 当前已落地

- `execute_safe(cmd, output)` 已接入 `run_lfs()` + `render_lfs_output()`。
- 成功路径已有 `LfsOutput`，`--json` / `--machine` 可覆盖 `track`、`untrack`、`locks`、`lock`、`unlock`、`ls-files`。
- `track` / `untrack` 不再在执行层直接打印，改由渲染层输出 human 文案。
- `ls-files` JSON 输出包含 LFS path、显示 OID、pointer/full marker 和可选 size。
- `lfs locks` 的 human path-width 计算不再使用生产 `unwrap()`。
- `LFSClient::get_locks()` 不再使用 `unwrap()`，改为 typed `LockListError`（request/http/decode）；`lfs` 命令层已映射到稳定错误码（`LBR-NET-001` / `LBR-NET-002` / `LBR-AUTH-002`）。
- `src/command/lfs.rs` 增加 `map_lock_list_error()` 单测，覆盖 forbidden、decode、http-5xx detail 映射。
- `src/internal/protocol/lfs_client.rs` 新增本地 mock server 合约测试，覆盖 lock API 关键路径：`get_locks` 成功解析、`get_locks` 403、`lock` 409、`unlock` 500。
- `tests/command/lfs_test.rs` 新增 CLI 级 mock 回归，覆盖 `locks` 成功 / 403→`LBR-AUTH-002`、`lock` 成功 / 409→`LBR-CONFLICT-002`、`unlock --force --id` 成功，验证 `--json` envelope 和稳定错误码。
- `src/internal/protocol/lfs_client.rs` 新增 batch 协议合约回归：`push_object` 在 batch response 返回错误对象数（v0.17.232）、`download_object` 在 batch response 缺少 `download` action（v0.17.233）、`upload_object` 在 batch response 缺少 `upload` action（v0.17.234），均必须返回 typed 错误（`LfsPushError` 或 `anyhow::Error`）并在 detail/消息中携带请求 oid，不再 panic。
- `ls-files --json` 新增 `full_oid` 字段，始终携带 64 字符 canonical hash；原 `oid` 字段保持显示语义（默认 10 字符前缀，`--long` 时为全长）向后兼容。`tests/command/lfs_test.rs::test_lfs_ls_files_json_output` 已断言 `full_oid.len() == 64` 且 `full_oid.starts_with(oid)`。
- `libra lfs --help` 与 [docs/commands/lfs.md](../commands/lfs.md) 已同步实际属性文件名 `.libra_attributes`；`tests/command/lfs_test.rs::test_lfs_help_lists_examples_banner` 防止回归到旧的 `.libraattributes` 拼写。
- `src/internal/protocol/lfs_client.rs` 生产路径 unwrap 审计已完成：v0.17.237 / v0.17.254-256 将所有 production `.unwrap()` 调用迁移为 `.expect("...")` 并附带 INVARIANT 注释，覆盖 `from_url` trait 实现、`new()` 构造路径、`lock` / `unlock` / `verify_locks` / `get_locks` / `upload_object` 内部 retry loop 和 URL join，违约时的 panic 消息会指明被破坏的具体契约（例如 `LFSClient::from_url: derived LFS server URL did not parse`、`LFS upload file disappeared between metadata check and File::open`、`LFS lock id failed to compose a valid relative URL segment`）。
- `tests/compat/lfs_client_production_unwrap_guard.rs`（v0.17.260）作为防御性 guard 扫描 `src/internal/protocol/lfs_client.rs` 生产区域；新增 bare `.unwrap()` 会让 `compat_lfs_client_production_unwrap_guard` 测试失败，且失败消息会指向 `.expect("INVARIANT: ...")` 修复路径。`src/utils/lfs.rs` 也被 `compat_extra_production_unwrap_guard`（v0.17.266）一并扫描。

## 当前未完成

- 暂无（`LfsOutput` 与 `LfsUploadSummary` 已抽取到 `src/command/lfs_schema.rs`，并通过共享类型接入 `push` 的 LFS 摘要输出）。
