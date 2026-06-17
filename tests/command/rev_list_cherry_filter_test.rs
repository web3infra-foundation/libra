use super::*;

struct CherryFilterRepo {
    repo: tempfile::TempDir,
    left_same_id: String,
    left_unique_id: String,
    right_same_id: String,
    right_unique_id: String,
}

fn create_cherry_filter_repo() -> CherryFilterRepo {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("base.txt"), "base\n").expect("failed to write base.txt");
    let output = run_libra_command(&["add", "base.txt"], repo.path());
    assert_cli_success(&output, "add base.txt");
    let output = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit base");

    let output = run_libra_command(&["branch", "right"], repo.path());
    assert_cli_success(&output, "create right branch");

    fs::write(repo.path().join("same.txt"), "same\n").expect("failed to write same.txt");
    let output = run_libra_command(&["add", "same.txt"], repo.path());
    assert_cli_success(&output, "add same.txt on left");
    let output = run_libra_command(&["commit", "-m", "left same", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit left same");
    let left_same_id = head_commit_id(repo.path());

    fs::write(repo.path().join("left.txt"), "left\n").expect("failed to write left.txt");
    let output = run_libra_command(&["add", "left.txt"], repo.path());
    assert_cli_success(&output, "add left.txt");
    let output = run_libra_command(&["commit", "-m", "left unique", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit left unique");
    let left_unique_id = head_commit_id(repo.path());

    let output = run_libra_command(&["switch", "right"], repo.path());
    assert_cli_success(&output, "switch to right branch");

    fs::write(repo.path().join("same.txt"), "same\n").expect("failed to write same.txt");
    let output = run_libra_command(&["add", "same.txt"], repo.path());
    assert_cli_success(&output, "add same.txt on right");
    let output = run_libra_command(&["commit", "-m", "right same", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit right same");
    let right_same_id = head_commit_id(repo.path());

    fs::write(repo.path().join("right.txt"), "right\n").expect("failed to write right.txt");
    let output = run_libra_command(&["add", "right.txt"], repo.path());
    assert_cli_success(&output, "add right.txt");
    let output = run_libra_command(
        &["commit", "-m", "right unique", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "commit right unique");
    let right_unique_id = head_commit_id(repo.path());

    CherryFilterRepo {
        repo,
        left_same_id,
        left_unique_id,
        right_same_id,
        right_unique_id,
    }
}

fn head_commit_id(repo: &Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_same_lines(mut actual: Vec<String>, mut expected: Vec<String>) {
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn test_rev_list_left_right_marks_symmetric_difference_sides() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["rev-list", "--left-right", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "rev-list --left-right main...right");

    assert_same_lines(
        stdout_lines(&output),
        vec![
            format!("<{}", graph.left_same_id),
            format!("<{}", graph.left_unique_id),
            format!(">{}", graph.right_same_id),
            format!(">{}", graph.right_unique_id),
        ],
    );
}

#[test]
fn test_rev_list_side_only_filters_symmetric_difference_side() {
    let graph = create_cherry_filter_repo();

    let right = run_libra_command(
        &["rev-list", "--right-only", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&right, "rev-list --right-only main...right");
    assert_same_lines(
        stdout_lines(&right),
        vec![graph.right_same_id.clone(), graph.right_unique_id.clone()],
    );

    let left = run_libra_command(
        &["rev-list", "--left-only", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&left, "rev-list --left-only main...right");
    assert_same_lines(
        stdout_lines(&left),
        vec![graph.left_same_id.clone(), graph.left_unique_id.clone()],
    );
}

#[test]
fn test_rev_list_cherry_pick_omits_patch_equivalent_sides() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["rev-list", "--cherry-pick", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "rev-list --cherry-pick main...right");

    assert_same_lines(
        stdout_lines(&output),
        vec![graph.left_unique_id.clone(), graph.right_unique_id.clone()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(&graph.left_same_id));
    assert!(!stdout.contains(&graph.right_same_id));
}

#[test]
fn test_rev_list_cherry_mark_marks_equivalent_and_unique_commits() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["rev-list", "--cherry-mark", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "rev-list --cherry-mark main...right");

    assert_same_lines(
        stdout_lines(&output),
        vec![
            format!("={}", graph.left_same_id),
            format!("={}", graph.right_same_id),
            format!("+{}", graph.left_unique_id),
            format!("+{}", graph.right_unique_id),
        ],
    );
}

#[test]
fn test_rev_list_count_uses_git_compatible_side_fields() {
    let graph = create_cherry_filter_repo();

    let left_right = run_libra_command(
        &["rev-list", "--count", "--left-right", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&left_right, "rev-list --count --left-right main...right");
    assert_eq!(String::from_utf8_lossy(&left_right.stdout).trim(), "2\t2");

    let cherry_mark = run_libra_command(
        &["rev-list", "--count", "--cherry-mark", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&cherry_mark, "rev-list --count --cherry-mark main...right");
    assert_eq!(String::from_utf8_lossy(&cherry_mark.stdout).trim(), "2\t2");

    let combined = run_libra_command(
        &[
            "rev-list",
            "--count",
            "--left-right",
            "--cherry-mark",
            "main...right",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &combined,
        "rev-list --count --left-right --cherry-mark main...right",
    );
    assert_eq!(String::from_utf8_lossy(&combined.stdout).trim(), "1\t1\t2");
}

#[test]
fn test_rev_list_json_includes_cherry_filter_flags_and_entries() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["--json", "rev-list", "--left-right", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "--json rev-list --left-right main...right");
    let json = parse_json_stdout(&output);

    assert_eq!(json["data"]["left_right"], true);
    assert_eq!(json["data"]["left_only"], false);
    assert_eq!(json["data"]["right_only"], false);
    assert_eq!(json["data"]["cherry_pick"], false);
    assert_eq!(json["data"]["entries"].as_array().map(Vec::len), Some(4));
}
