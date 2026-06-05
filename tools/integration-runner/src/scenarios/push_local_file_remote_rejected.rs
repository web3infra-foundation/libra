use super::prelude::*;

pub(crate) fn scenario_push_local_file_remote_rejected(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let remote_dir = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("remote.git");
    let work_dir = ctx.run_dir.join("work");
    let remote = remote_dir.to_string_lossy().to_string();
    let work = work_dir.to_string_lossy().to_string();
    ctx.command(&["init", "--bare", &remote], ctx.run_dir.clone(), true)?;
    ctx.command(&["init", &work], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Push Rejection Test"],
        work_dir.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "push-reject@example.invalid"],
        work_dir.clone(),
        true,
    )?;
    fs::write(work_dir.join("push.txt"), "push\n").context("write push fixture")?;
    ctx.command(&["add", "push.txt"], work_dir.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: push rejection base", "--no-verify"],
        work_dir.clone(),
        true,
    )?;
    ctx.command(
        &["remote", "add", "origin", &remote],
        work_dir.clone(),
        true,
    )?;
    ctx.command(
        &["remote", "set-url", "--add", "--push", "origin", &remote],
        work_dir.clone(),
        true,
    )?;
    let urls = ctx.command(
        &["--json", "remote", "get-url", "--all", "origin"],
        work_dir.clone(),
        true,
    )?;
    assert_json_ok(&urls, "remote")?;
    assert_stdout_contains(&urls, &remote)?;

    for args in [
        vec!["--json=compact", "push", "origin", "main"],
        vec!["--json=compact", "push", "--dry-run", "origin", "main"],
        vec!["--json=compact", "push", "--force", "origin", "main"],
        vec!["--json=compact", "push", "--tags", "origin"],
        vec!["--json=compact", "push", "--mirror", "--dry-run", "origin"],
    ] {
        let output = ctx.command(&args, work_dir.clone(), false)?;
        assert_json_error_code(&output, "LBR-CLI-003")?;
        assert_lbr_or_text(&output, "local file")?;
        ctx.command(&["fsck", "--connectivity-only"], work_dir.clone(), true)?;
    }
    Ok(())
}
