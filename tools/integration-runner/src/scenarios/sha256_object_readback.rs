use super::prelude::*;

pub(crate) fn scenario_sha256_object_readback(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("sha256-repo");
    let repo_arg = repo.to_string_lossy().to_string();
    ctx.command(
        &["init", "--object-format", "sha256", &repo_arg],
        ctx.run_dir.clone(),
        true,
    )?;
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
    fs::write(repo.join("sha.txt"), "sha256\n").context("write sha fixture")?;
    ctx.command(&["add", "sha.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "sha256", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let format = ctx.command(&["config", "get", "core.objectformat"], repo.clone(), true)?;
    assert_stdout_contains(&format, "sha256")?;
    let head = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    if head.len() != 64 {
        bail!("sha256 HEAD id was not 64 hex chars: {head}");
    }
    let cat = ctx.command(&["cat-file", "-t", &head], repo.clone(), true)?;
    assert_stdout_contains(&cat, "commit")?;
    assert_stdout_contains(
        &ctx.command(&["show", "HEAD:sha.txt"], repo.clone(), true)?,
        "sha256",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "--oneline"], repo.clone(), true)?,
        "log",
    )?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
