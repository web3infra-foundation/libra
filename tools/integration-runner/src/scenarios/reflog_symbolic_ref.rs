use super::prelude::*;

pub(crate) fn scenario_reflog_symbolic_ref(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let head = ctx.command(&["symbolic-ref", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&head, "refs/heads/main")?;
    ctx.command(&["branch", "other"], repo.clone(), true)?;
    ctx.command(
        &["symbolic-ref", "HEAD", "refs/heads/other"],
        repo.clone(),
        true,
    )?;
    let head = ctx.command(&["symbolic-ref", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&head, "refs/heads/other")?;
    let short = ctx.command(&["symbolic-ref", "--short", "HEAD"], repo.clone(), true)?;
    if stdout_trim(&short) != "other" {
        bail!(
            "symbolic-ref --short HEAD expected bare `other`, got {:?}",
            stdout_trim(&short)
        );
    }

    // Second commit so the reflog has commit + switch entries and -p/--stat
    // have a parent tree to diff against.
    fs::write(repo.join("tracked.txt"), "base\nmore\n").context("update tracked file")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "reflog second", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let reflog = ctx.command(&["reflog", "show"], repo.clone(), true)?;
    assert_stdout_contains(&reflog, "commit: reflog second")?;
    assert_stdout_contains(&reflog, "switch: moving from main to other")?;
    assert_not_contains(&reflog, "PRIVATE KEY")?;
    let by_ref = ctx.command(&["reflog", "show", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&by_ref, "HEAD@{0}: commit: reflog second")?;
    let oneline = ctx.command(
        &["reflog", "show", "--pretty", "oneline"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&oneline, "HEAD@{0}: commit: reflog second")?;
    let stat = ctx.command(
        &["reflog", "show", "--stat", "-n", "1"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&stat, "tracked.txt |")?;
    assert_stdout_contains(&stat, "1 insertion(+)")?;
    let patch = ctx.command(&["reflog", "show", "-p", "-n", "1"], repo.clone(), true)?;
    assert_stdout_contains(&patch, "+++ b/tracked.txt")?;
    assert_stdout_contains(&patch, "+more")?;
    let grep = ctx.command(
        &["reflog", "show", "--grep", "second"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&grep, "commit: reflog second")?;
    assert_not_contains(&grep, "commit: initial")?;
    let author = ctx.command(
        &["reflog", "show", "--author", "Libra Integration"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&author, "commit: reflog second")?;
    let until = ctx.command(
        &["reflog", "show", "--until", "2000-01-01"],
        repo.clone(),
        true,
    )?;
    if !stdout_trim(&until).is_empty() {
        bail!(
            "reflog show --until 2000-01-01 expected empty output, got {:?}",
            stdout_trim(&until)
        );
    }
    // Intentional difference: `reflog show <missing-ref>` returns an empty
    // list with exit 0 instead of failing (cannot be a negative assertion).
    let missing_show = ctx.command(
        &["reflog", "show", "refs/heads/no-such-branch"],
        repo.clone(),
        true,
    )?;
    if !stdout_trim(&missing_show).is_empty() {
        bail!(
            "reflog show on a missing ref expected empty output, got {:?}",
            stdout_trim(&missing_show)
        );
    }

    ctx.command(&["reflog", "exists", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(
            &[
                "--json",
                "reflog",
                "expire",
                "--all",
                "--dry-run",
                "--expire=all",
            ],
            repo.clone(),
            true,
        )?,
        "reflog.expire",
    )?;
    // Explicit-ref expire carrying the full dry-run-safe flag batch.
    let expire_head = ctx.command(
        &[
            "--json",
            "reflog",
            "expire",
            "HEAD",
            "--dry-run",
            "--expire=all",
            "--expire-unreachable=all",
            "--rewrite",
            "--updateref",
            "--stale-fix",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&expire_head, "reflog.expire")?;
    assert_stdout_contains(&expire_head, "\"ref_name\": \"HEAD\"")?;
    let expire_verbose = ctx.command(
        &["reflog", "expire", "--all", "--dry-run", "--expire=all", "-v"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&expire_verbose, "expired HEAD@{0}")?;
    assert_stdout_contains(&expire_verbose, "pruned")?;
    // Every expire above was a dry run: the reflog must be intact.
    let after = ctx.command(&["reflog", "show"], repo.clone(), true)?;
    assert_stdout_contains(&after, "commit: reflog second")?;

    assert_json_ok(
        &ctx.command(&["--json", "show-ref", "--heads"], repo.clone(), true)?,
        "show-ref",
    )?;

    let bad = ctx.command(
        &["symbolic-ref", "refs/custom", "refs/heads/main"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad, "HEAD")?;
    let bad_target = ctx.command(
        &["symbolic-ref", "HEAD", "refs/tags/not-a-branch"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_target, "unsupported symbolic ref target")?;
    // Intentional difference: deleting a symbolic ref is rejected in Libra.
    let delete = ctx.command(&["symbolic-ref", "-d", "HEAD"], repo.clone(), false)?;
    assert_lbr_or_text(&delete, "intentionally unsupported")?;
    let missing_exists = ctx.command(
        &["reflog", "exists", "refs/heads/no-such-branch"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing_exists, "not found")?;
    let missing_expire = ctx.command(
        &[
            "reflog",
            "expire",
            "refs/heads/no-such",
            "--dry-run",
            "--expire=all",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing_expire, "not found")?;
    let no_ref_expire = ctx.command(&["--json", "reflog", "expire"], repo.clone(), false)?;
    assert_json_error_code(&no_ref_expire, "LBR-CLI-002")?;
    Ok(())
}
