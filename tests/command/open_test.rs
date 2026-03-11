//! Tests open command integration to ensure it finds remote correctly.
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

struct PathEnvGuard {
    old_path: Option<String>,
}

impl PathEnvGuard {
    fn prepend(dir: &Path) -> Self {
        let old_path = std::env::var("PATH").ok();
        let new_path = match &old_path {
            Some(existing) if !existing.is_empty() => format!("{}:{}", dir.display(), existing),
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
