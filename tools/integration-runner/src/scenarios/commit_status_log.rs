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
    Ok(())
}
