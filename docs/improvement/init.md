## Init 命令改进详细计划

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 目标与非目标

**本批目标：**
- 消除 `libra init` 成功路径上的长时间沉默，在默认 human 模式下提供实时进度
- 为顶层 `libra init` 提供稳定的 JSON 输出
- 统一成功消息、错误码、hint 和 reinit 语义
- 与 `docs/improvement/config.md` 对齐，明确 `config_kv` 迁移的前置条件
- 停止在 `init` 中生成仓库专属 SSH key，改为检测系统 SSH key 并给出后续提示
- 彻底移除 `--separate-libra-dir`（及其别名 `--separate-git-dir`）功能，不再支持将存储目录与工作树分离

**本批非目标：**
- **不把默认 `libra init` 总耗时优化到 `<500ms`**。只要 `--vault true` 仍然在 init 阶段生成 PGP key，总耗时仍会显著高于 500ms；本批解决的是“等待时无反馈”，不是“立即完成”
- **不在本批承诺 objects/branches/tags 转换统计**。在 `fetch`/`convert` 返回结构化统计前，不把这类字段写进对外 JSON 契约
- **不让 `init` 的内部复用路径直接打印 human/JSON 输出**。`clone` 等命令复用初始化逻辑时必须保持静默

### 设计原则

1. **执行层与渲染层拆分**：把“执行初始化逻辑”和“输出 human/JSON 结果”分开。顶层 `libra init` 负责渲染；`clone` 等内部调用只拿执行结果，不直接打印
2. **实时进度只属于顶层 human 模式**：进度输出仅在“非 `--json`、非 `--quiet`、顶层 `init` 调用”时显示，写入 stderr；不复用当前写 stdout 的 `info_println!()`
3. **`--quiet` 成功时完全静默**：与现有全局输出语义和测试保持一致。`--quiet init` 不输出进度，也不输出最终确认行
4. **`--json` 只输出最终结果**：顶层 `init` 在 JSON 模式下只输出一个 success envelope 到 stdout；中间进度一律抑制
5. **GPG key 策略保持不变**：`--vault true`（默认）仍在 init 阶段完成 vault 初始化和 PGP key 生成；本批只改善反馈，不改变默认安全行为
6. **SSH key 不在 init 中生成**：依赖 transport 的 fallback 链使用系统 key 或后续 `config generate-ssh-key`
7. **`config_kv` 直接迁移**：遵循 `config.md` 规范，本批次中 `init` 彻底切换到纯 `config_kv` 表，绝对禁止对旧 `config` 表进行双写
8. **错误分类不靠 `io::ErrorKind::InvalidInput` 猜测**：参数错误、无效 Git 源、已初始化等场景需要显式的 `InitError` 变体，避免错误码漂移
9. **reinit 语义在 worktree 和 bare 模式下一致**：不能只检查工作树下的 `.libra/`
10. **成功消息使用过去时**：统一为 `Initialized empty ...`，并补充 branch/signing 等关键结果

### 特性 1：实时进度输出与渲染边界

**背景：** 当前 `libra init` 在成功路径上会等待数秒后才输出一行确认信息。真正耗时主要来自 vault 初始化和 PGP key 生成，而不是前面的目录/数据库创建。

**修正后的方案：**

- 新增一个“纯执行”入口，例如 `run_init(args) -> Result<InitResult, InitError>`
- 顶层 `execute_safe(args, output)` 调用 `run_init()` 后，根据 `OutputConfig` 渲染 human 或 JSON
- `clone` 等内部复用路径调用 `run_init()`，不触发 init 自己的进度和 JSON 渲染

**human 模式下的新流程：**

```text
步骤 1. 参数校验（branch、object-format、shared mode、路径模式）
步骤 2. 预检查是否已初始化（worktree / bare）
步骤 3. 创建目录结构                      -> stderr: "Creating repository layout ..."
步骤 4. 创建 SQLite 主库并写入初始配置     -> stderr: "Initializing database ..."
步骤 5. 创建 HEAD 和 intent ref           -> stderr: "Setting up refs ..."
步骤 6. 可选：转换 Git 仓库                -> stderr: "Converting from Git repository ..."
步骤 7. 可选：初始化 vault + 生成 PGP key  -> stderr: "Generating PGP signing key ..."
步骤 8. 检测系统 SSH key
步骤 9. stdout 输出最终确认消息与 tip
```

**输出规则：**

- 进度输出：
  - 仅顶层 `libra init`
  - 仅 human 模式
  - 写入 stderr
  - 使用专门 helper，例如 `init_progressln!(output, ...)` 或 `emit_legacy_stderr()`，**不要**复用 `info_println!()`
- 最终确认消息：
  - 仅顶层 `libra init`
  - human 模式下写 stdout
  - 使用过去时 `Initialized empty ...`
- `--quiet`：
  - 抑制所有 progress
  - 抑制最终成功消息
  - 保留错误输出
- `--json`：
  - 不输出 progress
  - 只输出最终 JSON envelope

**为何不能直接在当前 `execute_safe()` 里边执行边输出：**

- `src/utils/output.rs` 中的 `info_println!()` 当前写的是 stdout，不是 stderr
- `clone` 目前直接复用 `init::execute_safe(...)`；如果 `init` 自己输出 JSON，会污染 `clone` 的 JSON 流
- `--quiet init` 现有测试要求成功时静默，不能改成“仍输出最终确认行”

### 特性 2：JSON 输出设计

**目标：** 给顶层 `libra init --json` 提供稳定结果，但不把当前底层拿不到的数据提前承诺出去。

**成功输出结构：**

```rust
struct InitOutput {
    path: String,                    // 存储目录绝对路径；bare 时为 repo 根目录
    bare: bool,
    initial_branch: String,
    object_format: String,           // sha1 / sha256
    ref_format: String,              // strict / filesystem
    repo_id: String,
    vault_signing: bool,
    converted_from: Option<String>,  // 规范化后的源 Git 目录
    ssh_key_detected: Option<String>,
}
```

**明确不纳入本批 JSON 契约的字段：**

- `gpg_fingerprint`
  - 当前 `vault::generate_pgp_key()` 返回的是 public key，不是 fingerprint
  - 除非 vault 层补充结构化 fingerprint，否则文档不承诺该字段
- `conversion.objects_fetched / branches / tags`
  - 当前 `convert_from_git_repository()` 不返回结构化统计
  - 在 `fetch`/`convert` 改成结构化返回前，不把统计字段写进对外 schema

**实现边界：**

- `emit_json_data("init", &output, output_config)` 只在顶层 `init` 分发路径调用
- 内部初始化函数返回 `InitOutput` 或等价结构；不在内部直接 `emit_json_data`
- 未来如果 `convert` 能返回统计，新增 `conversion` 字段应当是**向后兼容的增量扩展**

### 特性 3：Config 存储迁移策略

**现状：** 当前初始化逻辑仍残留 legacy `config` 表写入和相应测试假设。按照 `config.md` 第 6 条“依赖命令同步迁移”，`init` 本批次不保留 fallback，不做 dual-write，直接切到纯 `config_kv`。

**修正后的绝对策略（纯 `config_kv` 写入 + 去掉 legacy）：**

`init` 成功创建 `libra.db` 后，写入 `config_kv` 的 canonical seed keys 必须完整且唯一，至少包括：

```text
core.repositoryformatversion = 0
core.filemode                = true / false
core.bare                    = true / false
core.logallrefupdates        = true
core.objectformat            = sha1 / sha256
core.initrefformat           = strict / filesystem
libra.repoid                 = <uuid>
```

**平台特定 seed keys：**

```text
# Windows only
core.symlinks                = false
core.ignorecase              = true
```

**明确不属于基础 seed 批次的键：**

```text
vault.signing
vault.gpg.pubkey
```

- `vault.signing=false`：仅在 `--vault false` 时显式写入
- `vault.signing=true`：只能在 vault 初始化和 PGP key 生成成功后单独写入
- `vault.gpg.pubkey`：只能在 PGP key 生成成功后写入

**执行路径要求：**
1. 删除 `init_config()` 中关于 `config::ActiveModel` 的所有数据库插入操作。
2. 用 `ConfigKv::set_with_conn()` 或等价的批量写入 helper 完成上述 canonical seed keys 的写入。
3. 在 `init` 改进中**完全去掉 legacy 部分**：
   - 删除 `init` 路径上对旧 `config` 表的写入
   - 删除与 `init` 初始化相关的 legacy 测试断言
   - 删除或改写 `init` 迁移所需的 legacy helper，使 `init` 路径不再依赖旧表
4. 更新测试为只断言 `config_kv` 的 canonical keys，不再把旧 `config` 表作为兼容读取来源。

### 特性 4：Vault/GPG 初始化与 SSH key 检测

**GPG 策略：**

- `--vault true`（默认）：
  - 初始化 vault
  - 生成 PGP signing key
  - **仅在上述两步都成功后**写入 `vault.signing=true`
  - 成功后将生成的公钥存入 `vault.gpg.pubkey`（明文）
- `--vault false`：
  - 跳过 vault 初始化
  - 显式写入 `vault.signing=false` 到 `config_kv`

**SSH 策略：**

- init 不生成仓库专属 SSH key
- init 结束后检测系统 SSH key
- human 模式下仅输出 tip；JSON 模式下通过 `ssh_key_detected` 返回路径

**SSH 检测建议实现：**

```rust
fn detect_system_ssh_keys() -> Option<String> {
    let home = dirs::home_dir()?;
    let ssh_dir = home.join(".ssh");
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = ssh_dir.join(name);
        if path.exists() {
            return Some(path.display().to_string());
        }
    }
    None
}
```

**Human tip：**

检测到系统 SSH key：

```text
Tip: using existing SSH key at ~/.ssh/id_ed25519
     to generate a repo-specific key later, run: libra config generate-ssh-key --remote origin
```

未检测到系统 SSH key：

```text
Tip: no SSH key found at ~/.ssh/
     push/pull via SSH will require a key
     generate one with: libra config generate-ssh-key --remote origin
     or create a system key: ssh-keygen -t ed25519
```

### 特性 5：`--from-git-repository` 改进

**当前问题：**

- `execute_safe()` 在调用 `init()` 前就先对源路径做 `canonicalize()`，缺失路径会直接变成通用 fatal 错误，绕开 init 自己的错误映射
- `convert_from_git_repository()` 目前调用的是 `fetch::fetch_repository()`，它内部自己打印错误，不返回结构化失败
- helper 只返回 `Result<(), InitError>`，没有结构化元数据，无法稳定支持 human/JSON 输出

**修正后的方案：**

1. 源路径校验改成 `InitError` 的一部分，而不是在 `execute_safe()` 里直接构造 `CliError`
2. `convert_from_git_repository()` 改为调用 `fetch_repository_safe(...)`
3. `convert_from_git_repository()` 返回一个最小结构化结果，例如：

```rust
struct ConversionReport {
    source_git_dir: String,
    remote_url: String,
}
```

4. 本批 human 输出只展示来源路径，不展示统计数字
5. JSON 输出只包含 `converted_from`

**本批 human 成功输出：**

```text
Creating repository layout ...
Initializing database ...
Setting up refs ...
Converting from Git repository at /Users/eli/projects/old-project/.git ...
Generating PGP signing key ...
Initialized empty Libra repository in /Users/eli/projects/my-repo/.libra
  branch: main
  signing: enabled
```

### 特性 6：错误处理、退出码与兼容性

**关键修正：**

- 退出码以现有 `CliError` / `StableErrorCode` 框架为准，遵循 `0 / 128 / 129`
- `CliInvalidArguments` / `CliInvalidTarget` 默认退出码都是 `129`
- 不再用 `InitError::Io(ErrorKind::InvalidInput)` 笼统承载所有参数/目标错误

**建议的 `InitError` 方向：**

```rust
enum InitError {
    InvalidArgument { message: String, hint: Option<String> },
    AlreadyInitialized { path: PathBuf },
    SourcePathNotFound { path: PathBuf },
    InvalidGitRepository { path: PathBuf },
    TemplateNotFound { path: PathBuf },
    ConversionFailed { source: PathBuf, stage: &'static str, message: String },
    Io(io::Error),
    Database(DbErr),
}
```

**`InitError -> CliError` 映射原则：**

- 参数错误、无效 branch、无效 object-format
  - `CliError::command_usage(...)`
  - `StableErrorCode::CliInvalidArguments`
  - exit `129`
- 无效 Git 源（路径存在但不是合法 Git repo）
  - `CliError::command_usage(...)`
  - `StableErrorCode::CliInvalidTarget`
  - exit `129`
- 已初始化（worktree / bare）
  - `CliError::fatal(...)`
  - `StableErrorCode::RepoStateInvalid`
  - exit `128`
- 模板/源路径缺失
  - `CliError::fatal(...)`
  - `StableErrorCode::IoReadFailed`
  - exit `128`
- 数据库/vault 不变量破坏
  - `CliError::fatal(...)`
  - `StableErrorCode::InternalInvariant`
  - exit `128`

**Reinit 语义补齐：**

- 非 bare：检查工作树下 `.libra` 是否存在
- bare：在 repo 根目录显式检查 `libra.db` / 初始化标记，而不是等到 DB 创建时报 “Database file already exists.”

### 特性 7：彻底移除 Separate Directory 功能

**本批彻底移除 `--separate-libra-dir` 及其 alias `--separate-git-dir`。**

- 不再支持将 `.libra` 存储目录作为 `gitdir:` 文本文件链接到外部路径
- 简化底层路径解析逻辑，降低后续所有命令（尤其是 Agent 交互）对异常仓库结构的适配成本
- 从 CLI 参数（`InitArgs`）中彻底删除对应定义

### 特性 8：成功消息与 Human Output

#### `libra init`

```text
Creating repository layout ...
Initializing database ...
Setting up refs ...
Generating PGP signing key ...
Initialized empty Libra repository in /Users/eli/projects/my-repo/.libra
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
     to generate a repo-specific key later, run: libra config generate-ssh-key --remote origin
```

#### `libra init --bare`

```text
Creating repository layout ...
Initializing database ...
Setting up refs ...
Generating PGP signing key ...
Initialized empty bare Libra repository in /Users/eli/projects/my-repo.git
  branch: main
  signing: enabled
```

#### `libra init --vault false`

```text
Creating repository layout ...
Initializing database ...
Setting up refs ...
Initialized empty Libra repository in /Users/eli/projects/my-repo/.libra
  branch: main
  signing: disabled

Tip: to enable commit signing later, run: libra config generate-gpg-key
```

#### `libra init --quiet`

```text
(no output on success)
```

#### 错误：已初始化

```text
fatal: repository already initialized at '/Users/eli/projects/my-repo/.libra'
Hint: remove .libra/ to reinitialize.
Error-Code: LBR-REPO-003
```

#### 错误：无效 object format

```text
error: unsupported object format 'sha265'
Hint: did you mean 'sha256'?
Error-Code: LBR-CLI-002
```

#### 错误：源路径不是 Git 仓库

```text
error: '/path/to/dir' is not a valid Git repository
Hint: a valid Git repository must contain HEAD, config, and objects.
Error-Code: LBR-CLI-003
```

### 全部场景 JSON Output 设计（`--json`）

所有 JSON 输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。

#### 成功 envelope

```json
{
  "ok": true,
  "command": "init",
  "data": { ... }
}
```

#### `libra init --json`

```json
{
  "ok": true,
  "command": "init",
  "data": {
    "path": "/Users/eli/projects/my-repo/.libra",
    "bare": false,
    "initial_branch": "main",
    "object_format": "sha1",
    "ref_format": "strict",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "converted_from": null,
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519"
  }
}
```

#### `libra init --from-git-repository ../old-project --json`

```json
{
  "ok": true,
  "command": "init",
  "data": {
    "path": "/Users/eli/projects/my-repo/.libra",
    "bare": false,
    "initial_branch": "main",
    "object_format": "sha1",
    "ref_format": "strict",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "converted_from": "/Users/eli/projects/old-project/.git",
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519"
  }
}
```

#### `libra init --vault false --json`

```json
{
  "ok": true,
  "command": "init",
  "data": {
    "path": "/Users/eli/projects/my-repo/.libra",
    "bare": false,
    "initial_branch": "main",
    "object_format": "sha1",
    "ref_format": "strict",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": false,
    "converted_from": null,
    "ssh_key_detected": null
  }
}
```

#### 错误 JSON：无效目标 Git 仓库

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "'/path/to/dir' is not a valid Git repository",
  "hints": [
    "a valid Git repository must contain HEAD, config, and objects."
  ]
}
```

#### 错误 JSON：已初始化

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "repository already initialized at '/Users/eli/projects/my-repo/.libra'",
  "hints": [
    "remove .libra/ to reinitialize."
  ]
}
```

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra init                                 Initialize in current directory
    libra init my-project                      Initialize in a new directory
    libra init --bare my-repo.git              Create a bare repository
    libra init -b develop                      Use 'develop' as initial branch
    libra init --from-git-repository ../old    Convert from existing Git repo
    libra init --vault false                   Skip vault / GPG setup
    libra init --object-format sha256          Use SHA-256 hashing
```

### 测试要求

- `tests/command/output_flags_test.rs`
  - `--quiet init` 成功时无输出
  - `--json init` 不输出 progress
- `tests/command/init_test.rs`
  - success message 改为过去时
  - `--vault false` 输出 `signing: disabled`
  - `--vault true` 只有在 keygen 成功后才写入 `vault.signing=true`
  - bare reinit 返回统一的 `LBR-REPO-003`
- `tests/command/init_from_git_test.rs`
  - 缺失路径、非 Git 仓库、空仓库的错误码与 hint
  - `converted_from` JSON 字段
- `tests/command/init_separate_libra_dir_test.rs`
  - `--separate-git-dir` 仍可用且继续告警
- `tests/command/clone_cli_test.rs`
  - `libra --json clone ...` 不泄漏 `init` 的 JSON envelope 或 progress
- `tests/command/config_test.rs` / 其他受影响测试
  - 删除旧 `config` 表初始化断言
  - 补齐所有受迁移影响的 remote/branch/config `config_kv` 读写链路测试

### 文档与变更记录

- 创建或更新 `docs/commands/init.md`
  - 说明进度输出仅适用于顶层 human 模式
  - 说明 `--quiet` 成功时静默
  - 标注 `--separate-git-dir` 为 deprecated
- 更新 `CHANGELOG.md`
  - 记录 `libra init` 的 progress / JSON / error handling 改进
  - 记录 `--separate-git-dir` 仍保留但已废弃

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/init.rs` | **重构** | 拆分执行层与渲染层；添加 progress；补齐 bare/separate reinit 检测；成功消息改过去时；显式错误映射 |
| `src/command/clone.rs` | **修改** | 改为复用纯初始化执行路径，避免 `clone` 被 `init` 的输出污染 |
| `src/utils/convert.rs` | **修改** | 改为返回结构化 `ConversionReport`，并使用 `fetch_repository_safe()` |
| `src/utils/output.rs` | **小改** | 视实现需要新增 stderr progress helper |
| `src/internal/config.rs` | **修改** | 删除 `init` 迁移所需的 legacy `config` 表辅助逻辑；提供初始化阶段使用的 `config_kv` 批量写入辅助函数 |
| `tests/command/init_test.rs` | **扩展** | 更新成功消息、错误码、bare reinit 覆盖 |
| `tests/command/init_from_git_test.rs` | **扩展** | 覆盖结构化转换结果和错误场景 |
| `tests/command/init_separate_libra_dir_test.rs` | **保留并扩展** | 继续验证 deprecated alias warning |
| `tests/command/output_flags_test.rs` | **扩展** | 验证 quiet/json/progress 约束 |
| `tests/command/clone_cli_test.rs` | **扩展** | 验证 clone 不泄漏 init 输出 |
| `tests/command/config_test.rs` 及相关测试 | **清理/扩展** | 删除对 legacy `config` 表的初始化断言，改为验证 `config_kv` canonical seed keys |
