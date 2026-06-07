//! Editor, cleanup-mode, and template behavior for `libra commit` (Batch 0).
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Editor launch is exercised with a tiny script editor (no TTY needed for an
//! *explicitly configured* editor); the `vi` fallback path is exercised by
//! clearing all editor env vars in a non-TTY subprocess.

use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

fn run_libra_env(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> std::process::Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .args(args)
        .current_dir(cwd)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .env_remove("GIT_EDITOR")
        .env_remove("VISUAL")
        .env_remove("EDITOR");
    for (k, v) in env {
        command.env(k, v);
    }
    command.output().unwrap()
}

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    run_libra_env(args, cwd, &[])
}

fn init_repo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    assert!(run_libra(&["init"], repo).status.success(), "init failed");
    assert!(
        run_libra(&["config", "user.name", "Test User"], repo)
            .status
            .success()
    );
    assert!(
        run_libra(&["config", "user.email", "test@example.com"], repo)
            .status
            .success()
    );
}

fn stage_file(repo: &Path, name: &str, content: &str) {
    fs::write(repo.join(name), content).unwrap();
    assert!(
        run_libra(&["add", name], repo).status.success(),
        "add failed"
    );
}

/// Write a `#!/bin/sh` editor script that writes `body` to its last argument
/// (the COMMIT_EDITMSG path), make it executable, and return its absolute path.
#[cfg(unix)]
fn write_editor_script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{body}' > \"$1\"\n")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path.to_string_lossy().into_owned()
}

fn last_commit_message(repo: &Path) -> String {
    let out = run_libra(&["log", "-1"], repo);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[cfg(unix)]
#[test]
fn editor_launched_when_no_message_and_writes_message() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");
    let editor = write_editor_script(temp.path(), "ed.sh", "editor subject\\n");

    let out = run_libra_env(&["commit"], &repo, &[("EDITOR", &editor)]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "editor commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        last_commit_message(&repo).contains("editor subject"),
        "commit should use the editor-written message"
    );
}

#[cfg(unix)]
#[test]
fn editor_priority_visual_over_editor() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");
    let visual = write_editor_script(temp.path(), "visual.sh", "from-visual\\n");
    let editor = write_editor_script(temp.path(), "editor.sh", "from-editor\\n");

    let out = run_libra_env(
        &["commit"],
        &repo,
        &[("VISUAL", &visual), ("EDITOR", &editor)],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        last_commit_message(&repo).contains("from-visual"),
        "VISUAL should take precedence over EDITOR"
    );
}

#[cfg(unix)]
#[test]
fn edit_flag_uses_message_as_initial_then_edits() {
    // --edit launches the editor even with -m; here the script overwrites it.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");
    let editor = write_editor_script(temp.path(), "ed.sh", "edited-final\\n");

    let out = run_libra_env(
        &["commit", "-e", "-m", "initial"],
        &repo,
        &[("EDITOR", &editor)],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "edit commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(last_commit_message(&repo).contains("edited-final"));
}

#[cfg(unix)]
#[test]
fn editor_nonzero_exit_aborts_with_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    // `false` exits non-zero without writing the file.
    let out = run_libra_env(&["commit"], &repo, &[("EDITOR", "false")]);
    assert_eq!(
        out.status.code(),
        Some(128),
        "editor failure must abort with 128: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn no_edit_coexists_with_message_flag() {
    // --no-edit is now allowed outside --amend and may carry -m.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit", "--no-edit", "-m", "msg via no-edit"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-edit with -m must commit: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(last_commit_message(&repo).contains("msg via no-edit"));
}

#[test]
fn bare_no_edit_without_message_source_errors_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit", "--no-edit"], &repo);
    assert_eq!(
        out.status.code(),
        Some(128),
        "bare --no-edit with no message must error 128: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn non_tty_without_editor_errors_no_hang() {
    // No editor env (cleared) + non-TTY subprocess → must error, not hang.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit"], &repo);
    assert_ne!(
        out.status.code(),
        Some(0),
        "must not succeed without a message"
    );
}

#[test]
fn edit_conflicts_with_no_edit_exits_129() {
    // Libra maps clap parse errors to 129 (classify_parse_error), not clap's
    // native 2.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit", "-e", "--no-edit", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(129),
        "--edit conflicts with --no-edit (clap parse error → 129 in Libra)"
    );
}

#[test]
fn invalid_cleanup_mode_exits_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit", "--cleanup=bogus", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(129),
        "invalid --cleanup mode → exit 129 (Libra maps clap errors to 129)"
    );
}

#[test]
fn cleanup_verbatim_keeps_comment_lines() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(
        &["commit", "--cleanup=verbatim", "-m", "#issue-1\nkeep me"],
        &repo,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "verbatim commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        last_commit_message(&repo).contains("#issue-1"),
        "--cleanup=verbatim must keep the # line"
    );
}

#[test]
fn cleanup_strip_drops_comment_lines() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(
        &["commit", "--cleanup=strip", "-m", "subject\n# a comment"],
        &repo,
    );
    assert_eq!(out.status.code(), Some(0));
    let msg = last_commit_message(&repo);
    assert!(msg.contains("subject") && !msg.contains("# a comment"));
}

#[test]
fn cleanup_config_default_strips_comment_lines() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    assert!(
        run_libra(&["config", "commit.cleanup", "strip"], &repo)
            .status
            .success()
    );
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(&["commit", "-m", "subject\n# configured comment"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "configured cleanup commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let msg = last_commit_message(&repo);
    assert!(
        msg.contains("subject") && !msg.contains("# configured comment"),
        "commit.cleanup=strip should drop comments by default: {msg}"
    );
}

#[test]
fn cleanup_flag_overrides_config() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    assert!(
        run_libra(&["config", "commit.cleanup", "strip"], &repo)
            .status
            .success()
    );
    stage_file(&repo, "a.txt", "x\n");

    let out = run_libra(
        &[
            "commit",
            "--cleanup=verbatim",
            "-m",
            "subject\n# explicit comment",
        ],
        &repo,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "explicit cleanup commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        last_commit_message(&repo).contains("# explicit comment"),
        "--cleanup=verbatim should override commit.cleanup=strip"
    );
}

#[test]
fn template_t_flag_loads_initial_content() {
    // -t supplies the initial message; with --no-edit it is used directly.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage_file(&repo, "a.txt", "x\n");
    let tpl = temp.path().join("tpl.txt");
    fs::write(&tpl, "templated subject\n").unwrap();

    let out = run_libra(&["commit", "--no-edit", "-t", tpl.to_str().unwrap()], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "template commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(last_commit_message(&repo).contains("templated subject"));
}
