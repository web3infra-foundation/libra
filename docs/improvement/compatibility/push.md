# C8：Push 引用更新面补齐

## 所属批次

C8（后续 Git surface P1）

## 当前代码状态

- [`docs/commands/push.md`](../../commands/push.md) 已同步 C8 surface：裸 `libra push`、多个 update refspec、delete syntax `:ref`、显式 tag refspec、`--tags`、`--mirror --dry-run` 与 JSON `updates[].kind`。
- [`src/command/push.rs`](../../../src/command/push.rs) 的 CLI 参数已改为 `refspecs: Vec<String>`，新增 `--tags` / `--mirror`，并在任何网络写入前构建完整 ref update plan、拒绝非法 refname 和重复 destination。
- receive-pack 请求现在一次发送完整 update set；delete 使用 zero oid；branch/tag 更新、mirror delete 和 dry-run 共享同一 `PushRefUpdate` schema。服务端逐 ref status 行会被校验，缺失/拒绝状态不会被渲染成全成功。
- `validate_receive_pack_response` 已有回归契约覆盖：所有 expected refs 必须收到 `ok`，服务端 `ng` 会保留 ref 与原因，缺失 expected status 或未知 status 行会 fail closed。
- 本地 file remote push 仍显式拒绝，并继续由 [`declined.md`](declined.md#d2-本地-file-remote-的-push) 记录为 intentionally-different。
- [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) 中 `push` 仍为 `partial`，notes 已更新为 C8 已支持的 branch/tag update、multi-refspec、delete、`--tags` 与 `--mirror`。

## 为什么排第二

`push` 是远端协作的出口。当前基础分支推送可用，但 tag 发布、远端删除、多 ref 更新、mirror 同步和完整 refspec 都是 Git 用户迁移时会遇到的高频管理操作。补齐这部分可以显著降低“代码能提交但发布/清理远端 refs 仍要回退到 Git”的割裂感。

## 目标与非目标

**目标：**

- 支持多个 refspec：`libra push origin main feature:release refs/tags/v1.0:refs/tags/v1.0`。
- 支持删除远端 ref：`libra push origin :feature`，并评估是否同时暴露 `--delete <name>...`。
- 支持 tag 推送：显式 tag refspec、单个 tag、`--tags` 批量推送本地 tags。
- 支持 `--mirror`，按 Git 语义同步 refs 并删除远端多余 refs；必须有清晰危险提示和 dry-run 覆盖。
- 扩展 structured output，让 `updates` 能同时表达 branch update、tag update、delete、mirror delete、forced update。
- 保持 `--dry-run` 对所有新增 destructive / bulk 行为可用，且输出足够说明将更新或删除哪些 ref。

**非目标：**

- 不重启本地 file remote push；该项继续由 [`declined.md`](declined.md#d2-本地-file-remote-的-push) 记录为 intentionally-different。
- 不实现 Git LFS filter / hook bridge；`libra push` 的内置 LFS 上传路径保持 Libra 自身设计。
- 不绕过远端 branch protection；服务端拒绝仍映射为用户可理解的 remote rejected 错误。
- 不让普通 `libra push origin main` 隐式删除或 mirror 任何 ref； destructive 行为必须来自显式语法或 flag。

## 设计要点

### Refspec parser

把当前单个 `ParsedRefspec` 扩展为 ref update plan。计划中至少区分：

| 类型 | 示例 | 语义 |
|-----|------|-----|
| Update | `main`, `src:dst`, `refs/tags/v1.0:refs/tags/v1.0` | 上传对象并更新远端 ref |
| Delete | `:feature`, `:refs/heads/feature` | 删除远端 ref，不收集 source objects |
| Tags | `--tags` | 推送本地 tags 中远端缺失或需要更新的 tags |
| Mirror | `--mirror` | 同步本地 refs 到远端，包含删除远端多余 refs |

解析阶段必须在任何网络写入前完成全部 ref validation，避免前几个 ref 已推送、后一个 refspec 才发现非法的局部失败。

### 传输与原子性

优先使用一次 receive-pack 请求提交完整 update set。若某个 transport 无法提供等价原子语义，必须在文档和 structured output 中明确；不允许本地状态显示为全部成功而远端只完成部分更新。

### 安全默认值

`--mirror` 和 deletion 是高风险操作。human 输出需要突出 deleted refs；JSON/machine 必须有 `kind: "delete"` 或等价字段。`--dry-run --mirror` 是验收必测路径。

## `COMPATIBILITY.md` 行更新

C8 落地后更新 `push` 行：

```markdown
| push | partial | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected intentionally |
```

`push` 仍保持 `partial`，因为本地 file remote push 继续是有意差异，且完整 Git push 还包含更多 server-option / atomic / signed push 等高级 surface。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/push.rs`](../../../src/command/push.rs) | 修改 | CLI flags、multi-refspec parser、update plan、delete/tag/mirror 输出 |
| [`src/internal/protocol/`](../../../src/internal/protocol/) | 评估/修改 | receive-pack update set、delete ref、tag object 传输 |
| [`docs/commands/push.md`](../../commands/push.md) | 修改 | 删除旧的 unsupported 表述，补 tag/delete/mirror 示例 |
| [`docs/improvement/compatibility/declined.md`](declined.md) | 保持/引用 | D2 本地 file remote push 仍是 intentional difference |
| [`tests/command/push_test.rs`](../../../tests/command/push_test.rs) | 修改 | parser、delete、tags、mirror、dry-run、JSON schema |
| [`tests/compat/`](../../../tests/compat/) | 新增/修改 | Git surface 兼容回归，可用本地 fake remote 或 test-network fixture |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | `push` notes 更新 |

## 测试与验收

- [x] `parse_refspec` 接受多个 update refspec，拒绝非法 refname，并在网络写入前一次性报错。
- [x] `libra push origin :feature` 删除远端 `refs/heads/feature`，JSON 输出中该 update 标记为 delete。
- [x] `libra push origin refs/tags/v1.0:refs/tags/v1.0` 推送 tag ref 和对应 tag/object 数据；`--tags` 共享同一路径并由 fake SSH remote 回归验证。
- [x] `libra push --tags origin` 推送本地 tags，已存在且相同的 tag 不重复更新。
- [x] `libra push --mirror --dry-run origin` 展示将新增、更新、强制更新和删除的 refs，不写远端。
- [x] 服务端拒绝部分 ref update 时，错误信息列出被拒 ref 和原因，不产生误导性的全成功摘要。
- [x] receive-pack parser 回归覆盖 `ok` 全量状态、`ng` 拒绝、缺失 expected status 与未知 status 行。
- [x] 本地 file remote push 仍返回 documented intentional difference，不被新增 refspec parser 意外放开。
- [x] `cargo +nightly fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`source .env.test && cargo test --test command_test push_test` 通过。

## 风险与缓解

1. **bulk / mirror 操作误删远端 refs**：destructive 行为只由显式 `:ref`、`--delete` 或 `--mirror` 触发，并要求 dry-run 测试覆盖。
2. **multi-refspec 部分成功导致用户误判**：先构建完整 update plan，再一次提交；transport 不支持时必须 fail closed 或明确降级语义。
3. **tag 推送遗漏 annotated tag object**：tag ref 更新必须把 tag object 及其指向对象纳入 object collection。
4. **与 D2 决策冲突**：本批只补网络 remote push surface，不重启本地 file remote push。
