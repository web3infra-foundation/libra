use super::{
    rev_list_cherry_filter_test::{assert_same_lines, create_cherry_filter_repo, stdout_lines},
    *,
};

#[test]
fn test_rev_list_cherry_shorthand_matches_git_right_side_marking() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(&["rev-list", "--cherry", "main...right"], graph.repo.path());
    assert_cli_success(&output, "rev-list --cherry main...right");

    assert_same_lines(
        stdout_lines(&output),
        vec![
            format!("={}", graph.right_same_id),
            format!("+{}", graph.right_unique_id),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(&graph.left_same_id));
    assert!(!stdout.contains(&graph.left_unique_id));
}

#[test]
fn test_rev_list_left_right_cherry_uses_git_marker_precedence() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["rev-list", "--left-right", "--cherry", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "rev-list --left-right --cherry main...right");

    assert_same_lines(
        stdout_lines(&output),
        vec![
            format!("={}", graph.right_same_id),
            format!(">{}", graph.right_unique_id),
        ],
    );
}

#[test]
fn test_rev_list_cherry_count_uses_git_compatible_fields() {
    let graph = create_cherry_filter_repo();

    let cherry = run_libra_command(
        &["rev-list", "--count", "--cherry", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&cherry, "rev-list --count --cherry main...right");
    assert_eq!(String::from_utf8_lossy(&cherry.stdout).trim(), "1\t1");

    let left_right_cherry = run_libra_command(
        &[
            "rev-list",
            "--count",
            "--left-right",
            "--cherry",
            "main...right",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &left_right_cherry,
        "rev-list --count --left-right --cherry main...right",
    );
    assert_eq!(
        String::from_utf8_lossy(&left_right_cherry.stdout).trim(),
        "0\t1\t1"
    );
}

#[test]
fn test_rev_list_json_includes_cherry_shorthand_flags_and_entries() {
    let graph = create_cherry_filter_repo();

    let output = run_libra_command(
        &["--json", "rev-list", "--cherry", "main...right"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "--json rev-list --cherry main...right");
    let json = parse_json_stdout(&output);

    assert_eq!(json["data"]["cherry"], true);
    assert_eq!(json["data"]["cherry_mark"], false);
    assert_eq!(json["data"]["commits"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["data"]["entries"].as_array().map(Vec::len), Some(2));
}
