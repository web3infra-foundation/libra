## Verify-Pack 命令改进详细计划

> 最后更新：2026-06-08

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

> **实施状态：✅ 已落地** — `verify-pack` 已完成 `.idx`/`.pack` 一致性校验、v1/v2 索引解析、SHA-1/SHA-256 格式推断、`-v`/`-s`、`--json`/`--machine`、`--pack` 显式 pack 路径、损坏诊断和 fsck 进程内复用。兼容级别保持 `partial`，因为 deltified 对象的 verbose 行不会输出 Git 的 `<chain-depth> <base-oid>` 两列。

### 已确认落地

- `src/command/verify_pack.rs` 提供 `verify_pack_path(idx_file, explicit_pack)`，供 `fsck` 进程内复用；不 fork 子进程，不打印输出，由调用方决定报告和退出码。
- `src/command/fsck.rs` 的 pack-integrity stage 会遍历 `.libra/objects/pack/*.idx`，对每个 pack 独立调用 `verify-pack` 核心校验；单个坏 pack 不会中断后续 pack 检查，最终让 fsck 以退出码 `1` 失败。
- `tests/command/verify_pack_test.rs` 覆盖 v1/v2、SHA-256、multi-index、stat-only、verbose、JSON、重复对象、缺失 pack、index checksum、pack checksum、offset、CRC32 损坏诊断。
- `tests/command/fsck_test.rs` 覆盖 healthy pack、corrupt pack、multi-corrupt continue 和 no-pack no-op。
- `docs/commands/verify-pack.md`、`docs/commands/zh-CN/verify-pack.md`、`docs/commands/fsck.md`、`COMPATIBILITY.md` 均记录 delta verbose 差异、fsck 联动和错误诊断。

### 当前契约

- 成功校验输出 `<idx>: ok`，退出码 `0`。
- 读不到 idx/pack 返回 `LBR-IO-001`，默认退出码 `128`。
- index/pack 校验和、对象数、OID、offset、CRC32、重复 OID 或解码失败返回 `LBR-REPO-002`，默认退出码 `128`。
- fsck pack-integrity stage 发现任一坏 pack 时退出 `1`，并继续报告其余坏 pack。
- 多索引 `libra verify-pack a.idx b.idx` 当前在第一个失败索引处短路；这是已文档化的有意差异。`--json`/`--machine` 只在全部索引成功时输出成功 payload。

### 后续维护目标

- 如果后续 `git-internal` 解码层暴露原始 delta base OID，可补齐 verbose 行尾 `<chain-depth> <base-oid>` 并重新评估兼容级别。
- 如需更贴近 Git 的 multi-index 失败行为，可把当前短路逻辑改为全量继续并汇总失败；该行为反转需要同步命令文档、兼容矩阵和新增测试。
- 若 `fsck` 未来增加结构化成功输出，再把 pack-integrity 摘要加入新的 fsck JSON schema；当前 fsck 仅复用全局错误 envelope。

### 已执行验收

- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- verify_pack --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- fsck --test-threads=1 --nocapture`

