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

## 当前未完成

- 远端 lock API 的 mock server / contract test 仍未补齐。
- `LfsOutput` 仍是命令内 schema；如后续和 `push` 的 LFS upload summary 共享字段，需要再抽公共类型。

## 后续切片建议

1. 为 `locks` / `lock` / `unlock` 建 LFS mock server，覆盖成功、403、409、unexpected status。
2. 视需要扩展 `ls-files` JSON：增加 `full_oid`，保持当前 `oid` 显示语义向后兼容。
