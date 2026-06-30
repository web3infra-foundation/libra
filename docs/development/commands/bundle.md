# bundle 命令开发设计

## 命令实现目标

`libra bundle create/verify/list-heads` —— 创建与检查 Git v2 bundle 文件。GGT-13 互操作池命令之一（独立增量）。create 产出可被系统 Git `clone`/`fetch` 的完整 bundle。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`create <file> <rev>...`（完整 bundle：`# v2 git bundle` 头 + `<oid> <ref>` heads + 空行 + v2 pack）、`verify <file>`（头/pack 魔数/prerequisite 存在性）、`list-heads <file>`。pack 用仓库 hash kind 编码（SHA-1 + SHA-256）。
- **延后**：prerequisite/thin/增量 `<rev>..<rev>` bundle；`unbundle` 与通过 libra 从 bundle 克隆（用 `git clone`）；`verify` 仅查头+pack 魔数，非完整 pack 校验和（用 index-pack/fsck）。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::Bundle` → `command::bundle::execute_safe`（require_repo）→ create/verify/list_heads。
- 源码分层：`src/command/bundle.rs`：`BundleArgs`/`BundleSubcommand`（Create{file,revs}/Verify{file}/ListHeads{file}）、`create`/`collect_tree`/`encode_pack`/`verify`/`list_heads`/`parse_header`/`resolve_ref_name`。
- create：每 rev → `util::get_commit_base`（tip）+ `resolve_ref_name`（head 名）；可达对象 = `log::get_reachable_commits` 的每个 commit（Entry::from）+ `collect_tree`（递归收集 tree+subtree+blob 的 oid，gitlink 跳过）；去重用 HashSet。
- encode_pack：复用生产 pack 写入器 `PackEncoder`（push.rs/local_client.rs 同款）—— channel 喂 `MetaAttached<Entry, EntryMeta>`，spawned task 内 `set_hash_kind(get_hash_kind())` 后 `encoder.encode(rx)`，收集 pack 字节。hash-kind 正确（SHA-1/256）。
- 写文件：先写临时 `.{name}.tmp` 再 `fs::rename` 到目标；任一步失败删临时文件（无半成品）。
- parse_header：逐行到空行；首行必须 `# v2 git bundle`（`# v3` 拒绝）；`-<oid> <comment>` = prerequisite，`<oid> <ref>` = head；返回 pack_offset。
- verify：prerequisite 必须本地存在（`util::objects_storage().get`），pack 必须 `PACK`+version2；否则退出 1。
- 底层操作对象：读对象库（commit/tree/blob）→ pack；写 bundle 文件。不写对象库/refs。

## 实现历史

- 2026-06-30（GGT-13 / 3，`grit-gap.md` 阶段 6）：互操作池第三个命令；独立增量。

## 当前状态

- 公开状态：已公开（`Commands::Bundle`）。
- 测试：`tests/command/bundle_test.rs`（create 写 v2 bundle [签名+ref+`\n\nPACK`]、list-heads 列 refs、verify 接受所创 bundle [`is okay`]、verify 拒绝非 bundle **1**（与 `git bundle verify` 一致；128 仅用于用法错误）、create 坏 rev 128 且无半成品、非仓库 128）+ `bundle.rs` 单测（解析 v2 头/prerequisite、拒绝缺签名/v3）。
- 用户文档：`docs/commands/bundle.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 范围 | prerequisite/thin/增量 `<rev>..<rev>` | 延后；仅完整 bundle。 |
| 消费 | `unbundle` / 从 bundle 克隆（libra 侧） | 用 `git clone <file>`；记录延后。 |
| 校验 | 完整 pack 校验和 | verify 仅查头+魔数；完整用 index-pack/fsck。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- pack 编码必须复用 `PackEncoder` 并在 spawned task 内传播 hash kind；不得手写 SHA-1-only 的 pack（参考已弃用的 maintenance::create_pack_from_hashes 教训）。
