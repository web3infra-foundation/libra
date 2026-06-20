use super::{
    assert_cli_success, create_committed_repo_via_cli, init_repo_via_cli, parse_cli_error_stderr,
    parse_json_stdout, run_libra_command,
};

#[test]
fn publish_sync_without_site_id_returns_invalid_arguments_without_clap_json_panic() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    for args in [&["publish", "sync"][..], &["--json", "publish", "sync"][..]] {
        let output = run_libra_command(args, repo.path());
        assert!(
            !output.status.success(),
            "{args:?} should fail when publish.site_id is not configured"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked"),
            "{args:?} must not panic on the JSON envelope path: {stderr}"
        );

        let (_, report) = parse_cli_error_stderr(&output.stderr);
        assert_eq!(report.error_code, "LBR-CLI-002");
        assert!(
            report.message.contains("publish.site_id"),
            "{args:?} should explain that publish.site_id is required: {stderr}"
        );
    }
}

#[test]
fn publish_unpublish_requires_confirmation_before_cloud_steps() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(
        &[
            "publish",
            "unpublish",
            "--site-id",
            "00000000-0000-0000-0000-000000000001",
        ],
        repo.path(),
    );

    assert!(
        !output.status.success(),
        "publish unpublish should require explicit --yes confirmation"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("--yes"),
        "error should explain the confirmation requirement: {:?}",
        report.message
    );
}

#[test]
fn publish_deploy_requires_worker_template_before_cloud_steps() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["publish", "deploy", "--skip-deploy"], repo.path());

    assert!(
        !output.status.success(),
        "publish deploy should fail fast when the Worker template is missing"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        report.message.contains("Worker template"),
        "error should explain that publish init is required: {:?}",
        report.message
    );
}

#[test]
fn publish_status_reports_local_template_state() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "publish", "status"], repo.path());

    assert!(
        output.status.success(),
        "publish status should inspect the local template state: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"missing\""),
        "status before publish init should be missing: {stdout}"
    );
    assert!(
        stdout.contains("\"publishedRefs\":"),
        "status JSON should include the cloud ref comparison envelope: {stdout}"
    );
    assert!(
        stdout.contains("\"state\": \"unconfigured\""),
        "status without a site id should make the cloud ref comparison state explicit: {stdout}"
    );
}

#[test]
fn publish_sync_dry_run_reports_local_ref_plan_without_cloud_writes() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0.0"], repo.path());
    assert_cli_success(&tag, "create publish dry-run tag");

    let output = run_libra_command(&["--json", "publish", "sync", "--dry-run"], repo.path());
    assert_cli_success(&output, "publish sync dry-run should produce a local plan");

    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["dryRun"], true);
    assert_eq!(data["refsCount"], 2);
    assert_eq!(data["revisionCount"], 1);
    assert_eq!(data["defaultRef"], "refs/heads/main");
    assert!(data["latestRevisionOid"].as_str().is_some());
    assert!(data["fileCount"].as_u64().unwrap_or_default() > 0);
    assert_eq!(data["aiObjectCount"], 0);
    assert_eq!(data["aiBundleCount"], 0);
    assert_eq!(data["updatesFullRefsGeneration"], true);
    assert!(
        !repo.path().join(".libra/publish").exists(),
        "dry-run must not create publish manifests or cloud sync state"
    );
}

#[test]
fn publish_sync_dry_run_ref_filters_to_full_ref() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0.0"], repo.path());
    assert_cli_success(&tag, "create publish dry-run tag");

    let output = run_libra_command(
        &[
            "--json",
            "publish",
            "sync",
            "--dry-run",
            "--ref",
            "refs/tags/v1.0.0",
        ],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "publish sync dry-run --ref should produce a local plan",
    );

    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["refsCount"], 1);
    assert_eq!(data["selectedRef"], "refs/tags/v1.0.0");
    assert_eq!(data["updatesFullRefsGeneration"], false);
    assert_eq!(data["refs"][0]["refName"], "refs/tags/v1.0.0");
}

#[test]
fn publish_sync_dry_run_ambiguous_short_ref_requires_full_ref() {
    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "release"], repo.path());
    assert_cli_success(&branch, "create release branch");
    let tag = run_libra_command(&["tag", "release"], repo.path());
    assert_cli_success(&tag, "create release tag");

    let output = run_libra_command(
        &["--json", "publish", "sync", "--dry-run", "--ref", "release"],
        repo.path(),
    );

    assert!(
        !output.status.success(),
        "ambiguous short ref should fail instead of choosing branch or tag"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("ambiguous publish ref"),
        "error should identify the short ref collision: {:?}",
        report.message
    );
}

#[test]
fn publish_sync_dry_run_fail_on_dirty_rejects_dirty_tree() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "dirty\n").expect("dirty tracked file");

    let output = run_libra_command(
        &["--json", "publish", "sync", "--dry-run", "--fail-on-dirty"],
        repo.path(),
    );

    assert!(
        !output.status.success(),
        "publish sync --dry-run --fail-on-dirty should reject a dirty tree"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        report.message.contains("dirty working tree"),
        "error should explain the dirty tree: {:?}",
        report.message
    );
}

#[test]
fn publish_sync_dry_run_warns_for_builtin_sensitive_paths() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join(".env.local"), "SECRET=1\n").expect("write env file");
    let add = run_libra_command(&["add", ".env.local"], repo.path());
    assert_cli_success(&add, "stage sensitive file fixture");
    let commit = run_libra_command(
        &["commit", "-m", "add sensitive fixture", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&commit, "commit sensitive file fixture");

    let output = run_libra_command(&["--json", "publish", "sync", "--dry-run"], repo.path());
    assert_cli_success(&output, "publish sync dry-run should warn but still plan");

    let json = parse_json_stdout(&output);
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains(".env.local") && text.contains("builtin"))),
        "dry-run warnings should identify builtin sensitive path: {warnings:?}"
    );
}

#[test]
fn publish_sync_dry_run_warns_for_librapublishignore_paths() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join(".librapublishignore"), "secret.txt\n")
        .expect("write publish ignore");
    std::fs::write(repo.path().join("secret.txt"), "redacted\n").expect("write ignored file");
    let add = run_libra_command(&["add", ".librapublishignore", "secret.txt"], repo.path());
    assert_cli_success(&add, "stage publish ignore fixture");
    let commit = run_libra_command(
        &["commit", "-m", "add publish ignore fixture", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&commit, "commit publish ignore fixture");

    let output = run_libra_command(&["--json", "publish", "sync", "--dry-run"], repo.path());
    assert_cli_success(&output, "publish sync dry-run should warn but still plan");

    let json = parse_json_stdout(&output);
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("secret.txt") && text.contains("user_ignore"))),
        "dry-run warnings should identify .librapublishignore path: {warnings:?}"
    );
}

/// `libra publish --help` surfaces the EXAMPLES banner so users see
/// the canonical invocation per sub-command (`init` / `sync` /
/// `status` / `deploy` / `unpublish`) plus a dry-run sync, a
/// sensitive-path allowance, a site-scoped status, and a JSON variant
/// for agents without reading the design doc. Cross-cutting `--help`
/// EXAMPLES rollout per `docs/development/commands/_general.md` item B.
#[test]
fn test_publish_help_lists_examples_banner() {
    let repo = tempfile::tempdir().expect("tempdir for publish --help");
    let output = run_libra_command(&["publish", "--help"], repo.path());
    assert!(
        output.status.success(),
        "publish --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "publish --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra publish init",
        "libra publish status",
        "libra publish status --site-id",
        "libra publish sync --dry-run",
        "libra publish sync --ref refs/heads/main",
        "libra publish sync --force",
        "libra publish sync --allow-sensitive-path",
        "libra publish deploy",
        "libra publish deploy --skip-deploy",
        "libra publish unpublish --site-id",
        "libra publish --json sync",
    ] {
        assert!(
            stdout.contains(invocation),
            "publish --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
