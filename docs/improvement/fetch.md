# Fetch 命令改进详细计划

## 所属批次

第五批：远程管理（P1 对齐）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `FetchRepositoryResult` 已升级为稳定的结构化结果模型，顶层 `FetchOutput` 支持单 remote 与 `--all`
- 顶层 `fetch` 已完成统一渲染：human 摘要、`--json` envelope、`--machine` 单行 JSON
- `FetchError` 已完成显式 `StableErrorCode` 映射，覆盖 invalid remote spec、discovery/auth/network、object-format mismatch、packet/protocol、pack/index、refs 更新和本地状态损坏
- JSON 模式默认会把进度事件以 NDJSON 写到 `stderr`，同时保持 `stdout` 只承载最终结果 envelope；`--machine` 会关闭进度并保持 `stderr` 干净
- `docs/commands/fetch.md` 已记录 JSON schema、human 输出和进度语义
- `tests/command/fetch_test.rs` 已补充 JSON schema、machine 单行输出和 JSON progress 事件回归；原有本地/SSH/host-key/vault key 测试继续覆盖传输层行为

### 基于当前代码的 Review 结论
- 本批之前，`fetch` 的核心传输能力已经成熟，但缺少顶层统一输出契约和稳定错误码
- 本批已把 `fetch` 与 `pull` / `clone` 共享的底层 helper 保持不变，只补命令层结果渲染、错误分类和进度契约，避免与现有调用方冲突
- JSON 模式的 `stderr` 不再承诺“成功时完全为空”；其职责改为承载可选 NDJSON progress 事件，这一点已在命令文档中明确

## 目标与非目标

**已完成目标：**
- JSON / machine 输出
- 显式 `StableErrorCode`
- human 摘要输出
- JSON progress 事件

**后续维护目标：**
- 继续维护 local/SSH/invalid-remote/progress 回归测试
- 若未来需要暴露 bytes received、sideband message 等更细粒度指标，应以向后兼容字段扩展现有 schema

**本批非目标：**
- 不改动底层 pack 协议、索引写入和 refs 更新算法
- 不在本批引入 `fetch --prune`，该职责继续由 `remote prune` 承担
- 不让 `pull` / `clone` 直接继承顶层 fetch 的 human/JSON 渲染

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test fetch_test`
4. `docs/commands/fetch.md` 与命令输出、错误码和进度语义保持一致

## 审计驱动增量（C3 浅克隆契约）

C3（[`compatibility/shallow.md`](compatibility/shallow.md)）在第五批已落地的
fetch 基线上叠加一个用户决策：把 `fetch_repository(..., depth)` 已存在的内部
能力公开为稳定 CLI flag。本节只承担兼容契约，不重新设计 fetch 主流程。

- `FetchArgs` 新增 `--depth <N>`，`run_fetch()` 把 depth 透传给现有
  `fetch_repository_with_result()` 调用点；不新增 `FetchError` 变体。
- `--depth` 在 `--help` 中**不带 experimental 标记**（用户决策：稳定公开）。
- 与 `--all` 组合时，depth 同时作用于全部被 fetch 的 remote。
- `clone --depth` 已存在；`COMPATIBILITY.md` 与 `docs/commands/clone.md` 同步
  记为 `supported`。`clone --sparse` / `clone --recurse-submodules` 显式登记
  `unsupported`，链接 [`compatibility/declined.md`](compatibility/declined.md)。
- 测试覆盖在 `tests/command/fetch_test.rs` 增量补齐 ≥3 条浅克隆用例（单分支
  depth 1、`--all --depth N`、已 shallow 仓库幂等 fetch）。
