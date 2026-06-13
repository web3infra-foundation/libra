use super::prelude::*;

pub(crate) fn scenario_fetch_depth_local(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let remote_dir = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("git-source");
    fs::create_dir_all(&remote_dir).context("create depth fixture")?;
    ctx.gitfix(&["init", "-b", "main"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["config", "user.name", "Depth Fixture"],
        remote_dir.clone(),
        true,
    )?;
    ctx.gitfix(
        &["config", "user.email", "depth@example.invalid"],
        remote_dir.clone(),
        true,
    )?;
    for (content, message) in [
        ("first\n", "first"),
        ("second\n", "second"),
        ("third\n", "third"),
    ] {
        fs::write(remote_dir.join("a.txt"), content).context("write depth fixture")?;
        ctx.gitfix(&["add", "a.txt"], remote_dir.clone(), true)?;
        ctx.gitfix(&["commit", "-m", message], remote_dir.clone(), true)?;
    }
    let remote = remote_dir.to_string_lossy().to_string();
    ctx.command(
        &["clone", "--depth", "1", &remote, "shallow-1"],
        ctx.run_dir.clone(),
        true,
    )?;
    let shallow1 = ctx.run_dir.join("shallow-1");
    let content = fs::read_to_string(shallow1.join("a.txt")).context("read shallow file")?;
    if !content.contains("third") {
        bail!("shallow clone did not check out latest content: {content}");
    }
    ensure_file(shallow1.join(".libra").join("shallow"))?;
    ctx.command(&["fetch", "origin", "--depth", "2"], shallow1.clone(), true)?;
    ensure_file(shallow1.join(".libra").join("shallow"))?;
    ctx.command(
        &["clone", "--depth", "2", &remote, "shallow-2"],
        ctx.run_dir.clone(),
        true,
    )?;
    let shallow2 = ctx.run_dir.join("shallow-2");
    ensure_file(shallow2.join(".libra").join("shallow"))?;
    let bad = ctx.command(
        &["clone", "--depth", "0", &remote, "bad-depth"],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad, "depth")?;
    Ok(())
}
