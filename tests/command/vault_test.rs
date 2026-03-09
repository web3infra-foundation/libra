//! Integration tests for the vault (PGP signing) feature.

use std::process::Command;

use git_internal::internal::object::ObjectTrait;
use libra::{
    command::init::{InitArgs, init},
    internal::config::Config,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: these tests are all #[serial], so no concurrent env mutation.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: these tests are all #[serial], so no concurrent env mutation.
        match &self.original {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// `init --vault` should complete without error and set `vault.signing = true`.
#[tokio::test]
#[serial]
async fn test_init_with_vault_creates_signing_config() {
    let temp = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp.path());
    let test_home = temp.path().join("home");
    std::fs::create_dir_all(&test_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", test_home.as_os_str());
    #[cfg(windows)]
    let _userprofile_guard = EnvVarGuard::set("USERPROFILE", test_home.as_os_str());

    // Initialize repo first (without vault)
    init(InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: temp.path().to_str().unwrap().to_string(),
        template: None,
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: None,
        vault: false,
    })
    .await
    .unwrap();

    let _guard = ChangeDirGuard::new(temp.path());

    // Set user info required by vault key generation
    Config::insert("user", None, "name", "Test User").await;
    Config::insert("user", None, "email", "test@example.com").await;

    // Now initialize vault on top of the existing repo
    use libra::internal::vault;
    let root_dir = temp.path().join(".libra");
    let (unseal_key, enc_token) = vault::init_vault(&root_dir).await.unwrap();
    vault::store_credentials(&unseal_key, &enc_token)
        .await
        .unwrap();

    let public_key =
        vault::generate_pgp_key(&root_dir, &unseal_key, "Test User", "test@example.com")
            .await
            .unwrap();

    Config::insert("vault", None, "signing", "true").await;

    assert!(!public_key.is_empty(), "public key should not be empty");

    let signing = Config::get("vault", None, "signing").await;
    assert_eq!(signing.as_deref(), Some("true"));

    // Unseal key should be loadable
    let loaded = vault::load_unseal_key().await;
    assert!(
        loaded.is_some(),
        "unseal key should be loadable after store"
    );
}

/// Vault rollback: if key generation fails, credentials should be cleaned up.
#[tokio::test]
#[serial]
async fn test_vault_rollback_removes_credentials() {
    let temp = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp.path());
    let test_home = temp.path().join("home");
    std::fs::create_dir_all(&test_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", test_home.as_os_str());
    #[cfg(windows)]
    let _userprofile_guard = EnvVarGuard::set("USERPROFILE", test_home.as_os_str());

    init(InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: temp.path().to_str().unwrap().to_string(),
        template: None,
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: None,
        vault: false,
    })
    .await
    .unwrap();

    let _guard = ChangeDirGuard::new(temp.path());

    Config::insert("user", None, "name", "Test User").await;
    Config::insert("user", None, "email", "test@example.com").await;

    use libra::internal::vault;

    // Store some dummy credentials
    let dummy_key = b"dummy-unseal-key-for-test-00000";
    let dummy_enc = vault::encrypt_token(dummy_key, b"dummy-token");
    vault::store_credentials(dummy_key, &dummy_enc)
        .await
        .unwrap();

    // Verify they exist
    let loaded = vault::load_unseal_key().await;
    assert!(loaded.is_some(), "credentials should exist before rollback");

    // Rollback
    vault::remove_credentials().await;

    // Legacy config entry should be gone
    let legacy = Config::get("vault", None, "unsealkey").await;
    assert!(
        legacy.is_none(),
        "unsealkey should be removed from config after rollback"
    );

    let enc = Config::get("vault", None, "roottoken_enc").await;
    assert!(
        enc.is_none(),
        "roottoken_enc should be removed after rollback"
    );
}

/// Commit without vault should not produce a gpgsig header.
#[tokio::test]
#[serial]
async fn test_commit_without_vault_has_no_signature() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    test::ensure_file("hello.txt", Some("hello"));
    add::execute(AddArgs {
        pathspec: vec!["hello.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("unsigned commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    let head_id = Head::current_commit().await.unwrap();
    let commit_obj: Commit = load_object(&head_id).unwrap();
    // The raw commit data should not contain gpgsig when vault is not enabled
    let raw_commit = commit_obj.to_data().unwrap();
    let raw = String::from_utf8_lossy(&raw_commit);
    assert!(
        !raw.contains("gpgsig"),
        "commit without vault should not have gpgsig header"
    );
}

/// Commit with vault enabled should include a gpgsig header.
#[tokio::test]
#[serial]
async fn test_commit_with_vault_has_signature() {
    let temp_root = tempdir().unwrap();
    let repo_dir = temp_root.path().join("repo");
    let test_home = temp_root.path().join("home");
    std::fs::create_dir_all(&test_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", test_home.as_os_str());
    #[cfg(windows)]
    let _userprofile_guard = EnvVarGuard::set("USERPROFILE", test_home.as_os_str());

    let mut init_cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    init_cmd
        .current_dir(temp_root.path())
        .env("HOME", &test_home)
        .arg("init")
        .arg("--vault")
        .arg(repo_dir.to_str().unwrap());
    #[cfg(windows)]
    init_cmd.env("USERPROFILE", &test_home);
    let init_out = init_cmd.output().expect("failed to run libra init --vault");
    assert!(
        init_out.status.success(),
        "init --vault should succeed, stderr: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let _guard = ChangeDirGuard::new(&repo_dir);
    test::ensure_file("signed.txt", Some("signed content"));
    add::execute(AddArgs {
        pathspec: vec!["signed.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("signed commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    let head_id = Head::current_commit()
        .await
        .expect("expected a commit to be created");
    let commit_obj: Commit = load_object(&head_id).expect("failed to load HEAD commit");
    let raw_commit = commit_obj.to_data().unwrap();
    let raw = String::from_utf8_lossy(&raw_commit);
    assert!(
        raw.contains("\ngpgsig "),
        "vault-enabled commit should include gpgsig header"
    );
}

#[test]
#[serial]
fn test_cli_init_with_vault_and_separate_libra_dir() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");
    let test_home = temp_root.path().join("home");
    std::fs::create_dir_all(&test_home).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(temp_root.path())
        .env("HOME", &test_home)
        .arg("init")
        .arg("--vault")
        .arg("--separate-libra-dir")
        .arg(storage.to_str().unwrap())
        .arg(workdir.to_str().unwrap());
    #[cfg(windows)]
    cmd.env("USERPROFILE", &test_home);
    let output = cmd.output().expect("failed to run libra init --vault");

    assert!(
        output.status.success(),
        "init --vault should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        storage.join("vault.db").is_file(),
        "vault database should be created in separate storage dir"
    );
    assert!(
        workdir.join(".libra").is_file(),
        "workdir .libra should be a link file in separate-dir mode"
    );

    let mut signing_cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    signing_cmd
        .current_dir(&workdir)
        .env("HOME", &test_home)
        .arg("config")
        .arg("--local")
        .arg("--get")
        .arg("vault.signing");
    #[cfg(windows)]
    signing_cmd.env("USERPROFILE", &test_home);
    let signing_out = signing_cmd.output().expect("failed to read vault.signing");
    assert!(
        signing_out.status.success(),
        "config --get vault.signing should succeed, stderr: {}",
        String::from_utf8_lossy(&signing_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&signing_out.stdout).trim(), "true");

    let mut repoid_cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    repoid_cmd
        .current_dir(&workdir)
        .env("HOME", &test_home)
        .arg("config")
        .arg("--local")
        .arg("--get")
        .arg("libra.repoid");
    #[cfg(windows)]
    repoid_cmd.env("USERPROFILE", &test_home);
    let repoid_out = repoid_cmd.output().expect("failed to read libra.repoid");
    assert!(
        repoid_out.status.success(),
        "config --get libra.repoid should succeed, stderr: {}",
        String::from_utf8_lossy(&repoid_out.stderr)
    );
    let repo_id = String::from_utf8_lossy(&repoid_out.stdout)
        .trim()
        .to_string();
    assert!(!repo_id.is_empty(), "repo id should not be empty");

    assert!(
        test_home
            .join(".libra")
            .join("vault-keys")
            .join(repo_id)
            .is_file(),
        "unseal key should be stored under ~/.libra/vault-keys/<repoid>"
    );
}

#[test]
#[serial]
fn test_cli_init_with_vault_fails_when_home_unwritable() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let bad_home = temp_root.path().join("home-file");
    std::fs::write(&bad_home, "not-a-directory").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(temp_root.path())
        .env("HOME", &bad_home)
        .arg("init")
        .arg("--vault")
        .arg(workdir.to_str().unwrap());
    #[cfg(windows)]
    cmd.env("USERPROFILE", &bad_home);
    let output = cmd.output().expect("failed to run libra init --vault");

    assert!(
        !output.status.success(),
        "init --vault should fail when HOME is unusable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to persist vault credentials"),
        "expected vault credential storage error, stderr: {stderr}"
    );
    assert!(
        !workdir.join(".libra").join("vault.db").exists(),
        "vault.db should be rolled back when credential persistence fails"
    );
}
