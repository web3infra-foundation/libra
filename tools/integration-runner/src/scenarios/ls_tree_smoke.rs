use super::prelude::*;

pub(crate) fn scenario_ls_tree_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("ls-tree-repo");
    create_committed_repo(ctx, &repo)?;
    fs::create_dir_all(repo.join("src/nested")).context("create ls-tree nested fixture dir")?;
    fs::write(repo.join("README.md"), "root\n").context("write ls-tree root fixture")?;
    fs::write(repo.join("src/lib.rs"), "lib\n").context("write ls-tree lib fixture")?;
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
    assert_stdout_contains(&root, "README.md")?;
    assert_stdout_contains(&root, "src")?;
    let recursive = ctx.command(&["ls-tree", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_stdout_contains(&recursive, "src/lib.rs")?;
    assert_stdout_contains(&recursive, "src/nested/deep.txt")?;
    let names = ctx.command(&["ls-tree", "--name-only", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&names, "README.md")?;
    let src_dir = repo.join("src");
    let scoped = ctx.command(&["ls-tree", "HEAD"], src_dir.clone(), true)?;
    assert_stdout_contains(&scoped, "lib.rs")?;
    assert_stdout_contains(&scoped, "nested")?;
    assert_not_contains(&scoped, "README.md")?;
    assert_not_contains(&scoped, "src/lib.rs")?;
    let full_name = ctx.command(&["ls-tree", "--full-name", "HEAD"], src_dir.clone(), true)?;
    assert_stdout_contains(&full_name, "src/lib.rs")?;
    let full_tree = ctx.command(&["ls-tree", "--full-tree", "HEAD"], src_dir, true)?;
    assert_stdout_contains(&full_tree, "README.md")?;
    assert_stdout_contains(&full_tree, "src")?;
    let json = ctx.command(&["--json", "ls-tree", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_json_ok(&json, "ls-tree")?;
    let missing = ctx.command(&["ls-tree", "HEAD", "missing"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "pathspec 'missing' did not match")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
