use super::prelude::*;

pub(crate) fn scenario_object_readback(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("object-repo");
    ctx.command(&["init", "object-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Object Test"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "object@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::create_dir_all(repo.join("docs")).context("create docs fixture dir")?;
    fs::create_dir_all(repo.join("src")).context("create src fixture dir")?;
    fs::write(repo.join("README.md"), "object root\n").context("write README fixture")?;
    fs::write(repo.join("docs/guide.md"), "object docs\n").context("write docs fixture")?;
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").context("write src fixture")?;
    ctx.command(
        &["add", "README.md", "docs/guide.md", "src/main.rs"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "test: object readback", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let head = ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?;
    let head_id = stdout_trim(&head);
    if head_id.len() < 40 {
        bail!("rev-parse HEAD returned an unexpectedly short id: {head_id}");
    }
    ctx.command(&["rev-parse", "--short", "HEAD"], repo.clone(), true)?;
    let top = ctx.command(&["rev-parse", "--show-toplevel"], repo.clone(), true)?;
    assert_stdout_contains(&top, repo.to_string_lossy().as_ref())?;
    let rev_list = ctx.command(&["rev-list", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&rev_list, &head_id)?;
    let show = ctx.command(&["show", "--no-patch", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&show, "test: object readback")?;
    let guide = ctx.command(&["show", "HEAD:docs/guide.md"], repo.clone(), true)?;
    assert_stdout_contains(&guide, "object docs")?;
    ctx.command(&["show-ref", "--head"], repo.clone(), true)?;
    ctx.command(&["show-ref", "--heads"], repo.clone(), true)?;
    let object_type = ctx.command(&["cat-file", "-t", &head_id], repo.clone(), true)?;
    assert_stdout_contains(&object_type, "commit")?;
    ctx.command(&["cat-file", "-s", &head_id], repo.clone(), true)?;
    let pretty = ctx.command(&["cat-file", "-p", &head_id], repo.clone(), true)?;
    assert_stdout_contains(&pretty, "tree ")?;
    ctx.command(&["cat-file", "-e", &head_id], repo.clone(), true)?;
    fs::write(repo.join("loose.txt"), "loose blob\n").context("write loose blob fixture")?;
    let blob = ctx.command(&["hash-object", "-w", "loose.txt"], repo.clone(), true)?;
    let blob_id = stdout_trim(&blob);
    let blob_type = ctx.command(&["cat-file", "-t", &blob_id], repo.clone(), true)?;
    assert_stdout_contains(&blob_type, "blob")?;
    let blob_content = ctx.command(&["cat-file", "-p", &blob_id], repo.clone(), true)?;
    assert_stdout_contains(&blob_content, "loose blob")?;
    ctx.command(&["fsck"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    ctx.command(&["fsck", &head_id], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "show-ref", "--heads"], repo.clone(), true)?,
        "show-ref",
    )?;
    let missing = ctx.command(&["cat-file", "-t", "deadbeef"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "object not found")?;
    Ok(())
}
