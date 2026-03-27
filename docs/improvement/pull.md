## Pull 命令改进详细计划

> 最后编写时间：2026-03-27

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config`、`init`、`clone`、`add`、`status`、`commit` 的主改造已在当前代码库落地（或已有改进计划）。`push` 改进计划已编写。`pull` 是 fetch + merge 的组合命令，其改进依赖 fetch 和 merge 的底层能力。

**已确认落地的基线：**

- `config_kv` 后端已落地；`pull` 已通过 `ConfigKv` 读取 branch tracking 配置
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`pull.rs`）
- Pull 当前实现仅 81 行，是 fetch + merge 的薄包装层
- 远程/refspec 自动检测已实现：从 `branch.<name>.remote` + `branch.<name>.merge` 推导
- `fetch::execute_safe()` 和 `merge::execute_safe()` 是当前委托目标
- 当前 `merge` 仅稳定支持 `Already up to date.` 和 fast-forward；非 fast-forward / three-way merge 仍返回错误，而不是生成 merge commit 或冲突状态

**基于当前代码的 Review 结论（pull 仍需改进的部分）：**

- **零 JSON / machine 输出**：`pull` 本身无结构化输出；虽然委托给 fetch 和 merge，但 pull 层面没有聚合的结构化结果
- **零显式 `StableErrorCode`**：仅有 2 个 `CliError::failure()` 调用（no tracking info、not on a branch），均无显式错误码
- **fetch/merge 的输出未经 pull 协调**：fetch 和 merge 各自直接写 stdout，pull 无法控制它们的输出交织
- **缺少 pull 级别的成功摘要**：成功后无任何 pull 特有的输出——用户看到的是 fetch 输出 + merge 输出的简单拼接
- **错误传播不透明**：fetch 或 merge 失败时，pull 直接传播底层 `CliError`，没有添加"这是在 pull 的 fetch/merge 阶段失败"的上下文
- **当前能力边界与文档承诺不一致**：底层 `merge` 尚不支持 three-way merge / 冲突结构化，pull 计划不能在本批承诺这些输出
- **实现方案不能靠解析子命令 stdout**：`fetch::execute_safe()` / `merge::execute_safe()` 当前输出的是 human 文本，不是稳定机器接口；pull 必须依赖内部 typed helper，而不是捕获文本再反解析
- **结构化输出的 stderr 契约尚未定义**：若沿用当前 fetch/merge 输出，`--json` / `--machine` 成功路径会被子命令文本或 progress 污染
- **对 fetch / merge 前置改造量的评估偏乐观**：当前 pull 计划要成立，必须先让 `fetch.rs` 和 `merge.rs` 暴露可静默复用的 typed 执行层与结构化结果；这不是“顺手小改”，而是 pull 本批的明确前置依赖
- **缺少 `--rebase` 选项**：Git 的 `pull --rebase` 是常用模式，当前不支持（标为非目标，留后续）
- **hint 不完整**：`"there is no tracking information for the current branch"` 已有 hint，但 not-on-branch 场景无 hint
- **测试覆盖极度不足**：仅有 1 个测试验证 no-tracking 场景的退出码

### 目标与非目标

**本批目标：**
- 引入 `PullError` typed error enum，覆盖 pull 层面的错误场景
- 所有 `PullError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_pull(args) -> Result<PullOutput, PullError>` 纯执行入口
- 在 `fetch.rs` / `merge.rs` 建立 pull 可复用的 typed helper 与静默子级输出边界

> **前置依赖说明**：Pull 依赖 fetch/merge 的 typed helper，为打破批次依赖，本批将 **fetch 和 merge 的基础 typed helper** 纳入第一批前置工作。第五批（远程管理）将对 fetch 做完整 JSON/进度改造，本批仅要求 fetch/merge 提供 pull 可用的最小内部接口。

- `PullOutput` 聚合 fetch 和 merge 的结构化结果，但**只覆盖当前底层真正支持的 pull 语义**（fast-forward / already-up-to-date）
- JSON 输出只承诺当前底层确定可得的数据；拿不到稳定统计值的字段不进入首版 schema
- fetch/merge 阶段的输出经 `OutputConfig` 协调，保证 `--json` / `--machine` 成功路径 stderr 默认保持干净
- 错误传播添加阶段上下文（"during fetch"/"during merge"）
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不引入 `--rebase`**。`pull --rebase` 需要 rebase 命令的配合，留后续批次
- **不在本批实现 three-way merge、自动冲突解决或冲突结构化输出**。这些能力依赖 `merge` 命令自身改造，留到 README 第六批统一处理
- **不通过解析子命令 stdout/stderr 实现结构化 pull**。pull 只接受内部 typed helper / 纯执行入口，不接受“先打印再反解析”的过渡实现
- **不改变 fetch/merge 的核心算法**。本批只做 pull 作为组合命令的执行层/渲染层拆分，以及必要的内部 helper 暴露
- **不引入 `--ff-only` / `--no-ff`**。merge 策略选项留 merge 命令改进时一并处理

### 设计原则

1. **Pull 是组合命令，聚合而非替代**：pull 的结构化输出聚合 fetch 和 merge 的结果，而不是重新实现它们的逻辑
2. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_pull()` 收集结构化结果，再渲染
3. **阶段性错误上下文**：fetch/merge 失败时，pull 在外层添加阶段标识（"failed during fetch phase"/"failed during merge phase"），帮助用户定位问题
4. **JSON 只承诺当前支持语义**：`PullOutput` 覆盖 fetch + fast-forward / already-up-to-date；不伪造 three-way merge / conflicted_files 这类当前拿不到的结果
5. **首版 schema 只包含当前稳定可得字段**：若 `fetch` / `merge` 还不能稳定返回对象数、文件变更数等统计，则这些字段不进入首版 pull JSON 契约，留后续增量扩展
6. **禁止解析子命令文本**：`run_pull()` 依赖 fetch/merge 的内部 typed helper；不得通过捕获 `execute_safe()` 的 stdout/stderr 再解析 human 文本来组装 JSON
7. **结构化模式默认保持 stderr 干净**：`--json` / `--machine` 成功路径只输出一个 envelope；fetch/merge progress 和 human 装饰输出必须被 pull 的子级 `OutputConfig` 抑制
8. **hint 覆盖常见失败**：no tracking info、not on branch、manual merge required 等场景提供可操作的 hint

### 特性 1：PullError typed error enum

**当前问题：** pull 仅有 2 个 `CliError::failure()` 调用，且无显式 `StableErrorCode`。fetch/merge 的错误直接透传。

**修正后的方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error("you are not currently on a branch")]
    NotOnBranch,

    #[error("there is no tracking information for the current branch")]
    NoTrackingInfo { branch: String },

    #[error("remote '{0}' not found")]
    RemoteNotFound(String),

    #[error("pull failed during fetch phase: {0}")]
    FetchFailed(String),

    #[error("pull requires a non-fast-forward merge from '{upstream}'")]
    ManualMergeRequired { upstream: String },

    #[error("pull failed during merge phase: {0}")]
    MergeFailed(String),

    #[error("failed to read repository state: {0}")]
    RepoState(String),
}
```

**`PullError → CliError` 显式映射：**

| PullError 变体 | StableErrorCode | 退出码 | hint |
|---------------|-----------------|--------|------|
| `NotOnBranch` | `RepoStateInvalid` | 128 | `checkout a branch before pulling` + `use 'libra switch <branch>' to switch` |
| `NoTrackingInfo` | `RepoStateInvalid` | 128 | `specify the remote and branch: 'libra pull <remote> <branch>'` + `or set upstream with 'libra branch --set-upstream-to=<remote>/<branch>'` |
| `RemoteNotFound` | `CliInvalidTarget` | 129 | `use 'libra remote -v' to see configured remotes` |
| `FetchFailed` | 保留原始错误码 | 128 | 保留原始 hint + 添加 `this error occurred during the fetch phase of pull` |
| `ManualMergeRequired` | `ConflictOperationBlocked` | 128 | `run 'libra fetch' first if needed, then merge manually with 'libra merge <upstream>'` |
| `MergeFailed` | 保留原始错误码 | 128 | 保留原始 hint + 添加 `this error occurred during the merge phase of pull` |
| `RepoState` | `RepoCorrupt` | 128 | `try 'libra status' to verify repository state` |

> **阶段错误透传策略：** `FetchFailed` 和 `MergeFailed` 保留原始错误的 `StableErrorCode`（通过 `CliError::with_detail("phase", "fetch")` 附加阶段信息），而不是覆盖为新的错误码。这样 Agent 可以同时获取根因错误码和阶段上下文。
>
> **实现注意：** 为了真正透传原始错误码，`FetchFailed` / `MergeFailed` 的内部载体应为 `CliError`（或 `Box<CliError>`），而不是 `String`。`From<PullError> for CliError` 实现中直接取出内部 `CliError`，附加 `with_detail("phase", ...)` 后返回，避免错误码丢失。若内部 typed helper 返回的不是 `CliError` 而是自身的 error enum，则需要在 pull 层先转换为 `CliError` 再包装。

### 特性 2：执行层与渲染层拆分

**当前问题：** `execute_safe()` 直接调用 fetch 和 merge 的 `execute_safe()`，它们各自独立写 stdout/stderr。pull 无法协调输出格式。

**修正后的方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct PullFetchResult {
    /// 拉取的 remote name
    pub remote: String,
    /// 拉取的 remote URL
    pub url: String,
    /// 更新的远端 tracking ref
    pub refs_updated: Vec<PullRefUpdate>,
    /// 拉取的对象数量（从 pack header 提取，确定性值）
    pub objects_fetched: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRefUpdate {
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullMergeResult {
    /// merge 策略（fast-forward / already-up-to-date）
    pub strategy: String,
    /// merge commit hash（fast-forward 时为目标 commit）
    pub commit: Option<String>,
    /// 变更的文件数量（通过对比 source/target tree 计算，确定性值）
    pub files_changed: usize,
    /// 是否 already up to date
    pub up_to_date: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullOutput {
    /// 当前分支
    pub branch: String,
    /// upstream tracking ref
    pub upstream: String,
    /// fetch 阶段结果
    pub fetch: PullFetchResult,
    /// merge 阶段结果
    pub merge: PullMergeResult,
}
```

改造后的调用链：
- `execute_safe(args, output)` → `run_pull(args)` → 返回 `PullOutput`
- `run_pull()` 内部分别调用 fetch 和 merge 的内部 typed helper（而非 `execute_safe()`），收集结构化结果
- `execute_safe()` 根据 `OutputConfig` 选择渲染：human / JSON / machine

> **实现依赖：** 本批必须在 `fetch.rs` / `merge.rs` 增加 pull 可复用的内部 helper（名称可为 `run_fetch_for_pull()` / `run_merge_for_pull()` 或等价形式），返回 typed 结果。**不接受**“捕获 `execute_safe()` 的 stdout 再解析”的实现，因为那会把 human 文本错误地提升为机器契约。

> **范围约束：** `fetch.rs` / `merge.rs` 在本批中的改动量应按“前置依赖改造”评估，而不是文档/实现层面的顺手补丁。只有先拿到稳定的内部结果类型，pull 的聚合 JSON 和阶段错误上下文才有可靠基础。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | fetch 摘要 + merge 摘要 | fetch/merge 进度 |
| human + `--quiet` | 无 | 仅 warning/conflict |
| `--json` / `--machine` | JSON envelope | 默认保持干净，不输出 child progress / human 文本 |

**human 模式输出（改进后）：**

fast-forward：
```text
From github.com:user/repo
   abc1234..def5678  main       -> origin/main
Updating abc1234..def5678
Fast-forward
 src/main.rs | 10 +++++-----
 1 file changed, 5 insertions(+), 5 deletions(-)
```

already up to date：
```text
From github.com:user/repo
Already up to date.
```

### 特性 3：JSON 输出设计

**成功输出（fast-forward）：**

```json
{
  "ok": true,
  "command": "pull",
  "data": {
    "branch": "main",
    "upstream": "origin/main",
    "fetch": {
      "remote": "origin",
      "url": "git@github.com:user/repo.git",
      "refs_updated": [
        {
          "remote_ref": "refs/remotes/origin/main",
          "old_oid": "abc1234...",
          "new_oid": "def5678..."
        }
      ],
      "objects_fetched": 128
    },
    "merge": {
      "strategy": "fast-forward",
      "commit": "def5678...",
      "files_changed": 3,
      "up_to_date": false
    }
  }
}
```

**already up to date：**

```json
{
  "ok": true,
  "command": "pull",
  "data": {
    "branch": "main",
    "upstream": "origin/main",
    "fetch": {
      "remote": "origin",
      "url": "git@github.com:user/repo.git",
      "refs_updated": [],
      "objects_fetched": 0
    },
    "merge": {
      "strategy": "already-up-to-date",
      "commit": null,
      "files_changed": 0,
      "up_to_date": true
    }
  }
}
```

**错误 JSON：no tracking information**

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "there is no tracking information for the current branch",
  "hints": [
    "specify the remote and branch: 'libra pull <remote> <branch>'",
    "or set upstream with 'libra branch --set-upstream-to=<remote>/<branch>'"
  ]
}
```

**错误 JSON：not on a branch**

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "you are not currently on a branch",
  "hints": [
    "checkout a branch before pulling",
    "use 'libra switch <branch>' to switch"
  ]
}
```

**错误 JSON：manual merge required**

```json
{
  "ok": false,
  "error_code": "LBR-CONFLICT-002",
  "category": "conflict",
  "exit_code": 128,
  "message": "pull requires a non-fast-forward merge from 'origin/main'",
  "hints": [
    "run 'libra fetch' first if needed, then merge manually with 'libra merge origin/main'"
  ],
  "details": {
    "phase": "merge"
  }
}
```

**错误 JSON：fetch 阶段网络失败**

```json
{
  "ok": false,
  "error_code": "LBR-NET-001",
  "category": "network",
  "exit_code": 128,
  "message": "pull failed during fetch phase: connection timed out",
  "hints": [
    "check network connectivity and retry"
  ],
  "details": {
    "phase": "fetch"
  }
}
```

### 特性 4：Cross-Cutting Improvements 在 pull 中的具体落地

| ID | 改进 | pull 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 remote 名）→ exit `129`；运行时错误（网络失败、认证失败、manual merge required、no tracking info、not on branch）→ exit `128`；成功 / already-up-to-date → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | remote 名不匹配时提示 `did you mean '<closest>'?`（复用 push 的 fuzzy match 逻辑） |
| **G** | Issues URL | 仅在 `RepoState` / `MergeFailed` 等内部不变式错误时输出 Issues URL。网络/认证/manual-merge-required 等用户可修复问题不输出 |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra pull                             Pull from tracking remote
    libra pull origin main                 Pull specific branch from origin
    libra pull --json                      Structured JSON output for agents
    libra pull --quiet                     Suppress progress output
```

### 测试要求

#### `tests/command/pull_test.rs`（核心执行路径，重大扩展）

- **（已有）** no-tracking-info 退出码验证
- **（新增）`PullError` 变体覆盖**：
  - `NotOnBranch`：detached HEAD 状态下 pull 返回对应错误
  - `NoTrackingInfo`：无 tracking 配置时返回对应错误 + hint
  - `RemoteNotFound`：指定不存在的 remote 名时返回对应错误
  - `ManualMergeRequired`：本地与远端无法 fast-forward 时返回对应错误 + hint
- **（新增）fast-forward pull**：本地落后于远端时 pull 成功，HEAD 更新到最新 commit
- **（新增）already-up-to-date**：本地与远端一致时返回 up-to-date 结果
- **（新增）`--quiet` 静默**：成功路径下 stdout 为空

#### `tests/command/pull_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性：
  - `branch` 是 string
  - `upstream` 是 string
  - `fetch` 是 object，包含 `remote`（string）、`url`（string）、`refs_updated`（array）
  - `fetch.refs_updated` 元素包含 `remote_ref`（string）、`old_oid`（string 或 null）、`new_oid`（string）
  - `merge` 是 object，包含 `strategy`（string）、`commit`（string 或 null）、`up_to_date`（bool）
- **fast-forward `--json`**：`merge.strategy == "fast-forward"`，`merge.up_to_date == false`
- **already-up-to-date `--json`**：`merge.up_to_date == true`，`merge.commit == null`
- **`--machine pull`**：stdout 按 `\n` 分割后恰好 1 行非空行，可被 `serde_json::from_str()` 解析
- **错误 JSON 格式**：no tracking info、not on branch、manual merge required 等场景返回结构化错误 JSON 到 stderr
- **阶段上下文**：fetch 阶段失败的错误 JSON 包含 `details.phase == "fetch"`
- **结构化输出隔离**：`--json` / `--machine` 成功路径下 stderr 不出现 fetch/merge 的 human 输出或 progress 文本

#### CLI 错误码验证（放入 `tests/command/pull_test.rs`）

- `NotOnBranch` 返回 `LBR-REPO-003`
- `NoTrackingInfo` 返回 `LBR-REPO-003`
- `RemoteNotFound` 返回 `LBR-CLI-003`
- `ManualMergeRequired` 返回 `LBR-CONFLICT-002`

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/pull.rs` | **重构** | 从 81 行薄包装扩展为完整的组合命令；新增 `PullError` typed enum；新增 `PullOutput` / `PullFetchResult` / `PullMergeResult` 结构体；新增 `run_pull()` 纯执行入口；`PullError → CliError` 显式 `StableErrorCode` 映射；fetch/merge 子级 `OutputConfig` 隔离；补齐 `--help` EXAMPLES |
| `src/command/fetch.rs` | **前置依赖（第一批）** | 新增 pull 可复用的内部 typed helper `run_fetch_for_pull()` 与静默 child-output 边界；返回结构化 `FetchResult`；**注意：完整 JSON/进度改造留到第五批** |
| `src/command/merge.rs` | **前置依赖（第一批）** | 新增 pull 可复用的内部 typed helper `run_merge_for_pull()`；仅返回 fast-forward / already-up-to-date / requires-manual-merge 结果；**注意：three-way merge 能力留到第六批** |
| `tests/command/pull_test.rs` | **重大扩展** | 新增 `PullError` 变体覆盖、fast-forward、up-to-date、quiet 场景 |
| `tests/command/pull_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/pull_error_test.rs` | **新增** | CLI 错误码验证（exit code、StableErrorCode、阶段上下文） |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
