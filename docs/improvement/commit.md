## Commit 命令改进详细计划

> 最后编写时间：2026-03-27
> **实施状态：✅ 已落地** — 架构改造、typed error、JSON 向后兼容扩展、hook I/O 隔离、集成测试均已完成。

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config`、`init`、`clone`、`add`、`status` 的主改造均已在当前代码库落地。README 当前将 `commit` 标记为“部分完成，需对齐改进模式”，这一判断是准确的：`commit` 虽然功能面较完整，但与已改进命令（init/clone/add/status）的统一模式相比，仍有明显的架构、输出契约和测试覆盖缺口，需要继续收口。

**已确认落地的基线：**

- `config_kv` 后端已落地；`commit` 通过 `get_user_config_value()` 读取 `user.name`/`user.email` 等配置
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_exit_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`commit.rs:574-587`）
- `execute_impl(args, output)` 内部执行入口已存在（`commit.rs:339`），返回 `Result<(), CommitExecError>`
- **JSON 输出已部分实现**（`commit.rs:285-302`）：`emit_commit_summary()` 中通过 `emit_json_data("commit", ...)` 输出成功摘要
- `CommitExecError` 桥接类型已存在（`commit.rs:127-152`），将内部 `String` 错误映射为 `CliError`
- `classify_commit_error()` 已存在（`commit.rs:182-194`），将错误消息字符串映射到 `StableErrorCode`
- `resolve_committer_identity()` 已存在（`commit.rs:196-234`），从 config/env 解析提交者身份
- `parse_author()` 已存在（`commit.rs:91-125`），解析 `--author` 参数
- 已有功能完整的 flag 支持：`--message/-m`、`--file/-F`、`--allow-empty`、`--conventional`、`--amend`、`--no-edit`、`--signoff/-s`、`--disable-pre`/`--no-verify`、`--all/-a`、`--author`

**基于当前代码的 Review 结论（commit 与已改进命令模式对比后仍需改进的部分）：**

- **无 typed error enum**：`execute_impl()` 内部大量使用 `Result<(), String>` 错误——至少 18 处 `.map_err(|e| format!("failed to ...: {}", e))?`，没有像 `AddError`/`StatusError`/`InitError` 那样的 `thiserror` 枚举。`CommitExecError` 是一个简单的桥接类型，不是结构化错误分类
- **StableErrorCode 依赖推断而非显式映射**：`classify_commit_error()` 仅对 `"nothing to commit"` 显式映射为 `RepoStateInvalid`，其余错误（包括身份认证失败）全部依赖 `infer_stable_error_code()` 的字符串子串匹配——这是脆弱的，任何错误消息措辞变化都会导致错误码偏移
- **legacy `execute()` 包装仍使用默认 `OutputConfig`**：`execute()` 入口（`commit.rs:575`）仍以 `OutputConfig::default()` 调用 `execute_safe()`；但 CLI 主路径已经通过顶层 dispatcher 直接调用 `execute_safe(args, &output)`。因此这更像兼容包装层的收尾项，而不是阻断本批的主问题
- **执行层与渲染层未拆分**：`execute_impl()` 是一个 233 行的单体函数，混合了索引操作、验证、树创建、签名、对象存储、HEAD 更新和输出渲染。没有像 `run_add()`/`run_init()` 那样的纯执行入口返回结构化结果
- **JSON 输出不完整**：当前 JSON 仅输出成功摘要（commit hash + 文件统计），不包含 `amend` 标记、分支名、`signoff` 状态、`conventional` 验证状态等元数据；也没有 `--dry-run` 支持
- **JSON 向后兼容方案当前写错了**：计划后文把现有字段 `head` 改名为 `branch`，并把 `files_changed.total/new/...` 改为另一套字段名；这与“仅做增量扩展”的原则冲突，会破坏已有 JSON consumer
- **hook 输出当前会污染结构化输出**：pre-commit hook 直接继承父进程 stdout/stderr；若不在本批定义隔离规则，`--json` / `--machine` 成功路径无法保证 stdout/stderr 契约稳定
- **`--signoff` 写入逻辑已存在但缺少回归测试**：`commit.rs:463-469` 和 `522-528` 两处路径已实现 `Signed-off-by` trailer 拼接到 `final_message`；但没有任何测试验证 trailer 在所有路径（含 amend + signoff 组合）下稳定写入，重构中可能意外丢失
- **核心 helper 的错误传播仍不统一**：`vault_sign_commit()`、`create_tree()`、`auto_stage_tracked_changes()`、`update_head()`、`update_head_and_reflog()` 等关键路径仍以 `String` 为主要错误载体，无法精确映射到 `StableErrorCode`
- **缺少 `--help` EXAMPLES 段**：`CommitArgs` 没有 doc comment 提供 clap 的 `EXAMPLES` 输出段
- **部分 `unwrap()` 残留**：`ObjectHash::from_bytes(...).unwrap()`（`commit.rs:841`）在字节格式异常时会 panic
- **`missing_identity_error()` 未显式设置 `StableErrorCode`**：返回的 `CliError::fatal("author identity unknown")` 依赖推断得到 `LBR-AUTH-001`，但这不是显式映射——消息措辞变化会导致推断失败
- **测试覆盖有盲区**：无 JSON 输出格式测试、无 `--conventional` 验证测试、无 `--signoff` 格式测试、无 `-F`（file）消息源测试、无 vault 签名集成测试

### 目标与非目标

**本批目标：**
- 引入 `CommitError` typed error enum，替代内部 `String` 错误
- 所有 `CommitError → CliError` 映射使用显式 `StableErrorCode`，消除对 `infer_stable_error_code()` 的依赖
- 拆分执行层与渲染层：新增 `run_commit(args) -> Result<CommitOutput, CommitError>` 纯执行入口
- 保持现有 JSON schema 向后兼容，仅做增量扩展
- 完善 JSON 输出 schema（`CommitOutput`），补齐缺失元数据
- 为 hook 输出建立结构化隔离规则，确保 `--json` / `--machine` 成功路径不被污染
- 补齐 `--signoff` 回归测试，验证 Signed-off-by trailer 在所有路径（含 amend + signoff 组合）下稳定写入
- 补齐 `--help` EXAMPLES 段
- 清理 `unwrap()` 残留，改为安全的 `Result` 传播
- 保留 `execute()` 作为兼容包装层；其输出配置问题仅作为低优先级收尾项处理，不影响 CLI 主路径

**本批非目标：**
- **不重写 commit 的核心对象模型**。树创建、索引操作、签名、reflog 更新的主体流程保持不变；仅修复 `--signoff` 这类已识别的行为缺口
- **不引入 `--dry-run`**。commit dry-run 语义复杂（需要回滚树创建），留后续批次
- **不改变 hook 的启用/禁用语义**。`--disable-pre`/`--no-verify` 行为保持不变；本批只调整 hook I/O 的捕获和渲染边界
- **不改变 LFS pointer 处理**。`blob_from_file()` 保持现有 LFS 行为
- **不改变 vault GPG 签名逻辑**。`vault_sign_commit()` 的加密流程不变，仅改善错误类型

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_commit()` 收集结构化结果，再根据 `OutputConfig` 渲染 human/JSON/machine
2. **typed error enum 取代 String 错误**：每个失败场景有确定的 `CommitError` 变体，消除字符串匹配推断
3. **StableErrorCode 显式映射**：`CommitError → CliError` 的每条路径都有确定的错误码，不依赖 `infer_stable_error_code()`
4. **JSON 严格向后兼容**：现有 JSON 字段（`head`、`commit`、`short_id`、`subject`、`root_commit`、`files_changed.total/new/modified/deleted`）保持名称、类型和语义不变；新增字段只能增量追加，不能重命名或改形状
5. **`--quiet` 仅抑制标准 stdout**：与全局语义一致；error/warning 仍写 stderr
6. **hook 输出边界必须稳定**：成功的 hook 输出只能在 human 模式可见；`--json` / `--machine` 下必须被捕获并抑制，失败时通过结构化错误传递必要上下文，而不是裸泄漏到 stdout/stderr
7. **helper 函数返回 `CommitError` 而非 `String`**：减少错误上下文丢失

### 特性 1：CommitError typed error enum

**当前问题：** `execute_impl()` 内部大量使用 `.map_err(|e| format!(...))` 生成 `String` 错误，再通过 `CommitExecError` 桥接到 `CliError`。`classify_commit_error()` 基于消息子串推断错误码，脆弱且不完整。

**修正后的方案：**

新增 `CommitError` 枚举替代内部 `String` 错误：

```rust
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("failed to load index: {0}")]
    IndexLoad(String),

    #[error("failed to save index: {0}")]
    IndexSave(String),

    #[error("nothing to commit, working tree clean")]
    NothingToCommit,

    #[error("nothing to commit (create/copy files and use 'libra add' to track)")]
    NothingToCommitNoTracked,

    #[error("{0}")]
    IdentityMissing(String),

    #[error("there is no commit to amend")]
    NoCommitToAmend,

    #[error("amend is not supported for merge commits with multiple parents")]
    AmendUnsupported,

    #[error("invalid author format: {0}")]
    InvalidAuthor(String),

    #[error("failed to read message file '{path}': {detail}")]
    MessageFileRead { path: String, detail: String },

    #[error("aborting commit due to empty commit message")]
    EmptyMessage,

    #[error("failed to create tree: {0}")]
    TreeCreation(String),

    #[error("failed to store commit object: {0}")]
    ObjectStorage(String),

    #[error("failed to load parent commit '{commit_id}': {detail}")]
    ParentCommitLoad { commit_id: String, detail: String },

    #[error("failed to update HEAD: {0}")]
    HeadUpdate(String),

    #[error("pre-commit hook failed: {0}")]
    PreCommitHook(String),

    #[error("conventional commit validation failed: {0}")]
    ConventionalCommit(String),

    #[error("failed to sign commit: {0}")]
    VaultSign(String),

    #[error("failed to auto-stage tracked changes: {0}")]
    AutoStage(String),

    #[error("failed to calculate staged changes: {0}")]
    StagedChanges(String),
}
```

**`CommitError → CliError` 显式映射：**

| CommitError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted; try 'libra status' to verify` |
| `IndexSave` | `IoWriteFailed` | 128 | 无 |
| `NothingToCommit` | `RepoStateInvalid` | 128 | `use 'libra add' to stage changes` + `use 'libra status' to see what changed` |
| `NothingToCommitNoTracked` | `RepoStateInvalid` | 128 | `create/copy files and use 'libra add' to track` |
| `IdentityMissing` | `AuthMissingCredentials` | 128 | `run 'libra config --global user.name "Your Name"' and 'libra config --global user.email "you@example.com"'` + `omit '--global' to set the identity only in this repository.` |
| `NoCommitToAmend` | `RepoStateInvalid` | 128 | `create a commit before using --amend` |
| `AmendUnsupported` | `RepoStateInvalid` | 128 | `create a new commit instead of amending a merge commit` |
| `InvalidAuthor` | `CliInvalidArguments` | 129 | `expected format: 'Name <email>'` |
| `MessageFileRead` | `IoReadFailed` | 128 | 无 |
| `EmptyMessage` | `RepoStateInvalid` | 128 | `use -m to provide a commit message` |
| `TreeCreation` | `InternalInvariant` | 128 | 无（附 Issues URL） |
| `ObjectStorage` | `IoWriteFailed` | 128 | 无 |
| `ParentCommitLoad` | `RepoCorrupt` | 128 | `the parent commit is missing or corrupted` |
| `HeadUpdate` | `IoWriteFailed` | 128 | 无 |
| `PreCommitHook` | `RepoStateInvalid` | 128 | `use --no-verify to bypass the hook` |
| `ConventionalCommit` | `CliInvalidArguments` | 129 | `see https://www.conventionalcommits.org for format rules` |
| `VaultSign` | `AuthMissingCredentials` | 128 | `check vault configuration with 'libra config --list'` |
| `AutoStage` | `IoReadFailed` | 128 | 无 |
| `StagedChanges` | `RepoCorrupt` | 128 | `failed to compute staged changes` |

### 特性 2：执行层与渲染层拆分

**当前问题：** `execute_impl()` 是一个 233 行单体函数，混合执行逻辑和输出渲染。`emit_commit_summary()` 虽然独立存在，但整个成功路径的元数据没有结构化收集。

**修正后的方案：**

新增纯执行入口 `run_commit(args, output) -> Result<CommitOutput, CommitError>`：

> **为什么 `run_commit()` 需要接收 `&OutputConfig`？** 与 `run_init()` / `run_add()` 不同，commit 的 pre-commit hook **必须在执行层内部运行**（hook 在树创建之前执行，无法提取到 `execute_safe()` 层）。`run_commit()` 接收 `&OutputConfig` 仅用于控制 hook 子进程的 stdio 策略——human 模式下 `Stdio::inherit()`，`--json`/`--machine` 模式下 `Stdio::piped()` 捕获。`run_commit()` 自身**不做任何渲染输出**，仍然只返回 `CommitOutput`；hook 的原始输出（如果被捕获）在失败时通过 `CommitError::PreCommitHook` 的 detail 字段传递，由 `execute_safe()` 的渲染层决定是否暴露。

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CommitOutput {
    /// 兼容旧 JSON consumer：分支名，detached HEAD 时为 "detached"
    pub head: String,
    /// 新增字段：显式表达当前是否附着在分支上
    pub branch: Option<String>,
    /// 完整 commit hash
    pub commit: String,
    /// 短 commit hash
    pub short_id: String,
    /// commit message 第一行
    pub subject: String,
    /// 是否为 root commit（无父提交）
    pub root_commit: bool,
    /// 是否为 amend 操作
    pub amend: bool,
    /// 文件变更统计
    pub files_changed: FilesChanged,
    /// 是否附加了 sign-off
    pub signoff: bool,
    /// 是否通过了 conventional commit 验证
    pub conventional: Option<bool>,
    /// vault GPG 签名状态
    pub signed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilesChanged {
    pub total: usize,
    pub new: usize,
    pub modified: usize,
    pub deleted: usize,
}
```

改造后的调用链：
- `execute_safe(args, output)` → `run_commit(args, output)` → 返回 `CommitOutput`
- `execute_safe()` 根据 `OutputConfig` 选择渲染：human / JSON / machine
- `execute()` 正确构造 `OutputConfig`（而非 `OutputConfig::default()`）

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | `[main abc1234] Add new feature` + 统计摘要 | hook 输出（如有） |
| human + `--quiet` | 无 | hook 输出（如有） |
| `--json` / `--machine` | JSON envelope | 默认保持干净；成功 hook 输出不外泄，失败时仅输出结构化错误 JSON |

**human 模式输出格式（保持当前风格）：**

```text
[main abc1234] Add new feature
 2 files changed, 15 insertions(+), 3 deletions(-)
```

root commit：
```text
[main (root-commit) abc1234] Initial commit
 1 file changed, 10 insertions(+)
```

amend：
```text
[main abc1234] Updated commit message
 Date: Thu Mar 27 10:00:00 2026 +0800
 2 files changed, 5 insertions(+), 1 deletion(-)
```

### 特性 3：JSON 输出设计

**成功输出结构：**

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc123def456789...",
    "short_id": "abc1234",
    "subject": "Add new feature",
    "root_commit": false,
    "amend": false,
    "files_changed": {
      "total": 2,
      "new": 1,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": false
  }
}
```

**amend 场景：**

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "def456abc789...",
    "short_id": "def4567",
    "subject": "Updated commit message",
    "root_commit": false,
    "amend": true,
    "files_changed": {
      "total": 2,
      "new": 0,
      "modified": 2,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": false
  }
}
```

**root commit + conventional + signoff：**

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc123...",
    "short_id": "abc1234",
    "subject": "feat: initial project setup",
    "root_commit": true,
    "amend": false,
    "files_changed": {
      "total": 5,
      "new": 5,
      "modified": 0,
      "deleted": 0
    },
    "signoff": true,
    "conventional": true,
    "signed": false
  }
}
```

**detached HEAD：**

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "detached",
    "branch": null,
    "commit": "abc123...",
    "short_id": "abc1234",
    "subject": "Fix in detached HEAD",
    "root_commit": false,
    "amend": false,
    "files_changed": {
      "total": 1,
      "new": 0,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": false
  }
}
```

**向后兼容说明：** 当前 JSON 输出的字段 `head`、`commit`、`short_id`、`subject`、`root_commit`、`files_changed.total/new/modified/deleted` 在重构中保持原样；本批只新增 `branch`、`amend`、`signoff`、`conventional`、`signed` 等字段。`head` 不重命名，`files_changed` 不改形状。

**明确不纳入 JSON 契约的字段：**

- `parent_commits` — 可通过 `libra log --json` 获取
- `tree_hash` — 内部实现细节
- `full_message` — 可通过 `libra show --json` 获取
- `gpg_signature` — 二进制数据，不适合 JSON

**错误 JSON：nothing to commit**

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "nothing to commit, working tree clean",
  "hints": [
    "use 'libra add' to stage changes",
    "use 'libra status' to see what changed"
  ]
}
```

**错误 JSON：身份缺失**

```json
{
  "ok": false,
  "error_code": "LBR-AUTH-001",
  "category": "auth",
  "exit_code": 128,
  "message": "author identity unknown",
  "hints": [
    "run 'libra config user.name \"Your Name\"' and 'libra config user.email \"you@example.com\"'"
  ]
}
```

**错误 JSON：conventional commit 验证失败**

```json
{
  "ok": false,
  "error_code": "LBR-CLI-002",
  "category": "cli",
  "exit_code": 129,
  "message": "conventional commit validation failed: missing type prefix",
  "hints": [
    "see https://www.conventionalcommits.org for format rules"
  ]
}
```

### 特性 4：Helper 函数错误类型改造

**当前问题：** 核心 helper 函数全部返回 `Result<T, String>`，上层通过 `format!()` 生成错误消息，导致错误上下文丢失。

**修正后的方案：**

将以下 helper 函数的返回类型从 `Result<T, String>` 改为 `Result<T, CommitError>`：

| 函数 | 当前返回 | 改为 |
|------|---------|------|
| `create_tree()` | `Result<Tree, String>` | `Result<Tree, CommitError>` |
| `auto_stage_tracked_changes()` | `Result<bool, String>` | `Result<bool, CommitError>` |
| `update_head()` | `Result<(), String>` | `Result<(), CommitError>` |
| `update_head_and_reflog()` | `Result<(), String>` | `Result<(), CommitError>` |
| `vault_sign_commit()` | `Result<Option<String>, String>` | `Result<Option<String>, CommitError>` |

> **注意：**
> - `create_tree()` 是递归函数，`CommitError` 需要支持嵌套上下文。使用 `CommitError::TreeCreation(detail)` 承载内层错误文本即可，不需要 `Box<CommitError>` 嵌套。
> - `blob_from_file()` 和 `get_parents_ids()` 当前不是 `Result` 返回值，不应在本节误写为错误类型改造目标；本批保持它们原有签名，除非实现中新增真正需要传播的失败路径

### 特性 5：Cross-Cutting Improvements 在 commit 中的具体落地

| ID | 改进 | commit 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（`--author` 格式错误、conventional 验证失败）→ exit `129`；运行时错误（索引损坏、I/O 失败、身份缺失、nothing to commit）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——`commit` 的参数是消息文本和标志，不是 enum 值 |
| **G** | Issues URL | 仅在 `TreeCreation` / `ObjectStorage` / `HeadUpdate` 等内部不变式错误时输出 Issues URL。身份缺失、空消息等用户可修复问题不输出 |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra commit -m "Add new feature"          Create a commit with message
    libra commit -m "feat: add login" --conventional  Validate conventional commit format
    libra commit --amend                       Amend the last commit
    libra commit --amend --no-edit             Amend without changing the message
    libra commit -a -m "Fix typo"              Auto-stage tracked changes and commit
    libra commit -F message.txt                Read commit message from file
    libra commit -s -m "Add feature"           Add Signed-off-by trailer
    libra commit --allow-empty -m "Trigger CI" Create an empty commit
    libra commit --json -m "Add feature"       Structured JSON output for agents
```

### 全部场景结构化 Output 设计（`--json` / `--machine`）

所有结构化输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。`--machine` 与 `--json` 使用同一 schema，仅格式化方式不同（紧凑单行）。

**hook 输出隔离规则：**
- human 模式：成功 hook 的 stdout/stderr 维持可见，行为尽量接近当前实现
- `--json` / `--machine`：成功 hook 的 stdout/stderr 必须被捕获并抑制，保证 success path 只有一个结构化 envelope
- hook 失败：不直接继承子进程输出；将必要的 hook 诊断收敛为 `CommitError::PreCommitHook`，再由 `CliError` 统一渲染。若需要保留原始输出，放入错误 `details`，不裸写 stderr

#### 成功 envelope

```json
{
  "ok": true,
  "command": "commit",
  "data": { ... }
}
```

#### `libra commit -m "Add feature" --json`

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc123def456789...",
    "short_id": "abc1234",
    "subject": "Add feature",
    "root_commit": false,
    "amend": false,
    "files_changed": {
      "total": 1,
      "new": 1,
      "modified": 0,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": false
  }
}
```

#### `libra commit --amend --no-edit --json`

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "feature-x",
    "branch": "feature-x",
    "commit": "def456...",
    "short_id": "def4567",
    "subject": "Original message preserved",
    "root_commit": false,
    "amend": true,
    "files_changed": {
      "total": 3,
      "new": 0,
      "modified": 2,
      "deleted": 1
    },
    "signoff": false,
    "conventional": null,
    "signed": false
  }
}
```

#### 错误 JSON：empty commit message

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "aborting commit due to empty commit message",
  "hints": [
    "use -m to provide a commit message"
  ]
}
```

#### 错误 JSON：pre-commit hook 失败

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "pre-commit hook failed: exit code 1",
  "hints": [
    "use --no-verify to bypass the hook"
  ]
}
```

### 测试要求与实施记录

> **测试实施状态：✅ 已落地**

#### `src/command/commit.rs` 内 `mod test`（CommitError 映射单元测试）

- ✅ 全部 18 个 `CommitError` 变体的 `→ CliError` 映射测试（`StableErrorCode` + 退出码）
- ✅ `parse_author()` 返回 `CommitError::InvalidAuthor` 验证
- ✅ 参数解析测试（原有，保留不变）

#### `tests/command/commit_test.rs`（核心执行路径，已有 + 扩展）

- ✅ 空索引拒绝、身份验证、完整提交流程、SHA-256、自定义 author、amend、`--all` 标志
- ✅ `--signoff` 持久化：commit message 末尾真实包含 `Signed-off-by:` 行（normal + amend 两条路径）
- ✅ amend 无 prior commit 返回 `LBR-REPO-003`
- ✅ CLI 退出码（nothing to commit exit 128、missing identity exit 128 + `LBR-AUTH-001`）

#### `tests/command/commit_error_test.rs`（✅ 新增文件，CLI 级错误码 + 退出码 + human 输出验证）

- ✅ `nothing_to_commit_returns_exit_128` — staged 无变更
- ✅ `nothing_to_commit_no_tracked_returns_exit_128` — 空索引（触发 `NothingToCommitNoTracked`）
- ✅ `missing_identity_returns_auth_exit_code` — `LBR-AUTH-001`
- ✅ `invalid_author_format_returns_exit_129` — `LBR-CLI-002`
- ✅ `conventional_validation_failure_returns_exit_129` — `LBR-CLI-002`
- ✅ `message_from_file_works` — `-F msg.txt` 成功路径
- ✅ `message_from_missing_file_returns_exit_128` — `-F nonexistent.txt` → `LBR-IO-001`
- ✅ `human_output_shows_branch_and_subject` — `[branch short_id] subject` 格式
- ✅ `root_commit_shows_root_marker` — `(root-commit)` 标记
- ✅ `quiet_commit_suppresses_stdout` — `--quiet` 下 stdout 为空
- ✅ `amend_without_prior_commit_returns_repo_state_error` — `LBR-REPO-003`

#### `tests/command/commit_json_test.rs`（✅ 新增文件，JSON schema 稳定性）

- ✅ `json_commit_has_all_required_fields` — 全字段类型断言（backward-compatible + 新增）
- ✅ `json_root_commit_fields` — `root_commit == true`，`branch` 非 null
- ✅ `json_signoff_field` — `signoff == true`
- ✅ `json_conventional_field` — `conventional == true`
- ✅ `json_conventional_null_when_not_requested` — `conventional` 为 null
- ✅ `json_amend_field` — `amend == true`
- ✅ `machine_commit_is_single_line_json` — `--machine` 输出单行
- ✅ `json_nothing_to_commit_returns_structured_error` — stderr JSON `LBR-REPO-003`
- ✅ `json_commit_stdout_is_clean_json_only` — stdout 无 human 文本混入

#### `tests/command/output_flags_test.rs`（已有，补充覆盖）

- ✅ `json_commit_returns_structured_summary` — JSON 全字段验证（含新增字段 `branch`/`amend`/`signoff`/`conventional`）
- ✅ `json_commit_suppresses_successful_hook_output` — hook stdout/stderr 不泄漏
- ✅ `json_commit_conventional_check_does_not_pollute_stdout` — `--conventional` + `--json`
- ✅ `quiet_commit_suppresses_summary` — `--quiet` 下 stdout 为空

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/commit.rs` | **✅ 重构** | 新增 `CommitError`（18 变体）typed enum + `From<CommitError> for CliError` 显式映射；新增 `CommitOutput` / `FilesChanged` 结构体；新增 `run_commit(args, output)` 纯执行入口 + `render_commit_output()` 渲染层 + `run_pre_commit_hook()` hook I/O 隔离 + `save_commit_object()` + `build_commit_output()`；删除 `CommitExecError` / `classify_commit_error()` / `emit_commit_summary()` / `execute_impl()`；helper 函数改为返回 `CommitError`（`create_tree`/`auto_stage_tracked_changes`/`vault_sign_commit`/`update_head`/`update_head_and_reflog`/`resolve_committer_identity`/`create_commit_signatures`/`parse_author`）；清理 `unwrap()` 残留；补齐 `--help` EXAMPLES；全部 18 个变体有单元映射测试 |
| `tests/command/commit_test.rs` | **✅ 已有（含扩展）** | signoff 持久化（normal + amend）、amend 无 prior commit 错误码、CLI 退出码验证等 |
| `tests/command/commit_error_test.rs` | **✅ 新增** | 11 个 CLI 级集成测试：退出码分类（128 vs 129）、`-F` 消息源、human 输出格式、root commit 标记、`--quiet` 静默、amend 错误码 |
| `tests/command/commit_json_test.rs` | **✅ 新增** | 9 个 JSON schema 稳定性测试：全字段类型断言、root commit / signoff / conventional / amend JSON 字段、machine 单行、错误 JSON、stdout 隔离 |
| `tests/command/output_flags_test.rs` | **✅ 已有（验证通过）** | JSON 结构化摘要、hook 输出隔离、conventional JSON、quiet 静默（4 个 commit 相关测试） |
| `tests/command/mod.rs` | **✅ 修改** | 注册 `commit_error_test` 和 `commit_json_test` |
