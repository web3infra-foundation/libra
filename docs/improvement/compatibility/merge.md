# C7：Merge 三方合并与冲突生命周期

## 所属批次

C7（后续 Git surface P1）

## 当前代码状态

- [`docs/commands/merge.md`](../../commands/merge.md) 当前对外契约已覆盖 fast-forward、single-head three-way、冲突 lifecycle、`--continue` / `--abort`。
- [`src/command/merge.rs`](../../../src/command/merge.rs) 保留 fast-forward / already-up-to-date 路径；diverged 单目标 merge 会执行三方合并，无冲突时创建双父 merge commit，有冲突时写 marker、index stage 和 Libra merge state。
- [`src/command/status.rs`](../../../src/command/status.rs) 会在 merge state 存在时提示 `libra merge --continue` / `libra merge --abort`。
- [`src/command/pull.rs`](../../../src/command/pull.rs) 复用同一 merge engine；non-fast-forward pull 可 clean three-way，冲突时返回 merge-owned `LBR-CONFLICT-002` 并带 `phase: "merge"`。
- [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) 中 `merge` 已升级为 `supported`（见下文「后续批次」），`pull` 仍为 `partial`。

## 为什么排第一

`merge` 是 Git 真实协作工作流的核心缺口。只支持 fast-forward 可以服务线性历史，但不能覆盖多人同时提交后的常规 `pull` / `merge` 路径，也无法给迁移用户提供可恢复的冲突生命周期。补齐三方合并会直接改善 `merge`、`pull`、协作开发和 AI agent 自动集成分支的可靠性。

## 目标与非目标

**目标：**

- `libra merge <branch>` 在当前分支与目标分支已分叉时执行真实三方合并，使用 merge base 计算差异。
- 无冲突时创建双父 merge commit，更新 HEAD、索引、工作树和 reflog，并保留现有 fast-forward / already-up-to-date 行为。
- 有冲突时写入用户可编辑的冲突标记，记录可恢复的 merge-in-progress 状态，并返回稳定冲突错误码。
- 增加 `merge --continue` 与 `merge --abort`，让用户在解决冲突后完成合并或安全回退。
- 让 `pull` 在 fetch 后可以进入相同的三方合并与冲突生命周期，而不是直接停在 non-fast-forward。
- 更新 human / JSON / machine 输出，明确 `strategy`、`conflicted_paths`、`merge_commit`、`parents`、`aborted`、`continued` 等字段的稳定语义。

**非目标：**

- 不在本批实现 octopus merge、多目标 merge、`--strategy` / `-X` 自定义策略、`ours` / `subtree` 等高级策略。
- 不在本批实现 `--squash`、`--no-ff`、签名校验、自动编辑 merge message 等完整 Git flag 面。
- 不降低 dirty worktree / untracked overwrite 保护；任何会覆盖用户本地修改的 merge 必须先拒绝。
- 不承诺暴露原生 `.git/MERGE_HEAD` 文件契约；Libra 可以用自身仓库状态层保存等价 merge state。

## 设计要点

### 三方合并核心路径

实现前必须先明确对象和索引层是否已经支持冲突表达。如果当前 index 不能表达 staged conflict entries，本批应先引入 Libra 自身的 merge state 记录，再开启冲突路径，避免只写工作树冲突标记却无法可靠 continue / abort。

最小成功路径：

```text
resolve target -> find merge base -> verify clean worktree -> three-way merge trees
  -> no conflicts: write tree -> create merge commit with two parents -> update HEAD/index/worktree
  -> conflicts: write conflict markers -> record merge state -> return conflict error
```

### 冲突生命周期

`merge --abort` 必须能恢复 merge 开始前的 HEAD、index 和 worktree 快照。`merge --continue` 必须验证所有冲突路径已经解决并 staged，然后创建双父 merge commit。错误信息需要说明下一步命令，例如 `libra status`、`libra add <path>`、`libra merge --continue`、`libra merge --abort`。

### `pull` 复用

`pull` 不应实现第二套冲突状态机。fetch 完成后，如果需要整合远端提交，应调用同一套 merge engine。`pull --json` / `--machine` 应在输出中标明 fetch 阶段结果和 merge 阶段结果，冲突时返回 merge-owned stable code。

## `COMPATIBILITY.md` 行更新

C7 落地后（仅单目标三方合并）一度保持 `partial`。后续的「完整 merge 面补齐」批次（见下文，v0.17.1266..1274）补齐了 Git merge 的高频 flag/config 面，`merge` 行已升级为 `supported`：

```markdown
| merge | supported | fast-forward, best-base single-head three-way (criss-cross LCA), clean disjoint octopus, --squash, --no-ff/--ff-only/merge.ff, --no-commit, -m/-F/--signoff/--log/--into-name, -e/--edit, ours strategy, -X ours/theirs, diff3 markers, --stat/merge.stat, --autostash/merge.autoStash, -S/--gpg-sign, --verify-signatures/merge.verifySignatures, whitespace-insensitive merge, rename detection (--find-renames/merge.renames), --diff-algorithm/--cleanup validation. Remaining gaps: subtree/custom strategies and drivers, conflicted-octopus resolution, directory-rename tracking; signature verification is a presence check |
| pull | partial | fetch + fast-forward/three-way merge supported; advanced strategy flags still partial |
```

## 后续批次：完整 merge 面补齐（v0.17.1266..1274）

C7 的最小三方合并落地后，`.omo/plans/merge-improvement-plan.md` 规划并实现了 Git merge 的剩余高频面，使 `merge` 行从 `partial` 升级为 `supported`。

**新增 flag / config（每项 ≥2 测试，`docs/commands/merge.md` 已同步）：**

- 提交面：`--into-name`、`-e`/`--edit`/`--no-edit`（`$GIT_EDITOR`/`core.editor`/`$VISUAL`/`$EDITOR`，无可用编辑器时原样使用）、`--cleanup <mode>` 校验。
- 输出面：`--stat`/`-n`/`--no-stat`（及 `--summary` 别名）与 `merge.stat` config，复用 `git_internal::diff::compute_diff` 生成 diffstat，默认 off 以保持既有输出稳定。
- 工作树面：`--autostash`/`--no-autostash` 与 `merge.autoStash`，冲突时把 reapply 延迟到 `--continue`/`--abort`（复用 `stash` push/pop）。
- 签名面：`-S`/`--gpg-sign`（vault key，不接受 keyid）与 `--verify-signatures`/`--no-verify-signatures` + `merge.verifySignatures`（presence check，非完整密钥学校验）。
- 文本面：`--ignore-space-change`/`--ignore-all-space`/`--ignore-space-at-eol`/`--ignore-cr-at-eol` 让仅空白改动的一侧让位给有真实改动的一侧。
- 合并质量：criss-cross 历史下的真实 LCA merge-base 选择；rename detection（`--find-renames`/`--no-renames` + `merge.renames`，50% Dice 相似度阈值），让一侧的编辑跟随另一侧的重命名，重命名后若仍冲突则回退到 delete/modify 冲突。
- 兼容接收 flag：`--diff-algorithm`（校验 `myers`/`histogram`/`patience`/`minimal`，后端为单一 Myers）、`--no-verify`、`--overwrite-ignore`/`--no-overwrite-ignore`、`--rerere-autoupdate`/`--no-rerere-autoupdate`、no-* 反向 flag。进度由全局 `--progress=<mode>` 覆盖，不再单独提供 merge 级开关。

**仍然 deferred / 受限（保留在 `docs/commands/merge.md` 的 Deferred 一节）：**

- 自定义 merge driver、`ours` 之外的自定义策略、subtree 策略。
- 进阶 octopus 冲突解决（多目标 octopus 仍只接受 clean disjoint 改动）。
- 目录级 rename 跟踪（仅处理 clean rename + 单侧编辑）。
- 签名校验为 presence check，不针对 keyring 做密钥学验证。

`merge.commit` 不实现，因为原生 Git 并未定义该 config key。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/merge.rs`](../../../src/command/merge.rs) | 修改 | 三方合并、merge state、`--continue` / `--abort`、输出 schema |
| [`src/command/pull.rs`](../../../src/command/pull.rs) | 修改 | 复用 merge engine，处理 non-fast-forward pull |
| `git_internal::internal::index` 及调用点 | 评估/修改 | 冲突路径表达与 staged resolution 检查 |
| [`src/internal/reflog.rs`](../../../src/internal/reflog.rs) / HEAD 相关模块 | 修改 | merge commit 与 abort/continue reflog 记录 |
| [`docs/commands/merge.md`](../../commands/merge.md) | 修改 | fast-forward-only 文档升级为三方合并契约 |
| [`docs/commands/pull.md`](../../commands/pull.md) | 修改 | 说明 pull 的 merge 阶段和冲突处理 |
| [`tests/command/merge_test.rs`](../../../tests/command/merge_test.rs) | 修改 | divergent clean merge、conflict、abort、continue、dirty refusal |
| [`tests/command/pull_test.rs`](../../../tests/command/pull_test.rs) | 修改 | fetch 后 non-FF pull 进入同一 merge lifecycle |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | `merge` / `pull` notes 更新 |

## 测试与验收

- [x] Fast-forward merge 与 already-up-to-date 行为保持兼容，现有 JSON schema 不破坏。
- [x] 两个分支修改不同文件时，`libra merge <branch>` 创建双父 merge commit。
- [x] 两个分支修改同一文件同一区域时，工作树出现冲突标记，命令返回 `LBR-CONFLICT-002`，`status` 能提示 continue / abort。
- [x] 解决冲突并 `libra add <path>` 后，`libra merge --continue` 创建双父 merge commit。
- [x] `libra merge --abort` 恢复 merge 开始前 HEAD、index 和 worktree。
- [x] Dirty worktree / untracked overwrite 场景在写入任何 merge state 前拒绝。
- [x] `libra pull` 在远端与本地分叉时复用同一 merge engine，并在冲突时给出相同下一步提示。
- [x] `cargo +nightly fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`source .env.test && cargo test --test command_test merge_test -- --test-threads=1`、`source .env.test && cargo test --test command_test pull_test -- --test-threads=1` 通过。

## 风险与缓解

1. **冲突状态只写工作树但不能可靠恢复**：先设计 merge state 与 abort 快照，再开放冲突路径。
2. **`pull` 和 `merge` 出现两套行为**：强制 `pull` 调用 merge engine，不复制冲突处理逻辑。
3. **自动 merge 覆盖本地修改**：所有写入前执行 dirty / untracked overwrite 检查，测试覆盖拒绝路径。
4. **把 `partial` 误升为 `supported`**：C7 本身只覆盖单目标三方合并，故当时保持 `partial`；后续「完整 merge 面补齐」批次落地后才升级为 `supported`，且 Deferred 一节明确记录 subtree/自定义策略/驱动、进阶 octopus 冲突解决、目录级 rename 与 presence-only 签名校验仍未实现。
