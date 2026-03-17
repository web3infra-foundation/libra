//! Test support utilities including change-dir guards, repository setup/cleanup helpers, fixture copying, and isolated command execution helpers.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use tracing::level_filters::LevelFilter;

use crate::{
    command,
    utils::{pager::LIBRA_TEST_ENV, util},
};

pub struct ScopedEnvVar {
    key: String,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    pub fn set(key: impl Into<String>, value: impl AsRef<OsStr>) -> Self {
        let key = key.into();
        let previous = env::var_os(&key);
        // SAFETY: command tests mutate process env only in controlled test flows.
        unsafe {
            env::set_var(&key, value.as_ref());
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: this restores the exact previous value for the same process env key.
        unsafe {
            if let Some(value) = &self.previous {
                env::set_var(&self.key, value);
            } else {
                env::remove_var(&self.key);
            }
        }
    }
}

pub struct ChangeDirGuard {
    old_dir: PathBuf,
}

impl ChangeDirGuard {
    /// Creates a new `ChangeDirGuard` that changes the current directory to `new_dir`.
    /// This will automatically change the directory back to the original one when the guard is dropped.
    ///
    /// However, it **MUST** be used in a single-threaded context.
    ///
    /// # Arguments
    ///
    /// * `new_dir` - The new directory to change to.
    ///
    /// # Returns
    ///
    /// * A `ChangeDirGuard` instance that will change the directory back to the original one when dropped.
    ///
    pub fn new(new_dir: impl AsRef<Path>) -> Self {
        let old_dir = env::current_dir().unwrap_or_else(|_| find_cargo_dir());
        env::set_current_dir(new_dir).unwrap();
        Self { old_dir }
    }
}

impl Drop for ChangeDirGuard {
    fn drop(&mut self) {
        let fallback = find_cargo_dir_opt().unwrap_or_else(std::env::temp_dir);
        let target = if self.old_dir.exists() {
            &self.old_dir
        } else {
            // Temp test directories may already be gone when the guard drops.
            &fallback
        };
        // Silently ignore errors to avoid aborting during stack unwinding.
        let _ = env::set_current_dir(target);
    }
}

/// Returns `Some(path)` to the workspace root (containing `Cargo.toml`),
/// or `None` if it cannot be determined.
fn find_cargo_dir_opt() -> Option<PathBuf> {
    if let Ok(path) = env::var("CARGO_MANIFEST_DIR") {
        return Some(PathBuf::from(path));
    }
    // vscode DEBUG test does not have the CARGO_MANIFEST_DIR macro, manually try to find cargo.toml
    println!("CARGO_MANIFEST_DIR not found, try to find Cargo.toml manually");
    let mut path = util::cur_dir();
    loop {
        path.push("Cargo.toml");
        if path.exists() {
            path.pop();
            return Some(path);
        }
        if !path.pop() || !path.pop() {
            return None;
        }
    }
}

pub fn find_cargo_dir() -> PathBuf {
    find_cargo_dir_opt().expect("Could not find CARGO_MANIFEST_DIR")
}

/// Sets up a clean environment for testing.
///
/// This function first calls `setup_env()` to switch the current directory to the test directory.
/// Then, it checks if the Libra root directory (`.libra`) exists in the current directory.
/// If it does, the function removes the entire `.libra` directory.
pub fn setup_clean_testing_env_in(temp_path: impl AsRef<Path>) {
    mark_test_process_non_interactive();

    assert!(temp_path.as_ref().exists(), "temp_path does not exist");
    assert!(temp_path.as_ref().is_dir(), "temp_path is not a directory");
    assert!(
        temp_path.as_ref().read_dir().unwrap().count() == 0,
        "temp_path is not empty"
    );

    tracing::info!("Using libra testing path: {:?}", temp_path.as_ref());

    // Define the directories that are present in a bare repository
    let owned = temp_path.as_ref().to_path_buf();
    let bare_repo_dirs = ["objects", "info", "description", "libra.db"];

    // Remove the directories that are present in a bare repository if they exist
    for dir in bare_repo_dirs.iter() {
        let bare_repo_path = owned.join(dir);
        if bare_repo_path.exists() && bare_repo_path.is_dir() {
            fs::remove_dir_all(&bare_repo_path).unwrap();
        } else if bare_repo_path.exists() && !bare_repo_path.is_dir() {
            // Remove the file if it exists
            fs::remove_file(&bare_repo_path).unwrap();
        }
    }
}

fn mark_test_process_non_interactive() {
    // SAFETY: command tests set a stable process-wide default before executing CLI code.
    unsafe {
        env::set_var(LIBRA_TEST_ENV, "1");
    }
}

/// switch to test dir and create a new .libra
pub async fn setup_with_new_libra_in(temp_path: impl AsRef<Path>) {
    setup_clean_testing_env_in(temp_path.as_ref());
    let args = command::init::InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: temp_path.as_ref().to_str().unwrap().to_string(),
        template: None,
        quiet: false,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_libra_dir: None,
        from_git_repository: None,
        vault: false,
    };
    command::init::init(args).await.unwrap();

    // Most tests don't exercise identity flows. Seed a deterministic identity so
    // commit-related tests don't depend on host-level config.
    let _guard = ChangeDirGuard::new(temp_path.as_ref());
    crate::internal::config::Config::insert("user", None, "name", "Libra Test User").await;
    crate::internal::config::Config::insert("user", None, "email", "libra-test@example.com").await;
}
/// change the log level to reduce verbose output.
pub fn init_debug_logger() {
    init_logger_with_default_level(LevelFilter::DEBUG);
}

pub fn init_logger() {
    init_logger_with_default_level(LevelFilter::INFO);
}

fn init_logger_with_default_level(default_level: LevelFilter) {
    let effective_level =
        if env::var_os("LIBRA_TEST_LOG").is_some() || env::var_os("RUST_LOG").is_some() {
            default_level
        } else {
            // Keep tests quiet by default; opt in with LIBRA_TEST_LOG=1 when debugging.
            LevelFilter::OFF
        };

    let _ = tracing_subscriber::fmt()
        .with_max_level(effective_level)
        .try_init(); // avoid multi-init
}

/// create file related to working directory
pub fn ensure_file(path: impl AsRef<Path>, content: Option<&str>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap(); // ensure父目录
    let mut file = fs::File::create(util::working_dir().join(path))
        .unwrap_or_else(|_| panic!("Cannot create file：{path:?}"));
    if let Some(content) = content {
        file.write_all(content.as_bytes()).unwrap();
    } else {
        // write filename if no content
        file.write_all(path.file_name().unwrap().as_encoded_bytes())
            .unwrap();
    }
}

/// reset working directory to the root of the module
pub fn reset_working_dir() {
    env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
}
