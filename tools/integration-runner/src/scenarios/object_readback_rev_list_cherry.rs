use super::prelude::*;

pub(crate) fn assert_rev_list_cherry_filters(
    ctx: &mut ScenarioCtx<'_>,
    repo: &Path,
    latest_id: &str,
) -> Result<()> {
    ctx.command(&["branch", "rev-right", "HEAD~1"], repo.to_path_buf(), true)?;
    ctx.command(&["switch", "rev-right"], repo.to_path_buf(), true)?;

    fs::write(repo.join("docs/rev-list.md"), "rev-list second\n")
        .context("write equivalent rev-list fixture on right branch")?;
    ctx.command(&["add", "docs/rev-list.md"], repo.to_path_buf(), true)?;
    ctx.command(
        &[
            "commit",
            "-m",
            "test: rev-list equivalent right",
            "--no-verify",
        ],
        repo.to_path_buf(),
        true,
    )?;
    let right_same_id = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.to_path_buf(), true)?);

    fs::write(repo.join("docs/right-only.md"), "right only\n")
        .context("write right-only rev-list fixture")?;
    ctx.command(&["add", "docs/right-only.md"], repo.to_path_buf(), true)?;
    ctx.command(
        &[
            "commit",
            "-m",
            "test: rev-list right only",
            "--no-verify",
        ],
        repo.to_path_buf(),
        true,
    )?;
    let right_unique_id =
        stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.to_path_buf(), true)?);

    let rev_left_right = ctx.command(
        &["rev-list", "--left-right", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    assert_same_lines(
        &rev_left_right,
        vec![
            format!("<{latest_id}"),
            format!(">{right_same_id}"),
            format!(">{right_unique_id}"),
        ],
    )?;

    let rev_right_only = ctx.command(
        &["rev-list", "--right-only", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    assert_same_lines(
        &rev_right_only,
        vec![right_same_id.clone(), right_unique_id.clone()],
    )?;

    let rev_left_only = ctx.command(
        &["rev-list", "--left-only", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    assert_same_lines(&rev_left_only, vec![latest_id.to_string()])?;

    let rev_cherry_pick = ctx.command(
        &["rev-list", "--cherry-pick", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_cherry_pick) != right_unique_id {
        bail!("rev-list --cherry-pick main...rev-right did not omit equivalent patches");
    }

    let rev_cherry_mark = ctx.command(
        &["rev-list", "--cherry-mark", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    assert_same_lines(
        &rev_cherry_mark,
        vec![
            format!("={latest_id}"),
            format!("={right_same_id}"),
            format!("+{right_unique_id}"),
        ],
    )?;

    let rev_cherry_count = ctx.command(
        &[
            "rev-list",
            "--count",
            "--left-right",
            "--cherry-mark",
            "main...rev-right",
        ],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_cherry_count) != "0\t1\t2" {
        bail!("rev-list cherry count fields did not match Git-compatible side counts");
    }

    let rev_cherry_json = ctx.command(
        &["--json", "rev-list", "--cherry-pick", "main...rev-right"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_cherry_json: serde_json::Value = serde_json::from_slice(&rev_cherry_json.stdout)
        .context("parse rev-list cherry-pick JSON output")?;
    if rev_cherry_json["data"]["cherry_pick"] != serde_json::json!(true) {
        bail!("rev-list cherry-pick JSON did not echo flag: {rev_cherry_json}");
    }
    if rev_cherry_json["data"]["commits"] != serde_json::json!([right_unique_id]) {
        bail!("rev-list cherry-pick JSON did not limit commits: {rev_cherry_json}");
    }

    ctx.command(&["switch", "main"], repo.to_path_buf(), true)?;
    Ok(())
}

fn assert_same_lines(output: &std::process::Output, mut expected: Vec<String>) -> Result<()> {
    let mut actual = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    actual.sort();
    expected.sort();
    if actual != expected {
        bail!("unexpected rev-list output: actual={actual:?}, expected={expected:?}");
    }
    Ok(())
}
