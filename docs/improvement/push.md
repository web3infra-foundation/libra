## Push 命令改进详细计划

> 最后编写时间：2026-03-27

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

**基于当前代码的 Review 结论（push 仍需改进的部分）：**

- **零 JSON / machine 输出**：`execute_safe()` 接受 `_output` 参数但**完全未使用**（变量名带下划线）；所有输出为裸 `println!()` / `eprintln!()`，不经过 `OutputConfig`
- **零显式 `StableErrorCode`**：18+ 个 `CliError::fatal()` 调用无一附带 `.with_stable_code()`，全部依赖 `infer_stable_error_code()` 字符串推断
- **无进度可见性控制**：pack encoding、对象上传等耗时操作的进度直接写 stdout，不尊重 `--quiet`/`--json`/`--progress` 标志
- **错误消息可操作性差**：网络超时、认证失败、协议错误等场景缺少 hint。仅 `"no configured push destination"` 和 `"non-fast-forward"` 提供了指导性 hint
- **`--dry-run` 输出不可机读**：`--dry-run` 仅跳过实际推送，无结构化输出告知"将会推送什么"
- **缺少推送摘要**：成功后仅输出 `"Push success"` 一行，不显示推送了哪些 ref、多少对象、数据量等
- **README 中已列为 P0 的两项需求尚未写进子计划**：`10s` 超时和 refspec 语法修复在 README 已明确列出，但本文件的目标/设计没有覆盖
- **关键错误路径尚未纳入 typed error 设计**：显式指定的本地分支不存在、local file remote 不支持、无效 refspec 等高频失败场景尚未在 `PushError` 中建模
- **生产路径仍有 panic 和散落的裸 stdout/stderr**：例如 remote URL 读取失败仍会 `panic!`，执行层和 helper 中也分散存在 `println!()` / `eprintln!()`；若不把这类路径纳入本批目标，结构化输出契约仍会被少数分支破坏
- **结构化输出的 stderr 契约尚未定义**：当前文档一边要求 JSON envelope，一边又默认在结构化模式输出 progress 事件，这与 `init` / `clone` 已落地的“成功路径 stderr 保持干净”模式冲突
- **`10s` 超时语义需要重新收敛**：对 discovery / upload / receive-pack 整个阶段统一施加 10 秒硬截止，不适合大仓库、LFS 和慢链路场景；更合理的是连接超时 + 空闲超时，而不是总时长硬上限
- **测试设计缺少 transport seam**：仅靠真实网络超时或外部 GitHub token 难以稳定验证 timeout / protocol / auth 错误；需要把 fake transport / mock helper 作为本批前置测试基础设施
- **测试覆盖极度不足**：仅有参数解析测试和一个依赖外部 GitHub token 的集成测试桩

### 目标与非目标

**本批目标：**
- 引入 `PushError` typed error enum，替代内部 `CliError::fatal()` 散射
- 所有 `PushError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_push(args) -> Result<PushOutput, PushError>` 纯执行入口
- 明确定义并实现本批支持的 refspec 语义（默认同名分支、`<name>`、`<src>:<dst>`）
- 清理生产路径中的 `panic!`、裸 `println!()` / `eprintln!()`，统一回收到 `OutputConfig` 和 `PushError`
- 为 discovery 建立 `10s` 连接超时，并为 upload / receive-pack 建立 `10s` 空闲超时；timeout 作为稳定的网络错误处理，而不是整个大 push 的总时长硬截止
- 完善 JSON 输出 schema（`PushOutput`），包含推送详情
- 进度输出经过 `OutputConfig` 管控，但 `--json` / `--machine` 成功路径默认保持 stderr 干净
- `--dry-run` 输出结构化预览
- 为 push transport 建立可替换的测试 seam（fake transport / mock helper），使 timeout、auth、protocol 分支可以稳定测试
- 补齐 `--help` EXAMPLES 段
- 完善 hint 体系，覆盖常见失败场景

**本批非目标：**
- **不改变 SSH/HTTP 传输核心逻辑**。协议层行为不变
- **不改变 LFS 上传逻辑**。LFS 文件检测和上传流程不变
- **不改变 pack 增量/delta 压缩算法**。性能优化留后续批次
- **不引入 In-process SSH Client**。这是全局改进项 H，留后续批次
- **不改变 reflog 记录格式**。`ReflogAction::Push` 保持不变
- **不引入 push mirror/tags/delete 语义**。这些是新特性，不在本批范围
- **不在本批承诺 push 的 NDJSON progress 契约**。本批先保证 human 进度和结构化 success envelope 不互相污染；若后续需要结构化进度事件，再与 transport/fetch 批次统一设计

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_push()` 收集结构化结果，再根据 `OutputConfig` 渲染 human/JSON/machine
2. **typed error enum 取代散射的 `CliError::fatal()`**：每个失败场景有确定的 `PushError` 变体
3. **StableErrorCode 显式映射**：消除对 `infer_stable_error_code()` 的依赖
4. **refspec 语义必须先收敛再实现**：本批只支持三种输入形态：省略（推当前分支）、`<name>`（同名分支）、`<src>:<dst>`（显式映射）；其余语法显式报 `InvalidRefspec`
5. **超时必须是显式契约，但不能把大 push 当作 10 秒总时长任务**：discovery 使用 `10s` 连接超时；upload / receive-pack 使用 `10s` 空闲超时（无数据进展才触发）；超时视为 `NetworkUnavailable`，并在错误 details 中标注 phase
6. **结构化模式默认保持 stderr 干净**：`--json` / `--machine` 成功路径只输出一个 envelope；human 进度和 warning 不得污染结构化输出
7. **`--dry-run` 可被 Agent 消费**：JSON 模式下返回结构化预览（将推送的 ref 和对象数）
8. **hint 覆盖常见失败**：网络超时、认证失败、non-fast-forward、missing remote、invalid refspec 等每种场景提供可操作的 hint
9. **测试先于超时落地**：在没有 fake transport / mock helper 之前，不把 timeout 映射和协议分支完全绑定到真实网络集成测试

### 特性 1：PushError typed error enum

**当前问题：** `execute_safe()` 内部 18+ 处直接调用 `CliError::fatal(msg)` 构造错误，无结构化分类，无显式错误码。

**修正后的方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("HEAD is detached; cannot determine what to push")]
    DetachedHead,

    #[error("no configured push destination")]
    NoRemoteConfigured,

    #[error("remote '{0}' not found")]
    RemoteNotFound(String),

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

**`PushError → CliError` 显式映射：**

| PushError 变体 | StableErrorCode | 退出码 | hint |
|---------------|-----------------|--------|------|
| `DetachedHead` | `RepoStateInvalid` | 128 | `checkout a branch before pushing` + `use 'libra switch <branch>' to switch` |
| `NoRemoteConfigured` | `RepoStateInvalid` | 128 | `use 'libra remote add <name> <url>' to configure a remote` + `or specify the remote explicitly: 'libra push <remote> <branch>'` |
| `RemoteNotFound` | `CliInvalidTarget` | 129 | `use 'libra remote -v' to see configured remotes` |
| `InvalidRefspec` | `CliInvalidArguments` | 129 | `use '<name>' or '<src>:<dst>'` |
| `SourceRefNotFound` | `CliInvalidTarget` | 129 | `verify the local branch/ref exists before pushing` |
| `UnsupportedLocalFileRemote` | `CliInvalidTarget` | 129 | `use fetch/clone for local-path repositories; push currently supports network remotes only` |
| `InvalidRemoteUrl` | `CliInvalidArguments` | 129 | `check the remote URL with 'libra remote get-url <name>'` |
| `AuthenticationFailed` | `AuthMissingCredentials` | 128 | `check SSH key or HTTP credentials` + `use 'libra config --list' to verify auth settings` |
| `DiscoveryFailed` | `NetworkUnavailable` | 128 | `check the remote URL and network connectivity` |
| `Timeout` | `NetworkUnavailable` | 128 | `check network connectivity and retry` |
| `NonFastForward` | `ConflictOperationBlocked` | 128 | `pull and integrate remote changes first: 'libra pull'` + `or use --force to overwrite (data loss risk)` |
| `ObjectCollection` | `InternalInvariant` | 128 | 无（附 Issues URL） |
| `PackEncoding` | `InternalInvariant` | 128 | 无（附 Issues URL） |
| `RemoteUnpackFailed` | `NetworkProtocol` | 128 | `the remote server failed to process the pack; retry or check server logs` |
| `RemoteRefUpdateFailed` | `NetworkProtocol` | 128 | `the remote rejected the update; check branch protection rules` |
| `Network` | `NetworkUnavailable` | 128 | `check network connectivity and retry` |
| `LfsUploadFailed` | `NetworkUnavailable` | 128 | `check LFS endpoint configuration`；`.with_detail("oid", oid)` 暴露对象标识供 Agent 使用 |
| `TrackingRefUpdate` | `IoWriteFailed` | 128 | 无 |
| `RepoState` | `RepoCorrupt` | 128 | `try 'libra status' to verify repository state` |

### 特性 2：执行层与渲染层拆分

**当前问题：** `execute_safe()` 是一个 400+ 行的单体函数，混合远程发现、对象收集、pack 编码、传输、reflog 更新和输出渲染。进度输出直接写 stdout。

**修正后的方案：**

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
    /// 推送目标 remote name
    pub remote: String,
    /// 推送目标 URL
    pub url: String,
    /// 推送的 ref 更新列表
    pub updates: Vec<PushRefUpdate>,
    /// 推送的对象数量
    pub objects_pushed: usize,
    /// 推送的数据量（字节）
    pub bytes_pushed: u64,
    /// 是否有 LFS 文件上传
    pub lfs_files_uploaded: usize,
    /// 是否为 dry-run
    pub dry_run: bool,
    /// 是否 everything up-to-date（无需推送）
    pub up_to_date: bool,
    /// 是否设置了 upstream tracking
    pub upstream_set: Option<String>,
    /// warning 列表（如 force push 警告）
    pub warnings: Vec<String>,
}
```

改造后的调用链：
- `execute_safe(args, output)` → `run_push(args)` → 返回 `PushOutput`
- 进度回调通过 `ProgressReporter` 发送，尊重 `OutputConfig`
- `execute_safe()` 根据 `OutputConfig` 选择渲染：human / JSON / machine

**refspec 语义（本批收敛版）：**

```text
libra push
  -> 当前分支推送到 tracking remote；若未配置 upstream，则推送到默认 remote 的同名分支

libra push origin main
  -> 本地 refs/heads/main 推送到远端 refs/heads/main

libra push origin local_branch:release
  -> 本地 refs/heads/local_branch 推送到远端 refs/heads/release
```

约束：
- 不支持空 src / 空 dst / 删除语法（如 `:dst`、`src:`、`:dst`）
- 不支持一次推送多个 refspec
- 非法形态统一返回 `PushError::InvalidRefspec`

**超时策略：**
- discovery / 建连：`10s` 连接超时
- send-pack / upload：`10s` 空闲超时（持续有数据上传时不触发）
- receive-pack 响应读取：`10s` 空闲超时

超时错误统一映射为 `PushError::Timeout { phase, seconds: 10 }`，并在错误 JSON 的 `details.phase` 中保留阶段名。

> **约束说明：** README 中的 `10s` 目标在 push 上应解释为“连接/空闲超时”，而不是对整个 push 生命周期设置 10 秒硬截止；否则会把合法的大仓库 push 误判为网络失败，与仓库的大规模场景定位冲突。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 推送摘要（见下方） | 进度条（pack encoding、uploading） |
| human + `--quiet` | 无 | 仅 warning（如 force push 警告） |
| `--json` / `--machine` | JSON envelope | 默认保持干净，不输出 progress / human 文本 |
| `--dry-run` | 预览摘要（不执行推送） | 无进度 |

**human 模式推送摘要（改进后）：**

```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main
 1 file changed via LFS
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

**当前问题：** pack encoding、对象上传等耗时操作的进度直接通过裸 `println!()` 写 stdout，`--json`/`--quiet` 模式下会泄漏到 stdout 污染输出。

**修正后的方案：**

使用 `ProgressReporter` 替代裸进度输出；同时把现有执行层和 helper 中残留的 `println!()` / `eprintln!()` 一并收口到统一渲染边界：

```rust
// 在 run_push() 中
let progress = ProgressReporter::new("Compressing objects", Some(total_objects), output);
for (i, obj) in objects.iter().enumerate() {
    // ... pack encoding ...
    progress.tick(i as u64 + 1);
}
progress.finish();

let progress = ProgressReporter::new("Writing objects", Some(pack_size), output);
// ... upload with progress callback ...
progress.finish();
```

各模式行为：
- **human + TTY**：stderr 显示 indicatif 进度条（`Compressing objects: 100% (256/256)`）
- **human + `--quiet`**：静默
- **`--json` / `--machine`**：本批默认静默，保证 success path stderr 干净

### 特性 5：Cross-Cutting Improvements 在 push 中的具体落地

| ID | 改进 | push 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 remote 名、无效 refspec、source ref 不存在）→ exit `129`；运行时错误（网络失败、认证失败、non-fast-forward、远端拒绝、timeout）→ exit `128`；成功 / up-to-date → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | remote 名不匹配时提示 `did you mean '<closest>'?`（基于 `libra remote -v` 的已配置 remote 列表做 fuzzy match） |
| **G** | Issues URL | 仅在 `ObjectCollection` / `PackEncoding` 等内部不变式错误时输出 Issues URL。网络/认证/协议等用户可修复问题不输出 |

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

### 测试要求

#### `tests/command/push_test.rs`（核心执行路径，重大扩展）

- **（已有）** 参数解析测试（`test_parse_args_success`、`test_parse_dry_run_args`、`test_parse_args_fail`）、`is_ancestor` 工具函数
- **（前置）transport seam**：为 discovery / send-pack / receive-pack 建立 fake transport 或 mock helper，避免把 timeout / auth / protocol 测试绑定到真实网络或外部 token
- **（新增）`PushError` 变体覆盖**：
  - `DetachedHead`：detached HEAD 状态下推送返回对应错误
  - `NoRemoteConfigured`：无 remote 配置时返回对应错误 + hint
  - `RemoteNotFound`：指定不存在的 remote 名时返回对应错误
  - `InvalidRefspec`：非法 refspec 返回对应错误
  - `SourceRefNotFound`：显式指定不存在的本地分支时返回对应错误
  - `UnsupportedLocalFileRemote`：local file remote 返回对应错误，且不写 reflog
  - `Timeout`：通过可控 fake transport / mock helper 验证连接超时 / 空闲超时被正确映射
- **（新增）`--dry-run` 结构化输出**：验证 `PushOutput.dry_run == true`，实际远端 ref 未被更新
- **（新增）up-to-date 场景**：无新提交时返回 `up_to_date == true`
- **（新增）`--set-upstream` 场景**：推送后 `upstream_set` 字段非 null，config 中写入 tracking 配置
- **（新增）force push warning**：`--force` 推送时 `warnings` 列表非空

#### `tests/command/push_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性：
  - `remote` 是 string
  - `url` 是 string
  - `updates` 是 array，元素包含 `local_ref`/`remote_ref`/`new_oid`（string）、`old_oid`（string 或 null）和 `forced`（bool）
  - `objects_pushed` 是 number
  - `bytes_pushed` 是 number
  - `lfs_files_uploaded` 是 number
  - `dry_run` 是 bool
  - `up_to_date` 是 bool
  - `upstream_set` 是 string 或 null
  - `warnings` 是 string 数组
- **`--dry-run --json`**：`dry_run == true`
- **up-to-date `--json`**：`up_to_date == true`，`updates` 为空数组
- **`--machine push`**：stdout 按 `\n` 分割后恰好 1 行非空行，可被 `serde_json::from_str()` 解析
- **错误 JSON 格式**：non-fast-forward、authentication failed、invalid refspec、timeout 等场景返回结构化错误 JSON 到 stderr
- **结构化输出隔离**：`--json` / `--machine` 成功路径下 stderr 不出现 progress / human 文本

> **测试边界要求：** 真实网络集成测试可以保留为补充验证，但不再作为 timeout / auth / protocol 分支的主覆盖手段；这些场景必须有本地可重复的 deterministic 测试。

#### CLI 错误码验证（放入 `tests/command/push_test.rs`）

- `DetachedHead` 返回 `LBR-REPO-003`
- `NoRemoteConfigured` 返回 `LBR-REPO-003`
- `RemoteNotFound` 返回 `LBR-CLI-003`
- `InvalidRefspec` 返回 `LBR-CLI-002`
- `SourceRefNotFound` 返回 `LBR-CLI-003`
- `UnsupportedLocalFileRemote` 返回 `LBR-CLI-003`
- `NonFastForward` 返回 `LBR-CONFLICT-002`
- `AuthenticationFailed` 返回 `LBR-AUTH-001`
- `Timeout` 返回 `LBR-NET-001`

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/push.rs` | **重构** | 新增 `PushError` typed enum；新增 `PushOutput` / `PushRefUpdate` 结构体；新增 `run_push()` 纯执行入口；补齐 refspec 解析；移除生产路径 `panic!` 和裸 stdout/stderr；`PushError → CliError` 显式 `StableErrorCode` 映射；进度输出改用 `ProgressReporter`；补齐 `--help` EXAMPLES |
| `src/internal/protocol/https_client.rs` | **前置依赖改造** | 为 push 的 discovery / send-pack 提供连接超时 / 空闲超时包装，并暴露可测试的 transport seam，避免 HTTP 路径无限等待 |
| `src/internal/protocol/ssh_client.rs` | **前置依赖改造** | 为 receive-pack / send-pack 提供连接超时 / 空闲超时包装，并暴露可测试的 transport seam，避免 SSH 子进程无限等待 |
| `tests/command/push_test.rs` | **重大扩展** | 新增 `PushError` 变体覆盖、dry-run、up-to-date、force push 场景 |
| `tests/command/push_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/push_error_test.rs` | **新增** | CLI 错误码验证（exit code、StableErrorCode） |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
