use super::{object_readback_rev_list_cherry::assert_rev_list_cherry_filters, prelude::*};

pub(crate) fn assert_rev_list_filters(
    ctx: &mut ScenarioCtx<'_>,
    repo: &Path,
    head_id: &str,
    latest_id: &str,
) -> Result<()> {
    let rev_since = ctx.command(
        &["rev-list", "--count", "--since", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_since) != "2" {
        bail!("rev-list --count --since 0 HEAD returned unexpected count");
    }

    let rev_after = ctx.command(
        &["rev-list", "--count", "--after", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_after) != "2" {
        bail!("rev-list --after alias returned unexpected count");
    }

    let rev_until = ctx.command(
        &["rev-list", "--count", "--until", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_until) != "0" {
        bail!("rev-list --count --until 0 HEAD returned unexpected count");
    }

    let rev_before = ctx.command(
        &["rev-list", "--count", "--before", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_before) != "0" {
        bail!("rev-list --before alias returned unexpected count");
    }

    let rev_min_parents = ctx.command(
        &["rev-list", "--min-parents", "1", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_min_parents) != latest_id {
        bail!("rev-list --min-parents 1 HEAD did not return the non-root commit");
    }

    let rev_max_parents = ctx.command(
        &["rev-list", "--max-parents", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_max_parents) != head_id {
        bail!("rev-list --max-parents 0 HEAD did not return the root commit");
    }

    let rev_no_min = ctx.command(
        &[
            "rev-list",
            "--count",
            "--min-parents",
            "1",
            "--no-min-parents",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_no_min) != "2" {
        bail!("rev-list --no-min-parents did not clear the lower parent bound");
    }

    let rev_no_max = ctx.command(
        &[
            "rev-list",
            "--count",
            "--max-parents",
            "0",
            "--no-max-parents",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_no_max) != "2" {
        bail!("rev-list --no-max-parents did not clear the upper parent bound");
    }

    let rev_first_parent = ctx.command(
        &["rev-list", "--count", "--first-parent", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_first_parent) != "2" {
        bail!("rev-list --first-parent HEAD returned unexpected count in linear history");
    }

    let rev_author = ctx.command(
        &["rev-list", "--author", "rev-list@example.com", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_author) != latest_id {
        bail!("rev-list --author did not return only the matching author commit");
    }

    let rev_author_missing = ctx.command(
        &["rev-list", "--count", "--author", "missing-author", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_author_missing) != "0" {
        bail!("rev-list --count --author missing-author returned unexpected count");
    }

    let rev_committer = ctx.command(
        &[
            "rev-list",
            "--committer",
            "rev-list-committer@example.com",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_committer) != latest_id {
        bail!("rev-list --committer did not return only the matching committer commit");
    }

    let rev_committer_missing = ctx.command(
        &[
            "rev-list",
            "--count",
            "--committer",
            "missing-committer",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_committer_missing) != "0" {
        bail!("rev-list --count --committer missing-committer returned unexpected count");
    }

    let rev_grep = ctx.command(
        &["rev-list", "--grep", "rev-list second", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_grep) != latest_id {
        bail!("rev-list --grep did not return only the matching message commit");
    }

    let rev_multi_grep = ctx.command(
        &[
            "rev-list",
            "--grep",
            "object readback",
            "--grep",
            "rev-list second",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    let rev_multi_grep_output = String::from_utf8_lossy(&rev_multi_grep.stdout);
    if rev_multi_grep_output.lines().collect::<Vec<_>>() != vec![latest_id, head_id] {
        bail!("rev-list multiple --grep patterns did not use OR message matching");
    }

    let rev_grep_case_miss = ctx.command(
        &["rev-list", "--count", "--grep", "REV-LIST SECOND", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_grep_case_miss) != "0" {
        bail!("rev-list --grep should be case-sensitive by default");
    }

    let rev_path = ctx.command(
        &["rev-list", "HEAD", "--", "docs/rev-list.md"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_path) != latest_id {
        bail!("rev-list HEAD -- docs/rev-list.md did not return only the matching path commit");
    }

    let rev_root_path = ctx.command(
        &["rev-list", "HEAD", "--", "README.md"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_root_path) != head_id {
        bail!("rev-list HEAD -- README.md did not return only the root path commit");
    }

    let rev_path_json = ctx.command(
        &["--json", "rev-list", "HEAD", "--", "docs/rev-list.md"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_path_json: serde_json::Value = serde_json::from_slice(&rev_path_json.stdout)
        .context("parse rev-list pathspec JSON output")?;
    if rev_path_json["data"]["pathspecs"] != serde_json::json!(["docs/rev-list.md"]) {
        bail!("rev-list pathspec JSON did not echo pathspecs: {rev_path_json}");
    }
    if rev_path_json["data"]["commits"] != serde_json::json!([latest_id]) {
        bail!("rev-list pathspec JSON did not limit commits: {rev_path_json}");
    }

    let rev_no_merges = ctx.command(
        &["rev-list", "--no-merges", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_no_merges_output = String::from_utf8_lossy(&rev_no_merges.stdout);
    if rev_no_merges_output.lines().collect::<Vec<_>>() != vec![latest_id, head_id] {
        bail!("rev-list --no-merges HEAD did not keep linear commits in traversal order");
    }

    let rev_merges = ctx.command(&["rev-list", "--merges", "HEAD"], repo.to_path_buf(), true)?;
    if !stdout_trim(&rev_merges).is_empty() {
        bail!("rev-list --merges HEAD returned commits for a linear history");
    }

    let rev_merge_count = ctx.command(
        &["rev-list", "--count", "--merges", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_merge_count) != "0" {
        bail!("rev-list --count --merges HEAD returned unexpected count");
    }

    assert_rev_list_cherry_filters(ctx, repo, latest_id)?;

    Ok(())
}
