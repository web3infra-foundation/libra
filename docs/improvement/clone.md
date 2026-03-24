## Clone 命令改进详细计划

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config` 的主改造已经在当前代码库落地。clone 改进建立在 config 现状与 init 已确定方案之上；其中 `run_init()` 等执行层拆分能力由 init 批次先行交付，clone 批次再切换调用。

**已确认落地的基线：**
- `init` 方案已明确拆分为纯执行入口 `run_init()` + 顶层渲染层；clone 以该接口作为前置依赖进行切换
- `init` 方案已明确移除 `separate_libra_dir` 参数；clone 构造 `InitArgs` 时不再传递该字段
- `config_kv` 后端、`ScopedConfig` cascade lookup、`resolve_env()` 均已落地
- clone 继承 config 的环境变量解析优先级约束：CLI 参数 > 系统环境变量 > 仓库 config > 全局 config
- `fetch_repository_safe()` 已存在并接受 `OutputConfig`
- `OutputConfig` 支持 `--json` / `--machine` / `--quiet` / `--progress`
- `emit_json_data()` 信封格式已标准化

**基于当前代码的 Review 结论：**
- clone 仍调用 `init::execute_safe()` 而非纯执行层 `run_init()`（init 改造后需同步切换）
- `CloneError → CliError` 映射几乎全部落入 `CliError::fatal()`，没有显式的 `StableErrorCode`；当前依赖 `infer_stable_error_code()` 的消息子串匹配，脆弱且不可控
- 成功路径无 JSON / machine 输出；`execute_safe()` 接受 `OutputConfig` 但仅用于 stderr 装饰消息的抑制
- `"Cloning into '{}' ..."` 和 `"done."` 直接写 stderr，不经过 `OutputConfig` 的 progress helper
- 网络错误（discovery 失败、fetch 失败）的错误消息直接透传底层 error string，缺少 actionable hint
- `cleanup_failed_clone()` 静默吞掉清理失败的 io::Error，用户无从知晓磁盘残留

### 目标与非目标

**本批目标：**
- 为 clone 补齐稳定的结构化成功输出（`--json` / `--machine`）
- 将 `CloneError → CliError` 映射改为显式 `StableErrorCode`，消除消息子串推断
- 对齐 init 改造后的纯执行层调用（`run_init()` 替代 `execute_safe()`）
- 为网络错误、认证失败等高频场景补齐 actionable hint
- 在 human 模式下提供阶段性进度（discovery → init → fetch → setup → checkout），与 init 的 stderr progress 风格一致

**本批非目标：**
- **不在本批做性能优化**。README.md 提到"目标 <1s"，但 clone 耗时的主要瓶颈是网络 I/O（discovery + pack download）和 vault keygen，这些不是输出层改造能解决的。性能优化留到后续独立批次
- **不在本批改变 fetch 内部的 progress 机制**。fetch 改进是独立的第五批工作（README.md #21）；clone 只负责在自己的渲染层控制 fetch progress 的可见性
- **不在 JSON 中暴露 pack 下载统计**。`fetch_repository_safe()` 当前不返回结构化统计（objects_fetched / bytes_received），在 fetch 改进前不把这类字段写进对外 schema
- **不改变 vault 策略**。clone 始终 `vault: true`，与 init 的默认行为一致

### 设计原则

1. **clone 的渲染层独立于 init / fetch**：clone 有自己的阶段性进度和最终输出；init 和 fetch 作为内部步骤只返回结果，不产生任何 stdout/stderr 输出
2. **结构化输出只在 `execute_safe()` 最终渲染**：`execute_clone()` 返回 `CloneResult`；`execute_safe()` 根据 `OutputConfig` 渲染 human / JSON / machine
3. **错误码显式映射，不依赖消息推断**：每个 `CloneError` 变体都有确定的 `StableErrorCode`，不再经过 `infer_stable_error_code()`
4. **网络错误必须有 hint**：discovery 失败、认证失败、fetch 超时等场景必须给出用户可行动的建议（检查 URL、检查 SSH key、检查网络）
5. **清理失败不静默**：`cleanup_failed_clone()` 的 io::Error 应作为 warning 输出到 stderr，而非仅 tracing::error

### 特性 1：执行层与渲染层拆分

**当前问题：** `execute_clone()` 既执行逻辑又写 stderr 装饰消息（`"Cloning into..."` / `"done."`）。成功时没有返回结构化结果，`execute_safe()` 无法渲染 JSON。

**修正后的方案：**

- `execute_clone()` 改为返回 `Result<CloneResult, CloneError>`，不做任何输出
- `execute_safe()` 调用 `execute_clone()` 后，根据 `OutputConfig` 渲染 human / JSON / machine
- human 模式下的阶段性进度由 `execute_safe()` 在调用 `execute_clone()` 的各阶段间插入 stderr 输出

**`CloneResult` 结构：**

```rust
struct CloneResult {
    path: String,                     // 仓库绝对路径（non-bare 时为工作树根目录，非 .libra）
    bare: bool,
    remote_url: String,               // 规范化后的 remote URL
    branch: String,                   // 实际 checkout 的分支名
    object_format: String,            // sha1 / sha256
    repo_id: String,                  // 从 init 结果透传
    vault_signing: bool,              // 从 init 结果透传
    ssh_key_detected: Option<String>, // 从 init 结果透传
    shallow: bool,                    // --depth 是否生效
    warnings: Vec<String>,            // 非致命警告（如 cleanup 失败）
}
```

**human 模式下的新流程：**

```text
步骤 1. 参数校验 + 目标路径推断
步骤 2. Remote discovery                -> stderr: "Connecting to <url> ..."
步骤 3. 目标路径预检查
步骤 4. 初始化仓库（调用 run_init）     -> stderr: "Initializing repository ..."
步骤 5. Fetch objects                    -> stderr: "Fetching objects ..." (fetch 自身的 progress bar 在 human 模式下可见)
步骤 6. 配置 remote + branch + checkout  -> stderr: "Setting up working copy ..."
步骤 7. stdout 输出最终确认消息
```

**输出规则：**

- 进度输出：
  - 仅 human 模式
  - 写入 stderr
  - 使用与 init 一致的 helper（`emit_legacy_stderr()` 或等价）
  - fetch 阶段：human 模式允许 fetch 的 `IndicatifProgressBar` 显示在 stderr；`--json` / `--machine` 下必须抑制
- 最终确认消息：
  - human 模式写 stdout
  - 格式见特性 5
- `--quiet`：抑制所有 progress 和最终成功消息；保留错误输出
- `--json`：不输出 progress；只输出最终 JSON envelope 到 stdout
- `--machine`：同 `--json`，但必须单行紧凑

**与 init 的交互：**

clone 调用 init 改造后的纯执行层（`run_init()` 或等价 API），传入必要参数，获取 `InitResult`。clone 不调用 `init::execute_safe()`——那是 init 自己的顶层渲染入口。

```rust
// clone 中的 init 调用
let init_result = command::init::run_init(InitArgs {
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
}).await.map_err(|e| CloneError::InitializeRepository { message: e.to_string() })?;
```

`InitResult` 中的 `repo_id`、`vault_signing`、`ssh_key_detected` 等字段直接透传到 `CloneResult`。

**与 fetch 的交互：**

clone 传递给 `fetch_repository_safe()` 的 `OutputConfig` 需要根据模式调整：
- human 模式：允许 fetch 显示 progress bar（原样传递 `output`）
- `--json` / `--machine`：传入"子级输出配置"，强制 `progress = ProgressMode::None`、`json_format = None`、`quiet = true`，确保 fetch 不产生任何 progress / JSON / human 装饰输出
- `--quiet`：传入 `quiet = true`

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

**`--depth` 浅克隆场景：**

`"shallow": true`，其余字段不变。

**与 init schema 的差异说明：**

- clone 成功 JSON 当前不暴露 `ref_format`。原因：clone 对外结果聚焦远端接入与工作副本落地状态，`ref_format` 在当前使用场景中不作为 clone 决策输入。
- 若后续 Agent/脚本出现明确需求，可按向后兼容方式增量添加该字段。

**明确不纳入本批 JSON 契约的字段：**

- `objects_fetched` / `bytes_received` / `pack_size`：fetch 改进前不承诺
- `checkout_files_count`：`restore::execute()` 当前不返回结构化结果

### 特性 3：错误处理、退出码与 Hint

**错误输出通道约束：**

- 成功结构化输出通过 `emit_json_data()` 输出到 stdout。
- 错误 JSON 统一通过 `CliError` 输出到 stderr，避免命令私有 envelope 分叉。

**`CloneError → CliError` 显式映射：**

| CloneError 变体 | StableErrorCode | exit | hint |
|-----------------|-----------------|------|------|
| `CannotInferDestination` | `CliInvalidArguments` | `129` | "please specify the destination path explicitly" |
| `DestinationExistsNonEmpty` | `CliInvalidTarget` | `129` | "choose a different path or empty the directory first" |
| `DestinationAlreadyRepo` | `RepoStateInvalid` | `128` | "the destination already contains a libra repository" |
| `CreateDestinationFailed` | `IoWriteFailed` | `128` | "check directory permissions and disk space" |
| `InvalidRemote` | `NetworkProtocol` | `128` | "check the URL and ensure the remote is reachable: `libra clone <url>`" |
| `ChangeDirectory` / `RestoreDirectory` | `InternalInvariant` | `128` | Issues URL |
| `InitializeRepository` | 透传 init 的错误码 | 透传 | 透传 |
| `RemoteBranchNotFound` | `RepoStateInvalid` | `128` | "use `-b <branch>` to specify an existing branch, or omit to use remote HEAD" |
| `FetchFailed` | 透传 fetch 的错误码 | 透传 | 按 fetch 错误子类型附加 hint |
| `SetupFailed` | `InternalInvariant` | `128` | Issues URL |

**网络错误 hint 细化：**

`FetchFailed` 包裹的 `FetchError` 有多个子变体，应按类型附加不同 hint：

| FetchError 子变体 | hint |
|-------------------|------|
| `Discovery` | "check the URL format and network connectivity" |
| `ObjectFormatMismatch` | "the remote uses a different hash algorithm; use `--object-format` if supported" |
| `RemoteBranchNotFound` | "the specified branch does not exist on the remote" |
| `FetchObjects` / `PacketRead` | "network error during transfer; check connectivity and retry" |
| `RemoteSideband` | "the remote server reported an error" |
| `ChecksumMismatch` | "downloaded data is corrupted; retry the clone" |
| `AuthMissingCredentials` 相关 | "check SSH key configuration: `libra config list --ssh-keys`" |

**Cross-Cutting Improvements 在 clone 中的具体落地：**

| ID | 改进 | clone 中的具体落地 |
|----|------|-------------------|
| **A** | 退出码 `0/128/129` | 参数错误（无法推断目标路径）→ exit `129`；运行时错误 → exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | 不适用——clone 没有 enum 类型的参数值需要 fuzzy match |
| **G** | Issues URL | `ChangeDirectory` / `RestoreDirectory` / `SetupFailed` → `LBR-INTERNAL-001` 时输出 Issues URL |

### 特性 4：清理失败可见性

**当前问题：** `cleanup_failed_clone()` 用 `tracing::error!()` 记录清理失败，但 tracing 在默认配置下不输出到用户 stderr，导致磁盘残留无声无息。

**修正后的方案：**

- 清理失败时，将失败消息收集到 `CloneResult.warnings`（如果 clone 整体失败，warnings 附加到 `CliError` 的 hint 中）
- human 模式：warning 输出到 stderr，格式 `warning: failed to clean up '<path>': <io_error>`
- JSON / machine 模式：warning 出现在错误 JSON 的 `hints` 数组中

### 特性 5：成功消息与 Human Output

#### `libra clone git@github.com:user/repo.git`

```text
Connecting to git@github.com:user/repo.git ...
Initializing repository ...
Fetching objects ...
████████████████████████████████████████ 100% (256 objects)
Setting up working copy ...
Cloned into 'repo'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
```

#### `libra clone --bare git@github.com:user/repo.git repo.git`

```text
Connecting to git@github.com:user/repo.git ...
Initializing repository ...
Fetching objects ...
████████████████████████████████████████ 100% (256 objects)
Cloned into bare repository 'repo.git'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled
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

#### 错误：远程仓库不可达

```text
fatal: failed to connect to 'git@github.com:user/nonexistent.git'
Error-Code: LBR-REPO-001

hint: check the URL format and network connectivity
```

#### 错误：指定分支不存在

```text
fatal: remote branch 'nonexistent' not found in upstream origin
Error-Code: LBR-REPO-003

hint: use -b <branch> to specify an existing branch, or omit to use remote HEAD
```

#### 错误 JSON：远程仓库不可达

```json
{
  "ok": false,
  "error_code": "LBR-REPO-001",
  "category": "repo",
  "exit_code": 128,
  "message": "failed to connect to 'git@github.com:user/nonexistent.git'",
  "hints": [
    "check the URL format and network connectivity"
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

- **（现有）invalid source 不 panic**：exit 128，error code `LBR-REPO-001`
- **（现有）missing branch 清理**：exit 128，error code `LBR-REPO-003`，**不仅预存在的空目录被还原，因 clone 创建的本地 `.libra` 数据库及目录也必须被彻底删除**。
- **（现有）successful clone 无 debug noise**：stderr 有阶段性进度，无 DEBUG/WARN/INFO
- **（现有）vault 初始化**：`.libra/vault.db` 存在，`vault.signing=true`，`vault.gpg.pubkey` 非空
- **（现有）`--machine` 抑制装饰 stderr**：stderr 无 `"Cloning into"`
- **（新增）`--json` 成功输出 schema**：验证 JSON envelope 包含所有 `CloneResult` 字段，类型正确
- **（新增）`--machine` 成功输出**：stdout 按 `\n` 分割恰好 1 行非空行，可被 `serde_json::from_str()` 解析
- **（新增）`--quiet` 成功时 stdout 和 stderr 均无输出**
- **（新增）错误码显式验证**：
  - `CannotInferDestination` → `LBR-CLI-002`，exit `129`
  - `DestinationExistsNonEmpty` → `LBR-CLI-003`，exit `129`
  - `RemoteBranchNotFound` → `LBR-REPO-003`，exit `128`
- **（新增）hint 存在性验证**：网络错误的 stderr 包含 "check" 等 actionable 关键词
- **（新增）cleanup warning 可见性**：模拟清理失败（只读目录），验证 stderr 包含 `warning:`
- **（新增）init 输出隔离**：`--json clone` 的 stdout 只有一个 JSON envelope，不混入 init 的 JSON 或 progress
- **（新增）fetch progress 隔离**：`--json clone` 的 stderr 不包含 fetch 的 NDJSON progress 事件
- **测试隔离要求**：所有涉及 `ssh_key_detected` 的断言必须使用隔离的 `HOME` / `USERPROFILE` / `XDG_CONFIG_HOME`，避免宿主机真实 `~/.ssh` 污染

#### `tests/command/clone_test.rs`（L2 网络测试，扩展）

- 现有 L2 测试保持不变（依赖 GitHub API）
- **（新增）`--json` L2 测试**：对真实 GitHub 仓库执行 `--json clone`，验证 `remote_url` / `branch` / `vault_signing` 字段与实际一致
- **（新增）`--depth` JSON**：验证 `shallow: true`

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
| `src/command/clone.rs` | **重构** | 拆分执行层与渲染层；`execute_clone()` 返回 `CloneResult`；`CloneError → CliError` 显式映射 `StableErrorCode`；切换到 `run_init()`；fetch OutputConfig 子级隔离；cleanup warning 可见化 |
| `src/command/init.rs` | **前置依赖（由 init 批次交付）** | clone 依赖 init 批次先交付 `run_init()` 纯执行入口与参数收口；clone 批次不在该文件新增需求 |
| `src/command/fetch.rs` | **小改** | 视需要暴露 `FetchError` 的子变体以便 clone 做 hint 细化；不改变 fetch 自身的输出逻辑 |
| `tests/command/clone_cli_test.rs` | **扩展** | 新增 JSON schema / machine 格式 / quiet / 错误码 / hint / cleanup warning / 隔离验证 |
| `tests/command/clone_test.rs` | **扩展** | 新增 L2 JSON 验证 |
