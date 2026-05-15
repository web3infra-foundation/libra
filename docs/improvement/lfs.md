# LFS 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 33 批中 `lfs` 的当前落地状态。`lfs` 同时包含本地 attributes / index 操作和远端 lock API；本轮先收口成功输出层，不改变网络协议行为。

## 当前已落地

- `execute_safe(cmd, output)` 已接入 `run_lfs()` + `render_lfs_output()`。
- 成功路径已有 `LfsOutput`，`--json` / `--machine` 可覆盖 `track`、`untrack`、`locks`、`lock`、`unlock`、`ls-files`。
- `track` / `untrack` 不再在执行层直接打印，改由渲染层输出 human 文案。
- `ls-files` JSON 输出包含 LFS path、显示 OID、pointer/full marker 和可选 size。
- `lfs locks` 的 human path-width 计算不再使用生产 `unwrap()`。

## 当前未完成

- `lock` / `unlock` / `locks` 仍依赖真实 LFS server，网络错误分层沿用现有 `LFSClient` 行为。
- 远端 lock API 的 mock server / contract test 仍未补齐。
- `LfsOutput` 仍是命令内 schema；如后续和 `push` 的 LFS upload summary 共享字段，需要再抽公共类型。

## 后续切片建议

1. 为 `locks` / `lock` / `unlock` 建 LFS mock server，覆盖成功、403、409、unexpected status。
2. 把 `LFSClient::get_locks()` 内部 JSON parse `unwrap()` 改为 typed network/protocol error，并让 `lfs` 命令映射到 `LBR-NET-*`。
3. 视需要扩展 `ls-files` JSON：增加 `full_oid`，保持当前 `oid` 显示语义向后兼容。
