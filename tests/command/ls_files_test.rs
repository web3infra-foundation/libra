//! Integration tests for `ls-files`.
//!
//! **Layer:** L1 — deterministic local repositories, no network.

use std::{fs, process::Output};

use super::*;

fn setup_ls_files_repo() -> tempfile::TempDir {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::create_dir_all(repo.path().join("tracked-dir")).expect("create tracked dir");
    fs::create_dir_all(repo.path().join("中文目录")).expect("create chinese tracked dir");
    fs::create_dir_all(repo.path().join("others-dir")).expect("create untracked dir");

    fs::write(
        repo.path().join(".libraignore"),
        "ignored.tmp\nothers-dir/*.tmp\n",
    )
    .expect("write ignore file");
    fs::write(repo.path().join("tracked.txt"), "tracked\n").expect("write tracked file");
    fs::write(repo.path().join("delete.txt"), "delete me\n").expect("write delete fixture");
    fs::write(repo.path().join("tracked-dir").join("alpha.txt"), "alpha\n")
        .expect("write tracked dir alpha");
    fs::write(repo.path().join("tracked-dir").join("bravo.txt"), "bravo\n")
        .expect("write tracked dir bravo");
    fs::write(repo.path().join("中文目录").join("条目.txt"), "unicode\n")
        .expect("write chinese tracked file");
    fs::write(repo.path().join("special [name].txt"), "special\n")
        .expect("write special tracked file");

    let add = run_libra_command(
        &[
            "add",
            ".libraignore",
            "tracked.txt",
            "delete.txt",
            "tracked-dir",
            "中文目录",
            "special [name].txt",
        ],
        repo.path(),
    );
    assert_cli_success(&add, "failed to add ls-files fixture files");

    let commit = run_libra_command(
        &["commit", "-m", "ls-files fixture", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&commit, "failed to commit ls-files fixture");

    fs::write(repo.path().join("tracked.txt"), "tracked and modified\n")
        .expect("modify tracked file");
    fs::remove_file(repo.path().join("delete.txt")).expect("delete tracked file");
    fs::write(repo.path().join("untracked.txt"), "untracked\n").expect("write untracked file");
    fs::write(repo.path().join("ignored.tmp"), "ignored\n").expect("write ignored file");
    fs::write(
        repo.path().join("others-dir").join("untracked.txt"),
        "nested untracked\n",
    )
    .expect("write nested untracked file");
    fs::write(
        repo.path().join("others-dir").join("ignored.tmp"),
        "nested ignored\n",
    )
    .expect("write nested ignored file");

    repo
}

fn stdout_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.to_string())
        .collect()
}

fn stdout_nul_fields(output: &Output) -> Vec<String> {
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()).expect("expected UTF-8 field"))
        .collect()
}

#[test]
#[serial]
fn ls_files_help_is_visible_and_renders_examples() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["ls-files", "--help"], repo.path());
    assert_cli_success(&output, "ls-files --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "ls-files --help should render examples, stdout={stdout}"
    );
}

#[test]
#[serial]
fn ls_files_defaults_to_cached_listing() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files"], repo.path());
    assert_cli_success(&output, "ls-files should succeed");

    assert_eq!(
        stdout_lines(&output),
        vec![
            ".libraignore".to_string(),
            "delete.txt".to_string(),
            "special [name].txt".to_string(),
            "tracked-dir/alpha.txt".to_string(),
            "tracked-dir/bravo.txt".to_string(),
            "tracked.txt".to_string(),
            "中文目录/条目.txt".to_string(),
        ]
    );
}

#[test]
#[serial]
fn ls_files_modified_lists_only_modified_tracked_paths() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--modified"], repo.path());
    assert_cli_success(&output, "ls-files --modified should succeed");

    assert_eq!(stdout_lines(&output), vec!["tracked.txt".to_string()]);
}

#[test]
#[serial]
fn ls_files_deleted_lists_only_missing_tracked_paths() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--deleted"], repo.path());
    assert_cli_success(&output, "ls-files --deleted should succeed");

    assert_eq!(stdout_lines(&output), vec!["delete.txt".to_string()]);
}

#[test]
#[serial]
fn ls_files_others_lists_untracked_paths_without_ignore_filtering() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--others"], repo.path());
    assert_cli_success(&output, "ls-files --others should succeed");

    assert_eq!(
        stdout_lines(&output),
        vec![
            "ignored.tmp".to_string(),
            "others-dir/ignored.tmp".to_string(),
            "others-dir/untracked.txt".to_string(),
            "untracked.txt".to_string(),
        ]
    );
}

#[test]
#[serial]
fn ls_files_exclude_standard_honors_libraignore_for_others() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--others", "--exclude-standard"], repo.path());
    assert_cli_success(
        &output,
        "ls-files --others --exclude-standard should succeed",
    );

    assert_eq!(
        stdout_lines(&output),
        vec![
            "others-dir/untracked.txt".to_string(),
            "untracked.txt".to_string()
        ]
    );
}

#[test]
#[serial]
fn ls_files_stage_and_short_alias_render_same_stage_output() {
    let repo = setup_ls_files_repo();

    let stage = run_libra_command(&["ls-files", "--stage"], repo.path());
    assert_cli_success(&stage, "ls-files --stage should succeed");

    let short = run_libra_command(&["ls-files", "-s"], repo.path());
    assert_cli_success(&short, "ls-files -s should succeed");

    let stage_stdout = String::from_utf8_lossy(&stage.stdout);
    assert!(
        stage_stdout
            .lines()
            .any(|line| line.contains(" 0\ttracked.txt")),
        "--stage output should include stage 0 tracked.txt entry, stdout={stage_stdout}"
    );
    assert_eq!(stage.stdout, short.stdout, "--stage and -s should match");
}

#[test]
#[serial]
fn ls_files_json_uses_standard_envelope() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["--json", "ls-files", "--modified"], repo.path());
    assert_cli_success(&output, "json ls-files --modified should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "ls-files");

    let data = json["data"]
        .as_array()
        .expect("ls-files data should be an array");
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["path"], "tracked.txt");
    assert_eq!(data[0]["status"], "modified");
    assert_eq!(data[0]["stage"], 0);
    assert!(data[0]["hash"].is_string());
    assert!(data[0]["mode"].is_string());
}

#[test]
#[serial]
fn ls_files_pathspec_filters_to_an_exact_file() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "tracked-dir/alpha.txt"], repo.path());
    assert_cli_success(&output, "ls-files <file> should succeed");

    assert_eq!(
        stdout_lines(&output),
        vec!["tracked-dir/alpha.txt".to_string()]
    );
}

#[test]
#[serial]
fn ls_files_pathspec_filters_to_a_directory_prefix() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "tracked-dir"], repo.path());
    assert_cli_success(&output, "ls-files <dir> should succeed");

    assert_eq!(
        stdout_lines(&output),
        vec![
            "tracked-dir/alpha.txt".to_string(),
            "tracked-dir/bravo.txt".to_string(),
        ]
    );
}

#[test]
#[serial]
fn ls_files_others_pathspec_lists_untracked_paths_under_directory() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--others", "others-dir"], repo.path());
    assert_cli_success(&output, "ls-files --others <dir> should succeed");

    assert_eq!(
        stdout_lines(&output),
        vec![
            "others-dir/ignored.tmp".to_string(),
            "others-dir/untracked.txt".to_string(),
        ]
    );
}

#[test]
#[serial]
fn ls_files_others_exclude_standard_honors_libraignore_for_directory_pathspec() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(
        &["ls-files", "--others", "--exclude-standard", "others-dir"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "ls-files --others --exclude-standard <dir> should succeed",
    );

    assert_eq!(
        stdout_lines(&output),
        vec!["others-dir/untracked.txt".to_string()]
    );
}

#[test]
#[serial]
fn ls_files_pathspec_is_resolved_from_nested_current_dir() {
    let repo = setup_ls_files_repo();
    let nested_cwd = repo.path().join("nested-cwd");
    fs::create_dir_all(&nested_cwd).expect("create nested cwd");

    let output = run_libra_command(&["ls-files", "../tracked-dir"], &nested_cwd);
    assert_cli_success(&output, "ls-files should resolve pathspecs from cwd");

    assert_eq!(
        stdout_lines(&output),
        vec![
            "tracked-dir/alpha.txt".to_string(),
            "tracked-dir/bravo.txt".to_string(),
        ]
    );
}

#[test]
#[serial]
fn ls_files_pathspec_accepts_chinese_names() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "中文目录/条目.txt"], repo.path());
    assert_cli_success(&output, "ls-files should accept chinese pathspecs");

    assert_eq!(stdout_lines(&output), vec!["中文目录/条目.txt".to_string()]);
}

#[test]
#[serial]
fn ls_files_pathspec_accepts_special_character_names() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "special [name].txt"], repo.path());
    assert_cli_success(
        &output,
        "ls-files should accept special-character pathspecs",
    );

    assert_eq!(
        stdout_lines(&output),
        vec!["special [name].txt".to_string()]
    );
}

#[test]
#[serial]
fn ls_files_stage_output_respects_pathspecs() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--stage", "tracked-dir"], repo.path());
    assert_cli_success(&output, "ls-files --stage <dir> should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .all(|line| line.ends_with("tracked-dir/alpha.txt")
                || line.ends_with("tracked-dir/bravo.txt")),
        "stage output should be limited to tracked-dir entries, stdout={stdout}"
    );
}

#[test]
#[serial]
fn ls_files_empty_pathspec_result_is_allowed_without_error_unmatch() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "missing.txt"], repo.path());
    assert_cli_success(
        &output,
        "ls-files without --error-unmatch should allow empty pathspec results",
    );

    assert!(
        output.stdout.is_empty(),
        "stdout should be empty: {:?}",
        output.stdout
    );
}

#[test]
#[serial]
fn ls_files_z_outputs_nul_delimited_records() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "-z", "tracked-dir"], repo.path());
    assert_cli_success(&output, "ls-files -z should succeed");

    assert_eq!(
        stdout_nul_fields(&output),
        vec![
            "tracked-dir/alpha.txt".to_string(),
            "tracked-dir/bravo.txt".to_string(),
        ]
    );
    assert!(
        !output.stdout.contains(&b'\n'),
        "nul output should not contain newlines: {:?}",
        output.stdout
    );
    assert_eq!(
        output.stdout.last(),
        Some(&0),
        "nul output should end with a NUL byte"
    );
}

#[test]
#[serial]
fn ls_files_error_unmatch_fails_for_missing_pathspec() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["ls-files", "--error-unmatch", "missing.txt"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report
            .message
            .contains("pathspec 'missing.txt' did not match any files"),
        "message was: {}",
        report.message
    );
}

#[test]
#[serial]
fn ls_files_error_unmatch_fails_when_any_pathspec_is_missing() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(
        &["ls-files", "--error-unmatch", "tracked.txt", "missing.txt"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(129));

    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report
            .message
            .contains("pathspec 'missing.txt' did not match any files"),
        "message was: {}",
        report.message
    );
}

#[test]
#[serial]
fn ls_files_pathspec_rejects_paths_outside_repo() {
    let repo = setup_ls_files_repo();
    let nested_cwd = repo.path().join("nested-cwd");
    fs::create_dir_all(&nested_cwd).expect("create nested cwd");

    let output = run_libra_command(
        &["ls-files", "--error-unmatch", "../../outside.txt"],
        &nested_cwd,
    );
    assert_eq!(output.status.code(), Some(129));

    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("outside repository"),
        "message was: {}",
        report.message
    );
}

#[test]
#[serial]
fn ls_files_rejects_z_with_json_output() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["--json", "ls-files", "-z"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("ls-files -z cannot be combined"),
        "message was: {}",
        report.message
    );
}

#[test]
#[serial]
fn ls_files_rejects_z_with_machine_output() {
    let repo = setup_ls_files_repo();

    let output = run_libra_command(&["--machine", "ls-files", "-z"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("ls-files -z cannot be combined"),
        "message was: {}",
        report.message
    );
}

#[test]
fn ls_files_t_prefixes_status_tags() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    // Cached files are tagged H.
    let out = run_libra_command(&["ls-files", "-t"], p);
    assert_cli_success(&out, "ls-files -t");
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        s.lines().any(|l| l == "H tracked.txt"),
        "cached -> H: {s:?}"
    );
    assert!(
        s.lines().all(|l| l.starts_with("H ")),
        "all cached -> H: {s:?}"
    );

    // Untracked files are tagged ?.
    fs::write(p.join("untracked.txt"), "x\n").unwrap();
    let out = run_libra_command(&["ls-files", "-t", "--others", "--exclude-standard"], p);
    assert_cli_success(&out, "ls-files -t --others");
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        s.lines().any(|l| l == "? untracked.txt"),
        "untracked -> ?: {s:?}"
    );

    // Modified files are tagged C.
    fs::write(p.join("tracked.txt"), "tracked changed\n").unwrap();
    let out = run_libra_command(&["ls-files", "-t", "--modified"], p);
    assert_cli_success(&out, "ls-files -t --modified");
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        s.lines().any(|l| l == "C tracked.txt"),
        "modified -> C: {s:?}"
    );

    // Deleted files are tagged R.
    fs::remove_file(p.join("tracked.txt")).unwrap();
    let out = run_libra_command(&["ls-files", "-t", "--deleted"], p);
    assert_cli_success(&out, "ls-files -t --deleted");
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        s.lines().any(|l| l == "R tracked.txt"),
        "deleted -> R: {s:?}"
    );
}

#[test]
fn ls_files_u_shows_unmerged_conflict_entries() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    // Build a merge conflict on conf.txt via two divergent branches.
    fs::write(p.join("conf.txt"), "base\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "conf.txt"], p), "add conf");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base-conf", "--no-verify"], p),
        "commit base-conf",
    );
    assert_cli_success(&run_libra_command(&["branch", "other"], p), "branch other");
    fs::write(p.join("conf.txt"), "main-change\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "conf.txt"], p), "add main");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "main-c", "--no-verify"], p),
        "commit main-c",
    );
    assert_cli_success(&run_libra_command(&["switch", "other"], p), "switch other");
    fs::write(p.join("conf.txt"), "other-change\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "conf.txt"], p), "add other");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "other-c", "--no-verify"], p),
        "commit other-c",
    );
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    // The merge conflicts (non-zero exit expected); the conflict stays in the index.
    let _ = run_libra_command(&["merge", "other"], p);

    // -u lists the three conflict stages for conf.txt in stage format.
    let out = run_libra_command(&["ls-files", "-u"], p);
    assert_cli_success(&out, "ls-files -u");
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        s.lines().any(|l| l.contains(" 1\tconf.txt")),
        "stage 1: {s:?}"
    );
    assert!(
        s.lines().any(|l| l.contains(" 2\tconf.txt")),
        "stage 2: {s:?}"
    );
    assert!(
        s.lines().any(|l| l.contains(" 3\tconf.txt")),
        "stage 3: {s:?}"
    );
    // -u shows ONLY unmerged entries, not cleanly-staged files.
    assert!(
        !s.contains("tracked.txt"),
        "clean entries excluded from -u: {s:?}"
    );
}

#[test]
fn ls_files_full_name_accepted_as_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // `--full-name` is accepted (Git compatibility) and produces the same
    // repo-root-relative output Libra emits by default.
    let plain = run_libra_command(&["ls-files"], p);
    assert_cli_success(&plain, "ls-files");
    let with_flag = run_libra_command(&["ls-files", "--full-name"], p);
    assert_cli_success(&with_flag, "ls-files --full-name");
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&with_flag.stdout),
        "--full-name is a no-op matching default output"
    );
    // Paths are repo-root-relative (the `git --full-name` form).
    assert!(
        String::from_utf8_lossy(&with_flag.stdout)
            .lines()
            .any(|l| l == "tracked.txt"),
        "root-relative path present"
    );
}

#[test]
fn test_ls_files_abbrev_shortens_object_name() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("f.txt"), "content\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add f");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c", "--no-verify"], p),
        "commit",
    );

    // -s shows the full 40-char object name.
    let full = run_libra_command(&["ls-files", "-s", "f.txt"], p);
    assert_cli_success(&full, "ls-files -s");
    let full_out = String::from_utf8_lossy(&full.stdout);
    let full_hash = full_out.split_whitespace().nth(1).expect("hash field");
    assert_eq!(full_hash.len(), 40, "full hash: {full_out:?}");

    // --abbrev=8 truncates the object name to 8 digits.
    let ab8 = run_libra_command(&["ls-files", "-s", "--abbrev=8", "f.txt"], p);
    assert_cli_success(&ab8, "ls-files -s --abbrev=8");
    let ab8_out = String::from_utf8_lossy(&ab8.stdout);
    let ab8_hash = ab8_out.split_whitespace().nth(1).expect("hash field");
    assert_eq!(ab8_hash.len(), 8, "abbrev=8 hash: {ab8_out:?}");
    assert!(
        full_hash.starts_with(ab8_hash),
        "abbrev is a prefix of the full hash"
    );

    // Bare --abbrev defaults to 7.
    let ab = run_libra_command(&["ls-files", "-s", "--abbrev", "f.txt"], p);
    assert_cli_success(&ab, "ls-files -s --abbrev");
    let ab_out = String::from_utf8_lossy(&ab.stdout);
    assert_eq!(
        ab_out.split_whitespace().nth(1).expect("hash").len(),
        7,
        "bare --abbrev = 7: {ab_out:?}"
    );
}
