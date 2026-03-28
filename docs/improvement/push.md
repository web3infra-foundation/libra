## Push 命令改进详细计划

> 最后编写时间：2026-03-28（已根据代码实现同步）

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config`、`init`、`clone`、`add`、`status`、`commit` 的主改造已在当前代码库落地（或已有改进计划）。`push` 是审计报告中"最严重的三个缺陷"之一，存在功能性问题和大量 UX 缺陷。

**已确认落地的基线：**

- `config_kv` 后端已落地；`push` 已通过 `ConfigKv` 读取 remote URL、upstream 配置
- `OutputConfig` + `emit_json_data()` + `info_println!()` + `ProgressReporter` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_exit_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, _output)` 双入口已存在（`push.rs`）
- SSH 和 HTTP 传输均已实现
- LFS 文件上传已实现（仅 HTTP 传输）
- Pack 增量优化已实现（fast-forward 检测减少发送对象数）
- Delta 压缩和 pack encoding 已实现
- Reflog 记录（`ReflogAction::Push`）已实现
- Remote tracking branch 更新已实现（数据库事务内）
- `--set-upstream` / `--force` / `--dry-run` 标志已实现
- Force push 检测与 warning 已实现

### 实施状态

> **所有本批目标均已完成并通过质量验收。** 以下各节已更新为实际落地的设计与代码。

### 目标与非目标

**本批目标（全部已完成 ✅）：**
- ✅ 引入 `PushError` typed error enum（20 个变体），替代内部 `CliError::fatal()` 散射
- ✅ 所有 `PushError → CliError` 映射使用显式 `StableErrorCode`
- ✅ 拆分执行层与渲染层：`run_push(args, output) -> Result<PushOutput, PushError>` 纯执行入口
- ✅ 明确定义并实现本批支持的 refspec 语义（默认同名分支、`<name>`、`<src>:<dst>`；多冒号形态如 `a:b:c` 显式拒绝）
- ✅ 清理生产路径中的 `panic!`、裸 `println!()` / `eprintln!()`，统一回收到 `OutputConfig` 和 `PushError`
- ✅ Transport 层空闲超时：HTTPS `connect_timeout(10s)` + `read_timeout(10s)`；SSH 每次 I/O 操作 `tokio::time::timeout(10s)` 包裹
- ✅ Discovery 阶段 `10s` 连接超时（`tokio::time::timeout` 包裹 `discovery_reference`）
- ✅ 完善 JSON 输出 schema（`PushOutput`），包含推送详情
- ✅ 进度输出经过 `ProgressReporter` + `OutputConfig` 管控；`--json` / `--machine` 成功路径 stderr 干净
- ✅ 执行层 warning 收集（`diff_tree_objs` 不再直接 `emit_warning`，而是收集到 `PushOutput.warnings`）
- ✅ `--dry-run` 输出结构化预览
- ✅ 补齐 `--help` EXAMPLES 段
- ✅ 完善 hint 体系，覆盖常见失败场景
- ✅ Cross-Cutting F：remote 名 fuzzy match（Levenshtein 距离 ≤ 2 时提示 `did you mean`）
- ✅ Cross-Cutting G：`ObjectCollection` / `PackEncoding` 附 Issues URL

**本批非目标：**
- **不改变 LFS 上传逻辑**。LFS 文件检测和上传流程不变
- **不改变 pack 增量/delta 压缩算法**。性能优化留后续批次
- **不引入 In-process SSH Client**。这是全局改进项 H，留后续批次
- **不改变 reflog 记录格式**。`ReflogAction::Push` 保持不变
- **不引入 push mirror/tags/delete 语义**。这些是新特性，不在本批范围
- **不在本批承诺 push 的 NDJSON progress 契约**。本批先保证 human 进度和结构化 success envelope 不互相污染；若后续需要结构化进度事件，再与 transport/fetch 批次统一设计

> **注：** 原计划中"不改变 SSH/HTTP 传输核心逻辑"已调整——为落地空闲超时契约，实际在 `ssh_client.rs` 和 `https_client.rs` 中做了超时包装改造，但未改变协议逻辑本身。

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_push()` 收集结构化结果，再根据 `OutputConfig` 渲染 human/JSON/machine
2. **typed error enum 取代散射的 `CliError::fatal()`**：每个失败场景有确定的 `PushError` 变体
3. **StableErrorCode 显式映射**：消除对 `infer_stable_error_code()` 的依赖
4. **refspec 语义必须先收敛再实现**：本批只支持三种输入形态：省略（推当前分支）、`<name>`（同名分支）、`<src>:<dst>`（显式映射）；多冒号形态（如 `a:b:c`）和空段（如 `:dst`、`src:`）显式报 `InvalidRefspec`
5. **超时是空闲超时，不是总时长硬截止**：HTTPS 使用 `reqwest::Client` 的 `connect_timeout(10s)` + `read_timeout(10s)`（socket 级无数据到达即触发）；SSH 使用 `tokio::time::timeout(10s)` 包裹每次 `read_exact` / `write_all` / `wait_with_output`（有数据流就续命）；discovery 阶段额外包裹 `tokio::time::timeout(10s)` 作为整体保险
6. **结构化模式默认保持 stderr 干净**：`--json` / `--machine` 成功路径只输出一个 envelope；执行层 warning（如 submodule 不支持）收集到 `PushOutput.warnings` 而非直接 `emit_warning`，由渲染层根据模式决定输出方式
7. **`--dry-run` 可被 Agent 消费**：JSON 模式下返回结构化预览（将推送的 ref 和对象数）
8. **hint 覆盖常见失败**：网络超时、认证失败、non-fast-forward、missing remote、invalid refspec 等每种场景提供可操作的 hint
9. **Cross-Cutting F：fuzzy match**：`RemoteNotFound` 携带 `suggestion: Option<String>`，基于已配置 remote 列表的 Levenshtein 距离匹配，edit distance ≤ 2 时以 `priority_hint` 形式提示 `did you mean '<closest>'?`

### 特性 1：PushError typed error enum

**已落地方案（20 个变体）：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("HEAD is detached; cannot determine what to push")]
    DetachedHead,

    #[error("no configured push destination")]
    NoRemoteConfigured,

    #[error("remote '{name}' not found")]
    RemoteNotFound { name: String, suggestion: Option<String> },

    #[error("invalid refspec '{0}'")]
    InvalidRefspec(String),

    #[error("source ref '{0}' not found")]
    SourceRefNotFound(String),

    #[error("pushing to local file repositories is not supported")]
    UnsupportedLocalFileRemote,

    #[error("invalid remote URL '{url}': {detail}")]
    InvalidRemoteUrl { url: String, detail: String },

    #[error("authentication failed for '{url}'")]
    AuthenticationFailed { url: String },

    #[error("failed to discover references from '{url}': {detail}")]
    DiscoveryFailed { url: String, detail: String },

    #[error("network timeout during {phase} after {seconds}s")]
    Timeout { phase: String, seconds: u64 },

    #[error("cannot push to '{remote_ref}': non-fast-forward update")]
    NonFastForward { local_ref: String, remote_ref: String },

    #[error("remote object format '{remote}' does not match local '{local}'")]
    HashKindMismatch { remote: String, local: String },

    #[error("failed to collect objects for push: {0}")]
    ObjectCollection(String),

    #[error("pack encoding failed: {0}")]
    PackEncoding(String),

    #[error("remote rejected push: unpack failed")]
    RemoteUnpackFailed,

    #[error("remote rejected ref update for '{refname}': {reason}")]
    RemoteRefUpdateFailed { refname: String, reason: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("LFS upload failed for '{path}': {detail}")]
    LfsUploadFailed { path: String, oid: String, detail: String },

    #[error("failed to update local tracking ref: {0}")]
    TrackingRefUpdate(String),

    #[error("failed to read repository state: {0}")]
    RepoState(String),
}
```

> **与原计划差异：**
> - `RemoteNotFound` 从 `RemoteNotFound(String)` 变为 struct variant `{ name, suggestion }` 以支持 Cross-Cutting F fuzzy match
> - 新增 `HashKindMismatch` 变体（remote/local object format 不匹配时触发，原代码中已有此检查但未建模为 typed error）

**`PushError → CliError` 显式映射（已落地）：**

| PushError 变体 | StableErrorCode | 退出码 | hint |
|---------------|-----------------|--------|------|
| `DetachedHead` | `RepoStateInvalid` | 128 | `checkout a branch before pushing` + `use 'libra switch <branch>' to switch` |
| `NoRemoteConfigured` | `RepoStateInvalid` | 128 | `use 'libra remote add <name> <url>' to configure a remote` + `or specify the remote explicitly: 'libra push <remote> <branch>'` |
| `RemoteNotFound` | `CliInvalidTarget` | 129 | `use 'libra remote -v' to see configured remotes` + （若有 suggestion）`did you mean '<closest>'?` 作为 priority_hint |
| `InvalidRefspec` | `CliInvalidArguments` | 129 | `use '<name>' or '<src>:<dst>'` |
| `SourceRefNotFound` | `CliInvalidTarget` | 129 | `verify the local branch/ref exists before pushing` |
| `UnsupportedLocalFileRemote` | `CliInvalidTarget` | 129 | `use fetch/clone for local-path repositories; push currently supports network remotes only` |
| `InvalidRemoteUrl` | `CliInvalidArguments` | 129 | `check the remote URL with 'libra remote get-url <name>'` |
| `AuthenticationFailed` | `AuthMissingCredentials` | 128 | `check SSH key or HTTP credentials` + `use 'libra config --list' to verify auth settings` |
| `DiscoveryFailed` | `NetworkUnavailable` | 128 | `check the remote URL and network connectivity` |
| `Timeout` | `NetworkUnavailable` | 128 | `check network connectivity and retry` |
| `NonFastForward` | `ConflictOperationBlocked` | 128 | `pull and integrate remote changes first: 'libra pull'` + `or use --force to overwrite (data loss risk)` |
| `HashKindMismatch` | `NetworkProtocol` | 128 | 无 |
| `ObjectCollection` | `InternalInvariant` | 128 | Issues URL |
| `PackEncoding` | `InternalInvariant` | 128 | Issues URL |
| `RemoteUnpackFailed` | `NetworkProtocol` | 128 | `the remote server failed to process the pack; retry or check server logs` |
| `RemoteRefUpdateFailed` | `NetworkProtocol` | 128 | `the remote rejected the update; check branch protection rules` |
| `Network` | `NetworkUnavailable` | 128 | `check network connectivity and retry` |
| `LfsUploadFailed` | `NetworkUnavailable` | 128 | `check LFS endpoint configuration`；`.with_detail("oid", oid)` 暴露对象标识供 Agent 使用 |
| `TrackingRefUpdate` | `IoWriteFailed` | 128 | 无 |
| `RepoState` | `RepoCorrupt` | 128 | `try 'libra status' to verify repository state` |

### 特性 2：执行层与渲染层拆分

**已落地方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct PushRefUpdate {
    pub local_ref: String,
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushOutput {
    pub remote: String,
    pub url: String,
    pub updates: Vec<PushRefUpdate>,
    pub objects_pushed: usize,
    pub bytes_pushed: u64,
    pub lfs_files_uploaded: usize,
    pub dry_run: bool,
    pub up_to_date: bool,
    pub upstream_set: Option<String>,
    pub warnings: Vec<String>,
}
```

已落地的调用链：
- `execute_safe(args, output)` → 参数校验 → `run_push(args, output)` → 返回 `PushOutput`
- `execute_safe` 调用 `render_push_output(&result, output)` 按模式渲染
- 进度回调通过 `ProgressReporter` 发送，`--json`/`--machine` 模式下静默
- 执行层 warning（如 submodule 不支持）收集到 `IncrementalObjsResult.warnings`，合并到 `PushOutput.warnings`

**refspec 语义（已落地）：**

```text
libra push
  -> 当前分支推送到 tracking remote；若未配置 upstream，则推送到默认 remote 的同名分支

libra push origin main
  -> 本地 refs/heads/main 推送到远端 refs/heads/main

libra push origin local_branch:release
  -> 本地 refs/heads/local_branch 推送到远端 refs/heads/release
```

约束：
- 不支持空 src / 空 dst / 删除语法（如 `:dst`、`src:`）
- 不支持多冒号形态（如 `a:b:c`、`a::b`）→ 显式报 `InvalidRefspec`
- 不支持一次推送多个 refspec
- 非法形态统一返回 `PushError::InvalidRefspec`

**超时策略（已落地）：**

| 阶段 | 超时类型 | 实现位置 | 语义 |
|------|---------|---------|------|
| Discovery / 建连 | 连接超时 10s | `push.rs`：`tokio::time::timeout` 包裹 `discovery_reference` | 整体调用超时 |
| HTTPS 建连 | 连接超时 10s | `https_client.rs`：`reqwest::Client::builder().connect_timeout()` | TCP+TLS 握手超时 |
| HTTPS 读取 | 空闲超时 10s | `https_client.rs`：`reqwest::Client::builder().read_timeout()` | socket 级无数据到达即触发，有数据流自动续命 |
| SSH advertisement 读取 | 空闲超时 10s | `ssh_client.rs`：每次 `read_exact` 包裹 `tokio::time::timeout` | 每帧独立计时 |
| SSH pack 写入 | 空闲超时 10s | `ssh_client.rs`：`write_all` + `shutdown` 包裹 `tokio::time::timeout` | 写入卡住即触发 |
| SSH receive-pack 等待 | 空闲超时 10s | `ssh_client.rs`：`wait_with_output` 包裹 `tokio::time::timeout` | 远端处理挂起即触发 |

超时错误经 `classify_transport_error()` 统一映射为 `PushError::Timeout { phase, seconds: 10 }`。

**渲染规则（已落地）：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 推送摘要 | 进度条（ProgressReporter） |
| human + `--quiet` | 无 | 仅 warning（如 force push 警告） |
| `--json` / `--machine` | JSON envelope | 默认保持干净，不输出 progress / human 文本 |
| `--dry-run` | 预览摘要（不执行推送） | 无进度 |

**human 模式推送摘要（已落地）：**

```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main
 256 objects pushed (1.2 MiB)
```

up-to-date 场景：
```text
Everything up-to-date
```

force push：
```text
To git@github.com:user/repo.git
 + abc1234...def5678 main -> main (forced update)
warning: force push overwrites remote history
```

`--dry-run`：
```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main (dry run)
 256 objects would be pushed
```

`--set-upstream` 成功：
```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main
branch 'main' set up to track 'origin/main'
```

### 特性 3：JSON 输出设计

**成功输出：**

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": false
      }
    ],
    "objects_pushed": 256,
    "bytes_pushed": 1258291,
    "lfs_files_uploaded": 1,
    "dry_run": false,
    "up_to_date": false,
    "upstream_set": "origin/main",
    "warnings": []
  }
}
```

**up-to-date：**

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [],
    "objects_pushed": 0,
    "bytes_pushed": 0,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": true,
    "upstream_set": null,
    "warnings": []
  }
}
```

**`--dry-run --json`：**

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": false
      }
    ],
    "objects_pushed": 256,
    "bytes_pushed": 0,
    "lfs_files_uploaded": 0,
    "dry_run": true,
    "up_to_date": false,
    "upstream_set": null,
    "warnings": []
  }
}
```

**force push + `--json`：**

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": true
      }
    ],
    "objects_pushed": 128,
    "bytes_pushed": 524288,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": false,
    "upstream_set": null,
    "warnings": ["force push overwrites remote history"]
  }
}
```

**错误 JSON：non-fast-forward**

```json
{
  "ok": false,
  "error_code": "LBR-CONFLICT-002",
  "category": "conflict",
  "exit_code": 128,
  "message": "cannot push to 'refs/heads/main': non-fast-forward update",
  "hints": [
    "pull and integrate remote changes first: 'libra pull'",
    "or use --force to overwrite (data loss risk)"
  ]
}
```

**错误 JSON：authentication failed**

```json
{
  "ok": false,
  "error_code": "LBR-AUTH-001",
  "category": "auth",
  "exit_code": 128,
  "message": "authentication failed for 'git@github.com:user/repo.git'",
  "hints": [
    "check SSH key or HTTP credentials",
    "use 'libra config --list' to verify auth settings"
  ]
}
```

**错误 JSON：no remote configured**

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "no configured push destination",
  "hints": [
    "use 'libra remote add <name> <url>' to configure a remote",
    "or specify the remote explicitly: 'libra push <remote> <branch>'"
  ]
}
```

### 特性 4：进度输出管控

**已落地方案：**

使用 `ProgressReporter` 替代裸进度输出；`--json`/`--machine` 模式通过 `progress_output_config()` 将 progress 设为 `None`；执行层 helper 中的 `emit_warning` 改为收集到 `warnings: Vec<String>`。

```rust
let progress_output = progress_output_config(output);
let progress = ProgressReporter::new("Compressing objects", Some(objs.len() as u64), &progress_output);
for (i, obj) in objs.iter().cloned().enumerate() {
    // ... pack encoding ...
    progress.tick((i + 1) as u64);
}
progress.finish();

let progress = ProgressReporter::new("Writing objects", None, &progress_output);
// ... upload with progress callback ...
progress.finish();
```

各模式行为：
- **human + TTY**：stderr 显示 indicatif 进度条
- **human + `--quiet`**：静默
- **`--json` / `--machine`**：本批默认静默，保证 success path stderr 干净

**Warning 收集机制：** `diff_tree_objs()` 接受 `&mut Vec<String>` 参数，将 submodule 等 warning 收集到 Vec 而非直接 `emit_warning()`。`incremental_objs()` 返回 `IncrementalObjsResult { objs, warnings }`。`run_push()` 将收集到的 warnings 合并到 `PushOutput.warnings`，由 `render_push_output()` 根据模式决定输出方式。

### 特性 5：Cross-Cutting Improvements 在 push 中的具体落地

| ID | 改进 | push 中的具体落地 | 状态 |
|----|------|-----------------|------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 remote 名、无效 refspec、source ref 不存在、local file remote、invalid URL）→ exit `129`；运行时错误（网络失败、认证失败、non-fast-forward、远端拒绝、timeout、hash mismatch）→ exit `128`；成功 / up-to-date → exit `0` | ✅ |
| **B** | `--help` EXAMPLES | 6 个示例覆盖基本推送、set-upstream、force、dry-run、json | ✅ |
| **F** | 拼写纠错 | remote 名不匹配时通过 `suggest_remote_name()` 基于 Levenshtein 距离 ≤ 2 提示 `did you mean '<closest>'?`（`priority_hint` 方式，排在常规 hint 前面） | ✅ |
| **G** | Issues URL | 仅在 `ObjectCollection` / `PackEncoding` 等内部不变式错误时附 `ISSUE_URL` hint。网络/认证/协议等用户可修复问题不输出 | ✅ |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra push                             Push current branch to tracking remote
    libra push origin main                 Push main branch to origin
    libra push -u origin feature-x         Push and set upstream tracking
    libra push --force origin main         Force push (overwrites remote history)
    libra push --dry-run                   Preview what would be pushed
    libra push --json                      Structured JSON output for agents
```

### 测试覆盖

#### `src/command/push.rs` 内 `mod test`（单元测试，23 个）

| 测试名称 | 覆盖内容 |
|---------|---------|
| `test_parse_args_success` | 参数解析：无参数、origin master、-u、--force、-f |
| `test_parse_dry_run_args` | --dry-run / -n 解析，与其他 flag 组合 |
| `test_parse_args_fail` | 非法参数组合拒绝 |
| `test_is_ancestor` | is_ancestor 相同 commit 返回 true |
| `test_parse_refspec_simple_name` | `main` → src=main, dst=main |
| `test_parse_refspec_src_dst` | `local:release` → src=local, dst=release |
| `test_parse_refspec_empty_rejected` | 空字符串拒绝 |
| `test_parse_refspec_empty_src_rejected` | `:dst` 拒绝 |
| `test_parse_refspec_empty_dst_rejected` | `src:` 拒绝 |
| `test_parse_refspec_multi_colon_rejected` | `a:b:c`、`a::b`、`:a:b` 拒绝 |
| `test_push_error_to_cli_error_detached_head` | DetachedHead → RepoStateInvalid / exit 128 |
| `test_push_error_to_cli_error_no_remote` | NoRemoteConfigured → RepoStateInvalid / exit 128 / hints 非空 |
| `test_push_error_to_cli_error_invalid_refspec` | InvalidRefspec → CliInvalidArguments / exit 129 |
| `test_push_error_to_cli_error_non_fast_forward` | NonFastForward → ConflictOperationBlocked / exit 128 |
| `test_push_error_to_cli_error_auth_failed` | AuthenticationFailed → AuthMissingCredentials |
| `test_push_error_to_cli_error_timeout` | Timeout → NetworkUnavailable |
| `test_push_error_to_cli_error_source_ref_not_found` | SourceRefNotFound → CliInvalidTarget / exit 129 |
| `test_push_error_to_cli_error_unsupported_local_remote` | UnsupportedLocalFileRemote → CliInvalidTarget |
| `test_push_error_to_cli_error_remote_not_found` | RemoteNotFound(无 suggestion) → CliInvalidTarget / exit 129 |
| `test_push_error_to_cli_error_remote_not_found_with_suggestion` | RemoteNotFound(有 suggestion) → hints 含 "did you mean" |
| `test_push_error_to_cli_error_object_collection_has_issue_url` | ObjectCollection → InternalInvariant / hints 含 Issues URL |
| `test_push_error_to_cli_error_pack_encoding_has_issue_url` | PackEncoding → InternalInvariant / hints 含 Issues URL |
| `test_levenshtein_basic` | Levenshtein 距离计算正确性 |

#### `tests/command/push_error_test.rs`（CLI 错误码验证，7 个）

| 测试名称 | 覆盖内容 |
|---------|---------|
| `test_push_detached_head_returns_repo_state_invalid` | DetachedHead → LBR-REPO-003 / exit 128 |
| `test_push_no_remote_returns_repo_state_invalid` | NoRemoteConfigured → LBR-REPO-003 / exit 128 / hint 含 "remote add" |
| `test_push_remote_not_found_returns_cli_invalid_target` | RemoteNotFound → LBR-CLI-003 / exit 129 |
| `test_push_remote_not_found_with_fuzzy_suggestion` | RemoteNotFound(typo) → hints 含 "did you mean" + "origin" |
| `test_push_invalid_refspec_returns_cli_invalid_arguments` | InvalidRefspec `:main` → LBR-CLI-002 / exit 129 |
| `test_push_source_ref_not_found_returns_cli_invalid_target` | SourceRefNotFound → LBR-CLI-003 / exit 129 |
| `test_push_local_file_remote_returns_cli_invalid_target` | UnsupportedLocalFileRemote → LBR-CLI-003 / exit 129 |

#### `tests/command/push_json_test.rs`（JSON schema 验证，5 个）

| 测试名称 | 覆盖内容 |
|---------|---------|
| `test_push_json_error_no_remote` | `--json push` → stderr JSON `ok: false`, error_code LBR-REPO-003 |
| `test_push_json_error_invalid_refspec` | `--json push origin src:` → stderr JSON error_code LBR-CLI-002 |
| `test_push_json_error_source_ref_not_found` | `--json push origin nonexistent` → stderr JSON error_code LBR-CLI-003 |
| `test_push_json_error_detached_head` | detached HEAD + `--json push` → stderr JSON LBR-REPO-003 |
| `test_push_machine_error_is_single_line_json` | `--machine push` → stderr 可解析为 JSON, ok=false |

#### `tests/command/push_test.rs`（集成测试，11 个）

| 测试名称 | 覆盖内容 |
|---------|---------|
| `test_push_cli_without_remote_returns_fatal_128` | 无 remote 推送 → exit 128 / LBR-REPO-003 / hint |
| `test_push_force_flag_parsing` | --force / -f flag 解析正确 |
| `test_push_file_remote_fails_without_reflog` | local file remote → 失败 + 无 reflog 写入 |
| `test_push_invalid_remote` | 无效远端 URL → 超时或失败（L2 网络测试） |
| `test_push_force_with_local_changes` | force push → 远端 HEAD 更新 |
| `test_push_ssh_remote_via_fake_ssh` | fake SSH → 推送成功 + 输出含 ref update 摘要 |
| `test_push_ssh_host_key_failure_is_reported` | SSH host key 验证失败 → 错误透传 |
| `test_push_explicit_refspec_uses_destination_branch_name` | `local:release` → 远端 refs/heads/release 更新 |
| `test_push_json_with_set_upstream_keeps_structured_output_clean` | `--json -u push` → JSON ok=true, upstream_set 非 null, stderr 干净 |
| `test_push_machine_success_is_single_json_line` | `--machine push` → stdout 恰好 1 行 JSON, stderr 干净 |
| `test_push_quiet_force_still_emits_warning_and_warning_exit_code` | `--quiet --force push` → stderr 含 warning, `--exit-code-on-warning` → exit 9 |

**测试总计：46 个**（23 单元 + 7 错误码 + 5 JSON + 11 集成）

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/push.rs` | **重构** | 新增 `PushError` typed enum（20 变体）；新增 `PushOutput` / `PushRefUpdate` / `IncrementalObjsResult` 结构体；新增 `run_push()` 纯执行入口 + `render_push_output()` 渲染层；`parse_refspec()` 含多冒号拒绝；`suggest_remote_name()` fuzzy match + `levenshtein()`；`classify_transport_error()` 超时分类；`diff_tree_objs()` warning 收集；移除生产路径 `panic!` 和裸 stdout/stderr；`PushError → CliError` 显式 `StableErrorCode` 映射；进度输出改用 `ProgressReporter`；补齐 `--help` EXAMPLES |
| `src/internal/protocol/https_client.rs` | **超时改造** | `reqwest::Client::builder()` 新增 `connect_timeout(10s)` + `read_timeout(10s)`（空闲超时：无数据到达即触发） |
| `src/internal/protocol/ssh_client.rs` | **超时改造** | 新增 `SSH_IDLE_TIMEOUT` 常量；`read_advertisement()` 每次 `read_exact` 包裹 `tokio::time::timeout`；`send_pack()` 的 `write_all` / `shutdown` / `wait_with_output` 分别包裹 `tokio::time::timeout` |
| `tests/command/push_test.rs` | **重大扩展** | 新增 explicit refspec、JSON+set-upstream、machine 输出、quiet+force warning 等 11 个集成测试 |
| `tests/command/push_json_test.rs` | **新增** | JSON/machine 模式错误输出 schema 验证，5 个测试 |
| `tests/command/push_error_test.rs` | **新增** | CLI 错误码验证（exit code、StableErrorCode、fuzzy match），7 个测试 |
| `tests/command/mod.rs` | **修改** | 注册 `push_error_test` 和 `push_json_test` 模块 |
| `docs/commands/push.md` | **新增** | Push 命令英文参考文档（Common Commands、Human Output、Structured Output、Error Handling、Feature Comparison） |
