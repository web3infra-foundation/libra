use super::prelude::*;

pub(crate) fn scenario_ls_tree_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("ls-tree-repo");
    create_committed_repo(ctx, &repo)?;
    fs::create_dir_all(repo.join("src/nested")).context("create ls-tree nested fixture dir")?;
    fs::write(repo.join("src/nested/deep.txt"), "deep\n")
        .context("write ls-tree nested fixture")?;
    ensure_file(repo.join("src/nested/deep.txt"))?;
    let missing = ctx.command(&["--json", "ls-tree", "HEAD"], repo.clone(), false)?;
    assert_json_error_code(&missing, "LBR-CLI-001")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
