## Init 命令改进详细计划

> 最后编写时间：2026-03-24

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config` 的主改造已经在当前代码库落地，因此 `init` 计划不再以“等待 config 完成”为前提，而是直接建立在现有实现之上。

**已确认落地的基线：**
- `libra config generate-ssh-key --remote <name>` 已存在，可作为 init human tip 的后续命令
- `libra config generate-gpg-key` 已存在，`--vault false` 的后续补救路径已经具备
- local / global scope、`config_kv` 后端以及 `resolve_env()` 已落地
- `init_config()` 已通过 `ConfigKv::set_with_conn()` 写入所有 canonical seed keys（`core.*`、`libra.repoid`），不再使用旧 `config` 表；config_kv 迁移本身已完成，不是本批的主体工作

**基于当前代码的 Review 结论：**
- `init` 仍未提供顶层 `--json` / `--machine` 成功输出
- `init` 仍把 `--from-git-repository` 的 `canonicalize()` 放在外层，错误无法稳定映射到 `InitError`
- `convert_from_git_repository()` 仍直接向 stderr 打印 `"Converting from Git repository..."`
- `init_vault_for_repo()` 仍只读取 local `user.name` / `user.email`，没有复用已经落地的 scope-aware config / env fallback 规则
- `--separate-libra-dir` 当前仍被 `init` / `util` / `worktree` / 测试广泛使用；既然本批要顺手完成移除，就必须把参数、路径解析、worktree 兼容分支和测试一起纳入范围，不能只删 `init` flag

### 目标与非目标

**本批目标：**
- 消除 `libra init` 成功路径上的长时间沉默，在默认 human 模式下提供实时进度
- 为顶层 `libra init` 提供稳定的结构化输出（`--json` / `--machine`）
- 统一成功消息、错误码、hint 和 reinit 语义
- 在 `config` 已完成的基线上，补齐 `init` 与 `config` / `clone` 的行为对齐
- 停止在 `init` 中生成仓库专属 SSH key，改为检测系统 SSH key 并给出后续提示
- 在本批顺手完成 `--separate-libra-dir` / `--separate-git-dir` 的全链路移除，并收口到标准 `.libra/` 布局

**本批非目标：**
- **不把默认 `libra init` 总耗时优化到 `<500ms`**。只要 `--vault true` 仍然在 init 阶段生成 PGP key，总耗时仍会显著高于 500ms；本批解决的是“等待时无反馈”，不是“立即完成”
- **不在本批承诺 objects/branches/tags 转换统计**。在 `fetch`/`convert` 返回结构化统计前，不把这类字段写进对外 JSON 契约
- **不让 `init` 的内部复用路径直接打印 human/JSON 输出**。`clone` 等命令复用初始化逻辑时必须保持静默
- **不在 JSON 中暴露 `--template` 和 `--shared` 的详细信息**。`--template` 和 `--shared` 是低频高级参数，当前底层不返回结构化结果（template 复制了哪些文件、shared 实际应用了什么权限），硬编码到 JSON schema 会增加维护负担且几乎不被 Agent 使用。如果后续有需求，以向后兼容的增量字段扩展
- **不重复做已经完成的 `config_kv` seed 迁移**。本批只做行为补洞、测试补齐和输出层改造，不把“把 init 改成写 `config_kv`”当成主体工作

### 设计原则

1. **执行层与渲染层拆分**：把“执行初始化逻辑”和“输出 human/JSON 结果”分开。顶层 `libra init` 负责渲染；`clone` 等内部调用只拿执行结果，不直接打印
2. **实时进度只属于顶层 human 模式**：进度输出仅在“非 `--json`、非 `--machine`、非 `--quiet`、顶层 `init` 调用”时显示，写入 stderr；不复用当前写 stdout 的 `info_println!()`
3. **`--quiet` 成功时完全静默**：与现有全局输出语义和测试保持一致。`--quiet init` 不输出进度，也不输出最终确认行
4. **`--json` / `--machine` 只输出最终结果**：两者都只输出一个 success envelope 到 stdout；`--machine` 与 `--json` 使用同一 schema，但必须是单行紧凑 JSON；中间进度一律抑制
5. **GPG key 策略保持不变**：`--vault true`（默认）仍在 init 阶段完成 vault 初始化和 PGP key 生成；本批只改善反馈，不改变默认安全行为
6. **SSH key 不在 init 中生成**：依赖 transport 的 fallback 链使用系统 key 或后续 `config generate-ssh-key`
7. **以现有 `config_kv` 为唯一配置基线**：`init` 已经切到 `config_kv`；本批不引入 legacy `config` fallback / dual-write，剩余工作是补齐行为一致性与测试
8. **错误分类不靠 `io::ErrorKind::InvalidInput` 猜测**：参数错误、无效 Git 源、已初始化等场景需要显式的 `InitError` 变体，避免错误码漂移
9. **reinit 语义在 worktree 和 bare 模式下一致**：不能只检查工作树下的 `.libra/`
10. **成功消息使用过去时**：统一为 `Initialized empty ...`，并补充 branch/signing 等关键结果
11. **移除 separate directory 必须是全链路清理**：这不是“只删一个 init flag”的改动。`src/utils/util.rs` 中的 `gitdir:` link 解析、`worktree` 对 separate layout 的兼容/修复逻辑、以及对应测试都必须同步删除或改写；否则是未完成的 breaking change

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
- `--machine`：
  - 不输出 progress
  - 只输出最终 JSON envelope
  - 与 `--json` 使用同一字段，但必须保持单行，便于 Agent/脚本逐行消费

**为何不能直接在当前 `execute_safe()` 里边执行边输出：**

- `src/utils/output.rs` 中的 `info_println!()` 当前写的是 stdout，不是 stderr
- `clone` 目前直接复用 `init::execute_safe(...)`；如果 `init` 自己输出 JSON / machine，会污染 `clone` 的结构化输出流
- `--quiet init` 现有测试要求成功时静默，不能改成“仍输出最终确认行”
- `--from-git-repository` 的内部 `fetch_repository_safe()` 不能直接继承顶层 `OutputConfig`；否则 `--json` 默认的 `ProgressMode::Json` 会把 fetch 的 NDJSON progress 泄漏到 stderr，破坏“init 只输出一个 envelope”的契约

### 特性 2：JSON 输出设计

**目标：** 给顶层 `libra init --json` / `--machine` 提供稳定结果，但不把当前底层拿不到的数据提前承诺出去。

**成功输出结构：**

```rust
struct InitOutput {
    path: String,                    // 存储目录绝对路径；bare 时为 repo 根目录
    bare: bool,
    initial_branch: String,
    object_format: String,           // sha1 / sha256
    ref_format: String,              // strict / filesystem（对应 config_kv 中的 core.initrefformat）
    repo_id: String,
    vault_signing: bool,
    converted_from: Option<String>,  // 规范化后的源 Git 目录
    ssh_key_detected: Option<String>,
    warnings: Vec<String>,           // 非致命警告（如 template 文件复制失败但 init 整体成功）；空时 JSON 序列化为 []
}
```

> **字段命名说明**：JSON 字段 `ref_format` 的值直接对应 config_kv 中 `core.initrefformat` 的值（`"strict"` / `"filesystem"`）。选择 `ref_format` 而非 `init_ref_format` 是因为对外 JSON 不需要暴露内部 config key 命名的历史包袱。

**明确不纳入本批 JSON 契约的字段：**

- `gpg_fingerprint`
  - 当前 `vault::generate_pgp_key()` 返回的是 public key，不是 fingerprint
  - 除非 vault 层补充结构化 fingerprint，否则文档不承诺该字段
- `conversion.objects_fetched / branches / tags`
  - 当前 `convert_from_git_repository()` 不返回结构化统计
  - 在 `fetch`/`convert` 改成结构化返回前，不把统计字段写进对外 schema
- `template_applied` / `shared_mode`
  - 低频高级参数，当前底层不返回结构化结果；见非目标一节说明

**实现边界：**

- `emit_json_data("init", &output, output_config)` 只在顶层 `init` 分发路径调用
- 内部初始化函数返回 `InitOutput` 或等价结构；不在内部直接 `emit_json_data`
- `--machine` 复用同一 `InitOutput` schema，只改变 JSON 格式化方式（单行紧凑）
- 未来如果 `convert` 能返回统计，新增 `conversion` 字段应当是**向后兼容的增量扩展**

### 特性 3：Config 存储现状确认与剩余补齐

**当前代码状态：** `init_config()` 已经通过 `ConfigKv::set_with_conn()` 把 canonical seed keys 写入 `config_kv`。因此，“把 init 迁移到 `config_kv`”本身已经完成；本批要做的是把剩余行为和测试补齐，而不是重做迁移。

**当前仍未对齐的缺口：**
- `--vault false` 还没有显式写入 `vault.signing=false`，会把“明确关闭”和“未配置”混在一起
- `init_vault_for_repo()` 只读取 local `ConfigKv::get("user.name")` / `ConfigKv::get("user.email")`，没有复用当前代码库已经具备的 scope-aware 配置与 env fallback 规则
- 测试计划仍停留在“迁移 legacy `config` 表”的旧叙事，没有直接验证 canonical `config_kv` seed keys

**本批的绝对策略（保持纯 `config_kv`，补齐剩余行为）：**

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

**vault 相关键的边界：**

```text
vault.gpg.pubkey
vault.signing
```

- `vault.gpg.pubkey`：继续沿用 `vault::generate_pgp_key()` 的现有写入逻辑；init 不要额外引入第二套存储路径
- `vault.signing=false`：仅在 `--vault false` 时显式写入
- `vault.signing=true`：只能在 vault 初始化和 PGP key 生成成功后单独写入

**执行路径要求：**
1. 保持 `config_kv` 为 init 唯一配置存储，不引入 legacy `config` fallback / dual-write。
2. `--vault false` 时显式写入 `vault.signing=false`。
3. `--vault true` 时仅在 keygen 成功后写入 `vault.signing=true`。
4. 更新测试为直接断言 `config_kv` 的 canonical keys，并确认 init 不新增旧 `config` 表写入。

### 特性 4：Vault/GPG 初始化与 SSH key 检测

**GPG 策略：**

- `--vault true`（默认）：
  - 初始化 vault
  - 生成 PGP signing key
  - 公钥继续沿用 `vault::generate_pgp_key()` 的现有写入逻辑，保持 config key 为 `vault.gpg.pubkey`
  - **仅在上述两步都成功后**写入 `vault.signing=true`
- `--vault false`：
  - 跳过 vault 初始化
  - 显式写入 `vault.signing=false` 到 `config_kv`
  - vault 留给 config 的 lazy init 机制——后续首次 `config set vault.env.XXX` 或 `config set --encrypt` 时自动触发 vault 初始化（见 `config.md` 特性 1 vault lazy init 行为）

**PGP key 生成的 user identity 读取优先级：**

当前代码只读 local config（`ConfigKv::get("user.name")`，仅查 `.libra/libra.db`），这是不够的。PGP key 生成时应按以下优先级获取 identity：

1. 目标仓库的 config lookup：**target-local → global**（`user.name` / `user.email`）。注意这里的 local 必须基于 `init` 正在创建/转换的目标仓库路径解析，**不能**直接复用 `ScopedConfig::get(ConfigScope::Local, ...)` 那套基于当前工作目录的仓库发现逻辑；否则在仓库 A 中执行 `libra init ../repo-b` 时，会错误读到仓库 A 的 local identity
2. 与 `commit` 保持一致的环境变量 fallback：
   - name：`GIT_COMMITTER_NAME` → `GIT_AUTHOR_NAME` → `LIBRA_COMMITTER_NAME`
   - email：`GIT_COMMITTER_EMAIL` → `GIT_AUTHOR_EMAIL` → `EMAIL` → `LIBRA_COMMITTER_EMAIL`
3. fallback 硬编码默认值：`"Libra User"` / `"user@libra.local"`

**实现方式**：不要让 `init` 直接依赖 `command::commit` 或 `command::config`。应将 `commit.rs` 中“按 scope 读取 user config”和 `env_first_non_empty()` 的可复用部分下沉到 `src/internal/config.rs`（或等价的 internal helper），然后让 `commit` 与 `init` 共同复用。共享 helper 必须保留“config 来源”和“env 来源”的边界，`user.useConfigOnly` 的严格语义保持只属于 `commit`，**不要**被共享 helper 提前吞掉。

建议的共享 API 方向：

```rust
/// src/internal/config.rs 新增

pub struct UserIdentitySources {
    pub config_name: Option<String>,
    pub config_email: Option<String>,
    pub env_name: Option<String>,
    pub env_email: Option<String>,
}

pub enum LocalIdentityTarget<'a> {
    /// 读取“当前仓库”的 local config（commit 等命令使用）
    CurrentRepo,
    /// 读取“显式目标仓库”的 local config（init ../repo-b 等场景使用）
    ExplicitDb(&'a Path),
    /// 跳过 local，只查 global + env
    None,
}

/// 保留 config / env 的来源边界；不要在 helper 内提前 merge。
pub async fn resolve_user_identity_sources(
    local_target: LocalIdentityTarget<'_>,
) -> anyhow::Result<UserIdentitySources> {
    let config_name = scoped_config_get_for_target(local_target, "user.name").await?;
    let config_email = scoped_config_get_for_target(local_target, "user.email").await?;
    let env_name = env_first_non_empty(&[
        "GIT_COMMITTER_NAME", "GIT_AUTHOR_NAME", "LIBRA_COMMITTER_NAME",
    ]);
    let env_email = env_first_non_empty(&[
        "GIT_COMMITTER_EMAIL", "GIT_AUTHOR_EMAIL", "EMAIL", "LIBRA_COMMITTER_EMAIL",
    ]);

    Ok(UserIdentitySources {
        config_name,
        config_email,
        env_name,
        env_email,
    })
}

/// local 读取必须支持“当前仓库”与“显式目标仓库”两种模式。
/// 不要从 internal 层反向依赖 `command::config`；如需复用现有
/// global/local 路径解析，应把最小 scope/path helper 一并下沉到
/// internal，或在这里直接实现等价逻辑。
async fn scoped_config_get_for_target(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> anyhow::Result<Option<String>> { ... }

/// 返回第一个非空的环境变量值。
fn env_first_non_empty(keys: &[&str]) -> Option<String> { ... }
```

调用方差异：
- `commit`：调用 `resolve_user_identity_sources(LocalIdentityTarget::CurrentRepo)` 后，先读取 `user.useConfigOnly` 的 config-only 结果；若为 `true`，则**只使用 `config_*` 字段**，缺失时报错；否则再按 `config_* -> env_*` 合并
- `init`：调用 `resolve_user_identity_sources(LocalIdentityTarget::ExplicitDb(<target_db_path>))`，按 `config_* -> env_* -> 默认值` 合并；**不报错**

> **注 0**：`init` 不能直接复用当前 `ScopedConfig::get(ConfigScope::Local, ...)`，因为那套逻辑绑定的是当前工作目录仓库，而不是 `init` 的目标路径。

> **注 1**：不要引入 `LIBRA_USER_NAME` / `LIBRA_USER_EMAIL` 这类仅 init 识别的新环境变量，否则会让 `init` 与 `commit` 的身份来源分叉，增加开发者和 Agent 的心智负担。
>
> **注 2**：init 后用户可通过 `libra config set user.name "xxx"` 设置 identity，但不影响已生成的 PGP key。如需更换 PGP key identity，使用 `libra config generate-gpg-key --name "xxx" --email "xxx"`。
>
> **注 3（与 commit 的有意区别）**：`commit` 在 identity 缺失时**报错退出**（`missing_identity_error`），而 `init` 使用硬编码默认值继续执行。这是有意的——init 不应因 identity 缺失而阻塞仓库创建，用户可事后通过 `libra config set user.name` + `libra config generate-gpg-key` 重新生成。

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
   - **关键约束**：这里必须传入一个“子级输出配置”，例如基于顶层 `OutputConfig` clone 后强制 `progress = ProgressMode::None`、`json_format = None`、`quiet = true`。目的不是改变顶层 init 的最终输出，而是确保嵌套 fetch 不产生任何 progress / JSON / human 装饰输出
   - human 模式下，转换阶段只保留 init 自己的 `Converting from Git repository at ...` 进度行；不显示 fetch spinner
   - `--json` / `--machine` 下，stderr 必须保持干净，不能出现 fetch 的 NDJSON progress 事件
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
- 构造 bare reinit 的典型步骤：`libra init --bare repo.git && cd repo.git && libra init --bare` → 应返回 `LBR-REPO-003`

**Cross-Cutting Improvements 在 init 中的具体落地：**

| ID | 改进 | init 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | init 不使用 exit `1`。参数错误 → exit `129`（`CliInvalidArguments`/`CliInvalidTarget`）；运行时错误 → exit `128`（`RepoStateInvalid`/`IoReadFailed`/`InternalInvariant`）；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 `--help` EXAMPLES 段，7 条覆盖主要场景 |
| **F** | 拼写纠错 | **范围**：仅对 `--object-format` 做 fuzzy match（`sha265` → `did you mean 'sha256'?`）。`--ref-format` 和 `--initial-branch` 参数不做 fuzzy match——前者只有两个合法值（clap enum 已提供完整候选列表），后者是自由文本（分支名）无法做有意义的纠错 |
| **G** | Issues URL | 仅在 `LBR-INTERNAL-001`（数据库/vault 不变量破坏）时输出 `hint: please report this issue at: https://github.com/web3infra-foundation/libra/issues`。其他错误（参数错误、已初始化、路径缺失）不输出 Issues URL——这些是用户可自行修复的问题 |

### 特性 7：Separate Directory 全链路移除（纳入本批定位目标）

**当前代码状态：** 虽然 `docs/improvement/config.md` 已把 `--separate-git-dir` / `--separate-libra-dir` 记为“系统全局取消”，但当前代码并非如此：

- `src/command/init.rs` 仍声明 `separate_libra_dir` 参数并保留 `--separate-git-dir` alias / warning
- `src/utils/util.rs` 仍把 `.libra` 文件当作 `gitdir:` link file 解析
- `tests/command/init_separate_libra_dir_test.rs` 和 `tests/command/worktree_test.rs` 仍把 separate layout 当成正常成功路径

既然本轮要把此项纳入本批定位目标，就必须把这些链路一起收口，而不是只删 CLI 参数定义。

**本批处理原则：**
- 删除 `InitArgs::separate_libra_dir` 与 `--separate-git-dir` alias
- 删除 `src/utils/util.rs` 中 `.libra` link file / `gitdir:` 解析逻辑
- 删除或改写 `worktree` 中所有 separate-layout 兼容/修复分支
- 删除或改写依赖 separate-layout 的测试与辅助函数
- 发布后的 non-bare 仓库统一只支持工作树根目录下的标准 `.libra/` 布局

**不做向后兼容。** 已用 `--separate-libra-dir` 创建的仓库（`.libra` 是内容为 `gitdir: <path>` 的文件而非目录）在移除后将无法被识别，所有命令会报 `not a libra repository`。这是预期的 breaking change——该功能使用率极低且增加了整个仓库发现链路的复杂度。CHANGELOG 中给出迁移指引：

```text
Migration: if you previously used --separate-libra-dir, move the storage
directory back into the working tree:
  rm .libra                          # remove the link file
  mv /path/to/separate/storage .libra   # move storage dir into place
```

**本批 release gate：**
- 删除 `InitArgs::separate_libra_dir` 及 `--separate-git-dir` alias
- 删除 `src/utils/util.rs` 中 `.libra` link file / `gitdir:` 解析逻辑
- 删除或改写 `src/command/worktree.rs` 中 `gitdir:` link 写入和 separate-layout 兼容逻辑
- 删除或改写 `tests/command/init_separate_libra_dir_test.rs` 和 `tests/command/worktree_test.rs` 中所有 separate-layout 成功路径
- 删除 `src/` 与 `tests/` 下**所有**构造 `InitArgs` 时传递 `separate_libra_dir` 字段的代码。当前已知影响面至少包括：`src/command/clone.rs`、`src/command/tag.rs`、`src/utils/test.rs`、`tests/command/{status,commit,blame,checkout,cherry_pick,init_separate_libra_dir,worktree}_test.rs`
- 只有当以上链路全部清理完成时，才能在 CHANGELOG / docs 中宣称该功能已移除

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

#### 错误：使用已移除的 `--separate-libra-dir`

```text
error: unexpected argument '--separate-libra-dir' found

Usage: libra init [OPTIONS] [REPO_DIRECTORY]

For more information, try '--help'.
```

#### 错误：数据库/vault 不变量破坏（触发 Issues URL）

```text
fatal: vault initialization failed: unexpected internal error
Error-Code: LBR-INTERNAL-001

hint: please report this issue at: https://github.com/web3infra-foundation/libra/issues
```

### 全部场景结构化 Output 设计（`--json` / `--machine`）

所有结构化输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。`--machine` 与 `--json` 使用同一 schema，仅格式化方式不同（紧凑单行）。

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
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519",
    "warnings": []
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
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519",
    "warnings": []
  }
}
```

> **注**：转换后的 remote URL 不在 init JSON 中返回。Agent 可通过 `libra config get remote.origin.url --json` 获取。

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
    "ssh_key_detected": null,
    "warnings": []
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

### Libra vs Git vs jj 初始化命令对比

| Use Case | Git | jj | Libra（本批目标） |
|----------|-----|----|-------------------|
| 当前目录初始化 | `git init` | `jj git init` | `libra init` |
| 新目录初始化 | `git init my-project` | `jj git init my-project` | `libra init my-project` |
| bare 仓库 | `git init --bare repo.git` | 无直接等价命令 | `libra init --bare repo.git` |
| 指定初始分支 | `git init -b main` | 无 direct init flag | `libra init -b main` |
| 指定 object format | `git init --object-format=sha256` | 无 direct init flag | `libra init --object-format sha256` |
| 分离存储目录 | `git init --separate-git-dir <dir>` | `jj git init --no-colocate` / `--git-repo <path>` 是不同语义 | **移除，不再支持** |
| 从已有 Git 仓库导入 | 无单命令等价物 | `jj git init --git-repo <path>` | `libra init --from-git-repository <path>` |
| 成功确认消息 | 简短 human 输出 | 简短 human 输出 | 更明确的 `Initialized empty ...` + branch/signing |
| 实时进度 | 基本无 | 基本无 | **human 模式 stderr progress** |
| 结构化输出 | 无 | 无 | **`--json` / `--machine`** |
| 默认签名引导 | 无 | 无 | **默认初始化 vault + PGP key；可 `--vault false` 跳过** |
| SSH key 行为 | 使用系统 SSH key | 使用系统 / Git 侧配置 | **不在 init 中生成；检测系统 key 并给 tip** |

> 设计意图：语法层尽量保持 Git 用户可预测；在 Git / jj 都没有结构化输出和初始化后续引导的地方，Libra 补齐对开发者与 Agent 更友好的交互。

### 测试要求

#### `tests/command/output_flags_test.rs`（输出边界）

- `--quiet init` 成功时 stdout 和 stderr 均无输出
- `--json init` stdout 只有一个 JSON envelope，stderr 无 progress 行
- `--machine init` stdout 只有一个单行 JSON envelope，stderr 无 progress 行
- **（新增）human 模式 init**：stderr 包含 `"Creating repository layout"` 等进度行；stdout 只有最终确认消息（`Initialized empty`）

#### `tests/command/init_test.rs`（核心路径）

> **先决清理**：当前 `tests/command/init_test.rs` 实际是过期的 `init` 实现拷贝，**没有任何 `#[test]` / `#[tokio::test]` 用例**。本批不能在这个文件上“继续补几条断言”了，必须先把它重写为真实测试文件，或拆出 helper 后新建真正的测试用例。

- success message 改为过去时（`Initialized empty Libra repository in ...`）
- `--vault false` 输出 `signing: disabled`
- `--vault false` 显式写入 `vault.signing=false`
- `--vault true` 只有在 keygen 成功后才写入 `vault.signing=true`
- **（新增）identity fallback**：仅配置 global `user.name` / `user.email` 时，`init --vault true` 仍能完成 keygen；仅配置 env fallback 时也能完成
- **（新增）target-local 隔离回归**：在仓库 A 中设置 local `user.name` / `user.email`，然后从仓库 A 内执行 `libra init ../repo-b --vault true`；断言 repo B **不会继承 repo A 的 local identity**，而是继续按“repo B local（空）→ global/env → 默认值”解析
- bare reinit 返回统一的 `LBR-REPO-003`：构造步骤 `libra init --bare repo.git && cd repo.git && libra init --bare`
- worktree reinit 返回统一的 `LBR-REPO-003`
- **（新增）`--object-format sha265` fuzzy match**：错误消息包含 `did you mean 'sha256'?`
- **（新增）config_kv seed keys 精确验证**：init 后直接查询 `config_kv` 表，逐一断言以下 canonical keys 存在且值正确：
  - `core.repositoryformatversion = "0"`
  - `core.filemode = "true"` (Unix) / `"false"` (Windows)
  - `core.bare = "true"` / `"false"`（与 `--bare` 参数一致）
  - `core.logallrefupdates = "true"`
  - `core.objectformat`（与 `--object-format` 参数一致，默认 `"sha1"`）
  - `core.initrefformat`（与 `--ref-format` 参数一致，默认 `"strict"`）
  - `libra.repoid`（非空 UUID 格式）
  - `--vault true` 时 `vault.signing = "true"` 存在
  - `--vault false` 时 `vault.signing = "false"` 存在
  - 旧 `config` 表**不存在**任何 init 写入的记录
  - `user.useConfigOnly=true` 且缺失 identity 时，`init --vault true` 仍会成功并回落到默认值（验证它没有误复用 commit 的严格失败语义）

#### `tests/command/init_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性：
  - `path` 是绝对路径（以 `/` 或盘符开头）
  - `bare` 是 bool
  - `initial_branch` 是非空 string
  - `object_format` 是 `"sha1"` 或 `"sha256"`
  - `ref_format` 是 `"strict"` 或 `"filesystem"`
  - `repo_id` 匹配 UUID 格式（`[0-9a-f]{8}-...`）
  - `vault_signing` 是 bool，与 `--vault` 参数一致
  - `converted_from` 在非转换模式下为 `null`
  - `ssh_key_detected` 在无 SSH key 时为 `null`
  - `warnings` 是 array（正常场景为空数组 `[]`）
- **`--vault false --json`**：`vault_signing == false`，`warnings` 为 `[]`
- **`--bare --json`**：`bare == true`，`path` 不以 `/.libra` 结尾
- **`--machine init`**：与 `--json` 的 schema 等价，但 stdout 按 `\n` 分割后恰好 1 行非空行，且该行能被 `serde_json::from_str()` 解析为与 `--json` 相同的 schema
- **测试隔离要求**：所有涉及 `ssh_key_detected` 的断言必须使用隔离的 `HOME` / `USERPROFILE` / `XDG_CONFIG_HOME`，避免宿主机真实 `~/.ssh` 污染结果

#### `tests/command/init_from_git_test.rs`（转换场景）

- 缺失路径、非 Git 仓库、空仓库的错误码与 hint
- `converted_from` JSON 字段（非 null，值为规范化后的源 Git 目录绝对路径）
- 错误 JSON 包含 `error_code` 和 `hints`
- stderr 不出现来自 `convert_from_git_repository()` 的额外裸打印
- `--from-git-repository --json` / `--machine` 时，stderr 不出现 fetch 的 progress / NDJSON 事件
- human 模式下只出现 init 自己的 `Converting from Git repository at ...` 阶段提示，不出现 fetch spinner / 额外装饰输出

#### `tests/command/init_separate_libra_dir_test.rs`（移除验证，改写）

- 使用 `--separate-libra-dir` 或 `--separate-git-dir` 时 clap 返回 `unexpected argument` 错误
- 进程退出码为非零
- 不创建任何 `.libra` 目录或 `gitdir:` 链接文件
- **（新增）历史仓库 breaking change 钉死**：构造一个旧式 separate-layout 仓库（工作树下 `.libra` 为 `gitdir: <path>` 文件，外部 storage 目录完整），验证 `libra status` / `libra config list` 稳定返回 `not a libra repository`

#### `tests/command/worktree_test.rs`（breaking change 收口）

- 删除或改写所有依赖 separate-layout 的场景，不能继续保留“`init --separate-libra-dir` 成功”的测试前提
- 验证在只支持标准 `.libra/` 布局后，main worktree 锚定、linked worktree state 修复、remove main 防护等能力仍然成立

#### `tests/command/clone_cli_test.rs`（隔离验证）

- `libra --json clone ...` 不泄漏 `init` 的 JSON envelope 或 progress
- `libra --machine clone ...` 不泄漏 `init` 的 JSON envelope 或 progress

#### `src/command/init.rs`（单元测试，执行层隔离）

- **（新增）`run_init()` 零输出隔离**：在 `src/command/init.rs` 的 `#[cfg(test)]` 单元测试中直接调用纯执行层 `run_init()`，断言 stdout 和 stderr 均无输出（验证执行层不产生任何渲染副作用）
- 不要为了这条测试把 `run_init()` 暴露成新的公共 API；集成测试继续只覆盖 CLI/command 边界

#### `tests/command/config_test.rs` / 其他受影响测试

- 删除旧 `config` 表初始化断言
- 补齐所有受迁移影响的 remote/branch/config `config_kv` 读写链路测试

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入迁移范围的命令、内部模块和转发路径，都必须有对应的集成测试覆盖新 config_kv 读写链路

### 文档与变更记录

- 创建或更新 `docs/commands/init.md`
  - 说明进度输出仅适用于顶层 human 模式
  - 说明 `--quiet` 成功时静默
  - 说明 `--json` / `--machine` 的 schema 和 progress 抑制规则
  - 说明 `--separate-libra-dir` / `--separate-git-dir` 已移除
  - 补充 Libra vs Git vs jj 初始化命令对比
- 更新 `CHANGELOG.md`
  - 记录 `libra init` 的 progress / JSON / error handling / warnings 改进
  - 记录 vault identity / `vault.signing=false` 对齐
  - 记录 `--separate-libra-dir` / `--separate-git-dir` 已移除

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/init.rs` | **重构** | 拆分执行层与渲染层；添加 progress；补齐 bare/separate reinit 检测；成功消息改过去时；显式错误映射；删除 `separate_libra_dir` 参数 |
| `src/command/clone.rs` | **修改** | 改为复用纯初始化执行路径，避免 `clone` 被 `init` 的输出污染；删除 `InitArgs` 构造中的 `separate_libra_dir` 字段 |
| `src/command/commit.rs` | **修改** | 改为复用下沉到 `internal/config.rs` 的 source-aware `resolve_user_identity_sources()` helper，并继续在本地保留 `user.useConfigOnly` 的严格语义 |
| `src/command/worktree.rs` | **修改** | 删除 `gitdir:` link 写入和 separate-layout 兼容逻辑 |
| `src/command/tag.rs` | **小改** | 删除 `InitArgs` 构造中的 `separate_libra_dir` 字段 |
| `src/utils/convert.rs` | **修改** | 改为返回结构化 `ConversionReport`，并使用 `fetch_repository_safe()` |
| `src/utils/output.rs` | **小改** | 视实现需要新增 stderr progress helper |
| `src/utils/util.rs` | **修改** | 删除 `.libra` link file / `gitdir:` separate-layout 解析（`parse_separate_libra_dir_file()` 及调用处） |
| `src/utils/test.rs` | **小改** | 删除 `InitArgs` 构造中的 `separate_libra_dir` 字段 |
| `src/internal/config.rs` | **修改** | 新增 `resolve_user_identity_sources()` / `scoped_config_get_for_target()` / `env_first_non_empty()` 共享 helper；如需 scope/path 复用，则把最小 DB lookup primitive 一并下沉到 internal，避免反向依赖 `command::config` |
| `tests/command/init_test.rs` | **重写** | 先移除过期实现拷贝，再补入真实 init 核心路径测试 |
| `tests/command/init_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/init_from_git_test.rs` | **扩展** | 覆盖结构化转换结果和错误场景 |
| `tests/command/init_separate_libra_dir_test.rs` | **改写** | 从“验证 deprecated alias warning”改为“验证已移除参数报错” |
| `tests/command/{status,commit,blame,checkout,cherry_pick}_test.rs` | **小改** | 删除 `InitArgs` 构造中的 `separate_libra_dir` 字段，消除 separate-layout 移除带来的编译影响面 |
| `tests/command/worktree_test.rs` | **清理/改写** | 删除对 separate-layout 的成功路径依赖，收口到标准 `.libra/` 布局 |
| `tests/command/output_flags_test.rs` | **扩展** | 验证 quiet/json/progress 约束；新增 human 模式 stderr 进度正面验证 |
| `tests/command/clone_cli_test.rs` | **扩展** | 验证 clone 不泄漏 init 输出 |
| `tests/command/mod.rs` | **修改** | 注册新增的 `init_json_test.rs`，并在重写测试文件后保持模块清单有效 |
| `tests/command/config_test.rs` 及相关测试 | **清理/扩展** | 删除对 legacy `config` 表的初始化断言，改为验证 `config_kv` canonical seed keys |
