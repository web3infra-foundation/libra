use super::prelude::*;

pub(crate) fn scenario_restore_reset_diff(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    fs::write(repo.join("tracked.txt"), "modified\n").context("modify tracked file")?;
    assert_stdout_contains(&ctx.command(&["diff"], repo.clone(), true)?, "modified")?;
    assert_stdout_contains(
        &ctx.command(&["diff", "tracked.txt"], repo.clone(), true)?,
        "modified",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--name-only"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--stat"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--numstat"], repo.clone(), true)?,
        "1\t1",
    )?;

    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged"], repo.clone(), true)?,
        "modified",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged", "--name-status"], repo.clone(), true)?,
        "M\ttracked.txt",
    )?;
    ctx.command(&["restore", "--staged", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["restore", "--worktree", "tracked.txt"],
        repo.clone(),
        true,
    )?;
    assert_file_content(&repo, "tracked.txt", "base\n")?;

    let restore_from_file = ctx.command(
        &[
            "--json",
            "restore",
            "--pathspec-from-file=restore-paths.txt",
        ],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&restore_from_file, "LBR-CLI-002")?;

    fs::write(repo.join("tracked.txt"), "second\n").context("modify tracked second")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["reset", "HEAD", "--", "tracked.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--name-only"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["diff", "--old", "HEAD~1", "--new", "HEAD", "--numstat"],
            repo.clone(),
            true,
        )?,
        "tracked.txt",
    )?;

    fs::write(repo.join("tracked.txt"), "source probe\n").context("write source probe")?;
    ctx.command(
        &["restore", "--source", "HEAD~1", "tracked.txt"],
        repo.clone(),
        true,
    )?;
    assert_file_content(&repo, "tracked.txt", "base\n")?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    assert_file_content(&repo, "tracked.txt", "second\n")?;

    ctx.command(&["reset", "--soft", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["reset", "--mixed", "HEAD"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    assert_file_content(&repo, "tracked.txt", "base\n")?;

    let reset_from_file = ctx.command(
        &["--json", "reset", "--pathspec-from-file=reset-paths.txt"],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&reset_from_file, "LBR-CLI-002")?;
    let reset_keep = ctx.command(&["--json", "reset", "--keep", "HEAD"], repo.clone(), false)?;
    assert_json_error_code(&reset_keep, "LBR-CLI-002")?;
    let reset_merge = ctx.command(&["--json", "reset", "--merge", "HEAD"], repo.clone(), false)?;
    assert_json_error_code(&reset_merge, "LBR-CLI-002")?;

    let overlay_restore = ctx.command(
        &[
            "--json",
            "restore",
            "--source",
            "HEAD",
            "--overlay",
            "tracked.txt",
        ],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&overlay_restore, "LBR-CLI-002")?;
    let no_overlay_restore = ctx.command(
        &[
            "--json",
            "restore",
            "--source",
            "HEAD",
            "--no-overlay",
            "tracked.txt",
        ],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&no_overlay_restore, "LBR-CLI-002")?;

    fs::write(repo.join("tracked.txt"), "diff output probe\n")
        .context("dirty tracked for --output")?;
    assert_json_ok(
        &ctx.command(&["--json", "diff"], repo.clone(), true)?,
        "diff",
    )?;
    let diff_to_file = ctx.command(
        &["diff", "--output", "diff-out.patch", "tracked.txt"],
        repo.clone(),
        true,
    )?;
    if String::from_utf8_lossy(&diff_to_file.stdout).contains("@@") {
        bail!("diff --output must not emit hunks on stdout");
    }
    let patch =
        fs::read_to_string(repo.join("diff-out.patch")).context("read diff --output file")?;
    if !patch.contains("diff --git") || !patch.contains("@@") {
        bail!("diff --output file should contain the patch, got: {patch:?}");
    }
    assert_stdout_contains(
        &ctx.command(
            &["diff", "--algorithm=histogram", "tracked.txt"],
            repo.clone(),
            true,
        )?,
        "diff output probe",
    )?;
    let bad_algorithm = ctx.command(
        &["diff", "--algorithm", "myers", "tracked.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_algorithm, "not supported yet")?;
    fs::remove_file(repo.join("diff-out.patch")).context("remove diff --output file")?;

    let bad_diff = ctx.command(
        &["diff", "--old", "no-such-revision", "--new", "HEAD"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_diff, "invalid revision")?;
    let bad_restore = ctx.command(&["restore", "nonexistent.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_restore, "pathspec")?;
    let tracked_before =
        fs::read_to_string(repo.join("tracked.txt")).context("read tracked before bad source")?;
    let bad_source = ctx.command(
        &["restore", "--source", "no-such-revision", "tracked.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_source, "failed to resolve checkout source")?;
    let tracked_after =
        fs::read_to_string(repo.join("tracked.txt")).context("read tracked after bad source")?;
    if tracked_before != tracked_after {
        bail!("restore --source <bad revision> must not modify the worktree");
    }
    let head_before = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    let bad_reset = ctx.command(&["reset", "--hard", "no-such-rev"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_reset, "invalid reference")?;
    let head_after = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    if head_before != head_after {
        bail!("reset --hard <bad revision> must not move HEAD");
    }

    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}

fn assert_file_content(repo: &Path, path: &str, expected: &str) -> Result<()> {
    let actual = fs::read_to_string(repo.join(path)).with_context(|| format!("read {path}"))?;
    if actual != expected {
        bail!("{path} content mismatch: expected {expected:?}, got {actual:?}");
    }
    Ok(())
}
