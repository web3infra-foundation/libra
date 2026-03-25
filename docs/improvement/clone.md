## Clone 命令改进详细计划

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config` 和 `init` 的主改造已经在当前代码库落地。clone 改进建立在 config 和 init 现状之上。

**已确认落地的基线（基于 init 改造后的实际代码）：**

- `init` 已完成执行层与渲染层拆分：
  - 纯执行入口 `run_init(args: InitArgs) -> Result<InitOutput, InitError>` 已交付（`init.rs:364`），内部调用 `run_init_internal()` 并禁用 progress
  - 顶层渲染入口 `execute_safe(args, output) -> CliResult<()>` 负责 human / JSON / machine 渲染
  - `render_init_result()` 独立处理 human 模式的 stdout 输出
- **clone 已完成 `run_init()` 切换**：`clone.rs:245` 已调用 `command::init::run_init()`，不再调用 `init::execute_safe()`
- `InitArgs` 已移除 `separate_libra_dir` 参数；clone 构造 `InitArgs` 时不传递该字段
- `InitOutput` 结构体（注意：不是 `InitResult`）包含以下字段：`path`, `bare`, `initial_branch`, `object_format`, `ref_format`, `repo_id`, `vault_signing`, `converted_from`, `ssh_key_detected`, `warnings`
- `InitError → CliError` 映射已完成显式 `StableErrorCode` 绑定（`init.rs:107-178`），每个 `InitError` 变体都有确定的 `StableErrorCode`
- `InitProgress` 用于 stderr 阶段性进度，`run_init()` 传入 `disabled()` 确保子调用不产生输出
- `config_kv` 后端已落地；clone 的 `setup_repository()` 已使用 `ConfigKv::set()` / `ConfigKv::set_with_conn()`
- `fetch_repository_safe()` 已存在并接受 `OutputConfig`
- `OutputConfig` 支持 `--json` / `--machine` / `--quiet` / `--progress`
- `emit_json_data()` 信封格式已标准化

**基于当前代码的 Review 结论（clone 仍需改进的部分）：**

- **clone 虽已调用 `run_init()`，但未利用其返回值**：当前代码 `.await.map_err(...)` 只取错误，丢弃了 `InitOutput`（其中的 `repo_id`、`vault_signing`、`ssh_key_detected` 等字段未透传到 clone 的输出）
- `CloneError → CliError` 映射几乎全部落入 `CliError::fatal()`，没有显式的 `StableErrorCode`；当前依赖 `infer_stable_error_code()` 的消息子串匹配，脆弱且不可控
- 成功路径无 JSON / machine 输出；`execute_safe()` 接受 `OutputConfig` 但仅用于 stderr 装饰消息的抑制
- `setup_repository()` 内部仍通过 `restore::execute()` 做 fire-and-forget checkout；如果工作树恢复失败，clone 可能错误返回成功并继续输出成功结果
- `"Cloning into '{}' ..."` 和 `"done."` 直接写 stderr，不经过 `OutputConfig` 的 progress helper
- discovery 阶段当前把 `FetchError` 降级成 `String`，会把本地路径错误、URL 语法错误、认证失败和真实网络故障混成同一类 `InvalidRemote`；而 `FetchError::InvalidRemoteSpec` 目前也只有 `reason: String`，不足以做稳定的显式错误码拆分
- 成功 schema 若继续使用 `branch: String`，将无法真实表达“空远端 / 无可 checkout HEAD”的成功场景
- `cleanup_failed_clone()` 静默吞掉清理失败的 io::Error，且失败路径没有 warning 承载通道，用户无从知晓磁盘残留
- `CliError::with_hint()` 当前最多只保留 2 条 hint；若不定义明确优先级，clone 在透传上游 hint 时可能把 cleanup warning 或根因 hint 挤掉

### 目标与非目标

**本批目标：**
- 为 clone 补齐稳定的结构化成功输出（`--json` / `--machine`）
- 将 `CloneError → CliError` 映射改为显式 `StableErrorCode`，消除消息子串推断
- 利用 `run_init()` 返回的 `InitOutput`，将 `repo_id`、`vault_signing`、`ssh_key_detected` 等字段透传到 `CloneOutput`
- 确保 non-bare clone 的 checkout 失败会中止命令并进入显式错误路径，而不是“打印错误后仍然报成功”
- 为网络错误、认证失败等高频场景补齐 actionable hint
- 在 human 模式下提供阶段性进度（discovery → init → fetch → setup → checkout），与 init 的 `InitProgress` stderr 风格一致

**本批非目标：**
- **不在本批做性能优化**。clone 耗时的主要瓶颈是网络 I/O（discovery + pack download）和 vault keygen，这些不是输出层改造能解决的。性能优化留到后续独立批次
- **不在本批改变 fetch 内部的 progress 机制**。fetch 改进是独立的第五批工作（README.md #21）；clone 只负责在自己的渲染层控制 fetch progress 的可见性
  - **边界明确**：clone 只控制 fetch progress 的**可见性**（通过调整传递给 `fetch_repository_safe()` 的 `OutputConfig`）
  - fetch 内部的 progress 格式（NDJSON、progress bar 渲染等）由 fetch 改进批次负责，clone 批次不做任何改动
- **不在 JSON 中暴露 pack 下载统计**。`fetch_repository_safe()` 当前不返回结构化统计（objects_fetched / bytes_received），在 fetch 改进前不把这类字段写进对外 schema
- **不改变 vault 策略**。clone 始终 `vault: true`，与 init 的默认行为一致

### 设计原则

1. **clone 的渲染层独立于 init / fetch**：clone 有自己的阶段性进度和最终输出；init 和 fetch 作为内部步骤只返回结果，不产生任何 stdout/stderr 输出
2. **结构化输出只在 `execute_safe()` 最终渲染**：`execute_clone()` 返回 `CloneOutput`；`execute_safe()` 根据 `OutputConfig` 渲染 human / JSON / machine
3. **错误码显式映射，不依赖消息推断**：每个 `CloneError` 变体都有确定的 `StableErrorCode`，不再经过 `infer_stable_error_code()`
4. **网络错误必须有 hint**：discovery 失败、认证失败、fetch 超时等场景必须给出用户可行动的建议（检查 URL、检查 SSH key、检查网络）
5. **清理失败不静默**：`cleanup_failed_clone()` 的 io::Error 应作为 warning 输出到 stderr，而非仅 tracing::error
6. **non-bare clone 只有在 checkout 完成后才算成功**：remote/refs 配置完成但工作树恢复失败，必须整体视为 clone 失败
7. **成功 schema 必须忠实表达 empty remote**：不伪造分支名；对“没有任何可 checkout 分支”的成功场景使用显式空值

### 特性 1：执行层与渲染层拆分

**当前问题：** `execute_clone()` 既执行逻辑又写 stderr 装饰消息（`"Cloning into..."` / `"done."`）。成功时返回 `Result<(), CloneError>`，丢弃了 `InitOutput` 中的结构化信息，`execute_safe()` 无法渲染 JSON；同时 `setup_repository()` 内部的 checkout 失败还可能被吞掉，导致 clone 错误报成功。

**修正后的方案：**

- `execute_clone()` 改为返回 `Result<CloneOutput, CloneError>`，不做任何输出
- `execute_safe()` 调用 `execute_clone()` 后，根据 `OutputConfig` 渲染 human / JSON / machine
- human 模式下的阶段性进度由 `execute_safe()` 在调用 `execute_clone()` 的各阶段间插入 stderr 输出（参考 init 的 `InitProgress` 模式）
- `setup_repository()` 与 checkout 失败必须可传播：不再调用会自行打印 stderr 且吞错的 `restore::execute()`
- **不能**直接把现有 `restore::execute_checked() -> io::Result<()>` 作为稳定错误码来源；该接口会把 resolve/read/write 等不同失败压平成 `io::Error`
- clone 批次需要在 `restore.rs` 增补一个 typed checkout API（名称可为 `execute_checked_typed()` 或等价），返回 `Result<(), RestoreError>`，再映射到 `CloneError::CheckoutFailed { source: RestoreError }`

**`CloneOutput` 结构：**

```rust
#[derive(Debug, Clone, Serialize)]
struct CloneOutput {
    path: String,                     // 仓库绝对路径（non-bare 时为工作树根目录，非 .libra）
    bare: bool,
    remote_url: String,               // 规范化后的 remote URL
    branch: Option<String>,           // 实际 checkout 的分支名；empty remote / 无可 checkout HEAD 时为 null
    object_format: String,            // sha1 / sha256（从 InitOutput.object_format 透传）
    repo_id: String,                  // 从 InitOutput.repo_id 透传
    vault_signing: bool,              // 从 InitOutput.vault_signing 透传
    ssh_key_detected: Option<String>, // 从 InitOutput.ssh_key_detected 透传
    shallow: bool,                    // --depth 是否生效
    warnings: Vec<String>,            // 非致命警告（如 empty remote / init warning）
}
```

> **命名说明**：使用 `CloneOutput` 而非 `CloneResult`，与 init 的 `InitOutput` 命名保持一致，避免与 `Result<T, E>` 类型混淆。

**human 模式下的新流程：**

```text
步骤 1. 参数校验 + 目标路径推断
步骤 2. Remote discovery                -> stderr: "Connecting to <url> ..."
步骤 3. 目标路径预检查
步骤 4. 初始化仓库（调用 run_init）     -> stderr: "Initializing repository ..."
步骤 5. Fetch objects                    -> stderr: "Fetching objects ..." (fetch 自身的 progress bar 在 human 模式下可见)
步骤 6. 配置 remote + refs               -> stderr: "Configuring repository ..."
步骤 7. Checkout working tree            -> stderr: "Checking out working copy ..."
步骤 8. stdout 输出最终确认消息
```

**输出规则：**

- 进度输出：
  - 仅 human 模式
  - 写入 stderr
  - 参考 init 的 `InitProgress` 模式（`enabled` / `disabled` + `emit()` 方法），clone 可定义类似的 `CloneProgress` 或直接复用条件判断
  - fetch 阶段：human 模式允许 fetch 的 `IndicatifProgressBar` 显示在 stderr；`--json` / `--machine` 下必须抑制
- 最终确认消息：
  - human 模式写 stdout
  - 格式见特性 5
  - `branch` 仅在 `CloneOutput.branch.is_some()` 时显示；空远端场景不伪造 `main`
- `--quiet`：抑制所有 progress 和最终成功消息；保留错误输出
- `--json`：不输出 progress；只输出最终 JSON envelope 到 stdout
- `--machine`：同 `--json`，但必须单行紧凑

**与 init 的交互：**

clone 已调用 `run_init()`（`clone.rs:245`），当前代码只需补充**捕获返回值**的改动。

**`run_init()` 函数签名（已存在）：**

```rust
// src/command/init.rs:364
pub(crate) async fn run_init(args: InitArgs) -> Result<InitOutput, InitError>
```

`pub(crate)` 可见性允许 `clone.rs` 通过 `command::init::run_init` 调用。

**改动前后对比：**

```rust
// ---- 改动前（当前代码，clone.rs:245-260）----
command::init::run_init(command::init::InitArgs {
    bare: args.bare,
    template: None,
    initial_branch: args.branch.clone(),
    repo_directory: local_path.to_string_lossy().into_owned(),
    quiet: true,
    shared: None,
    object_format: Some(object_format),
    ref_format: None,
    from_git_repository: None,
    vault: true,
})
.await
.map_err(|error| CloneError::InitializeRepository {
    message: error.to_string(),
})?;
// InitOutput 被丢弃

// ---- 改动后 ----
let init_output = command::init::run_init(command::init::InitArgs {
    bare: args.bare,
    template: None,
    initial_branch: args.branch.clone(),
    repo_directory: local_path.to_string_lossy().into_owned(),
    quiet: true,
    shared: None,
    object_format: Some(object_format),
    ref_format: None,
    from_git_repository: None,
    vault: true,
})
.await
.map_err(|source| CloneError::InitializeRepository { source })?;
// init_output.repo_id / .vault_signing / .ssh_key_detected 透传到 CloneOutput
```

`InitOutput` 字段到 `CloneOutput` 的映射：

| InitOutput 字段 | CloneOutput 字段 | 说明 |
|-----------------|-----------------|------|
| `repo_id` | `repo_id` | 直接透传 |
| `vault_signing` | `vault_signing` | 直接透传 |
| `ssh_key_detected` | `ssh_key_detected` | 直接透传 |
| `object_format` | `object_format` | 直接透传 |
| `initial_branch` | `branch` | 仅在实际 checkout 成功且存在目标分支时写入 `Some(...)`；empty remote 时返回 `None` |
| `path` | — | 不使用；clone 自己维护 `local_path` |
| `bare` | — | 不使用；clone 自己维护 `args.bare` |
| `ref_format` | — | 不暴露（见特性 2 差异说明） |
| `converted_from` | — | clone 场景永远为 `None`，不透传 |
| `warnings` | `warnings` | 合并：init 的 warnings 追加到 clone 的 warnings |

**与 fetch 的交互：**

clone 传递给 `fetch_repository_safe()` 的 `OutputConfig` 需要根据模式调整：
- human 模式：允许 fetch 显示 progress bar（原样传递 `output`）
- `--json` / `--machine`：传入"子级输出配置"，强制 `progress = ProgressMode::None`、`json_format = None`、`quiet = true`，确保 fetch 不产生任何 progress / JSON / human 装饰输出
- `--quiet`：传入 `quiet = true`

**与 restore / checkout 的交互：**

`restore.rs` 当前已有的 fallible API 还**不够** clone 直接复用做稳定错误码映射：

- `restore::execute_checked(args) -> io::Result<()>`（`restore.rs:72`）会把“引用无法解析”“索引读取失败”“对象读取失败”“工作树写入失败”等不同原因全部压平成 `io::Error`
- `restore::execute_safe(args, output) -> CliResult<()>`（`restore.rs:57`）又会进一步包成通用 `CliError::fatal(e.to_string())`

因此 clone 批次需要在 `restore.rs` 做一个**小的加法式改动**：

- 新增 typed checkout 入口（名称可调整）：

```rust
pub async fn execute_checked_typed(args: RestoreArgs) -> Result<(), RestoreError>
```

- `RestoreError` 至少区分以下类别：
  - `ResolveSource` / `ReferenceNotCommit` -> checkout 目标不存在或不是 commit
  - `ReadIndex` / `ReadObject` / `InvalidPathEncoding` -> 读取本地仓库状态失败
  - `WriteWorktree` -> 写工作树文件失败
  - `LfsDownload` -> checkout 过程中下载 LFS 文件失败

```rust
#[derive(thiserror::Error, Debug)]
pub enum RestoreError {
    #[error("failed to resolve checkout source")]
    ResolveSource,
    #[error("reference is not a commit")]
    ReferenceNotCommit,
    #[error("failed to read index")]
    ReadIndex,
    #[error("failed to read object")]
    ReadObject,
    #[error("invalid path encoding")]
    InvalidPathEncoding,
    #[error("failed to write worktree file")]
    WriteWorktree,
    #[error("failed to download LFS content")]
    LfsDownload,
}
```

clone 再基于 `RestoreError` 做显式映射。**只有 remote/ref 配置和 checkout 都成功后**才返回 `CloneOutput`。

### 特性 2：JSON 输出设计

**成功输出结构：**

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/my-repo",
    "bare": false,
    "remote_url": "git@github.com:user/repo.git",
    "branch": "main",
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519",
    "shallow": false,
    "warnings": []
  }
}
```

**`--bare` 场景：**

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/my-repo.git",
    "bare": true,
    "remote_url": "git@github.com:user/repo.git",
    "branch": "main",
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": null,
    "shallow": false,
    "warnings": []
  }
}
```

> **注**：此示例为非空远端的 bare clone。若远端为空仓库，bare clone 同样返回 `"branch": null`，与非 bare 空远端场景一致。
>
> 空远端 bare clone 示例：
> ```json
> {
>   "ok": true,
>   "command": "clone",
>   "data": {
>     "path": "/Users/eli/projects/empty-repo.git",
>     "bare": true,
>     "remote_url": "git@github.com:user/empty-repo.git",
>     "branch": null,
>     "object_format": "sha1",
>     "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
>     "vault_signing": true,
>     "ssh_key_detected": null,
>     "shallow": false,
>     "warnings": [
>       "You appear to have cloned an empty repository."
>     ]
>   }
> }
> ```

**`--depth` 浅克隆场景：**

`"shallow": true`，其余字段不变。

**空远端场景：**

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/empty-repo",
    "bare": false,
    "remote_url": "git@github.com:user/empty-repo.git",
    "branch": null,
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": null,
    "shallow": false,
    "warnings": [
      "You appear to have cloned an empty repository."
    ]
  }
}
```

**与 init schema 的差异说明：**

- clone 成功 JSON 不暴露 `ref_format` 和 `converted_from`。原因：clone 对外结果聚焦远端接入与工作副本落地状态；`ref_format` 在当前使用场景中不作为 clone 决策输入；`converted_from` 在 clone 场景永远为 `None`。
- clone 使用 `branch: Option<String>`（实际 checkout 的分支名；empty remote 时为 `null`），而 init 使用 `initial_branch`（创建时的默认分支名）。两者语义不同：clone 的 `branch` 取决于远端 HEAD 或 `--branch` 参数，并且可能不存在。
- 若后续 Agent/脚本出现明确需求，可按向后兼容方式增量添加 `ref_format` 字段。

**明确不纳入本批 JSON 契约的字段：**

- `objects_fetched` / `bytes_received` / `pack_size`：fetch 改进前不承诺
- `checkout_files_count`：typed checkout helper 只返回成功/失败，不返回结构化文件计数

### 特性 3：错误处理、退出码与 Hint

**错误输出通道约束：**

- 成功结构化输出通过 `emit_json_data()` 输出到 stdout。
- 错误 JSON 统一通过 `CliError` 输出到 stderr，避免命令私有 envelope 分叉。
- clone 失败后的 cleanup warning 也通过 `CliError.hints` 承载，而不是单独再写一套错误 envelope

**`CloneError` 结构调整：**

```rust
#[derive(thiserror::Error, Debug)]
pub enum CloneError {
    // ... 其他变体保持不变 ...

    /// 替换原有的 `InitializeRepository { message: String }`
    #[error("failed to initialize repository")]
    InitializeRepository { source: InitError },

    /// 新增：discovery 阶段保留原始类型信息
    #[error("remote discovery failed")]
    DiscoverRemote { source: fetch::FetchError },

    /// 新增：checkout 失败可传播
    #[error("failed to checkout working tree")]
    CheckoutFailed { source: RestoreError },

    // 删除：InvalidRemote { message: String }
    // 改为使用 DiscoverRemote 保留类型信息
}
```

变更说明：
- **删除** `InvalidRemote { message: String }`，discovery 阶段改为 `CloneError::DiscoverRemote { source: fetch::FetchError }`，保留 `FetchError` / `GitError` 的原始类型信息
- **`InitializeRepository`** 从 `{ message: String }` 改为 `{ source: InitError }`，透传 init 的完整错误类型
- **新增** `CheckoutFailed { source: RestoreError }`，保证 checkout 失败不再被吞掉，并且为 clone 保留足够的类型信息去做 `RepoStateInvalid` / `IoReadFailed` / `IoWriteFailed` / `NetworkUnavailable` 的稳定映射

**`FetchError::InvalidRemoteSpec` 的 typed 分类要求：**

`FetchError::InvalidRemoteSpec` 目前只有 `{ spec, reason: String }`，仍不足以做稳定的显式错误码拆分。为避免回退到字符串匹配，clone 批次需要在 `fetch.rs` 做一个**小的加法式改动**，例如：

```rust
pub enum RemoteSpecErrorKind {
    MissingLocalRepo,
    InvalidLocalRepo,
    MalformedUrl,
    UnsupportedScheme,
}

InvalidRemoteSpec {
    spec: String,
    kind: RemoteSpecErrorKind,
    reason: String,
}
```

或提供等价的 typed helper。下面的映射表以“已能拿到 typed kind”为前提。

**`CloneError / source → CliError` 显式映射：**

当前 `From<CloneError> for CliError`（`clone.rs:82-90`）只对 `CannotInferDestination` 做了特殊处理，其余全部 fallback 到 `CliError::fatal()`。改造后每个变体和关键 source 子类型都有显式 `StableErrorCode`：

| CloneError / source | StableErrorCode | exit | hint |
|---------------------|-----------------|------|------|
| `CannotInferDestination` | `CliInvalidArguments` | `129` | "please specify the destination path explicitly" |
| `DestinationExistsNonEmpty` | `CliInvalidTarget` | `129` | "choose a different path or empty the directory first" |
| `DestinationAlreadyRepo` | `RepoStateInvalid` | `128` | "the destination already contains a libra repository" |
| `CreateDestinationFailed` | `IoWriteFailed` | `128` | "check directory permissions and disk space" |
| `DiscoverRemote { source: FetchError::InvalidRemoteSpec { kind: MissingLocalRepo \| InvalidLocalRepo, .. } }` | `RepoNotFound` | `128` | "use a valid libra repository path or a reachable remote URL" |
| `DiscoverRemote { source: FetchError::InvalidRemoteSpec { kind: MalformedUrl \| UnsupportedScheme, .. } }` | `CliInvalidTarget` | `129` | "check the clone URL or scheme, for example `https://`, `ssh`, or a local path" |
| `DiscoverRemote { source: FetchError::Discovery { source: GitError::UnAuthorized(_) } }` | `AuthPermissionDenied` | `128` | "check SSH key / HTTP credentials and repository access rights" |
| `DiscoverRemote { source: FetchError::Discovery { source: GitError::NetworkError(_) \| GitError::IOError(_) } }` | `NetworkUnavailable` | `128` | "check the remote host, DNS, VPN/proxy, and network connectivity" |
| `DiscoverRemote { source: FetchError::Discovery { .. } }` | `NetworkProtocol` | `128` | "the remote did not complete discovery successfully; retry and inspect server/protocol settings" |
| `ChangeDirectory` / `RestoreDirectory` | `InternalInvariant` | `128` | Issues URL（对齐 init 的 `VaultInitializationFailed` / `Database` 处理方式） |
| `InitializeRepository` | 透传 init 的错误码 | 透传 | 透传（init 的 `InitError → CliError` 已有完整映射） |
| `RemoteBranchNotFound` | `RepoStateInvalid` | `128` | "use `-b <branch>` to specify an existing branch, or omit to use remote HEAD" |
| `FetchFailed` + `FetchError::ObjectFormatMismatch` | `RepoStateInvalid` | `128` | "the remote and local repository use different object formats" |
| `FetchFailed` + `FetchError::FetchObjects` / `PacketRead` | `NetworkUnavailable` | `128` | "network error during transfer; check connectivity and retry" |
| `FetchFailed` + `FetchError::RemoteSideband` / `ChecksumMismatch` | `NetworkProtocol` | `128` | "the remote transfer failed or returned corrupted data; retry the clone" |
| `FetchFailed` + `FetchError::RemoteBranchNotFound` | `RepoStateInvalid` | `128` | "the specified branch does not exist on the remote" |
| `CheckoutFailed { source: RestoreError::ResolveSource \| RestoreError::ReferenceNotCommit }` | `RepoStateInvalid` | `128` | "working tree checkout target could not be resolved" |
| `CheckoutFailed { source: RestoreError::ReadIndex \| RestoreError::ReadObject \| RestoreError::InvalidPathEncoding }` | `IoReadFailed` | `128` | "failed to read repository state while checking out the working tree" |
| `CheckoutFailed { source: RestoreError::WriteWorktree }` | `IoWriteFailed` | `128` | "working tree checkout did not complete because files could not be written" |
| `CheckoutFailed { source: RestoreError::LfsDownload }` | `NetworkUnavailable` | `128` | "checkout required downloading LFS content, but the transfer failed" |
| `SetupFailed` | `InternalInvariant` | `128` | Issues URL |

**hint 预算与优先级说明：**

当前 `CliError::with_hint()` 最多只保留 2 条 hint（`src/utils/error.rs:411-422`）。clone 不能假设可以无限追加 hint，必须遵守固定优先级：

1. cleanup warning（如果存在）
2. 根因对应的 primary actionable hint
3. clone 上下文说明（如 `during clone initialization`）**不占用 hint 配额**，应写入 message 或 detail，而不是再追加第三条 hint

当需要透传上游错误（如 `InitializeRepository -> InitError`）时：

- 保持原始错误的 `StableErrorCode` 和 exit code 不变
- 默认保留上游错误的 primary hint
- 如果 cleanup warning 存在且 hint 已满，优先保留 cleanup warning，替换掉较低优先级的 secondary hint
- 实现上不要直接链式调用 `source.into().with_hint(...)` 期待自动合并；需要时应显式重建最终 `CliError`，按优先级挑选最多 2 条 hint

**`InitializeRepository` 透传说明：**

当前 `CloneError::InitializeRepository` 只保存 `message: String`，丢失了 `InitError` 的类型信息。改造后直接存储 `InitError`：

```rust
InitializeRepository { source: InitError }
```

在 `From<CloneError> for CliError` 中对 `source` 调用 `.into()`，直接复用 init 已落地的映射链路。

```rust
impl From<CloneError> for CliError {
    fn from(error: CloneError) -> Self {
        match error {
            // ... 其他变体 ...
            CloneError::InitializeRepository { source } => source.into(), // 透传 InitError → CliError
            CloneError::DiscoverRemote { source } => /* 根据 source 类型映射，见映射表 */,
            CloneError::CheckoutFailed { source } => /* 根据 RestoreError 类型映射，见映射表 */
        }
    }
}
```

**`CheckoutFailed` 说明：**

`restore.rs` 当前只有 `io::Result<()>` 级别的 fallible API，这还不够 clone 做稳定错误码映射：

- `restore::execute_checked(args) -> io::Result<()>`（`restore.rs:72`）：底层 checkout 实现，跳过 `require_repo()` 检查
- `restore::execute_safe(args, output) -> CliResult<()>`（`restore.rs:57`）：包裹层，带 `require_repo()` 前置检查

因此 clone 批次需要在 `restore.rs` 增补 typed 入口，例如：

```rust
pub async fn execute_checked_typed(args: RestoreArgs) -> Result<(), RestoreError>
```

clone 再改为调用这个 typed 入口：

```rust
command::restore::execute_checked_typed(RestoreArgs {
    worktree: true,
    staged: true,
    source: None,
    pathspec: vec![util::working_dir_string()],
})
.await
.map_err(|e| CloneError::CheckoutFailed { source: e })?;
```
```

**`DiscoverRemote` 映射实现说明：**

`DiscoverRemote` 的映射依赖对 `FetchError` 和 `GitError`（来自 `git_internal::errors`）的嵌套 pattern match。`GitError` 是外部 crate 类型，其变体可能随版本变化。实现时必须包含 fallback 分支（映射表最后一行 `Discovery { .. }` → `NetworkProtocol`），确保新增的 `GitError` 变体不会导致编译错误或遗漏。`FetchError::InvalidRemoteSpec` 的本地路径/URL 拆分则必须依赖前文定义的 typed `kind`，**不能**再回退到解析 `reason` 字符串。

> hint 的完整映射已包含在上方「`CloneError / source → CliError` 显式映射」表中，不再单独列出。

**Cross-Cutting Improvements 在 clone 中的具体落地：**

| ID | 改进 | clone 中的具体落地 |
|----|------|-------------------|
| **A** | 退出码 `0/128/129` | 参数错误（无法推断目标路径）→ exit `129`；运行时错误 → exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | 不适用——clone 没有 enum 类型的参数值需要 fuzzy match |
| **G** | Issues URL | `ChangeDirectory` / `RestoreDirectory` / `SetupFailed` → `LBR-INTERNAL-001` 时输出 Issues URL |

### 特性 4：清理失败可见性

**当前问题：** `cleanup_failed_clone()`（`clone.rs:286-300`）用 `tracing::error!()` 记录清理失败，但 tracing 在默认配置下不输出到用户 stderr，导致磁盘残留无声无息。

**修正后的方案：**

- `cleanup_failed_clone()` 改为返回 `Option<String>`（或 `Vec<String>`）warning，而不是只写 tracing
- 成功路径：`CloneOutput.warnings` 只承载真正的**非致命成功 warning**（如 empty remote / init warning）；cleanup warning 不会出现在成功结果中
- 失败路径：`execute_safe()` 先把根因转成 `CliError`，再把 cleanup warning 通过 `.with_hint(...)` 附加到同一个错误对象
- human 模式：warning 仍由统一 `CliError` 渲染链路输出到 stderr，格式 `warning: failed to clean up '<path>': <io_error>` 或等价 hint
- JSON / machine 模式：warning 出现在错误 JSON 的 `hints` 数组中
- **优先级要求**：cleanup warning 的可见性高于泛化 retry hint；如果 `CliError` hint 数量受限，应优先保留 cleanup warning

### 特性 5：成功消息与 Human Output

#### `libra clone git@github.com:user/repo.git`

```text
Connecting to git@github.com:user/repo.git ...
Initializing repository ...
Fetching objects ...
████████████████████████████████████████ 100% (256 objects)
Configuring repository ...
Checking out working copy ...
Cloned into 'repo'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
```

> **注**：Tip 格式与 init 的 `render_init_result()` 保持一致（`init.rs:332-348`），使用 `display_home_relative()` 缩短路径显示。

#### `libra clone --bare git@github.com:user/repo.git repo.git`

```text
Connecting to git@github.com:user/repo.git ...
Initializing repository ...
Fetching objects ...
████████████████████████████████████████ 100% (256 objects)
Configuring repository ...
Cloned into bare repository 'repo.git'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled
```

#### `libra clone git@github.com:user/empty.git`

```text
Connecting to git@github.com:user/empty.git ...
Initializing repository ...
Fetching objects ...
Configuring repository ...
Cloned into 'empty'
  remote: origin → git@github.com:user/empty.git
  signing: enabled

warning: You appear to have cloned an empty repository.
```

#### `libra clone --quiet git@github.com:user/repo.git`

```text
(no output on success)
```

#### 错误：无法推断目标路径

```text
error: please specify the destination path explicitly
Error-Code: LBR-CLI-002

hint: please specify the destination path explicitly
```

#### 错误：远程仓库不可达（网络故障 / DNS 失败）

```text
fatal: remote discovery failed
Error-Code: LBR-NET-001

hint: check the remote host, DNS, VPN/proxy, and network connectivity
```

#### 错误：无效的 clone URL

```text
fatal: remote discovery failed
Error-Code: LBR-CLI-003

hint: check the clone URL or scheme, for example `https://`, `ssh`, or a local path
```

#### 错误：指定分支不存在

```text
fatal: remote branch 'nonexistent' not found in upstream origin
Error-Code: LBR-REPO-003

hint: use -b <branch> to specify an existing branch, or omit to use remote HEAD
```

#### 错误 JSON：远程仓库不可达（网络故障）

```json
{
  "ok": false,
  "error_code": "LBR-NET-001",
  "category": "network",
  "exit_code": 128,
  "message": "remote discovery failed",
  "hints": [
    "check the remote host, DNS, VPN/proxy, and network connectivity"
  ]
}
```

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra clone git@github.com:user/repo.git             Clone via SSH
    libra clone https://github.com/user/repo.git          Clone via HTTPS
    libra clone git@github.com:user/repo.git my-dir       Clone to specific directory
    libra clone --bare git@github.com:user/repo.git       Create bare clone
    libra clone -b develop git@github.com:user/repo.git   Clone specific branch
    libra clone --single-branch -b main <url>             Clone only one branch
    libra clone --depth 1 <url>                           Shallow clone (latest commit only)
```

### Libra vs Git vs jj 克隆命令对比

| Use Case | Git | jj | Libra（本批目标） |
|----------|-----|----|-------------------|
| 基本克隆 | `git clone <url>` | `jj git clone <url>` | `libra clone <url>` |
| 指定目标目录 | `git clone <url> <dir>` | `jj git clone <url> <dir>` | `libra clone <url> <dir>` |
| bare 克隆 | `git clone --bare <url>` | 无直接等价 | `libra clone --bare <url>` |
| 指定分支 | `git clone -b <branch> <url>` | `jj git clone -b <branch> <url>` | `libra clone -b <branch> <url>` |
| 浅克隆 | `git clone --depth N <url>` | 无直接等价 | `libra clone --depth N <url>` |
| single branch | `git clone --single-branch <url>` | 无直接等价 | `libra clone --single-branch <url>` |
| 成功输出 | 简短 stderr 消息 | 简短 stderr 消息 | **remote/branch/signing 摘要 + SSH tip** |
| 实时进度 | 有 progress bar | 有 progress bar | **阶段性 stderr progress + fetch progress bar** |
| 结构化输出 | 无 | 无 | **`--json` / `--machine`** |
| 认证引导 | 无 | 无 | **SSH key 检测 + 后续 tip** |
| 错误 hint | 基本无 | 基本无 | **每种错误类型均有 actionable hint** |

### 测试要求

#### `tests/command/clone_cli_test.rs`（L1 确定性测试，扩展）

- **（现有）invalid source 不 panic**：使用不存在的本地路径作为 source → exit 128，error code `LBR-REPO-001`（`RepoNotFound`）
- **（现有）missing branch 清理**：exit 128，error code `LBR-REPO-003`，**不仅预存在的空目录被还原，因 clone 创建的本地 `.libra` 数据库及目录也必须被彻底删除**。
- **（现有）successful clone 无 debug noise**：stderr 有阶段性进度，无 DEBUG/WARN/INFO
- **（现有）vault 初始化**：`.libra/vault.db` 存在，`vault.signing=true`，`vault.gpg.pubkey` 非空
- **（现有）`--machine` 抑制装饰 stderr**：stderr 无 `"Cloning into"`
- **（新增）`--json` 成功输出 schema**：验证 JSON envelope 包含所有 `CloneOutput` 字段，类型正确
- **（新增）empty remote JSON**：验证 `branch: null`，且 `warnings` 包含 `"You appear to have cloned an empty repository."`
- **（新增）`--machine` 成功输出**：stdout 按 `\n` 分割恰好 1 行非空行，可被 `serde_json::from_str()` 解析
- **（新增）`--quiet` 成功时 stdout 和 stderr 均无输出**
- **（新增）错误码显式验证**：
  - `CannotInferDestination` → `LBR-CLI-002`，exit `129`
  - `DestinationExistsNonEmpty` → `LBR-CLI-003`，exit `129`
  - `DiscoverRemote`（`InvalidRemoteSpec.kind = MissingLocalRepo | InvalidLocalRepo`）→ `LBR-REPO-001`，exit `128`
  - `DiscoverRemote`（`InvalidRemoteSpec.kind = MalformedUrl | UnsupportedScheme`）→ `LBR-CLI-003`，exit `129`
  - `RemoteBranchNotFound` → `LBR-REPO-003`，exit `128`
- **（新增）checkout failure 不得报成功**：分别模拟 `RestoreError::ResolveSource` / `RestoreError::ReadObject` / `RestoreError::WriteWorktree`，验证命令 exit 非零且错误码分别映射到 `LBR-REPO-003` / `LBR-IO-001` / `LBR-IO-002`
- **（新增）hint 存在性验证**：网络错误和认证错误的 stderr 包含 "check" 等 actionable 关键词
- **（新增）cleanup warning 优先级**：构造“根因已有 2 条 hint + cleanup warning”的场景，验证最终错误输出仍保留 cleanup warning，且总 hint 数不超过 2
- **（新增）init 输出隔离**：`--json clone` 的 stdout 只有一个 JSON envelope，不混入 init 的 JSON 或 progress
- **（新增）fetch progress 隔离**：`--json clone` 的 stderr 不包含 fetch 的 NDJSON progress 事件
- **测试隔离要求**：所有涉及 `ssh_key_detected` 的断言必须使用隔离的 `HOME` / `USERPROFILE` / `XDG_CONFIG_HOME`，避免宿主机真实 `~/.ssh` 污染

**测试隔离实现建议：**

复用或扩展 `tests/command/mod.rs` 中的 RAII guard 模式：用 `tempdir()` 创建临时 `HOME`，在 `Drop` 中恢复原始环境变量（`HOME` / `USERPROFILE` / `XDG_CONFIG_HOME`）。涉及 `ssh_key_detected` 的测试在临时 `HOME/.ssh/` 下放置 mock key 文件。具体实现留到编码阶段。

#### `tests/command/clone_test.rs`（L2 网络测试，扩展）

- 现有 L2 测试保持不变（依赖 GitHub API）
- **（新增）`--json` L2 测试**：对真实 GitHub 仓库执行 `--json clone`，验证 `remote_url` / `branch` / `vault_signing` 字段与实际一致
- **（新增）`--depth` JSON**：验证 `shallow: true`
- **（新增）认证失败 hint / code**：对受保护仓库或无权限 URL 验证 auth 类错误码与 hint（若测试环境不可稳定复现，至少保留本地可控的单元测试覆盖映射函数）

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入迁移范围的命令、内部模块和转发路径，都必须有对应的集成测试覆盖新 config_kv 读写链路

### 文档与变更记录

- 创建或更新 `docs/commands/clone.md`
  - 说明阶段性进度仅适用于 human 模式
  - 说明 `--quiet` 成功时静默
  - 说明 `--json` / `--machine` 的 schema
  - 补充 Libra vs Git vs jj 克隆命令对比
- 更新 `CHANGELOG.md`
  - 记录 clone 的 JSON / machine / error code / hint 改进
  - 记录 `CloneError → CliError` 从消息推断改为显式映射

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/clone.rs` | **重构** | 执行层返回 `CloneOutput`；显式错误码映射；typed checkout error 传播；cleanup warning；阶段性 progress |
| `src/command/init.rs` | **无改动** | init 改造已完成交付——`run_init()` 纯执行入口、`InitOutput` 结构体、`InitError → CliError` 显式映射均已就绪；clone 批次不在该文件新增需求 |
| `src/command/restore.rs` | **小改** | 新增 typed checkout 入口与 `RestoreError`，避免把 resolve/read/write/LFS 失败全部压平成 `io::Error` |
| `src/command/fetch.rs` | **小改** | 为 `InvalidRemoteSpec` 增补 typed `kind`（或等价 helper），避免 clone 为了显式错误码再次依赖字符串匹配 |
| `tests/command/clone_cli_test.rs` | **扩展** | 新增 JSON schema / machine 格式 / quiet / 错误码 / hint / cleanup warning / empty remote / checkout failure / hint 优先级 / 隔离验证 |
| `tests/command/clone_test.rs` | **扩展** | 新增 L2 JSON 验证 |
