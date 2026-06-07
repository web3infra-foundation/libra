//! Initializes a repository by creating .libra storage, seeding HEAD and
//! default refs/config, and preparing the backing database.
//!
//! Error rendering and stable-code expectations are part of the CLI contract:
//! see `docs/development/cli-error-contract-design.md`.

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
    libra init --object-format sha256          Use SHA-256 hashing
    libra init --shared=group                  Share the repo with your Unix group"#;

// NOTE: `src/command/init.rs` lines 3-20 are a protected merge-conflict block in this workspace.
// The imports inside that block must stay as-is. To avoid `unused_imports` warnings without
// changing that block, we reference the imported symbols here in a private, dead-code helper.
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

use crate::{internal::head::Head, utils::ignore};

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
    /// Non-serialized: drives the human banner (`Initialized empty ...` vs
    /// `Reinitialized existing ...`) without changing the `--json`/`--machine`
    /// schema, which is identical for first init and safe re-init.
    #[serde(skip)]
    pub reinitialized: bool,
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

    /// Filesystem sharing mode for the repository (Git's `--shared[=<mode>]`).
    ///
    /// Bare `--shared` defaults to `group`. With a value, use the `=` form:
    /// `--shared=<mode>` where `<mode>` is `umask`/`false`, `group`/`true`,
    /// `all`/`world`/`everybody`, or a 4-digit octal such as `0660`. The
    /// canonical mode is persisted to `core.sharedRepository` and the `.libra/`
    /// content tree is made group/world-shareable on Unix (no-op on Windows,
    /// where permissions follow NTFS ACLs). The vault and the `.libra/` root
    /// directory entry stay owner-only writable to protect signing keys.
    #[clap(
        long,
        required = false,
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "group",
        require_equals = true
    )]
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

    if result.reinitialized {
        println!("Reinitialized existing Libra repository in {}", result.path);
    } else {
        let repo_type = if result.bare { " bare" } else { "" };
        println!(
            "Initialized empty{repo_type} Libra repository in {}",
            result.path
        );
    }
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
        // An existing repository is safely re-initialized (template top-up,
        // `core.sharedRepository`/permission refresh) rather than rejected — but
        // only when no destructive conflict is requested. Identity/refs/vault
        // are preserved; see `run_reinit`.
        return run_reinit(
            &root_dir,
            &args,
            &object_format,
            &ref_format,
            template_dir.as_deref(),
            progress,
        )
        .await;
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
    let conn = create_database_connection(&database_path).await?;
    let repo_id = init_config(
        &conn,
        args.bare,
        &object_format,
        &ref_format,
        args.shared.as_deref(),
    )
    .await?;

    progress.emit("Setting up refs ...");
    // INVARIANT: refs are initialized after core config so HEAD/branch rows are
    // tied to the repository identity and hash/ref-format choices already stored
    // in config.
    initialize_refs(&conn, &initial_branch_name).await?;

    set_dir_hidden(&root_dir)?;

    let mut warnings = Vec::new();
    if !args.bare {
        ignore::ensure_root_libraignore(&target_dir)?;
        // Seed a project-local default skill so that `libra code` / agents
        // immediately have high-quality guidance about this libra-format repo.
        // Existing files are never overwritten.
        if let Err(e) = seed_default_libra_skills(&target_dir) {
            // Non-fatal: the embedded "libra" skill is always available as fallback.
            tracing::warn!(error = %e, "failed to seed default .libra/skills/libra.md");
        }
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
        let _guard = CurrentDirGuard::change_to(&target_guard_path)?;
        init_vault_for_repo(&root_dir, &database_path).await?;
    } else {
        set_vault_signing_value(&database_path, false).await?;
    }

    // Apply `--shared` permissions last so the chmod covers the fully populated
    // layout — including `vault.db` (forced back to owner-only `0o600`), any
    // objects/refs copied during `--from-git-repository` conversion, and the
    // seeded skills — and the `.libra/` root entry can be locked owner-only.
    if let Some(shared_mode) = args.shared.as_deref() {
        apply_shared(&root_dir, shared_mode)?;
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
        reinitialized: false,
    })
}

/// Reads a single `config_kv` value, mapping storage failures to [`InitError`].
async fn read_config_value(conn: &DbConn, key: &str) -> Result<Option<String>, InitError> {
    ConfigKv::get_with_conn(conn, key)
        .await
        .map(|entry| entry.map(|entry| entry.value))
        .map_err(|error| InitError::Database(DbErr::Custom(error.to_string())))
}

/// Installs `contents` at `dest` only when `dest` does not already exist, writing
/// through a sibling temporary file and an atomic `rename` so a crash mid-write
/// never leaves a half-written template. Returns `true` when a file was created.
///
/// `mode` (Unix only) is applied to the temporary file before the rename so the
/// published file already carries the requested permissions.
fn install_missing_file(dest: &Path, contents: &[u8], mode: Option<u32>) -> io::Result<bool> {
    if dest.exists() {
        return Ok(false);
    }
    let parent = dest.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "template destination has no parent directory",
        )
    })?;
    let file_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "template destination has no file name",
            )
        })?;
    let tmp = parent.join(format!(".{file_name}.libra-tmp"));
    fs::write(&tmp, contents)?;
    #[cfg(not(target_os = "windows"))]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(target_os = "windows")]
    let _ = mode;
    if let Err(error) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(true)
}

/// Tops up a repository layout for re-initialization: ensures the structural
/// directories exist and installs any **missing** template files, never
/// overwriting user-modified hooks or excludes.
fn topup_repository_layout(root_dir: &Path, template_dir: Option<&Path>) -> io::Result<()> {
    for dir in ["info", "hooks", "objects/pack", "objects/info"] {
        fs::create_dir_all(root_dir.join(dir))?;
    }

    if let Some(template_dir) = template_dir {
        // `copy_template` already skips destinations that already exist.
        return copy_template(template_dir, root_dir);
    }

    #[cfg(not(target_os = "windows"))]
    let hook_mode = Some(0o755);
    #[cfg(target_os = "windows")]
    let hook_mode = None;

    install_missing_file(
        &root_dir.join("info/exclude"),
        include_str!("../../template/exclude").as_bytes(),
        None,
    )?;
    install_missing_file(
        &root_dir.join("hooks").join("pre-commit.sh"),
        include_str!("../../template/pre-commit.sh").as_bytes(),
        hook_mode,
    )?;
    install_missing_file(
        &root_dir.join("hooks").join("pre-commit.ps1"),
        include_str!("../../template/pre-commit.ps1").as_bytes(),
        None,
    )?;
    Ok(())
}

/// Safely re-initializes an existing Libra repository.
///
/// Contract (see `.omo/plans/init-improvement-plan.md` Batch 2):
/// - Opens — never recreates — the existing database; preserves `libra.repoid`,
///   `vault.db`, and existing refs/HEAD (no `initialize_refs`/vault re-seed).
/// - Rejects destructive conflicts *before any disk mutation*: an explicit
///   `--object-format`/`--ref-format` that disagrees with the stored value
///   fails with `InvalidArgument` (exit 129). Omitted formats are inherited and
///   never treated as a conflict.
/// - Otherwise performs only additive, idempotent updates: missing template
///   top-up, then `core.sharedRepository` upsert (single transaction), then
///   `apply_shared` permission refresh.
async fn run_reinit(
    root_dir: &Path,
    args: &InitArgs,
    requested_object_format: &str,
    requested_ref_format: &RefFormat,
    template_dir: Option<&Path>,
    progress: &InitProgress,
) -> Result<InitOutput, InitError> {
    let database_path = root_dir.join(DATABASE);
    let conn = get_db_conn_instance_for_path(&database_path)
        .await
        .map_err(InitError::Io)?;

    // ── Step 1: validate conflicts first; no disk mutation on failure ──
    let existing_object_format = read_config_value(&conn, "core.objectformat")
        .await?
        .unwrap_or_else(|| "sha1".to_string());
    if args.object_format.is_some() && requested_object_format != existing_object_format {
        return Err(invalid_argument(
            format!(
                "cannot reinitialize with object format '{requested_object_format}': existing repository uses '{existing_object_format}'"
            ),
            Some("omit --object-format to reuse the existing object format.".to_string()),
        ));
    }

    let existing_ref_format = read_config_value(&conn, "core.initrefformat")
        .await?
        .unwrap_or_else(|| RefFormat::Strict.as_str().to_string());
    if args.ref_format.is_some() && requested_ref_format.as_str() != existing_ref_format {
        return Err(invalid_argument(
            format!(
                "cannot reinitialize with ref format '{}': existing repository uses '{existing_ref_format}'",
                requested_ref_format.as_str()
            ),
            Some("omit --ref-format to reuse the existing ref format.".to_string()),
        ));
    }

    // ── Step 2: template top-up (additive, atomic, never overwrites users) ──
    progress.emit("Reinitializing existing repository ...");
    topup_repository_layout(root_dir, template_dir)?;

    // ── Step 3: config update in a single transaction (DB written last) ──
    if let Some(mode) = args.shared.as_deref() {
        let txn = conn.begin().await.map_err(InitError::Database)?;
        ConfigKv::set_with_conn(
            &txn,
            "core.sharedRepository",
            &canonical_shared_value(mode),
            false,
        )
        .await
        .map_err(|error| InitError::Database(DbErr::Custom(error.to_string())))?;
        txn.commit().await.map_err(InitError::Database)?;

        // ── Step 4: permission refresh after the DB commit (idempotent) ──
        apply_shared(root_dir, mode)?;
    }

    // Echo the existing (inherited) repository identity. `--initial-branch` is
    // intentionally not applied on re-init: HEAD is never clobbered.
    let repo_id = read_config_value(&conn, "libra.repoid")
        .await?
        .unwrap_or_default();
    let vault_signing = read_config_value(&conn, "vault.signing")
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    let initial_branch = match Head::current_result_with_conn(&conn).await {
        Ok(Head::Branch(name)) => name,
        _ => DEFAULT_BRANCH.to_string(),
    };

    let mut warnings = Vec::new();
    if let Some(requested) = args.initial_branch.as_deref()
        && requested != initial_branch
    {
        warnings.push(format!(
            "ignored --initial-branch '{requested}': HEAD already points to '{initial_branch}'"
        ));
    }
    if args.from_git_repository.is_some() {
        warnings.push(
            "ignored --from-git-repository: cannot convert into an existing repository".to_string(),
        );
    }

    set_hash_kind(match existing_object_format.as_str() {
        "sha256" => HashKind::Sha256,
        _ => HashKind::Sha1,
    });

    let path = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf())
        .to_string_lossy()
        .to_string();

    Ok(InitOutput {
        path,
        bare: args.bare,
        initial_branch,
        object_format: existing_object_format,
        ref_format: existing_ref_format,
        repo_id,
        vault_signing,
        converted_from: None,
        ssh_key_detected: detect_system_ssh_key(),
        warnings,
        reinitialized: true,
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
            let source = entry.path();
            let contents = fs::read(&source)?;
            install_missing_file(&dest_path, &contents, template_file_mode(&source)?)?;
        }
    }
    Ok(())
}

fn template_file_mode(path: &Path) -> io::Result<Option<u32>> {
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).map(|metadata| Some(metadata.permissions().mode() & 0o777))
    }

    #[cfg(target_os = "windows")]
    {
        let _ = path;
        Ok(None)
    }
}

/// Seeds a default project-local "libra" skill into `.libra/skills/libra.md`
/// for newly initialized (non-bare) repositories.
///
/// Visible to crate tests so we can verify the non-overwrite behavior in isolation.
/// Errors from this helper are treated as non-fatal by the init flow (the
/// embedded "libra" skill in the binary is always available as fallback).
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn seed_default_libra_skills(worktree: &Path) -> io::Result<()> {
    let skills_dir = worktree.join(ROOT_DIR).join("skills");
    fs::create_dir_all(&skills_dir)?;

    let target = skills_dir.join("libra.md");
    if target.exists() {
        return Ok(());
    }

    let content = include_str!("../../template/skills/libra.md");
    fs::write(&target, content)
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

/// Canonicalizes a validated `--shared` mode into the value persisted to
/// `core.sharedRepository`, mirroring Git's normalization so that
/// `libra config get core.sharedRepository` reads back a stable name.
///
/// Aliases collapse as: `false`/`umask` → `umask`, `true`/`group` → `group`,
/// `all`/`world`/`everybody` → `all`. A 4-digit octal mode (e.g. `0660`) is
/// returned verbatim. The single source of truth for this mapping is the
/// canonicalization table in `.omo/plans/init-improvement-plan.md` (Batch 1).
fn canonical_shared_value(mode: &str) -> String {
    match mode {
        "false" | "umask" => "umask".to_string(),
        "true" | "group" => "group".to_string(),
        "all" | "world" | "everybody" => "all".to_string(),
        other => other.to_string(),
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_shared(root_dir: &Path, shared_mode: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Vault files must never be exposed to group/world by `--shared`; they are
    // forced to owner-only `0o600` so other users cannot read signing keys even
    // though the SQLite backend creates `vault.db` with the process umask.
    fn is_vault_file(path: &Path) -> bool {
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("vault.db") | Some("vault.db-wal") | Some("vault.db-shm")
        )
    }

    // Directories must stay searchable wherever they are readable, otherwise an
    // octal mode such as `0660` (no execute bits) would strip the search bit and
    // break traversal of the layout. Mirrors Git's `adjust_shared_perm`, which
    // copies each read bit into the matching execute bit for directories.
    fn dir_mode(mode: u32) -> u32 {
        mode | ((mode & 0o444) >> 2)
    }

    // Applies `content_mode` to everything under `root_dir`, with these rules:
    //   * directories propagate read→execute so the tree stays traversable;
    //   * the `root_dir` entry keeps an owner-only-writable mode (group and world
    //     write bits cleared) so other users cannot unlink/replace the vault file
    //     from the repository root even when the content tree is group-shared;
    //   * vault files are forced to `0o600` regardless of the shared mode.
    // Symlinks are skipped to avoid TOCTOU/permission escapes.
    fn set_recursive(root_dir: &Path, content_mode: u32) -> io::Result<()> {
        // Root is a directory: searchable, but with group/world write stripped.
        let root_mode = dir_mode(content_mode) & !0o022;
        for entry in walkdir::WalkDir::new(root_dir).follow_links(false) {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type().is_symlink() {
                continue;
            }
            let mode = if is_vault_file(path) {
                0o600
            } else if path == root_dir {
                root_mode
            } else if entry.file_type().is_dir() {
                dir_mode(content_mode)
            } else {
                content_mode
            };
            let metadata = fs::symlink_metadata(path)?;
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
    shared_mode: Option<&str>,
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

    // Persist the canonical `--shared` mode so Git-compatible tooling and a
    // later `libra config get core.sharedRepository` can observe the setting.
    // `umask`/`false` still records `umask` (no permission change), matching
    // Git's `core.sharedRepository=umask` default semantics.
    if let Some(mode) = shared_mode {
        ConfigKv::set_with_conn(
            &txn,
            "core.sharedRepository",
            &canonical_shared_value(mode),
            false,
        )
        .await
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    }

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

    /// `run_init` on a fresh non-bare directory must seed `.libra/skills/libra.md`
    /// from the template. This gives every new libra repo a project-local
    /// starting skill for agents.
    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn run_init_seeds_default_libra_skill() {
        use crate::utils::test::{self, ChangeDirGuard};

        let repo = tempdir().expect("failed to create temp repo");
        test::setup_clean_testing_env_in(repo.path());
        let _guard = ChangeDirGuard::new(repo.path());

        let _ = run_init(InitArgs {
            bare: false,
            template: None,
            initial_branch: None,
            repo_directory: ".".to_string(),
            quiet: true,
            shared: None,
            object_format: None,
            ref_format: None,
            from_git_repository: None,
            vault: false,
        })
        .await
        .expect("init must succeed");

        let skill_path = repo.path().join(".libra").join("skills").join("libra.md");
        assert!(
            skill_path.exists(),
            "default skill must be seeded at .libra/skills/libra.md"
        );

        let content = std::fs::read_to_string(&skill_path).expect("read seeded skill");
        assert!(
            content.contains("name = \"libra\""),
            "seeded skill must declare the libra skill name"
        );
        assert!(
            content.contains("This project's libra repository")
                || content.contains("project-local"),
            "seeded skill should contain project-oriented language"
        );
        // Must be a complete valid skill (frontmatter + body)
        assert!(
            content.starts_with("---\nname = \"libra\""),
            "seeded file must start with valid TOML frontmatter"
        );
    }

    /// The seeder helper itself must be idempotent / non-destructive: calling
    /// it when a user-owned skill file already exists must leave the file
    /// completely untouched (this can happen in conversion flows, partial
    /// .libra/ states, or future re-init support).
    #[test]
    fn seed_default_libra_skills_leaves_existing_file_alone() {
        let tmp = tempdir().expect("tmp");
        let worktree = tmp.path();

        let skills_dir = worktree.join(".libra").join("skills");
        std::fs::create_dir_all(&skills_dir).expect("mkdir");
        let skill_path = skills_dir.join("libra.md");
        std::fs::write(&skill_path, "MY CUSTOM SKILL\n---\nnever overwrite").expect("pre-create");

        // First call seeds nothing (file exists)
        super::seed_default_libra_skills(worktree).expect("first call");
        // Second call also does nothing
        super::seed_default_libra_skills(worktree).expect("second call");

        let content = std::fs::read_to_string(&skill_path).expect("read");
        assert_eq!(content, "MY CUSTOM SKILL\n---\nnever overwrite");
    }

    #[test]
    fn canonical_shared_value_collapses_aliases() {
        use super::canonical_shared_value;
        assert_eq!(canonical_shared_value("false"), "umask");
        assert_eq!(canonical_shared_value("umask"), "umask");
        assert_eq!(canonical_shared_value("true"), "group");
        assert_eq!(canonical_shared_value("group"), "group");
        assert_eq!(canonical_shared_value("all"), "all");
        assert_eq!(canonical_shared_value("world"), "all");
        assert_eq!(canonical_shared_value("everybody"), "all");
        // 4-digit octal modes are recorded verbatim.
        assert_eq!(canonical_shared_value("0660"), "0660");
        assert_eq!(canonical_shared_value("0777"), "0777");
    }

    /// `apply_shared` must not modify the permissions of symlink entries inside
    /// the layout (it skips them to avoid TOCTOU/permission escapes), and it
    /// must keep the `.libra/` root entry owner-only writable while making the
    /// content tree group-shareable.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn apply_shared_skips_symlinks_and_protects_root() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tmp");
        let root = tmp.path().join(".libra");
        let content_dir = root.join("objects");
        std::fs::create_dir_all(&content_dir).expect("mkdir objects");

        // An outside file the symlink will point at; its mode must be unchanged.
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"secret").expect("write outside");
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600))
            .expect("chmod outside");

        // A symlink inside the layout pointing at the outside file.
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

        super::apply_shared(&root, "group").expect("apply_shared group");

        let mode = |p: &std::path::Path| {
            std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
        };
        // Root entry: owner-only writable (no group/world write bits).
        assert_eq!(
            mode(&root) & 0o022,
            0,
            ".libra root must stay owner-only writable"
        );
        // Content subtree: group-writable.
        assert_eq!(
            mode(&content_dir) & 0o020,
            0o020,
            "content dir must be group-writable under shared=group"
        );
        // The symlink target's mode must be untouched (symlink skipped).
        assert_eq!(
            mode(&outside) & 0o777,
            0o600,
            "apply_shared must not chmod through a symlink"
        );
    }
}
