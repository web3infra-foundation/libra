//! Tests config command read/write behaviors, scope handling, and edge cases.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//
use std::process::Command;

use libra::{CliErrorKind, command::config, exec_async};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Guard for temporarily setting an environment variable during a test and restoring it on drop.
///
/// # Safety
/// Modifying environment variables is process-global state. These tests are all annotated with
/// `#[serial]`, ensuring no concurrent mutation happens across tests.
struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

#[tokio::test]
#[serial]
async fn test_cli_config_global_without_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli.db"));

    let result = exec_async(vec!["config", "--global", "user.name", "cli_global_user"]).await;
    assert!(result.is_ok());

    let read_result = exec_async(vec!["config", "--global", "--get", "user.name"]).await;
    assert!(read_result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_cli_config_list_global_without_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped =
        ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli_list.db"));

    let result = exec_async(vec!["config", "--list", "--global"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_cli_config_system_returns_error() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped =
        ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli_sys.db"));

    // --system scope is removed and should always error
    let result = exec_async(vec!["config", "--system", "user.name", "cli_system_user"]).await;
    assert!(result.is_err(), "--system should be rejected");

    let result = exec_async(vec!["config", "--system", "--get", "user.name"]).await;
    assert!(result.is_err(), "--system --get should be rejected");

    let result = exec_async(vec!["config", "--list", "--system"]).await;
    assert!(result.is_err(), "--system --list should be rejected");
}

#[tokio::test]
#[serial]
async fn test_cli_config_local_requires_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let result = exec_async(vec!["config", "--local", "--list"]).await;
    let err = result.unwrap_err();
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert!(err.message().contains("not a libra repository"));
}

#[tokio::test]
#[serial]
async fn test_config_system_scope_is_rejected_as_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(&["config", "--system", "list"], temp_path.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--system scope is not supported"),
        "stderr should describe the unsupported scope, got: {stderr}"
    );
    // config.md line 175: classifies as a CLI usage error (exit 2 fine /
    // 129 coarse). The previous `from_legacy_string` path collapsed this
    // to a generic failure (exit 128).
    assert_eq!(
        output.status.code(),
        Some(129),
        "--system rejection must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_import_global_from_git() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_import.db"));

    let fake_home = tempdir().unwrap();
    let _home_guard = EnvVarGuard::set("HOME", fake_home.path().as_os_str());
    let _xdg_guard = EnvVarGuard::set(
        "XDG_CONFIG_HOME",
        fake_home.path().join(".config").as_os_str(),
    );

    let set_name = Command::new("git")
        .args(["config", "--global", "user.name", "Git Global Import User"])
        .output()
        .unwrap();
    assert!(set_name.status.success());

    let set_email = Command::new("git")
        .args([
            "config",
            "--global",
            "user.email",
            "git-global-import@example.com",
        ])
        .output()
        .unwrap();
    assert!(set_email.status.success());

    let result = exec_async(vec!["config", "--global", "import"]).await;
    assert!(result.is_ok());

    let imported_name = config::ScopedConfig::get(config::ConfigScope::Global, "user.name")
        .await
        .unwrap();
    let imported_email = config::ScopedConfig::get(config::ConfigScope::Global, "user.email")
        .await
        .unwrap();
    assert_eq!(
        imported_name.map(|e| e.value).as_deref(),
        Some("Git Global Import User")
    );
    assert_eq!(
        imported_email.map(|e| e.value).as_deref(),
        Some("git-global-import@example.com")
    );
}

#[tokio::test]
#[serial]
async fn test_config_import_local_from_git_repository() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    use libra::internal::config::ConfigKv;
    ConfigKv::unset_all("user.name").await.unwrap();
    ConfigKv::unset_all("user.email").await.unwrap();

    let git_init = Command::new("git").args(["init"]).output().unwrap();
    assert!(git_init.status.success());

    let set_name = Command::new("git")
        .args(["config", "user.name", "Git Local Import User"])
        .output()
        .unwrap();
    assert!(set_name.status.success());

    let set_email = Command::new("git")
        .args(["config", "user.email", "git-local-import@example.com"])
        .output()
        .unwrap();
    assert!(set_email.status.success());

    let result = exec_async(vec!["config", "import"]).await;
    assert!(result.is_ok());

    let imported_names: Vec<String> =
        config::ScopedConfig::get_all(config::ConfigScope::Local, "user.name")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.value)
            .collect();
    let imported_emails: Vec<String> =
        config::ScopedConfig::get_all(config::ConfigScope::Local, "user.email")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.value)
            .collect();
    assert!(imported_names.iter().any(|v| v == "Git Local Import User"));
    assert!(
        imported_emails
            .iter()
            .any(|v| v == "git-local-import@example.com")
    );
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        match &self.original {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Sets `LIBRA_CONFIG_GLOBAL_DB` to point at a temp file for isolation.
///
/// This prevents tests from touching real host paths like `~/.libra/config.db`.
struct ScopedConfigPathGuard {
    _global: EnvVarGuard,
}

impl ScopedConfigPathGuard {
    fn new(global_db_path: &std::path::Path) -> Self {
        let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());
        Self { _global }
    }
}

#[tokio::test]
#[serial]
async fn test_config_get_failed() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // --default with --add (no --get or --get-all) should error
    let result = exec_async(vec![
        "config",
        "--add",
        "-d",
        "erasernoob",
        "user.name",
        "value",
    ])
    .await;
    assert!(result.is_err());
}

#[tokio::test]
#[serial]
async fn test_config_get_all() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_all_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let result = exec_async(vec!["config", "--get-all", "-d", "erasernoob", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let result = exec_async(vec!["config", "--get", "-d", "erasernoob", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_list() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "--add",
        "user.email",
        "erasernoob@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // List configs
    let result = exec_async(vec!["config", "--list"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_list_name_only() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "--add",
        "user.email",
        "erasernoob@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // List configs with name_only via subcommand
    let result = exec_async(vec!["config", "list", "--name-only"]).await;
    assert!(result.is_ok());
}

// New tests for scope functionality
#[tokio::test]
#[serial]
async fn test_config_scope_local_default() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Test that no scope specified defaults to local
    let result = exec_async(vec!["config", "user.name", "test_user_local_default"]).await;
    assert!(result.is_ok());

    // Verify the value was written to local scope by reading it back
    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_global() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config.db"));

    // Set a value in global scope
    let result = exec_async(vec![
        "config",
        "--global",
        "user.email",
        "global_user@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // Verify the value was written to global scope by reading it back
    let result = exec_async(vec!["config", "--global", "--get", "user.email"]).await;
    assert!(result.is_ok());

    // Verify that the global value is NOT accessible from local scope
    let result = exec_async(vec![
        "config",
        "--local",
        "--get",
        "-d",
        "not_found",
        "user.email",
    ])
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_system_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // --system scope is removed and should always error
    let result = exec_async(vec!["config", "--system", "user.name", "system_user"]).await;
    assert!(result.is_err(), "--system should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.message().contains("--system scope is not supported"),
        "unexpected error: {}",
        err.message()
    );
}

#[tokio::test]
#[serial]
async fn test_config_scope_explicit_local() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Set a value explicitly in local scope
    let result = exec_async(vec![
        "config",
        "--local",
        "user.name",
        "explicit_local_user",
    ])
    .await;
    assert!(result.is_ok());

    // Verify the value was written to local scope by reading it back
    let result = exec_async(vec!["config", "--local", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_isolation() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config.db"));

    // Set the same key with different values in different scopes
    let result = exec_async(vec!["config", "--local", "test.isolation", "local_value"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--global", "test.isolation", "global_value"]).await;
    assert!(result.is_ok());

    // Verify that each scope returns its own value
    println!("Reading from local scope:");
    let result = exec_async(vec!["config", "--local", "--get", "test.isolation"]).await;
    assert!(result.is_ok());

    println!("Reading from global scope:");
    let result = exec_async(vec!["config", "--global", "--get", "test.isolation"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_reveal_decrypt_failure_returns_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::vault::lazy_init_vault_for_scope("local")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::set("vault.env.TEST_SECRET", "not-valid-hex", true)
        .await
        .unwrap();

    let result = exec_async(vec!["config", "get", "--reveal", "vault.env.TEST_SECRET"]).await;
    let err = result.expect_err("decrypt failure should surface as an error");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(
        err.message()
            .contains("failed to decrypt value for key 'vault.env.TEST_SECRET'")
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_cascaded_global_read_failure_returns_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let bad_global_db = temp_path.path().join("bad-global.db");
    std::fs::write(&bad_global_db, "definitely-not-a-sqlite-database").unwrap();
    let _scoped = ScopedConfigPathGuard::new(&bad_global_db);

    let result = exec_async(vec!["config", "get", "user.missing"]).await;
    let err = result.expect_err("broken cascaded scope should not be ignored");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(err.message().contains("failed to read global config"));
}

#[tokio::test]
#[serial]
async fn test_config_add_rejects_implicit_encryption_mixed_with_existing_plaintext() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let result = exec_async(vec![
        "config",
        "set",
        "--plaintext",
        "custom.token",
        "plaintext-token",
    ])
    .await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "set",
        "--add",
        "custom.token",
        "second-token",
    ])
    .await;
    let err = result.expect_err("implicit auto-encryption should not mix with plaintext values");
    assert!(
        err.message()
            .contains("cannot mix encrypted and plaintext values for the same key"),
        "unexpected error: {}",
        err.message()
    );

    let entries = config::ScopedConfig::get_all(config::ConfigScope::Local, "custom.token")
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "mixed-state insert should be rejected");
    assert!(
        !entries[0].encrypted,
        "original plaintext entry should remain"
    );
    assert_eq!(entries[0].value, "plaintext-token");
}

#[tokio::test]
#[serial]
async fn test_config_set_encrypt_plaintext_mutex_is_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "set",
            "--encrypt",
            "--plaintext",
            "custom.token",
            "value",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--encrypt and --plaintext are mutually exclusive"),
        "stderr should describe the mutex violation, got: {stderr}"
    );
    // config.md line 77: classified as a usage error (exit 2 fine / 129 coarse).
    assert_eq!(
        output.status.code(),
        Some(129),
        "mutex flag error must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_stdin_with_positional_value_is_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "set", "--stdin", "custom.token", "value"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot use both value argument and --stdin"),
        "stderr should describe the --stdin vs positional mutex, got: {stderr}"
    );
    // config.md line 144: usage error (exit 2 fine / 129 coarse).
    assert_eq!(
        output.status.code(),
        Some(129),
        "--stdin + positional must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_plaintext_on_vault_internal_key_is_failure() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "set",
            "--plaintext",
            "vault.env.API_KEY",
            "secret-value",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--plaintext cannot be used with vault internal/secret keys"),
        "stderr should describe the secret-key plaintext reject, got: {stderr}"
    );
    // config.md line 77: validation reject (exit 1 fine / 128 coarse) — must
    // classify as a runtime Failure (exit 128) rather than the previous
    // legacy-string fallthrough that produced the same number but with the
    // internal-invariant stable code.
    assert_eq!(
        output.status.code(),
        Some(128),
        "vault internal key plaintext reject must classify as Failure (exit 128), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_read_failure_does_not_silently_skip_existing_state_check() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    // Prevent any interactive prompts from blocking the test.
    let _test_env = EnvVarGuard::set("LIBRA_TEST", std::ffi::OsStr::new("1"));

    let bad_global_dir = tempdir().unwrap();
    let bad_global_db = bad_global_dir.path().join("bad-global.db");
    std::fs::write(&bad_global_db, "definitely-not-a-sqlite-database").unwrap();
    let _scoped = ScopedConfigPathGuard::new(&bad_global_db);

    let fake_home = tempdir().unwrap();
    let _home_guard = EnvVarGuard::set("HOME", fake_home.path().as_os_str());
    let _userprofile_guard = EnvVarGuard::set("USERPROFILE", fake_home.path().as_os_str());

    let result = exec_async(vec![
        "config",
        "set",
        "--global",
        "vault.env.TEST_SECRET",
        "super-secret",
    ])
    .await;
    let err = result.expect_err("broken config read should surface before write/lazy-init");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(
        err.message()
            .contains("failed to read global config while checking existing values"),
        "unexpected error: {}",
        err.message()
    );

    assert!(
        !fake_home
            .path()
            .join(".libra")
            .join("vault-unseal-key")
            .exists(),
        "failed existing-state lookup should not trigger global vault lazy init"
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_missing_value_uses_protected_input_when_existing_key_is_encrypted() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    // Prevent rpassword::read_password() from blocking on stdin.
    let _test_env = EnvVarGuard::set("LIBRA_TEST", std::ffi::OsStr::new("1"));

    let result = exec_async(vec![
        "config",
        "set",
        "--encrypt",
        "custom.value",
        "encrypted-value",
    ])
    .await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "set", "custom.value"]).await;
    let err = result.expect_err("existing encrypted state should require protected input");
    assert_eq!(err.exit_code(), 2);
    assert!(
        err.message()
            .contains("missing value for protected key 'custom.value'"),
        "unexpected error: {}",
        err.message()
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_defaults_to_local_scope_without_global_entries() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set("user.name", "local-user", false)
        .await
        .unwrap();

    let child_home = temp_path.path().join(".libra-test-home");
    let child_global_dir = child_home.join(".libra");
    std::fs::create_dir_all(&child_global_dir).unwrap();
    let child_global_db = child_global_dir.join("config.db");
    let global_conn =
        libra::internal::db::create_database(child_global_db.to_string_lossy().as_ref())
            .await
            .unwrap();
    libra::internal::config::ConfigKv::set_with_conn(&global_conn, "core.editor", "vim", false)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list"], temp_path.path());
    assert!(
        output.status.success(),
        "config list should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("user.name=local-user"),
        "local entry should be listed, stdout: {stdout}"
    );
    assert!(
        !stdout.contains("core.editor"),
        "default list should not include global entries, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_ssh_keys_outputs_configured_public_keys() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set(
        "vault.ssh.origin.pubkey",
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC origin-key",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set(
        "vault.ssh.upstream.pubkey",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA upstream-key",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set("vault.ssh.origin.privkey", "ciphertext", true)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list", "--ssh-keys"], temp_path.path());
    assert!(
        output.status.success(),
        "config list --ssh-keys should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SSH keys:"), "stdout: {stdout}");
    assert!(stdout.contains("origin"), "stdout: {stdout}");
    assert!(stdout.contains("upstream"), "stdout: {stdout}");
    assert!(
        stdout.contains("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC origin-key"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("ciphertext"),
        "private key entries must not be listed, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_gpg_keys_outputs_configured_key_namespaces() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set(
        "vault.gpg.pubkey",
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\nSIGNING\n-----END PGP PUBLIC KEY BLOCK-----",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set(
        "vault.gpg.encrypt.pubkey",
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\nENCRYPT\n-----END PGP PUBLIC KEY BLOCK-----",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set("vault.signing", "true", false)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list", "--gpg-keys"], temp_path.path());
    assert!(
        output.status.success(),
        "config list --gpg-keys should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("GPG keys:"), "stdout: {stdout}");
    assert!(stdout.contains("signing"), "stdout: {stdout}");
    assert!(stdout.contains("encrypt"), "stdout: {stdout}");
    assert!(
        stdout.contains("vault.gpg.pubkey"),
        "signing pubkey key should be listed, stdout: {stdout}"
    );
    assert!(
        stdout.contains("vault.gpg.encrypt.pubkey"),
        "encrypt pubkey key should be listed, stdout: {stdout}"
    );
    assert!(
        stdout.contains("vault.signing = true"),
        "signing-enabled hint should be listed, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_replaces_vault_generate_ssh_key_flow() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote = run_libra_command(
        &["remote", "add", "origin", "git@github.com:example/repo.git"],
        temp_path.path(),
    );
    assert_cli_success(&remote, "remote add origin");

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "origin"],
        temp_path.path(),
    );
    assert_cli_success(&output, "config generate-ssh-key --remote origin");

    let pubkey = libra::internal::config::ConfigKv::get("vault.ssh.origin.pubkey")
        .await
        .unwrap()
        .expect("config generate-ssh-key should store a public key");
    assert!(
        pubkey.value.starts_with("ssh-rsa "),
        "expected RSA SSH public key, got: {}",
        pubkey.value
    );

    let privkey = libra::internal::config::ConfigKv::get("vault.ssh.origin.privkey")
        .await
        .unwrap()
        .expect("config generate-ssh-key should store an encrypted private key");
    assert!(privkey.encrypted, "private key must stay vault-encrypted");
    assert!(
        !privkey.value.contains("PRIVATE KEY"),
        "private key must not be stored as plaintext"
    );

    let get_output = run_libra_command(
        &["config", "get", "vault.ssh.origin.pubkey"],
        temp_path.path(),
    );
    assert_cli_success(&get_output, "config get vault.ssh.origin.pubkey");
    let stdout = String::from_utf8_lossy(&get_output.stdout);
    assert!(stdout.contains("ssh-rsa "), "stdout: {stdout}");
}

#[tokio::test]
#[serial]
async fn test_config_generate_global_ssh_key_is_rejected_without_local_side_effects() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote = run_libra_command(
        &["remote", "add", "origin", "git@github.com:example/repo.git"],
        temp_path.path(),
    );
    assert_cli_success(&remote, "remote add origin");

    libra::internal::config::ConfigKv::unset_all("vault.ssh.origin.pubkey")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::unset_all("vault.ssh.origin.privkey")
        .await
        .unwrap();

    let output = run_libra_command(
        &[
            "config",
            "--global",
            "generate-ssh-key",
            "--remote",
            "origin",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("generate-ssh-key only supports local scope"),
        "stderr should explain unsupported global SSH key generation, got: {stderr}"
    );
    assert!(
        stderr.contains("run without --global"),
        "stderr should tell users how to run the supported form, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "global generate-ssh-key should be a command usage error, got status: {:?}, stderr: {stderr}",
        output.status,
    );

    assert!(
        libra::internal::config::ConfigKv::get("vault.ssh.origin.pubkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-ssh-key must not write a local public key"
    );
    assert!(
        libra::internal::config::ConfigKv::get("vault.ssh.origin.privkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-ssh-key must not write a local private key"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_rejects_invalid_remote_name_as_command_usage() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "bad.name"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid remote name 'bad.name'"),
        "stderr should describe the validation failure, got: {stderr}"
    );
    // CLI usage errors map to exit code 129 in coarse mode (Cli category →
    // CliExitCode::Usage). The previous implementation collapsed both the
    // invalid-name and missing-remote branches into `failure` (exit 128),
    // which is the wrong category for a user-supplied bad argument.
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid remote name must classify as a CLI usage error (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_rejects_unknown_remote_with_invalid_target_code() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "no-such-remote"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("remote 'no-such-remote' not found"),
        "stderr should describe the missing remote, got: {stderr}"
    );
    // Missing remote is a Fatal failure (exit 128 in coarse mode) — the
    // user-supplied name passed validation but the resource does not exist
    // at the time of execution.
    assert_eq!(
        output.status.code(),
        Some(128),
        "unknown remote must classify as a fatal failure (exit 128), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_gpg_key_replaces_vault_generate_gpg_key_flow() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "generate-gpg-key",
            "--name",
            "Config User",
            "--email",
            "config@example.com",
        ],
        temp_path.path(),
    );
    assert_cli_success(&output, "config generate-gpg-key");

    let pubkey = libra::internal::config::ConfigKv::get("vault.gpg.pubkey")
        .await
        .unwrap()
        .expect("config generate-gpg-key should store the signing public key");
    assert!(
        pubkey.value.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "expected armored PGP public key, got: {}",
        pubkey.value
    );

    let generated_stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        generated_stdout.contains("Config User <config@example.com>"),
        "expected configured user ID in command output, stdout: {generated_stdout}"
    );

    let signing = libra::internal::config::ConfigKv::get("vault.signing")
        .await
        .unwrap()
        .expect("signing key generation should enable vault signing");
    assert_eq!(signing.value, "true");

    let get_output = run_libra_command(&["config", "get", "vault.gpg.pubkey"], temp_path.path());
    assert_cli_success(&get_output, "config get vault.gpg.pubkey");
    let stdout = String::from_utf8_lossy(&get_output.stdout);
    assert!(
        stdout.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_global_gpg_key_is_rejected_without_local_side_effects() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::unset_all("vault.gpg.pubkey")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::unset_all("vault.signing")
        .await
        .unwrap();

    let output = run_libra_command(
        &[
            "config",
            "--global",
            "generate-gpg-key",
            "--name",
            "Global User",
            "--email",
            "global@example.com",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("generate-gpg-key only supports local scope"),
        "stderr should explain unsupported global GPG key generation, got: {stderr}"
    );
    assert!(
        stderr.contains("run without --global"),
        "stderr should tell users how to run the supported form, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "global generate-gpg-key should be a command usage error, got status: {:?}, stderr: {stderr}",
        output.status,
    );

    assert!(
        libra::internal::config::ConfigKv::get("vault.gpg.pubkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-gpg-key must not write a local GPG public key"
    );
    assert!(
        libra::internal::config::ConfigKv::get("vault.signing")
            .await
            .unwrap()
            .is_none(),
        "--global generate-gpg-key must not enable local vault signing"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_gpg_key_rejects_invalid_usage() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let output = run_libra_command(
        &["config", "generate-gpg-key", "--usage", "archive"],
        temp_path.path(),
    );
    assert!(
        !output.status.success(),
        "generate-gpg-key should reject unsupported usage"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value 'archive'"),
        "stderr should explain invalid usage, stderr: {stderr}"
    );
    assert!(
        stderr.contains("signing") && stderr.contains("encrypt"),
        "stderr should list supported usages, stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_scope_path_logic() {
    // Test the path logic for different scopes without executing config operations

    // Local scope should return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope should return a path in the home directory (if available)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));
    } else {
        // In environments without home directory, should return None
        assert_eq!(global_path, None);
    }
}

#[tokio::test]
#[serial]
async fn test_config_cross_platform_paths() {
    // Test that all scopes return appropriate paths for the current platform

    // Local scope should always return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope behavior (should work on all platforms with home directory)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));

        // Verify the path uses the correct separator for the platform
        #[cfg(windows)]
        {
            // On Windows, paths should use backslashes or be properly normalized
            let path_str = path.to_string_lossy();
            assert!(path_str.contains("libra") && path_str.contains("config.db"));
        }
        #[cfg(unix)]
        {
            // On Unix, paths should use forward slashes
            assert!(path.to_string_lossy().contains("/"));
        }
    }
}

/// Regression: a corrupted/incompatible `~/.libra/config.db` must not block
/// identity resolution.
///
/// Reproduced from a real 0.17.500 user report: `libra clone` aborted with
/// "fatal: vault initialization failed: failed to open config database
/// '/home/eli/.libra/config.db'" because the global config DB existed but
/// could not be opened (the only fix path was to delete the file). After
/// v0.17.515 `resolve_user_identity_sources` downgrades that failure to a
/// warning and returns `Ok` with `config_*` set to `None`, letting init
/// fall back to env vars / "Libra User" defaults.
#[tokio::test]
#[serial]
async fn resolve_user_identity_sources_tolerates_corrupt_global_db() {
    use libra::internal::config::{LocalIdentityTarget, resolve_user_identity_sources};

    let temp_dir = tempdir().unwrap();
    let global_db_path = temp_dir.path().join("corrupt_config.db");
    // A non-SQLite payload: opening this file as a sea-orm SQLite connection
    // (or running the schema-compat check on it) is guaranteed to fail.
    std::fs::write(&global_db_path, b"this is not a sqlite database").unwrap();

    let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());

    // Ensure env-var fallbacks are empty so we can attribute the result to
    // config-read tolerance, not env shadowing.
    let _git_committer_name = EnvVarGuard::set("GIT_COMMITTER_NAME", std::ffi::OsStr::new(""));
    let _git_committer_email = EnvVarGuard::set("GIT_COMMITTER_EMAIL", std::ffi::OsStr::new(""));
    let _git_author_name = EnvVarGuard::set("GIT_AUTHOR_NAME", std::ffi::OsStr::new(""));
    let _git_author_email = EnvVarGuard::set("GIT_AUTHOR_EMAIL", std::ffi::OsStr::new(""));
    let _email = EnvVarGuard::set("EMAIL", std::ffi::OsStr::new(""));
    let _libra_committer_name = EnvVarGuard::set("LIBRA_COMMITTER_NAME", std::ffi::OsStr::new(""));
    let _libra_committer_email =
        EnvVarGuard::set("LIBRA_COMMITTER_EMAIL", std::ffi::OsStr::new(""));

    let sources = resolve_user_identity_sources(LocalIdentityTarget::None)
        .await
        .expect("identity resolution must not propagate global DB read failures");

    assert!(
        sources.config_name.is_none(),
        "expected config_name to be None when global DB is unreadable, got {:?}",
        sources.config_name
    );
    assert!(
        sources.config_email.is_none(),
        "expected config_email to be None when global DB is unreadable, got {:?}",
        sources.config_email
    );
}

/// `resolve_env_for_target` is the shared secret resolver used by provider,
/// D1, R2, and tool credential paths. Per the 12-Factor / docs/improvement/
/// config.md spec, the priority is **process env > local vault > global vault**
/// so a per-process override like `GEMINI_API_KEY=B libra push` always wins.
/// Local vault is the fallback when env is unset.
#[tokio::test]
#[serial]
async fn resolve_env_for_target_process_env_overrides_local_vault() {
    use libra::internal::config::{ConfigKv, LocalIdentityTarget, resolve_env_for_target};

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _cwd = test::ChangeDirGuard::new(temp_path.path());

    let _env = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        std::ffi::OsStr::new("env-value"),
    );
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-priority-local.db"),
    );

    ConfigKv::set(
        "vault.env.LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        "vault-value",
        false,
    )
    .await
    .unwrap();

    // env wins; per-process override is sacred (12-Factor).
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        LocalIdentityTarget::CurrentRepo,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("env-value"));

    // …and when the env is unset, the local vault fallback is used.
    drop(_env);
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        LocalIdentityTarget::CurrentRepo,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("vault-value"));
}

/// Same priority chain in the `LocalIdentityTarget::None` mode used by
/// commands that can run outside a Libra worktree (provider/bootstrap path).
/// process env > global vault.
#[tokio::test]
#[serial]
async fn resolve_env_for_target_process_env_overrides_global_vault() {
    use libra::internal::{
        config::{ConfigKv, LocalIdentityTarget, resolve_env_for_target},
        db,
    };

    let _guard = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        std::ffi::OsStr::new("env-value"),
    );
    let global_dir = tempdir().unwrap();
    let global_db_path = global_dir.path().join("global-config.db");
    let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());
    let global_conn = db::create_database(global_db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    ConfigKv::set_with_conn(
        &global_conn,
        "vault.env.LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        "global-vault-value",
        false,
    )
    .await
    .unwrap();

    // env wins.
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        LocalIdentityTarget::None,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("env-value"));

    // …and global vault is the fallback when env is unset.
    drop(_guard);
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        LocalIdentityTarget::None,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("global-vault-value"));
}

/// Process env remains the final fallback when neither local nor global Vault
/// supplies the key.
#[tokio::test]
#[serial]
async fn resolve_env_sync_falls_back_to_process_env_when_vault_missing() {
    use libra::internal::config::resolve_env_sync;

    let _guard = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_SYNC_TEST_KEY",
        std::ffi::OsStr::new("env-fallback"),
    );
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-sync-fallback-path.db"),
    );

    let value = resolve_env_sync("LIBRA_RESOLVE_ENV_SYNC_TEST_KEY").unwrap();
    assert_eq!(value.as_deref(), Some("env-fallback"));
}

/// Absence path: when no process env, no repo, and no global DB layer carries
/// the key, the wrapper returns `Ok(None)` (not an error). A schema-mismatch
/// on the global DB is treated as missing-value here (the underlying
/// `resolve_env_for_target` already downgrades that to `tracing::warn!`),
/// matching the v0.17.515 / v0.17.534 fallback contract.
#[tokio::test]
#[serial]
async fn resolve_env_sync_returns_none_when_no_layer_supplies_value() {
    use libra::internal::config::resolve_env_sync;

    let _guard = EnvVarGuard::unset("LIBRA_RESOLVE_ENV_SYNC_ABSENT_KEY");
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-sync-absent-path.db"),
    );

    let value = resolve_env_sync("LIBRA_RESOLVE_ENV_SYNC_ABSENT_KEY").unwrap();
    assert!(
        value.is_none(),
        "expected None for an unset key, got {value:?}"
    );
}
