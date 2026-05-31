//! Initializes a repository by creating .libra storage, seeding HEAD and
//! default refs/config, and preparing the backing database.
//!
//! Error rendering and stable-code expectations are part of the CLI contract:
//! see `docs/development/cli-error-contract-design.md`.
//!
//! 通过创建 .libra 存储、初始化 HEAD 和默认 refs/config，以及准备支持数据库来初始化存储库。
//!
//! 错误呈现和稳定代码期望是 CLI 契约的一部分：
//! 见 `docs/development/cli-error-contract-design.md`。

use std::{
    env, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use clap::{Parser, ValueEnum};
use git_internal::hash::{HashKind, set_hash_kind};
use sea_orm::{ActiveModelTrait, DbConn, DbErr, Set, TransactionTrait};
use serde::Serialize;

use crate::{
    internal::{
        config::{ConfigKv, LocalIdentityTarget, resolve_user_identity_sources},
        db::{self, get_db_conn_instance_for_path},
        model::{config, reference},
    },
    utils::{
        convert,
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, emit_json_data},
        util::{DATABASE, ROOT_DIR, cur_dir},
    },
};

const DEFAULT_BRANCH: &str = "main";
const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";
const EXAMPLES: &str = r#"EXAMPLES:
    libra init                                 Initialize in current directory
    libra init my-project                      Initialize in a new directory
    libra init --bare my-repo.git              Create a bare repository
    libra init -b develop                      Use 'develop' as initial branch
    libra init --from-git-repository ../old    Convert from existing Git repo
    libra init --vault false                   Skip vault / GPG setup
    libra init --object-format sha256          Use SHA-256 hashing"#;

// NOTE: `src/command/init.rs` lines 3-20 are a protected merge-conflict block in this workspace.
// The imports inside that block must stay as-is. To avoid `unused_imports` warnings without
// changing that block, we reference the imported symbols here in a private, dead-code helper.
//
// 注意：`src/command/init.rs` 第 3-20 行是此工作空间中受保护的合并冲突块。
// 该块内的导入必须保持原样。为了避免 `unused_imports` 警告而不更改该块，
// 我们在此私有、死代码助手中引用导入的符号。
#[allow(dead_code, deprecated)]
fn _touch_conflict_imports() {
    let _ = env::current_dir;
    let _ = DATABASE;
    let _ = cur_dir();
    let _ = db::create_database;
    let _ = std::mem::size_of::<config::Model>();
    let _ = std::mem::size_of::<reference::Model>();
    let _ = std::mem::size_of::<DbConn>();
    let _ = Set(1i32);

    fn _needs_active_model_trait<T: ActiveModelTrait>() {}
    fn _needs_transaction_trait<T: TransactionTrait>() {}
}

use crate::utils::ignore;

const MAX_BRANCH_NAME_LENGTH: usize = 255;
const LOCK_SUFFIX: &str = ".lock";
const HEAD_REF: &str = "HEAD";
const AT_REF: &str = "@";
const DOT_REF: &str = ".";
const DOUBLE_DOT_REF: &str = "..";
const SLASH: char = '/';
const DOUBLE_SLASH: &str = "//";
const DOUBLE_DOT: &str = "..";

#[derive(thiserror::Error, Debug)]
pub enum InitError {
    #[error("{message}")]
    InvalidArgument {
        message: String,
        hint: Option<String>,
    },

    #[error("repository already initialized at '{path}'")]
    AlreadyInitialized { path: PathBuf },

    #[error("source git repository '{path}' does not exist")]
    SourcePathNotFound { path: PathBuf },

    #[error("'{path}' is not a valid Git repository")]
    InvalidGitRepository { path: PathBuf },

    #[error("template directory '{path}' does not exist")]
    TemplateNotFound { path: PathBuf },

    #[error("path '{path}' is not valid UTF-8")]
    InvalidUtf8Path { path: PathBuf },

    #[error("conversion from git repository '{repo}' failed during {stage}: {message}")]
    ConversionFailed {
        repo: PathBuf,
        stage: &'static str,
        message: String,
    },

    #[error("vault initialization failed: {message}")]
    VaultInitializationFailed { message: String },

    #[error("{0}")]
    IgnoreFile(#[from] ignore::IgnoreFileError),

    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("initialization failed due to a storage error: {0}")]
    Database(#[from] DbErr),
}

impl From<InitError> for CliError {
    fn from(error: InitError) -> Self {
        match error {
            InitError::InvalidArgument { message, hint } => {
                // Intent: invalid init flags are user-correctable CLI usage
                // errors, not repository or filesystem failures.
                //
                // 意图：无效的 init 标志是用户可纠正的 CLI 使用错误，
                // 而不是存储库或文件系统故障。
                let mut cli = CliError::command_usage(message)
                    .with_stable_code(StableErrorCode::CliInvalidArguments);
                if let Some(hint) = hint {
                    cli = cli.with_hint(hint);
                }
                cli
            }
            InitError::AlreadyInitialized { path } => {
                // Intent: this is an invalid repository state, not an I/O
                // failure. The recovery is to remove the existing Libra state
                // before retrying.
                //
                // 意图：这是一个无效的存储库状态，而不是 I/O 故障。
                // 恢复是在重试之前移除现有的 Libra 状态。
                let remove_target = if path.file_name() == Some(std::ffi::OsStr::new(ROOT_DIR)) {
                    ".libra/".to_string()
                } else {
                    path.display().to_string()
                };
                CliError::fatal(format!(
                    "repository already initialized at '{}'",
                    path.display()
                ))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint(format!("remove {remove_target} to reinitialize."))
            }
            InitError::SourcePathNotFound { path } => {
                // Intent: conversion cannot read the requested source path; the
                // repository state is unchanged, so classify as a read failure.
                //
                // 意图：转换无法读取请求的源路径；
                // 存储库状态未改变，因此分类为读取失败。
                CliError::fatal(format!(
                    "source git repository '{}' does not exist",
                    path.display()
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            }
            InitError::InvalidGitRepository { path } => CliError::command_usage(format!(
                "'{}' is not a valid Git repository",
                path.display()
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("a valid Git repository must contain HEAD, config, and objects."),
            InitError::TemplateNotFound { path } => {
                // Intent: `--template` points at a filesystem resource that
                // could not be read; keep the user hint focused on the path.
                //
                // 意图：`--template` 指向无法读取的文件系统资源；
                // 将用户提示保持在路径上。
                CliError::fatal(format!(
                    "template directory '{}' does not exist",
                    path.display()
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            }
            InitError::InvalidUtf8Path { path } => {
                CliError::fatal(format!("path '{}' is not valid UTF-8", path.display()))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            InitError::ConversionFailed {
                repo,
                stage,
                message,
            } => {
                // Intent: conversion failures may leave partially initialized
                // repository state, so route agents toward cleanup/retry rather
                // than treating the source Git repository as merely unreadable.
                //
                // 意图：转换失败可能会留下部分初始化的存储库状态，
                // 因此将代理引导到清理/重试，而不是将源 Git 存储库视为仅不可读。
                CliError::fatal(format!(
                    "conversion from git repository '{}' failed during {stage}: {message}",
                    repo.display()
                ))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
            }
            InitError::VaultInitializationFailed { message } => {
                // Intent: vault setup runs after repository metadata exists;
                // failure here means an internal initialization invariant broke
                // and should be reported with enough context for maintainers.
                //
                // 意图：保险库设置在存储库元数据存在后运行；
                // 这里的失败意味着内部初始化不变量破裂，
                // 应该以足够的上下文为维护人员报告。
                CliError::fatal(format!("vault initialization failed: {message}"))
                    .with_stable_code(StableErrorCode::InternalInvariant)
                    .with_hint(format!("please report this issue at: {ISSUE_URL}"))
            }
            InitError::IgnoreFile(error) => {
                let stable_code = if error.is_write() {
                    StableErrorCode::IoWriteFailed
                } else {
                    StableErrorCode::IoReadFailed
                };
                CliError::fatal(error.to_string())
                    .with_stable_code(stable_code)
                    .with_hint(error.recovery_hint())
            }
            InitError::Io(error) => match error.kind() {
                io::ErrorKind::InvalidInput => CliError::command_usage(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidArguments),
                _ => CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::IoReadFailed),
            },
            InitError::Database(error) => {
                // Intent: schema/bootstrap failures violate the init contract
                // because a newly created repo must always have a usable DB.
                //
                // 意图：架构/引导失败违反了 init 契约，
                // 因为新创建的仓库必须始终有一个可用的数据库。
                CliError::fatal(format!("database initialization failed: {error}"))
                    .with_stable_code(StableErrorCode::InternalInvariant)
                    .with_hint(format!("please report this issue at: {ISSUE_URL}"))
            }
        }
    }
}

#[derive(ValueEnum, Debug, Clone, PartialEq)]
pub enum RefFormat {
    Strict,
    Filesystem,
}

impl RefFormat {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Filesystem => "filesystem",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InitOutput {
    pub path: String,
    pub bare: bool,
    pub initial_branch: String,
    pub object_format: String,
    pub ref_format: String,
    pub repo_id: String,
    pub vault_signing: bool,
    pub converted_from: Option<String>,
    pub ssh_key_detected: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
#[command(after_help = EXAMPLES)]
pub struct InitArgs {
    /// Create a bare repository (no working tree; metadata at the target directory itself)
    #[clap(long, required = false)]
    pub bare: bool,

    /// Copy hook and exclude templates from `template-directory` instead of using the built-in defaults
    #[clap(long = "template", name = "template-directory", required = false)]
    pub template: Option<String>,

    /// Override the initial branch name (default: `main`)
    #[clap(short = 'b', long, required = false)]
    pub initial_branch: Option<String>,

    /// Directory in which to create the new `.libra` repository (default: current directory)
    #[clap(value_name = "DIRECTORY", default_value = ".")]
    pub repo_directory: String,

    /// Suppress the "Initialized empty Libra repository" banner (errors still print)
    #[clap(long, short = 'q', required = false)]
    pub quiet: bool,

    /// Filesystem sharing mode for the repository (placeholder — see `git init --shared`)
    #[clap(long, required = false, value_name = "MODE")]
    pub shared: Option<String>,

    /// Object hash algorithm: `sha1` (default) or `sha256`
    #[clap(long = "object-format", name = "format", required = false)]
    pub object_format: Option<String>,

    /// Ref name validation strategy: `strict` (default) or `filesystem`
    #[clap(long = "ref-format", value_enum, required = false)]
    pub ref_format: Option<RefFormat>,

    /// Convert an existing Git repository at `path` into a Libra repository (copies objects, refs, config)
    #[clap(long = "from-git-repository", value_name = "path", required = false)]
    pub from_git_repository: Option<String>,

    /// Initialize the embedded libvault and a PGP signing key (default: true). Pass `--vault false` to skip
    #[clap(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub vault: bool,
}

struct InitProgress {
    enabled: bool,
}

impl InitProgress {
    fn enabled() -> Self {
        Self { enabled: true }
    }

    fn disabled() -> Self {
        Self { enabled: false }
    }

    fn emit(&self, message: impl AsRef<str>) {
        if self.enabled {
            eprintln!("{}", message.as_ref());
        }
    }
}

struct CurrentDirGuard {
    original_dir: PathBuf,
}

impl CurrentDirGuard {
    fn change_to(target: &Path) -> io::Result<Self> {
        let original_dir = env::current_dir()?;
        env::set_current_dir(target)?;
        Ok(Self { original_dir })
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.original_dir);
    }
}

/// Fire-and-forget CLI dispatcher entry for `libra init`.
///
/// # Side Effects
/// - Delegates to [`execute_safe`] with the default [`OutputConfig`].
/// - Prints any rendered [`CliError`] to stderr.
///
/// # Errors
/// This compatibility entry does not return errors. Call [`execute_safe`] when
/// the caller must observe failure details or stable error codes.
///
/// `libra init` 的快速执行 CLI 分发器入口。
///
/// # 副作用
/// - 使用默认的 [`OutputConfig`] 委托给 [`execute_safe`]。
/// - 将任何呈现的 [`CliError`] 打印到 stderr。
///
/// # 错误
/// 这个兼容性入口不返回错误。当调用者必须观察失败详情或稳定错误代码时，
/// 调用 [`execute_safe`]。
pub async fn execute(args: InitArgs) {
    if let Err(error) = execute_safe(args, &OutputConfig::default()).await {
        error.print_stderr();
    }
}

/// Executes repository initialization and renders the requested output format.
///
/// # Side Effects
/// - Creates the target repository storage layout (`.libra/` for non-bare
///   repositories, or the target directory for `--bare`).
/// - Initializes the SQLite database and writes core config plus HEAD/branch
///   reference rows.
/// - Installs default hook and exclude templates unless `--template` supplies
///   replacements.
/// - Creates or updates the root `.libraignore` for non-bare repositories.
/// - Optionally converts objects/refs from an existing Git repository.
/// - Initializes vault credentials and a PGP signing key unless `--vault false`.
/// - Emits human or JSON output according to [`OutputConfig`].
///
/// # Errors
/// Returns a structured [`CliError`] when validation fails, the repository is
/// already initialized, layout/database creation fails, Git conversion fails, or
/// vault/signing setup cannot complete. Stable error-code mapping follows
/// `docs/development/cli-error-contract-design.md`.
///
/// 执行存储库初始化并呈现请求的输出格式。
///
/// # 副作用
/// - 创建目标存储库存储布局（非裸库的 `.libra/`，或 `--bare` 的目标目录）。
/// - 初始化 SQLite 数据库并写入核心配置加 HEAD/分支参考行。
/// - 安装默认钩子和排除模板，除非 `--template` 提供替代品。
/// - 为非裸库创建或更新根 `.libraignore`。
/// - 可选择从现有 Git 存储库转换对象/参考。
/// - 初始化保险库凭据和 PGP 签名密钥，除非 `--vault false`。
/// - 根据 [`OutputConfig`] 发出人类可读或 JSON 输出。
///
/// # 错误
/// 当验证失败、存储库已初始化、布局/数据库创建失败、Git 转换失败或
/// 保险库/签名设置无法完成时，返回结构化的 [`CliError`]。稳定错误代码
/// 映射遵循 `docs/development/cli-error-contract-design.md`。
pub async fn execute_safe(args: InitArgs, output: &OutputConfig) -> CliResult<()> {
    let mut effective_output = output.clone();
    if args.quiet {
        effective_output.quiet = true;
        effective_output.progress = ProgressMode::None;
        effective_output.progress_preference = crate::utils::output::ProgressPreference::None;
    }

    let progress = if effective_output.is_json() || effective_output.quiet {
        InitProgress::disabled()
    } else {
        InitProgress::enabled()
    };
    let result = run_init_internal(args, &progress).await?;
    render_init_result(&result, &effective_output)
}

fn render_init_result(result: &InitOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("init", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    let repo_type = if result.bare { " bare" } else { "" };
    println!(
        "Initialized empty{repo_type} Libra repository in {}",
        result.path
    );
    println!("  branch: {}", result.initial_branch);
    println!(
        "  signing: {}",
        if result.vault_signing {
            "enabled"
        } else {
            "disabled"
        }
    );

    if !result.vault_signing {
        println!();
        println!("Tip: to enable commit signing later, run: libra config generate-gpg-key");
    }

    println!();
    match &result.ssh_key_detected {
        Some(path) => {
            println!(
                "Tip: using existing SSH key at {}",
                display_home_relative(path)
            );
            println!(
                "     to generate a repo-specific key later, run: libra config generate-ssh-key --remote origin"
            );
        }
        None => {
            println!("Tip: no SSH key found at ~/.ssh/");
            println!("     push/pull via SSH will require a key");
            println!("     generate one with: libra config generate-ssh-key --remote origin");
            println!("     or create a system key: ssh-keygen -t ed25519");
        }
    }

    for warning in &result.warnings {
        eprintln!("warning: {warning}");
    }

    Ok(())
}

fn display_home_relative(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    let home = home.to_string_lossy().to_string();
    if let Some(rest) = path.strip_prefix(&home) {
        return format!("~{rest}");
    }
    path.to_string()
}

/// Runs initialization without rendering.
///
/// # Side Effects
/// Same repository, database, refs, conversion, ignore-file, and vault writes as
/// [`execute_safe`], but no human/JSON success output is emitted.
///
/// # Errors
/// Returns [`InitError`] directly so tests and higher-level commands can assert
/// the domain failure before CLI error mapping.
///
/// 运行初始化而不呈现输出。
///
/// # 副作用
/// 与 [`execute_safe`] 相同的存储库、数据库、参考、转换、忽略文件和保险库写入，
/// 但不发出人类可读/JSON 成功输出。
///
/// # 错误
/// 直接返回 [`InitError`]，以便测试和更高级命令可以在 CLI 错误映射之前
/// 断言域失败。
pub(crate) async fn run_init(args: InitArgs) -> Result<InitOutput, InitError> {
    run_init_internal(args, &InitProgress::disabled()).await
}

#[allow(dead_code)]
/// Legacy initialization helper retained for tests and older call sites.
///
/// # Side Effects
/// Performs the same repository initialization writes as [`run_init`].
///
/// # Errors
/// Returns the underlying [`InitError`] and discards the success metadata.
///
/// 为测试和较旧的调用站点保留的旧初始化帮助器。
///
/// # 副作用
/// 执行与 [`run_init`] 相同的存储库初始化写入。
///
/// # 错误
/// 返回底层的 [`InitError`] 并丢弃成功元数据。
pub async fn init(args: InitArgs) -> Result<(), InitError> {
    run_init(args).await.map(|_| ())
}

async fn run_init_internal(
    args: InitArgs,
    progress: &InitProgress,
) -> Result<InitOutput, InitError> {
    let current_dir = cur_dir();
    let target_dir = resolve_cli_path(&current_dir, &args.repo_directory);
    let root_dir = storage_root(&target_dir, args.bare);
    let from_git = args
        .from_git_repository
        .as_ref()
        .map(|path| resolve_existing_cli_path(&current_dir, path))
        .transpose()?;
    let template_dir = args
        .template
        .as_ref()
        .map(|path| resolve_template_path(&current_dir, path))
        .transpose()?;
    let object_format = resolve_object_format(args.object_format.as_deref())?;
    let ref_format = args.ref_format.clone().unwrap_or(RefFormat::Strict);
    let initial_branch_name = args
        .initial_branch
        .clone()
        .unwrap_or_else(|| DEFAULT_BRANCH.to_string());

    validate_branch_name(&initial_branch_name, &ref_format)?;
    validate_shared_mode(args.shared.as_deref())?;

    if is_reinit(&target_dir, args.bare) {
        return Err(InitError::AlreadyInitialized {
            path: root_dir.clone(),
        });
    }

    if target_dir.exists() {
        is_writable(&target_dir)?;
    }

    progress.emit("Creating repository layout ...");
    fs::create_dir_all(&root_dir)?;
    prepare_repository_layout(&root_dir, template_dir.as_deref())?;

    progress.emit("Initializing database ...");
    let database_path = root_dir.join(DATABASE);
    // INVARIANT: the database must exist before refs, config, conversion, or
    // vault setup run; those later stages persist their durable state through
    // this connection/path and assume schema bootstrap has completed.
    //
    // 不变量：数据库必须在参考、配置、转换或保险库设置运行之前存在；
    // 这些后期阶段通过此连接/路径保持其持久状态，并假设架构引导已完成。
    let conn = create_database_connection(&database_path).await?;
    let repo_id = init_config(&conn, args.bare, &object_format, &ref_format).await?;

    progress.emit("Setting up refs ...");
    // INVARIANT: refs are initialized after core config so HEAD/branch rows are
    // tied to the repository identity and hash/ref-format choices already stored
    // in config.
    //
    // 不变量：在核心配置之后初始化参考，以便 HEAD/分支行与
    // 已存储在配置中的存储库身份和哈希/参考格式选择相关联。
    initialize_refs(&conn, &initial_branch_name).await?;

    set_dir_hidden(&root_dir)?;
    if let Some(shared_mode) = args.shared.as_deref() {
        apply_shared(&root_dir, shared_mode)?;
    }

    let mut warnings = Vec::new();
    if !args.bare {
        ignore::ensure_root_libraignore(&target_dir)?;
    }

    let target_guard_path = target_dir
        .canonicalize()
        .unwrap_or_else(|_| target_dir.clone());

    let converted_from = if let Some(source) = from_git {
        let source_git_dir = convert::resolve_git_source_dir(&source)?;
        progress.emit(format!(
            "Converting from Git repository at {} ...",
            source_git_dir.display()
        ));
        // INVARIANT: conversion helpers read/write paths relative to the target
        // worktree, so the temporary cwd switch must be active for the full
        // conversion call and must be dropped before later stages continue.
        //
        // 不变量：转换助手相对于目标工作树读取/写入路径，因此临时 cwd
        // 切换必须对整个转换调用处于活动状态，并且必须在后续阶段继续之前被放弃。
        let _guard = CurrentDirGuard::change_to(&target_guard_path)?;
        let report = convert::convert_from_git_repository(&source, args.bare).await?;
        warnings.extend(report.warnings);
        Some(report.source_git_dir)
    } else {
        None
    };

    if args.vault {
        progress.emit("Generating PGP signing key ...");
        // INVARIANT: vault bootstrap runs after DB/config/ref initialization
        // because it records signing state in the repo DB and must roll back its
        // own vault files if credential or key generation fails.
        //
        // 不变量：保险库引导在数据库/配置/参考初始化之后运行，
        // 因为它在仓库数据库中记录签名状态，并且如果凭据或密钥生成失败，
        // 必须回滚其自己的保险库文件。
        let _guard = CurrentDirGuard::change_to(&target_guard_path)?;
        init_vault_for_repo(&root_dir, &database_path).await?;
    } else {
        set_vault_signing_value(&database_path, false).await?;
    }

    set_hash_kind(match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => HashKind::Sha1,
    });

    let path = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.clone())
        .to_string_lossy()
        .to_string();
    Ok(InitOutput {
        path,
        bare: args.bare,
        initial_branch: initial_branch_name,
        object_format,
        ref_format: ref_format.as_str().to_string(),
        repo_id,
        vault_signing: args.vault,
        converted_from,
        ssh_key_detected: detect_system_ssh_key(),
        warnings,
    })
}

fn resolve_cli_path(base: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn resolve_existing_cli_path(base: &Path, raw: &str) -> Result<PathBuf, InitError> {
    let path = resolve_cli_path(base, raw);
    if !path.exists() {
        return Err(InitError::SourcePathNotFound { path });
    }
    path.canonicalize().map_err(InitError::Io)
}

fn resolve_template_path(base: &Path, raw: &str) -> Result<PathBuf, InitError> {
    let path = resolve_cli_path(base, raw);
    if !path.is_dir() {
        return Err(InitError::TemplateNotFound { path });
    }
    path.canonicalize().map_err(InitError::Io)
}

fn storage_root(target_dir: &Path, bare: bool) -> PathBuf {
    if bare {
        target_dir.to_path_buf()
    } else {
        target_dir.join(ROOT_DIR)
    }
}

fn invalid_argument(message: impl Into<String>, hint: Option<String>) -> InitError {
    InitError::InvalidArgument {
        message: message.into(),
        hint,
    }
}

fn resolve_object_format(raw: Option<&str>) -> Result<String, InitError> {
    let object_format = raw
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "sha1".to_string());
    match object_format.as_str() {
        "sha1" | "sha256" => Ok(object_format),
        _ => Err(invalid_argument(
            format!("unsupported object format '{object_format}'"),
            suggest_object_format(&object_format)
                .map(|suggestion| format!("did you mean '{suggestion}'?")),
        )),
    }
}

fn suggest_object_format(value: &str) -> Option<&'static str> {
    (value == "sha265").then_some("sha256")
}

fn is_reinit(target_dir: &Path, bare: bool) -> bool {
    if bare {
        return target_dir.join(DATABASE).exists()
            || target_dir.join("objects").exists()
            || target_dir.join("info").exists()
            || target_dir.join("hooks").exists();
    }
    target_dir.join(ROOT_DIR).exists()
}

fn is_writable(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the target directory is not a directory",
                ));
            }
            if metadata.permissions().readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the target directory is read-only",
                ));
            }
        }
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    Ok(())
}

fn prepare_repository_layout(root_dir: &Path, template_dir: Option<&Path>) -> io::Result<()> {
    if let Some(template_dir) = template_dir {
        copy_template(template_dir, root_dir)?;
    } else {
        for dir in ["info", "hooks"] {
            fs::create_dir_all(root_dir.join(dir))?;
        }
        fs::write(
            root_dir.join("info/exclude"),
            include_str!("../../template/exclude"),
        )?;
        fs::write(
            root_dir.join("hooks").join("pre-commit.sh"),
            include_str!("../../template/pre-commit.sh"),
        )?;
        #[cfg(not(target_os = "windows"))]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(root_dir.join("hooks").join("pre-commit.sh"), perms)?;
        }
        fs::write(
            root_dir.join("hooks").join("pre-commit.ps1"),
            include_str!("../../template/pre-commit.ps1"),
        )?;
    }

    for dir in ["objects/pack", "objects/info"] {
        fs::create_dir_all(root_dir.join(dir))?;
    }
    Ok(())
}

fn copy_template(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_template(&entry.path(), &dest_path)?;
        } else if !dest_path.exists() {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn validate_shared_mode(shared_mode: Option<&str>) -> Result<(), InitError> {
    let Some(shared_mode) = shared_mode else {
        return Ok(());
    };

    match shared_mode {
        "false" | "true" | "umask" | "group" | "all" | "world" | "everybody" => Ok(()),
        mode if mode.starts_with('0') && mode.len() == 4 => {
            u32::from_str_radix(&mode[1..], 8)
                .map_err(|_| invalid_argument(format!("invalid shared mode '{mode}'"), None))?;
            Ok(())
        }
        other => Err(invalid_argument(
            format!("invalid shared mode '{other}'"),
            Some(
                "supported values: umask, group, all, true, false, or a 4-digit octal mode."
                    .to_string(),
            ),
        )),
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_shared(root_dir: &Path, shared_mode: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fn set_recursive(dir: &Path, mode: u32) -> io::Result<()> {
        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::metadata(path)?;
            let mut perms = metadata.permissions();
            perms.set_mode(mode);
            fs::set_permissions(path, perms)?;
        }
        Ok(())
    }

    match shared_mode {
        "false" | "umask" => {}
        "true" | "group" => set_recursive(root_dir, 0o2775)?,
        "all" | "world" | "everybody" => set_recursive(root_dir, 0o2777)?,
        mode if mode.starts_with('0') && mode.len() == 4 => {
            if let Ok(bits) = u32::from_str_radix(&mode[1..], 8) {
                set_recursive(root_dir, bits)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_shared(_root_dir: &Path, _shared_mode: &str) -> io::Result<()> {
    Ok(())
}

fn validate_branch_name(branch_name: &str, ref_format: &RefFormat) -> Result<(), InitError> {
    match ref_format {
        RefFormat::Strict => validate_strict_branch_name(branch_name),
        RefFormat::Filesystem => validate_filesystem_branch_name(branch_name),
    }
}

fn validate_strict_branch_name(branch_name: &str) -> Result<(), InitError> {
    if branch_name.is_empty() {
        return Err(invalid_argument("branch name cannot be empty", None));
    }
    if branch_name.len() > MAX_BRANCH_NAME_LENGTH {
        return Err(invalid_argument(
            format!("branch name is too long (max {MAX_BRANCH_NAME_LENGTH} characters)"),
            None,
        ));
    }
    if branch_name == HEAD_REF {
        return Err(invalid_argument("branch name cannot be 'HEAD'", None));
    }
    if branch_name == AT_REF {
        return Err(invalid_argument("branch name cannot be '@'", None));
    }
    if branch_name.chars().any(|c| {
        c.is_control()
            || c == ' '
            || c == '~'
            || c == '^'
            || c == ':'
            || c == '\\'
            || c == '*'
            || c == '['
            || c == '?'
            || c == '"'
            || c == '@'
            || c == '\0'
    }) {
        return Err(invalid_argument(
            format!("branch name contains invalid characters: {branch_name}"),
            None,
        ));
    }
    if branch_name.starts_with(SLASH) || branch_name.ends_with(SLASH) {
        return Err(invalid_argument(
            "branch name cannot start or end with '/'",
            None,
        ));
    }
    if branch_name.contains(DOUBLE_SLASH) {
        return Err(invalid_argument(
            "branch name cannot contain consecutive slashes",
            None,
        ));
    }
    if branch_name.contains(DOUBLE_DOT) {
        return Err(invalid_argument("branch name cannot contain '..'", None));
    }
    if branch_name.ends_with(LOCK_SUFFIX) {
        return Err(invalid_argument(
            "branch name cannot end with '.lock'",
            None,
        ));
    }
    if branch_name.ends_with(DOT_REF) {
        return Err(invalid_argument("branch name cannot end with '.'", None));
    }
    Ok(())
}

fn validate_filesystem_branch_name(branch_name: &str) -> Result<(), InitError> {
    if branch_name.is_empty() {
        return Err(invalid_argument("branch name cannot be empty", None));
    }
    if branch_name.len() > MAX_BRANCH_NAME_LENGTH {
        return Err(invalid_argument(
            format!("branch name is too long (max {MAX_BRANCH_NAME_LENGTH} characters)"),
            None,
        ));
    }
    if branch_name.chars().any(|c| {
        c.is_control()
            || c == '<'
            || c == '>'
            || c == ':'
            || c == '"'
            || c == '|'
            || c == '?'
            || c == '*'
            || c == '\0'
            || (cfg!(windows) && (c == '\\' || c == '/' || c == '\n' || c == '\r'))
    }) {
        return Err(invalid_argument(
            format!("branch name contains filesystem-invalid characters: {branch_name}"),
            None,
        ));
    }
    if branch_name == DOT_REF || branch_name == DOUBLE_DOT_REF {
        return Err(invalid_argument("branch name cannot be '.' or '..'", None));
    }
    Ok(())
}

async fn create_database_connection(database: &Path) -> Result<DbConn, InitError> {
    #[cfg(target_os = "windows")]
    {
        let database = database
            .to_str()
            .ok_or_else(|| InitError::InvalidUtf8Path {
                path: database.to_path_buf(),
            })?
            .replace('\\', "/");
        db::create_database(&database).await.map_err(InitError::Io)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let database = database
            .to_str()
            .ok_or_else(|| InitError::InvalidUtf8Path {
                path: database.to_path_buf(),
            })?;
        db::create_database(database).await.map_err(InitError::Io)
    }
}

async fn initialize_refs(conn: &DbConn, initial_branch_name: &str) -> Result<(), InitError> {
    reference::ActiveModel {
        name: Set(Some(initial_branch_name.to_string())),
        kind: Set(reference::ConfigKind::Head),
        ..Default::default()
    }
    .insert(conn)
    .await?;

    reference::ActiveModel {
        name: Set(Some(crate::internal::branch::INTENT_BRANCH.to_string())),
        kind: Set(reference::ConfigKind::Branch),
        commit: Set(None),
        remote: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await?;

    // CEX-EntireIO Phase 1.7: register the parallel orphan branch used by the
    // external-agent capture subsystem. Mirrors the `intent` row above; the
    // first checkpoint commit will fill in its `commit` column via the same
    // `HistoryManager::create_append_commit` machinery used by `intent`.
    reference::ActiveModel {
        name: Set(Some(
            crate::internal::branch::AGENT_TRACES_BRANCH.to_string(),
        )),
        kind: Set(reference::ConfigKind::Branch),
        commit: Set(None),
        remote: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await?;

    Ok(())
}

async fn init_config(
    conn: &DbConn,
    is_bare: bool,
    object_format: &str,
    ref_format: &RefFormat,
) -> Result<String, DbErr> {
    let txn = conn.begin().await?;

    #[cfg(not(target_os = "windows"))]
    let entries = [
        ("repositoryformatversion", "0"),
        ("filemode", "true"),
        ("bare", if is_bare { "true" } else { "false" }),
        ("logallrefupdates", "true"),
    ];

    #[cfg(target_os = "windows")]
    let entries = [
        ("repositoryformatversion", "0"),
        ("filemode", "false"),
        ("bare", if is_bare { "true" } else { "false" }),
        ("logallrefupdates", "true"),
        ("symlinks", "false"),
        ("ignorecase", "true"),
    ];

    let repo_id = uuid::Uuid::new_v4().to_string();

    for (key, value) in &entries {
        ConfigKv::set_with_conn(&txn, &format!("core.{key}"), value, false)
            .await
            .map_err(|error| DbErr::Custom(error.to_string()))?;
    }
    ConfigKv::set_with_conn(&txn, "core.objectformat", object_format, false)
        .await
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    ConfigKv::set_with_conn(&txn, "core.initrefformat", ref_format.as_str(), false)
        .await
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    ConfigKv::set_with_conn(&txn, "libra.repoid", &repo_id, false)
        .await
        .map_err(|error| DbErr::Custom(error.to_string()))?;

    txn.commit().await?;
    Ok(repo_id)
}

#[cfg(target_os = "windows")]
fn set_dir_hidden(dir: &Path) -> io::Result<()> {
    use std::process::Command;

    let dir = dir.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("path '{}' is not valid UTF-8", dir.display()),
        )
    })?;
    Command::new("attrib").arg("+H").arg(dir).spawn()?.wait()?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_dir_hidden(_dir: &Path) -> io::Result<()> {
    Ok(())
}

async fn init_vault_for_repo(root_dir: &Path, database_path: &Path) -> Result<(), InitError> {
    use crate::internal::vault;

    let identity_sources =
        resolve_user_identity_sources(LocalIdentityTarget::ExplicitDb(database_path))
            .await
            .map_err(|error| InitError::VaultInitializationFailed {
                message: format!("{error:#}"),
            })?;
    let user_name = identity_sources
        .config_name
        .or(identity_sources.env_name)
        .unwrap_or_else(|| "Libra User".to_string());
    let user_email = identity_sources
        .config_email
        .or(identity_sources.env_email)
        .unwrap_or_else(|| "user@libra.local".to_string());

    let (unseal_key, enc_token) = vault::init_vault(root_dir).await.map_err(|error| {
        InitError::VaultInitializationFailed {
            message: format!("{error:#}"),
        }
    })?;

    if let Err(error) = vault::store_credentials(&unseal_key, &enc_token).await {
        rollback_failed_vault_init(root_dir).await;
        return Err(InitError::VaultInitializationFailed {
            message: format!("{error:#}"),
        });
    }

    if let Err(error) =
        vault::generate_pgp_key(root_dir, &unseal_key, &user_name, &user_email).await
    {
        rollback_failed_vault_init(root_dir).await;
        return Err(InitError::VaultInitializationFailed {
            message: format!("{error:#}"),
        });
    }

    set_vault_signing_value(database_path, true).await
}

async fn set_vault_signing_value(database_path: &Path, enabled: bool) -> Result<(), InitError> {
    let conn = get_db_conn_instance_for_path(database_path)
        .await
        .map_err(InitError::Io)?;
    ConfigKv::set_with_conn(
        &conn,
        "vault.signing",
        if enabled { "true" } else { "false" },
        false,
    )
    .await
    .map_err(|error| InitError::VaultInitializationFailed {
        message: format!("{error:#}"),
    })
}

async fn rollback_failed_vault_init(root_dir: &Path) {
    use crate::internal::vault;

    vault::remove_credentials().await;

    for suffix in ["", "-wal", "-shm"] {
        let path = root_dir.join(format!("vault.db{suffix}"));
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                "failed to remove partially initialized vault database '{}': {}",
                path.display(),
                error
            );
        }
    }
}

fn detect_system_ssh_key() -> Option<String> {
    let home = dirs::home_dir()?;
    let ssh_dir = home.join(".ssh");
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = ssh_dir.join(name);
        if path.exists() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        path::PathBuf,
    };

    use gag::BufferRedirect;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::{DEFAULT_BRANCH, InitArgs, InitError, run_init};
    use crate::utils::test::{self, ChangeDirGuard};

    /// Test scenario: Verify that InitError display trait correctly renders all error variants.
    /// This test ensures that error messages are human-readable and preserve all context
    /// information (path, message, stage) needed for debugging.
    ///
    /// 测试场景：验证 InitError 显示 trait 正确呈现所有错误变体。
    /// 此测试确保错误消息可被人类读取，并保留调试所需的所有上下文信息
    /// （路径、消息、阶段）。
    #[test]
    fn init_error_display_pins_owned_variants() {
        assert_eq!(
            InitError::InvalidArgument {
                message: "missing target".to_string(),
                hint: Some("provide a path".to_string()),
            }
            .to_string(),
            "missing target",
        );
        assert_eq!(
            InitError::AlreadyInitialized {
                path: PathBuf::from("/tmp/repo"),
            }
            .to_string(),
            "repository already initialized at '/tmp/repo'",
        );
        assert_eq!(
            InitError::SourcePathNotFound {
                path: PathBuf::from("/missing/repo"),
            }
            .to_string(),
            "source git repository '/missing/repo' does not exist",
        );
        assert_eq!(
            InitError::InvalidGitRepository {
                path: PathBuf::from("/tmp/not-git"),
            }
            .to_string(),
            "'/tmp/not-git' is not a valid Git repository",
        );
        assert_eq!(
            InitError::TemplateNotFound {
                path: PathBuf::from("/tmp/template"),
            }
            .to_string(),
            "template directory '/tmp/template' does not exist",
        );
        assert_eq!(
            InitError::InvalidUtf8Path {
                path: PathBuf::from("/tmp/utf8"),
            }
            .to_string(),
            "path '/tmp/utf8' is not valid UTF-8",
        );
        assert_eq!(
            InitError::ConversionFailed {
                repo: PathBuf::from("/tmp/source"),
                stage: "objects",
                message: "missing pack".to_string(),
            }
            .to_string(),
            "conversion from git repository '/tmp/source' failed during objects: missing pack",
        );
        assert_eq!(
            InitError::VaultInitializationFailed {
                message: "no keyring".to_string(),
            }
            .to_string(),
            "vault initialization failed: no keyring",
        );
    }

    /// Test scenario: Verify that run_init does not emit progress or summary output to stdout/stderr
    /// when called by internal callers. This ensures the function remains composable in library contexts
    /// where output would be unwanted. Tests three output scenarios: progress messages, summary messages,
    /// and correct return metadata despite silent execution.
    ///
    /// 测试场景：验证当由内部调用者调用时，run_init 不会向 stdout/stderr 发出进度或摘要输出。
    /// 这确保该函数在不需要输出的库上下文中保持可组合性。测试三种输出场景：
    /// 进度消息、摘要消息，以及尽管执行无声但仍然返回正确的元数据。
    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn run_init_is_silent_for_internal_callers() {
        let repo = tempdir().expect("failed to create temp repo");
        test::setup_clean_testing_env_in(repo.path());
        let _guard = ChangeDirGuard::new(repo.path());

        let mut stdout = BufferRedirect::stdout().expect("failed to redirect stdout");
        let mut stderr = BufferRedirect::stderr().expect("failed to redirect stderr");

        let result = run_init(InitArgs {
            bare: false,
            template: None,
            initial_branch: None,
            repo_directory: ".".to_string(),
            quiet: false,
            shared: None,
            object_format: None,
            ref_format: None,
            from_git_repository: None,
            vault: false,
        })
        .await
        .expect("run_init should succeed without rendering side effects");

        std::io::stdout()
            .flush()
            .expect("failed to flush captured stdout");
        std::io::stderr()
            .flush()
            .expect("failed to flush captured stderr");

        let mut captured_stdout = String::new();
        stdout
            .read_to_string(&mut captured_stdout)
            .expect("failed to read captured stdout");

        let mut captured_stderr = String::new();
        stderr
            .read_to_string(&mut captured_stderr)
            .expect("failed to read captured stderr");

        assert_eq!(result.initial_branch, DEFAULT_BRANCH);
        assert!(!result.vault_signing);
        assert!(
            !captured_stdout.contains("Initialized empty ")
                && !captured_stdout.contains("branch: ")
                && !captured_stdout.contains("signing: "),
            "run_init must not render init summary to stdout for internal callers, got: {captured_stdout:?}"
        );
        assert!(
            !captured_stderr.contains("Creating repository layout ...")
                && !captured_stderr.contains("Initializing database ...")
                && !captured_stderr.contains("Setting up refs ...")
                && !captured_stderr.contains("Converting from Git repository")
                && !captured_stderr.contains("Generating PGP signing key ..."),
            "run_init must not render init progress to stderr for internal callers, got: {captured_stderr:?}"
        );
    }
}
