use super::prelude::*;

pub(crate) fn scenario_init_bare_and_shared(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    ctx.command(&["init", "--bare", "bare-repo"], ctx.run_dir.clone(), true)?;
    let bare = ctx.run_dir.join("bare-repo");
    ensure_file(bare.join("libra.db"))?;
    ensure_file(bare.join("objects"))?;
    if bare.join(".libra").exists() {
        bail!("bare init unexpectedly created .libra");
    }
    let status = ctx.command(&["status"], bare, false)?;
    assert_lbr_or_text(&status, "not a libra repository")?;

    for (mode, dir) in [
        ("false", "shared-false"),
        ("true", "shared-true"),
        ("umask", "shared-umask"),
        ("group", "shared-group"),
        ("all", "shared-all"),
        ("world", "shared-world"),
        ("everybody", "shared-everybody"),
        ("0770", "shared-octal"),
    ] {
        let shared_arg = format!("--shared={mode}");
        ctx.command(&["init", shared_arg.as_str(), dir], ctx.run_dir.clone(), true)?;
        let repo = ctx.run_dir.join(dir);
        ctx.command(&["--json", "db", "status"], repo.clone(), true)?;
        ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    }
    let invalid = ctx.command(
        &["init", "--shared=invalid", "shared-invalid"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&invalid, "shared")?;
    let bad_octal = ctx.command(
        &["init", "--shared=8888", "shared-bad-octal"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_octal, "shared")?;
    Ok(())
}
