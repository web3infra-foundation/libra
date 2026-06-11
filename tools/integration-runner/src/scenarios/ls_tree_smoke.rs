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
    let root_stdout = String::from_utf8_lossy(&root.stdout).to_string();
    let readme_oid = root_stdout
        .lines()
        .find(|line| line.ends_with("\tREADME.md"))
        .and_then(|line| line.split_whitespace().nth(2))
        .context("extract README.md blob oid from ls-tree HEAD output")?
        .to_string();
    let readme_abbrev = readme_oid
        .get(..7)
        .context("README.md blob oid shorter than 7 hex chars")?;

    let recursive = ctx.command(&["ls-tree", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_stdout_contains(&recursive, "\tsrc/lib.rs")?;
    assert_stdout_contains(&recursive, "\tsrc/nested/deep.txt")?;

    let dirs = ctx.command(&["ls-tree", "-d", "-r", "HEAD", "src"], repo.clone(), true)?;
    assert_stdout_contains(&dirs, "\tsrc\n")?;
    assert_stdout_contains(&dirs, "\tsrc/nested\n")?;
    assert_not_contains(&dirs, "src/lib.rs")?;
    assert_not_contains(&dirs, "src/nested/deep.txt")?;

    // -r without -t omits tree entries; -t -r interleaves them with the blobs.
    let recursive_all = ctx.command(&["ls-tree", "-r", "HEAD"], repo.clone(), true)?;
    assert_not_contains(&recursive_all, " tree ")?;
    let with_trees = ctx.command(&["ls-tree", "-t", "-r", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&with_trees, " tree ")?;
    assert_stdout_contains(&with_trees, "\tsrc\n")?;
    assert_stdout_contains(&with_trees, "\tsrc/nested\n")?;
    assert_stdout_contains(&with_trees, "\tsrc/nested/deep.txt")?;

    // -l adds a size column for blobs and `-` for trees, still tab-separated from the path.
    let long = ctx.command(&["ls-tree", "-l", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&long, " 5\tREADME.md")?;
    assert_stdout_contains(&long, " -\tsrc")?;

    // --name-only prints paths only; --name-status is its Git-compatible alias.
    let name_only = ctx.command(&["ls-tree", "--name-only", "HEAD"], repo.clone(), true)?;
    let name_only_out = stdout_trim(&name_only);
    if name_only_out != "README.md\nsrc\ntracked.txt" {
        bail!("ls-tree --name-only HEAD output mismatch: {name_only_out:?}");
    }
    let name_status = ctx.command(
        &["ls-tree", "--name-status", "-r", "HEAD"],
        repo.clone(),
        true,
    )?;
    let name_status_out = stdout_trim(&name_status);
    if name_status_out != "README.md\nsrc/lib.rs\nsrc/nested/deep.txt\ntracked.txt" {
        bail!("ls-tree --name-status -r HEAD output mismatch: {name_status_out:?}");
    }

    // --object-only prints bare oids without mode/type/path columns.
    let object_only = ctx.command(&["ls-tree", "--object-only", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&object_only, &readme_oid)?;
    assert_not_contains(&object_only, "README.md")?;
    assert_not_contains(&object_only, "blob")?;

    // --abbrev=7 shortens oids to 7 hex chars; bare --abbrev defaults to 7 as well.
    let abbrev = ctx.command(&["ls-tree", "--abbrev=7", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&abbrev, &format!("blob {readme_abbrev}\t"))?;
    assert_not_contains(&abbrev, &readme_oid)?;
    let abbrev_default = ctx.command(
        &["ls-tree", "--abbrev", "--object-only", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&abbrev_default, readme_abbrev)?;
    assert_not_contains(&abbrev_default, &readme_oid)?;

    // -z terminates records with NUL instead of newline.
    let nul = ctx.command(
        &["ls-tree", "-z", "--name-only", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&nul, "README.md\0src\0")?;
    let nul_stdout = String::from_utf8_lossy(&nul.stdout);
    if nul_stdout.contains('\n') {
        bail!("ls-tree -z output unexpectedly contained a newline: {nul_stdout:?}");
    }

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
