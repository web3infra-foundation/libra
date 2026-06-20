use super::prelude::*;

pub(crate) fn scenario_commit_status_log(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    ctx.command(&["init", "repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Integration"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "integration@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("tracked.txt"), "hello\n").context("write tracked fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "initial", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let log = ctx.command(&["log", "--oneline"], repo.clone(), true)?;
    assert_stdout_contains(&log, "initial")?;
    let filtered_log = ctx.command(
        &[
            "log",
            "-n",
            "1",
            "--name-status",
            "--grep",
            "initial",
            "--author",
            "Libra Integration",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&filtered_log, "initial")?;
    assert_stdout_contains(&filtered_log, "tracked.txt")?;
    let stat_log = ctx.command(&["log", "--stat", "-n", "3"], repo.clone(), true)?;
    assert_stdout_contains(&stat_log, "tracked.txt")?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    let empty = ctx.command(
        &["commit", "-m", "empty", "--no-verify"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&empty, "nothing to commit")?;

    fs::rename(repo.join("tracked.txt"), repo.join("renamed.txt"))
        .context("rename tracked fixture")?;
    ctx.command(&["add", "renamed.txt"], repo.clone(), true)?;
    ctx.command(&["rm", "--cached", "tracked.txt"], repo.clone(), true)?;
    let rename_short = ctx.command(&["status", "--short"], repo.clone(), true)?;
    assert_stdout_contains(&rename_short, "A  renamed.txt")?;
    assert_stdout_contains(&rename_short, "D  tracked.txt")?;
    let rename_v2 = ctx.command(&["status", "--porcelain", "v2"], repo.clone(), true)?;
    assert_stdout_contains(&rename_v2, "1 A  ")?;
    assert_stdout_contains(&rename_v2, "renamed.txt")?;
    assert_stdout_contains(&rename_v2, "1 D  ")?;
    assert_stdout_contains(&rename_v2, "tracked.txt")?;
    let rename_v2_z = ctx.command(&["status", "--porcelain", "v2", "-z"], repo.clone(), false)?;
    assert_lbr_or_text(&rename_v2_z, "-z")?;
    let rename_short_z = ctx.command(&["status", "-z", "-s"], repo.clone(), false)?;
    assert_lbr_or_text(&rename_short_z, "-z")?;
    let rename_json = ctx.command(&["--json", "status"], repo.clone(), true)?;
    assert_json_ok(&rename_json, "status")?;
    ctx.command(
        &["commit", "-m", "rename tracked", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let path_log = ctx.command(&["log", "--oneline", "renamed.txt"], repo.clone(), true)?;
    assert_stdout_contains(&path_log, "rename tracked")?;
    let follow_log = ctx.command(
        &["log", "--follow", "--oneline", "renamed.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&follow_log, "--follow")?;
    let name_status = ctx.command(&["log", "--name-status", "renamed.txt"], repo.clone(), true)?;
    assert_stdout_contains(&name_status, "renamed.txt")?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "renamed.txt"], repo.clone(), true)?,
        "log",
    )?;

    fs::create_dir_all(repo.join("scratch")).context("create scratch dir")?;
    fs::write(repo.join("scratch").join("note.txt"), "untracked\n")
        .context("write untracked scratch file")?;
    let hidden_untracked = ctx.command(
        &["status", "--short", "--untracked-files=no"],
        repo.clone(),
        true,
    )?;
    assert_stdout_not_contains(&hidden_untracked, "scratch")?;
    let override_untracked = ctx.command(
        &["status", "--short", "--untracked-files=all"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&override_untracked, "?? scratch/note.txt")?;
    let branch_short = ctx.command(&["status", "--short", "--branch"], repo.clone(), true)?;
    assert_stdout_contains(&branch_short, "## main")?;

    #[cfg(unix)]
    {
        fs::write(repo.join("type-target.txt"), "target\n").context("write type target")?;
        ctx.command(&["add", "type-target.txt"], repo.clone(), true)?;
        ctx.command(
            &["commit", "-m", "add type target", "--no-verify"],
            repo.clone(),
            true,
        )?;
        fs::remove_file(repo.join("type-target.txt")).context("remove type target")?;
        std::os::unix::fs::symlink("renamed.txt", repo.join("type-target.txt"))
            .context("create typechange symlink")?;
        let typechange_v2 = ctx.command(&["status", "--porcelain", "v2"], repo.clone(), true)?;
        assert_stdout_contains(&typechange_v2, "120000")?;
        assert_stdout_contains(&typechange_v2, "type-target.txt")?;
    }
    Ok(())
}

fn assert_stdout_not_contains(output: &std::process::Output, unexpected: &str) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains(unexpected) {
        bail!("stdout unexpectedly contained {unexpected:?}: {stdout}");
    }
    Ok(())
}
