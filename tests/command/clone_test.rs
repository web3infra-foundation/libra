//! Tests clone command setup to ensure objects, refs, and working copies are created correctly.
//!
//! All tests in this file are **L2 (network)**: they require
//! `LIBRA_TEST_GITHUB_LIVE=1`, `LIBRA_TEST_GITHUB_TOKEN`, and
//! `LIBRA_TEST_GITHUB_NAMESPACE` to create and push to a temporary GitHub
//! repository. Without the explicit live-test flag and credentials, the tests
//! are skipped so normal acceptance runs do not depend on external GitHub state.

use std::{fs, process::Command, sync::OnceLock};

use libra::{command, command::clone::CloneArgs, internal::head::Head, utils::test};
use serial_test::serial;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// GitHub test-repo lifecycle helpers
// ---------------------------------------------------------------------------

struct GitHubTestRepo {
    full_name: String,
    https_url: String,
    token: String,
}

impl Drop for GitHubTestRepo {
    fn drop(&mut self) {
        // Safety: only delete repos whose name starts with "libra-test-"
        if !self.full_name.contains("/libra-test-") {
            return;
        }
        let _ = reqwest::blocking::Client::new()
            .delete(format!("https://api.github.com/repos/{}", self.full_name))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "libra-test")
            .header("Accept", "application/vnd.github+json")
            .send();
    }
}

static GITHUB_REPO: OnceLock<Option<GitHubTestRepo>> = OnceLock::new();
const LIVE_GITHUB_SKIP_MESSAGE: &str = "skipped (set LIBRA_TEST_GITHUB_LIVE=1, LIBRA_TEST_GITHUB_TOKEN, and LIBRA_TEST_GITHUB_NAMESPACE)";

/// Return whether the GitHub-backed clone tests should contact GitHub.
///
/// Test coverage: every clone scenario below flows through `github_test_repo`,
/// so the boundary between deterministic local acceptance and opt-in network
/// validation is exercised before any GitHub API call or authenticated push.
fn live_github_clone_tests_enabled() -> bool {
    std::env::var("LIBRA_TEST_GITHUB_LIVE")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Get or lazily create the shared temporary GitHub repo.
/// Returns `None` (and tests skip) when the live flag or env vars are absent.
fn github_test_repo() -> Option<&'static GitHubTestRepo> {
    GITHUB_REPO
        .get_or_init(|| {
            if !live_github_clone_tests_enabled() {
                return None;
            }

            let token = std::env::var("LIBRA_TEST_GITHUB_TOKEN")
                .ok()
                .filter(|v| !v.is_empty())?;
            let namespace = std::env::var("LIBRA_TEST_GITHUB_NAMESPACE")
                .ok()
                .filter(|v| !v.is_empty())?;
            Some(setup_github_repo(&token, &namespace))
        })
        .as_ref()
}

/// Resolve the shared GitHub fixture from an async test without dropping
/// `reqwest::blocking` internals inside Tokio's worker runtime.
///
/// Test coverage: every `#[tokio::test]` in this file calls this helper before
/// invoking `clone::execute`; missing credentials still return `None` so the L2
/// network scenarios skip cleanly, while configured environments exercise the
/// real GitHub repository setup on a blocking thread.
async fn github_test_repo_for_async_test() -> Option<&'static GitHubTestRepo> {
    tokio::task::spawn_blocking(github_test_repo)
        .await
        .expect("GitHub test-repo setup task panicked")
}

fn setup_github_repo(token: &str, namespace: &str) -> GitHubTestRepo {
    let suffix = &uuid::Uuid::new_v4().to_string()[..6];
    let repo_name = format!("libra-test-{suffix}");
    let full_name = format!("{namespace}/{repo_name}");

    // Create repo via GitHub API
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://api.github.com/user/repos")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "libra-test")
        .header("Accept", "application/vnd.github+json")
        .json(&serde_json::json!({
            "name": repo_name,
            "auto_init": false,
            "private": false,
        }))
        .send()
        .expect("failed to create GitHub repo");
    assert!(
        resp.status().is_success(),
        "GitHub repo creation failed: {}",
        resp.text().unwrap_or_default()
    );

    let https_url = format!("https://github.com/{full_name}.git");

    // Push test data: main branch with a commit, then dev branch with another commit.
    let work_dir = tempfile::tempdir().expect("failed to create workdir for push");
    let wd = work_dir.path();

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(wd)
            .args(args)
            .output()
            .expect("git command failed");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };

    let auth_url = format!("https://x-access-token:{token}@github.com/{full_name}.git");

    git(&["init"]);
    git(&["config", "user.name", "Libra Test"]);
    git(&["config", "user.email", "test@libra.dev"]);
    fs::write(wd.join("README.md"), "libra clone test repo").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "initial commit"]);

    // Detect the default branch name (may be main or master).
    let head_out = git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let default_branch = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    // Ensure we are on 'main'.
    if default_branch != "main" {
        git(&["branch", "-M", "main"]);
    }
    git(&["remote", "add", "origin", &auth_url]);
    git(&["push", "-u", "origin", "main"]);

    // Create dev branch with an extra commit.
    git(&["checkout", "-b", "dev"]);
    fs::write(wd.join("dev.txt"), "dev branch content").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "dev commit"]);
    git(&["push", "-u", "origin", "dev"]);

    GitHubTestRepo {
        full_name,
        https_url,
        token: token.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Clone tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_clone_branch() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: false,
        bare: false,
        depth: None,
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "dev"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_bare_repository() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    let repo_dir = temp_path.path().join("bare-clone.git");

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(repo_dir.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: false,
        bare: true,
        depth: None,
    })
    .await;

    assert!(
        repo_dir.join("libra.db").exists(),
        "bare clone should create libra.db at repo root"
    );
    assert!(
        repo_dir.join("info").join("exclude").exists(),
        "bare clone should create info/exclude"
    );
    assert!(
        repo_dir.join("objects").exists(),
        "bare clone should have objects directory"
    );
    assert!(
        !repo_dir.join(".libra").exists(),
        "bare clone should not create nested .libra"
    );

    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "dev"),
        _ => panic!("bare clone should still update HEAD to a branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_branch_single_branch() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: true,
        bare: false,
        depth: None,
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "dev"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_default_branch() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: None,
        single_branch: false,
        bare: false,
        depth: None,
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "main"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_default_branch_single_branch() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: None,
        single_branch: true,
        bare: false,
        depth: None,
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "main"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_to_existing_empty_dir() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    let repo_path = temp_path.path().join("clone-target");
    fs::create_dir(&repo_path).unwrap();

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(repo_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: false,
        bare: false,
        depth: None,
    })
    .await;

    assert!(repo_path.join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "dev"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_to_existing_dir() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let repo_path = temp_path.path().join("clone-target");
    fs::create_dir(&repo_path).unwrap();
    let dummy_file = repo_path.join("exists.txt");
    fs::write(&dummy_file, "test").unwrap();

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(repo_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: false,
        bare: false,
        depth: None,
    })
    .await;

    assert!(!repo_path.join(".libra").exists());
    assert!(dummy_file.exists(), "pre-existing file should still exist");
    assert_eq!(fs::read_to_string(&dummy_file).unwrap(), "test");
}

#[tokio::test]
#[serial]
async fn test_clone_to_dir_with_existing_file_name() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let conflict_path = temp_path.path().join("clone-target");
    fs::write(&conflict_path, "test").unwrap();

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(conflict_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: false,
        bare: false,
        depth: None,
    })
    .await;

    assert!(
        conflict_path.is_file(),
        "pre-existing file should remain a file"
    );
    assert_eq!(fs::read_to_string(&conflict_path).unwrap(), "test");
}

#[tokio::test]
#[serial]
async fn test_clone_with_depth() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: None,
        single_branch: false,
        bare: false,
        depth: Some(1),
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "main"),
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
async fn test_clone_with_depth_and_branch() {
    let repo = match github_test_repo_for_async_test().await {
        Some(r) => r,
        None => {
            eprintln!("{LIVE_GITHUB_SKIP_MESSAGE}");
            return;
        }
    };
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    command::clone::execute(CloneArgs {
        remote_repo: repo.https_url.clone(),
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
        single_branch: true,
        bare: false,
        depth: Some(5),
    })
    .await;

    assert!(temp_path.path().join(".libra").exists());
    match Head::current().await {
        Head::Branch(b) => assert_eq!(b, "dev"),
        _ => panic!("should be branch"),
    };
}
