## Checkout 命令改进详细计划

> 最后编写时间：2026-03-31

与 [switch.md](switch.md) 联动时，仅提前落地 Cross-Cutting Improvement **B**（`--help` EXAMPLES）；`A/F/G` 仍留待 [README.md](README.md#第六批辅助命令p2-增强) 中 `checkout` 的完整现代化批次统一处理。

本文是 `checkout` 的**兼容性收口计划**，用于和 [switch.md](switch.md) 同步落地，避免 `switch` 的 typed error 改造把 `checkout` 的现有行为搞坏。

> 当前工作区实现已按本文范围落地核心改动；以下内容继续作为验收边界、契约说明和后续批次分工文档。

**范围说明：**

- **本次落地**：`checkout` 对 `switch::ensure_clean_status()` 新返回类型的适配、`--help` EXAMPLES、相关回归测试补强。
- **留待后续**：`checkout` 自身完整的 JSON 输出、显式 `StableErrorCode`、执行层/渲染层拆分、typed `CheckoutError` 统一建模，仍按 [README.md](README.md#第六批辅助命令p2-增强) 的第六批执行。

因此，本文**不是**把 `checkout` 提前升级为完整现代化命令，而是记录与 `switch` 联动时必须一起收口的最小兼容面。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`switch` 的第二批改造正在推进，`checkout` 是它当前唯一直接依赖 `switch` 内部 helper 的用户可见命令。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `execute()` / `execute_safe(args, output)` 双入口已存在（`checkout.rs:33/42`）
- `checkout` 当前支持三种主场景：
  - `libra checkout`：显示当前分支
  - `libra checkout <branch>`：切换到现有本地/远程分支
  - `libra checkout -b <branch>`：创建并切换到新分支
- **当前分支 no-op 语义已实现**：若目标分支就是当前分支，直接输出 `Already on {branch}` 并返回成功，不先做脏状态检查（`checkout.rs:62-67`）
- **脏工作树检查已委托给 `switch::ensure_clean_status(output)`**（`checkout.rs:69-83`）
- `create_and_switch_new_branch()` 仍复用 `branch::create_branch_safe()` + `switch_branch_with_output()`（`checkout.rs:131-136`）
- 远程自动跟踪路径 `get_remote()` 仍复用 `branch::set_upstream_safe_with_output()` + `pull::execute_safe()`（`checkout.rs:138-148`）
- quiet / machine 下的人类文本抑制已有回归测试覆盖（`output_flags_test.rs:708-741`）
- `checkout_invalid_index_preserves_status_error()` 已验证状态损坏错误不会被折叠成“local changes would be overwritten”脏树消息（`output_flags_test.rs:746-766`）

**当前代码中已确认的跨命令依赖：**

- `checkout.rs` 直接调用 `switch::ensure_clean_status(output)`（**唯一会被 `switch` typed error 改造影响的接口**）
- `checkout.rs` 通过 `branch::create_branch_safe()` / `branch::set_upstream_safe_with_output()` 复用 branch 能力（不经过 switch，不受本次影响）
- `checkout.rs` 通过 `restore::execute_safe()` 复用工作树恢复逻辑（不经过 switch，不受本次影响）
- `checkout.rs` 通过 `pull::execute_safe()` 实现”发现 `origin/<branch>` 时自动创建 tracking branch 再 pull”（不受本次影响）

### 基于当前代码的 Review 结论

**当前代码已具备的合理行为：**

- **当前分支 no-op 不受脏工作树阻塞**：这和 Git 兼容，且已有测试覆盖
- **dirty worktree → checkout 专属文案**：当前 `checkout` 把 `switch` 的脏状态错误翻译成 `local changes would be overwritten by checkout`，保留了自己的命令语义
- **状态损坏错误不被误折叠**：索引损坏等 `status` 检测错误会继续透传，不会被错误映射成 dirty-tree
- **quiet / machine 语义基本正确**：现有输出 suppression 测试已覆盖成功路径
- **remote auto-track 兼容路径已存在**：虽然内部实现较旧，但行为上能工作

**当前代码仍需改进的部分：**

- **`checkout.rs` 依赖 `err.message()` 字符串匹配**：`checkout.rs:69-83` 通过错误文案判断是否是 dirty-tree，fragile 且无法在 message 变化时编译期发现
- **缺少 `--help` EXAMPLES 段**
- **无 checkout 专属改进文档**：当前所有联动约束都堆在 `switch.md`，后续容易双边漂移
- **完整现代化仍未开始**：无 JSON、无显式 `StableErrorCode`、无 `CheckoutError` typed enum、无执行层/渲染层拆分

### 目标与非目标

**本次目标：**

- 与 [switch.md](switch.md) 对齐：当 `switch::ensure_clean_status()` 改为返回 `Result<(), SwitchError>` 后，`checkout.rs` 同步改为**变体匹配**，消除字符串匹配
- 保持 `checkout` 现有对外行为不变：
  - 当前分支仍是 no-op
  - dirty-tree 仍报 `local changes would be overwritten by checkout`
  - 状态损坏仍直接透传原始错误
  - quiet / machine 输出约定保持不变
- 补齐 `checkout` 的 `--help` EXAMPLES 段
- 明确记录 `switch` / `checkout` 的边界，避免两个计划互相覆盖

**本次非目标：**

- **不实现 `checkout --json` / `--machine` 结构化成功输出**。这属于第六批完整现代化范围
- **不引入完整 `CheckoutError` typed enum**
- **不改写 `get_remote()` 的业务语义**。remote auto-track + pull 现有流程保持不变
- **不统一 `checkout` 和 `switch` 的成功文案**。`checkout` 继续保留自己的兼容语气，例如 `Already on {branch}`（无引号）
- **不新增 detach / commit / tag checkout 语义**
- **不把 `switch`/`checkout` 抽成共用执行层**

### 设计原则

1. **switch 联动优先，checkout 行为稳定优先**：`switch` 改 helper 签名时，`checkout` 必须同步，但不能借机改掉现有命令行为
2. **只替换脆弱实现，不扩大本批范围**：本次只把字符串匹配换成 `SwitchError` 变体匹配，不顺手做 JSON / typed error 全量重构
3. **checkout 保持自己的对外文案**：即使内部依赖 `switch`，外部仍是 `checkout` 语义，不和 `switch` 强行统一
4. **未来完整现代化留在第六批**：本次文档必须显式给未来 `CheckoutError` / JSON 输出留边界，避免与 README 排期冲突

### 特性 1：`switch::ensure_clean_status()` 新返回类型适配

**当前实现（fragile string matching）：**

```rust
match switch::ensure_clean_status(output).await {
    Ok(()) => {}
    Err(err)
        if matches!(
            err.message(),
            "unstaged changes, can't switch branch"
                | "uncommitted changes, can't switch branch"
        ) =>
    {
        return Err(CliError::failure(
            "local changes would be overwritten by checkout",
        ));
    }
    Err(err) => return Err(err),
}
```

**目标实现（typed variant matching）：**

```rust
use super::switch::SwitchError;

match switch::ensure_clean_status(output).await {
    Ok(()) => {}
    Err(SwitchError::DirtyUnstaged | SwitchError::DirtyUncommitted) => {
        return Err(CliError::failure(
            "local changes would be overwritten by checkout",
        ));
    }
    Err(err) => return Err(CliError::from(err)),
}
```

**关键约束：**

- 仅 `DirtyUnstaged` / `DirtyUncommitted` 被翻译成 checkout 文案
- `ensure_clean_status()` 当前只可能返回 `StatusCheck`、`DirtyUnstaged`、`DirtyUncommitted` 三个变体；catch-all arm 处理 `StatusCheck`（经 `impl From<SwitchError> for CliError` 转换为 `IoReadFailed`）以及未来可能新增的变体，不能被折叠成 dirty-tree
- `current branch` 的 no-op 快路径仍应在 cleanliness check 之前执行，避免脏工作树影响 `checkout <current-branch>`

### 特性 2：`--help` EXAMPLES 段

本次提前落地 Cross-Cutting **B**，与 `init` / `config` / `switch` 保持一致，通过 `const CHECKOUT_EXAMPLES: &str = ...` + clap `#[command(after_help = CHECKOUT_EXAMPLES)]` 接入。

```text
EXAMPLES:
    libra checkout                         Show the current branch
    libra checkout main                    Switch to an existing local branch
    libra checkout feature-x               Switch to another branch
    libra checkout -b feature-x            Create and switch to a new branch
    libra checkout --quiet main            Switch without informational stdout
```

**注意**：本次不写 `--json` 示例，因为 `checkout --json` 还不在当前范围内。

### 特性 3：与 `switch.md` 的边界约束

为避免两个文档互相打架，本计划明确以下边界：

- `switch.md` 负责定义 `SwitchError`、`ensure_clean_status()` 的新签名，以及 `switch` 自身输出/错误契约
- `checkout.md` 负责定义 `checkout` 如何消费这个新接口，并保持 `checkout` 的既有对外行为
- `run_switch()` 仍保持私有；`checkout` 不复用 `switch` 的执行层结果结构
- `checkout` 的完整 JSON / typed error / render split 不在本次联动范围内

### 特性 4：第六批完整现代化的预留边界

按照 [README.md](README.md#第六批辅助命令p2-增强)，`checkout` 的完整改造仍留在第六批。届时再单独推进：

- `CheckoutError` typed enum
- 显式 `StableErrorCode`
- `run_checkout()` + `render_checkout_output()` 执行/渲染拆分
- `checkout --json` 成功输出（至少覆盖 show-current / switch-local / create / remote-track）

本次**不提前设计这些 schema 细节**，只要求当前兼容收口不能阻碍第六批未来落地。

### 本次联动中的 Cross-Cutting Improvements 约束

| ID | 本次是否落地 | checkout 中的处理 |
|----|-------------|------------------|
| **A** | 否 | 当前兼容收口不引入新的 `StableErrorCode` / 退出码模型，继续保持既有 `checkout` 行为；完整退出码现代化留第六批 |
| **B** | 是 | 补齐 `--help` EXAMPLES 段，与 `switch` / `init` / `config` 风格保持一致 |
| **F** | 否 | 本次不为 `checkout` 单独设计 fuzzy suggestion；与分支目标相关的提示增强由 `switch` 侧承接 |
| **G** | 否 | 本次不新增 Issues URL 规则；待 `checkout` 自身进入 typed error / 显式错误码阶段后再统一定义 |

### 测试要求

#### `tests/command/checkout_test.rs`（核心兼容行为）

已有测试必须继续通过：

- `test_checkout_new_branch_with_dirty_worktree_returns_error()`（line 263）：dirty **staged** worktree（即 `SwitchError::DirtyUncommitted` 路径）仍应映射为 `local changes would be overwritten by checkout`
- `test_checkout_current_branch_with_dirty_worktree_succeeds()`（line 361）：checkout 当前分支仍应是 no-op，不受脏工作树阻塞

本次建议补一条：

- `test_checkout_existing_branch_with_unstaged_dirty_worktree_returns_error()`：覆盖 `SwitchError::DirtyUnstaged` 也能被 `checkout` 正确翻译，而不是只覆盖 staged dirty 路径

#### `tests/command/output_flags_test.rs`（输出契约回归）

已有测试必须继续通过：

- `quiet_checkout_existing_branch_suppresses_output()`：`--quiet checkout <branch>` 不输出 informational stdout
- `machine_checkout_existing_branch_suppresses_human_output()`：`--machine checkout <branch>` 不输出人类文本
- `quiet_checkout_dirty_repo_suppresses_status_summary()`：dirty repo 下的 `--quiet checkout <branch>` 不得泄漏 `status` human summary，且仍保持 checkout 专属错误文案
- `machine_checkout_dirty_repo_returns_only_json_error()`：dirty repo 下的 `--machine checkout <branch>` 仅输出 JSON error，不得泄漏 `status` human summary
- `checkout_invalid_index_preserves_status_error()`：索引损坏仍直接暴露 `failed to determine working tree status`，不得折叠成 dirty-tree 消息

#### 不新增 `checkout_json_test.rs`

本次不实现 `checkout --json`，因此**不新增** `checkout_json_test.rs`。该测试文件留待第六批完整现代化时引入。

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/checkout.rs` | **适配** | `switch::ensure_clean_status()` 改为 typed 返回后，用 `SwitchError` 变体匹配替代字符串匹配；补齐 `--help` EXAMPLES。**保持不变**：当前分支 no-op、dirty-tree 文案、remote auto-track 业务流 |
| `src/command/switch.rs` | **联动** | 仅变更共享 helper `ensure_clean_status()` 的返回类型与错误建模；`checkout` 只消费该接口，不共享执行层 |
| `tests/command/checkout_test.rs` | **扩展** | 保留 dirty staged / current branch no-op 测试，建议新增 dirty unstaged 路径覆盖 |
| `tests/command/output_flags_test.rs` | **回归** | 保留 quiet / machine / invalid index 三条 checkout 输出契约测试 |
