use super::prelude::*;

pub(crate) fn scenario_init_branch_and_format_options(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    ctx.command(
        &["init", "-b", "develop", "init-branch-short"],
        ctx.run_dir.clone(),
        true,
    )?;
    let short = ctx.run_dir.join("init-branch-short");
    let branches = ctx.command(&["branch"], short.clone(), true)?;
    assert_stdout_contains(&branches, "develop")?;
    ctx.command(&["status"], short, true)?;

    ctx.command(
        &["init", "--initial-branch", "trunk", "init-branch-long"],
        ctx.run_dir.clone(),
        true,
    )?;
    let long = ctx.run_dir.join("init-branch-long");
    let branches = ctx.command(&["branch"], long, true)?;
    assert_stdout_contains(&branches, "trunk")?;

    for (format, dir) in [("sha1", "object-sha1"), ("sha256", "object-sha256")] {
        ctx.command(
            &["init", "--object-format", format, dir],
            ctx.run_dir.clone(),
            true,
        )?;
        let repo = ctx.run_dir.join(dir);
        let value = ctx.command(&["config", "get", "core.objectformat"], repo.clone(), true)?;
        assert_stdout_contains(&value, format)?;
        let json = ctx.command(
            &["--json", "config", "get", "core.objectformat"],
            repo.clone(),
            true,
        )?;
        assert_json_ok(&json, "config")?;
        ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    }
    for (format, dir) in [("strict", "ref-strict"), ("filesystem", "ref-filesystem")] {
        ctx.command(
            &["init", "--ref-format", format, dir],
            ctx.run_dir.clone(),
            true,
        )?;
        let repo = ctx.run_dir.join(dir);
        let value = ctx.command(&["config", "get", "core.initrefformat"], repo.clone(), true)?;
        assert_stdout_contains(&value, format)?;
        ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    }
    let bad_object = ctx.command(
        &["init", "--object-format", "sha265", "bad-object-format"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_object, "object format")?;
    let bad_ref = ctx.command(
        &["init", "--ref-format", "unknown", "bad-ref-format"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_ref, "ref format")?;
    let bad_branch = ctx.command(
        &["init", "-b", "bad branch", "bad-branch-name"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_branch, "branch")?;
    Ok(())
}
