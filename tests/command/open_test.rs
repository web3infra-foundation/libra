//! Tests open command integration to ensure it finds remote correctly.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::path::{Path, PathBuf};

use libra::{
    command::{
        open,
        remote::{self, RemoteCmds},
    },
    utils::test,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

struct PathEnvGuard {
    old_path: Option<String>,
}

impl PathEnvGuard {
    fn prepend(dir: &Path) -> Self {
        let old_path = std::env::var("PATH").ok();
        let new_path = match &old_path {
            Some(existing) if !existing.is_empty() => {
                let paths: Vec<PathBuf> = std::iter::once(dir.to_path_buf())
                    .chain(std::env::split_paths(existing))
                    .collect();
                std::env::join_paths(&paths)
                    .unwrap_or_else(|_| dir.as_os_str().to_os_string())
                    .into_string()
                    .unwrap_or_else(|os| os.to_string_lossy().into_owned())
            }
            _ => dir.display().to_string(),
        };
        unsafe {
            std::env::set_var("PATH", new_path);
        }
        Self { old_path }
    }
}

impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        match &self.old_path {
            Some(path) => unsafe {
                std::env::set_var("PATH", path);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }
    }
}

fn install_browser_mock(work_dir: &Path) -> PathBuf {
    let bin_dir = work_dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let script = "#!/bin/sh\nexit 0\n";
        std::fs::write(bin_dir.join("open"), script).unwrap();
        std::fs::write(bin_dir.join("xdg-open"), script).unwrap();
        std::fs::set_permissions(bin_dir.join("open"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::set_permissions(
            bin_dir.join("xdg-open"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    #[cfg(windows)]
    {
        std::fs::write(bin_dir.join("xdg-open.cmd"), "@echo off\nexit /b 0\n").unwrap();
        std::fs::write(bin_dir.join("open.cmd"), "@echo off\nexit /b 0\n").unwrap();
    }

    bin_dir
}

#[tokio::test]
#[serial]
async fn test_open_remote_origin() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let browser_bin = install_browser_mock(repo_dir.path());
    let _path_guard = PathEnvGuard::prepend(&browser_bin);

    // Add origin remote
    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "git@github.com:web3infra-foundation/libra.git".into(),
    })
    .await;

    // Test explicit remote
    open::execute(open::OpenArgs {
        remote: Some("origin".to_string()),
    })
    .await;

    // Test default remote should find origin
    open::execute(open::OpenArgs { remote: None }).await;

    // Test non-existent remote
    open::execute(open::OpenArgs {
        remote: Some("nonexistent".to_string()),
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_open_no_remote() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let browser_bin = install_browser_mock(repo_dir.path());
    let _path_guard = PathEnvGuard::prepend(&browser_bin);

    // Should handle no remote configured
    open::execute(open::OpenArgs { remote: None }).await;
}

#[test]
fn test_open_json_output_uses_origin_remote() {
    let repo = create_committed_repo_via_cli();
    let browser_bin = install_browser_mock(repo.path());

    let add_remote = run_libra_command(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:web3infra-foundation/libra.git",
        ],
        repo.path(),
    );
    assert_cli_success(&add_remote, "failed to add origin for open test");

    let mut command = base_libra_command(&["open", "--json"], repo.path());
    let sys_paths: Vec<PathBuf> = vec![
        browser_bin.clone(),
        "/usr/bin".into(),
        "/bin".into(),
        "/usr/sbin".into(),
        "/sbin".into(),
    ];
    command.env("PATH", std::env::join_paths(&sys_paths).unwrap());
    let output = command.output().expect("failed to execute open --json");

    assert_cli_success(&output, "open --json should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "open");
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
}

#[test]
fn test_open_without_remote_reports_stable_error() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let browser_bin = install_browser_mock(repo.path());
    let mut command = base_libra_command(&["open"], repo.path());
    let sys_paths: Vec<PathBuf> = vec![
        browser_bin.clone(),
        "/usr/bin".into(),
        "/bin".into(),
        "/usr/sbin".into(),
        "/sbin".into(),
    ];
    command.env("PATH", std::env::join_paths(&sys_paths).unwrap());
    let output = command.output().expect("failed to execute open");

    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("libra remote add origin")),
        "expected hint to mention adding a remote, got {:?}",
        report.hints
    );
}
