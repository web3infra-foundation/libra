# Remote 命令改进详细计划

## 所属批次

第五批：远程管理（P1 对齐）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `run_remote()` + `render_remote_output()` 已完成执行层/渲染层拆分
- `RemoteOutput` 已覆盖 `add` / `remove` / `rename` / `-v` / `show` / `get-url` / `set-url` / `prune`
- `RemoteError` 已完成显式 `StableErrorCode` 映射，覆盖 duplicate remote、missing remote、URL pattern 不匹配、config read/write、prune 写失败和远端发现失败
- `remote -v` 已修复多 URL 展示：所有 fetch URL 都会逐行显示，显式 `pushurl` 会优先于 fetch URL fallback
- `docs/commands/remote.md` 已记录 human/JSON 输出、错误码和子命令行为
- `tests/command/remote_test.rs` 已补充 verbose 多 URL、JSON `get-url`、duplicate add 错误码，以及原有 prune 回归

### 基于当前代码的 Review 结论
- 本批之前，`remote` 功能基本齐全，但仍停留在裸 `println!()` / message-based error 的旧输出层
- 本批已把 `remote` 收口到和前四批一致的结构：稳定错误码、结构化输出、明确确认消息和文档同步
- 出于兼容性，本批保留现有 `remote show` 语义为“列出已配置 remote 名称”；如果后续要完全对齐 Git 的 `remote show <name>` 细节展示，应作为独立变更处理

## 目标与非目标

**已完成目标：**
- JSON / machine 输出
- duplicate/missing remote、prune 失败等场景的显式错误码
- `remote -v` 多 fetch/push URL 对齐
- prune 的结构化结果和 human 确认消息

**后续维护目标：**
- 继续维护 verbose 多 URL、JSON schema、prune dry-run/失败路径回归
- 继续观察 `remote show` 兼容语义是否需要独立 modernize

**本批非目标：**
- 不引入 `remote show <name>` 的 Git 全量展示模型
- 不新增 remote URL 语法校验策略
- 不改动 config_kv 中 remote/vault SSH key 的存储布局

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test remote_test`
4. `docs/commands/remote.md` 与命令输出、错误码保持一致
