use super::prelude::*;

pub(crate) fn scenario_ls_tree_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("ls-tree-repo");
    create_committed_repo(ctx, &repo)?;

    fs::create_dir_all(repo.join("src/nested")).context("create ls-tree nested fixture dir")?;
    fs::write(repo.join("README.md"), "root\n").context("write ls-tree README fixture")?;
    fs::write(repo.join("src/lib.rs"), "lib\n").context("write ls-tree src fixture")?;
    fs::write(repo.join("src/nested/deep.txt"), "deep\n")
        .context("write ls-tree nested fixture")?;
    ensure_file(repo.join("src/nested/deep.txt"))?;

    ctx.command(
        &["add", "README.md", "src/lib.rs", "src/nested/deep.txt"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "test: ls-tree fixture", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let root = ctx.command(&["ls-tree", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&root, "\tREADME.md")?;
    assert_stdout_contains(&root, "\tsrc")?;

    let recursive = ctx.command(&["ls-tree", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_stdout_contains(&recursive, "\tsrc/lib.rs")?;
    assert_stdout_contains(&recursive, "\tsrc/nested/deep.txt")?;

    let dirs = ctx.command(&["ls-tree", "-d", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_stdout_contains(&dirs, "\tsrc\n")?;
    assert_stdout_contains(&dirs, "\tsrc/nested\n")?;
    assert_not_contains(&dirs, "src/lib.rs")?;
    assert_not_contains(&dirs, "src/nested/deep.txt")?;

    assert_json_ok(
        &ctx.command(
            &["--json", "ls-tree", "-r", "HEAD", "src"],
            repo.clone(),
            true,
        )?,
        "ls-tree",
    )?;

    let missing = ctx.command(&["ls-tree", "HEAD", "missing"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "does not exist")?;
    ctx.command(&["fsck"], repo.clone(), true)?;
    Ok(())
}
