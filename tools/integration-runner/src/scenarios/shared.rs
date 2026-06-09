use super::prelude::*;

pub(crate) fn create_committed_repo(ctx: &mut ScenarioCtx<'_>, repo: &Path) -> Result<()> {
    let repo_arg = repo.to_string_lossy().to_string();
    ctx.command(&["init", &repo_arg], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Integration"],
        repo.to_path_buf(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "integration@example.invalid"],
        repo.to_path_buf(),
        true,
    )?;
    fs::write(repo.join("tracked.txt"), "base\n").context("write base tracked file")?;
    ctx.command(&["add", "tracked.txt"], repo.to_path_buf(), true)?;
    ctx.command(
        &["commit", "-m", "initial", "--no-verify"],
        repo.to_path_buf(),
        true,
    )?;
    Ok(())
}
