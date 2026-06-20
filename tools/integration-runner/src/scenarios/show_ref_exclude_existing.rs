use super::prelude::*;

pub(crate) fn scenario_show_ref_exclude_existing(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("show-ref-exclude-existing-repo");
    create_committed_repo(ctx, &repo)?;

    let head = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    let stdin = format!(
        "{head} refs/heads/main\n{head} refs/heads/new\nrefs/tags/newtag\n{head} refs/heads/main^{{}}\n"
    );
    let filtered =
        ctx.command_with_stdin(&["show-ref", "--exclude-existing"], repo.clone(), &stdin, true)?;
    let expected = format!("{head} refs/heads/new\nrefs/tags/newtag");
    if stdout_trim(&filtered) != expected {
        bail!(
            "show-ref --exclude-existing did not preserve only missing refs: {}",
            stdout_trim(&filtered)
        );
    }

    let pattern_stdin = format!("{head} refs/heads/new\n{head} refs/tags/newtag\n");
    let heads_only = ctx.command_with_stdin(
        &["show-ref", "--exclude-existing=refs/heads"],
        repo.clone(),
        &pattern_stdin,
        true,
    )?;
    if stdout_trim(&heads_only) != format!("{head} refs/heads/new") {
        bail!(
            "show-ref --exclude-existing=refs/heads did not filter tags: {}",
            stdout_trim(&heads_only)
        );
    }

    let json = ctx.command_with_stdin(
        &["--json", "show-ref", "--exclude-existing"],
        repo.clone(),
        &format!("{head} refs/heads/json-new\n{head} refs/heads/main\n"),
        true,
    )?;
    assert_json_ok(&json, "show-ref")?;
    let value: serde_json::Value =
        serde_json::from_slice(&json.stdout).context("parse show-ref exclude-existing JSON")?;
    let entry_refname = value["data"]["entries"][0]["refname"].as_str();
    if entry_refname != Some("refs/heads/json-new") {
        bail!("show-ref --exclude-existing JSON returned unexpected entry: {value}");
    }

    let conflict = ctx.command(
        &[
            "show-ref",
            "--exclude-existing",
            "--verify",
            "refs/heads/main",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&conflict, "cannot be used with")?;
    ctx.command(&["fsck"], repo, true)?;
    Ok(())
}
