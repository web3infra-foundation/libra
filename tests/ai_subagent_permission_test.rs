//! OC-Phase sub-agent path-traversal defense regression (opencode.md §4 测试要求,
//! line 510): "测试用例必须在 `tests/ai_subagent_permission_test.rs` 中包含路径
//! 穿越防御测试：尝试读写 `./src/../../secret.txt` 或构建软链接指向外部，确保被
//! 安全拦截".
//!
//! A sub-agent's file tools confine every path to the agent's `working_dir`.
//! The boundary check (`tools::utils::validate_path` /
//! `is_within_working_dir`) canonicalizes both the candidate path and the
//! working dir — resolving `..` segments *and* following symlinks — before
//! testing containment, so neither a `../../` escape nor a symlink pointing
//! outside the workspace can read or write a file beyond the boundary. These
//! tests run the production boundary functions against real on-disk escapes.

use std::{fs, path::Path};

use libra::internal::ai::tools::{
    ToolError,
    utils::{is_within_working_dir, resolve_path, validate_path},
};
use tempfile::TempDir;

/// A `../../` escape that climbs out of the working dir must be rejected.
/// Mirrors the doc's `./src/../../secret.txt` example: a relative path that, once
/// `..`-normalized, lands above `working_dir`.
#[test]
fn dotdot_escape_above_working_dir_is_rejected() {
    let outer = TempDir::new().expect("outer tempdir");
    let working_dir = outer.path().join("workspace");
    fs::create_dir(&working_dir).expect("create workspace");
    // The intermediate `src/` exists so the boundary canonicalization resolves
    // through real directories (a tool typically reads an existing path).
    fs::create_dir(working_dir.join("src")).expect("create src");
    // A secret living in the outer dir, a sibling of `workspace`.
    let secret = outer.path().join("secret.txt");
    fs::write(&secret, b"top secret").expect("write secret");

    // `workspace/src/../../secret.txt` normalizes to `outer/secret.txt` —
    // outside the workspace.
    let escape = working_dir.join("src/../../secret.txt");

    // `resolve_path` (relative form, what a tool receives) must reject the
    // escape — the security-critical property is that it is NOT `Ok`. The
    // workspace-escaping path surfaces as `PathOutsideWorkingDir`.
    let relative = Path::new("src/../../secret.txt");
    let resolved = resolve_path(relative, &working_dir);
    assert!(
        matches!(resolved, Err(ToolError::PathOutsideWorkingDir(_))),
        "a ../.. escape must be rejected as outside the working dir, got: {resolved:?}",
    );

    // And the absolute boundary check agrees.
    assert!(
        !is_within_working_dir(&escape, &working_dir).expect("boundary check runs"),
        "`{}` must not be considered inside the workspace",
        escape.display(),
    );
}

/// A `..` path that stays *inside* the working dir is allowed — the defense
/// rejects escapes, not every `..`.
#[test]
fn dotdot_within_working_dir_is_allowed() {
    let working_dir = TempDir::new().expect("workspace tempdir");
    fs::create_dir(working_dir.path().join("src")).expect("create src");
    fs::write(working_dir.path().join("a.txt"), b"hi").expect("write a.txt");

    // `src/../a.txt` normalizes back to `a.txt` inside the workspace.
    let inside = working_dir.path().join("src/../a.txt");
    assert!(
        is_within_working_dir(&inside, working_dir.path()).expect("boundary check runs"),
        "a `..` that stays inside the workspace must be allowed",
    );
    assert!(validate_path(&inside, working_dir.path()).is_ok());
}

/// A symlink inside the workspace that points to an external directory must not
/// become a read/write hole: a path *through* the symlink resolves (via
/// `canonicalize`) to the external target and is rejected.
#[cfg(unix)]
#[test]
fn symlink_escaping_working_dir_is_rejected() {
    use std::os::unix::fs::symlink;

    let outer = TempDir::new().expect("outer tempdir");
    let working_dir = outer.path().join("workspace");
    fs::create_dir(&working_dir).expect("create workspace");

    // An external directory holding a secret, outside the workspace.
    let external = outer.path().join("external");
    fs::create_dir(&external).expect("create external");
    fs::write(external.join("secret.txt"), b"top secret").expect("write external secret");

    // A symlink *inside* the workspace pointing at the external dir.
    let link = working_dir.join("link");
    symlink(&external, &link).expect("create symlink");

    // Reading `workspace/link/secret.txt` follows the symlink to
    // `external/secret.txt` — outside the workspace, so it must be rejected.
    let through_link = link.join("secret.txt");
    let within = is_within_working_dir(&through_link, &working_dir).expect("boundary check runs");
    assert!(
        !within,
        "a path through a symlink that escapes the workspace must be rejected",
    );
    assert!(
        matches!(
            validate_path(&through_link, &working_dir),
            Err(ToolError::PathOutsideWorkingDir(_)),
        ),
        "validate_path must reject a symlink-escaping path",
    );
}

/// A symlink inside the workspace that points to another in-workspace location
/// is still allowed — only escapes are blocked.
#[cfg(unix)]
#[test]
fn symlink_within_working_dir_is_allowed() {
    use std::os::unix::fs::symlink;

    let working_dir = TempDir::new().expect("workspace tempdir");
    let real_dir = working_dir.path().join("real");
    fs::create_dir(&real_dir).expect("create real dir");
    fs::write(real_dir.join("data.txt"), b"ok").expect("write data");

    // A symlink inside the workspace pointing to another in-workspace dir.
    let link = working_dir.path().join("link");
    symlink(&real_dir, &link).expect("create symlink");

    let through_link = link.join("data.txt");
    assert!(
        is_within_working_dir(&through_link, working_dir.path()).expect("boundary check runs"),
        "a symlink staying inside the workspace must be allowed",
    );
}
