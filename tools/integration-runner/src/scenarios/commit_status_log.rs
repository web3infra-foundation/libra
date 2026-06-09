use serde_json::Value;

use super::prelude::*;

pub(crate) fn scenario_commit_status_log(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    ctx.command(&["init", "repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Integration"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "integration@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("tracked.txt"), "hello\n").context("write tracked fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "initial", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let log = ctx.command(&["log", "--oneline"], repo.clone(), true)?;
    assert_stdout_contains(&log, "initial")?;
    let filtered_log = ctx.command(
        &[
            "log",
            "-n",
            "1",
            "--name-status",
            "--grep",
            "initial",
            "--author",
            "Libra Integration",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&filtered_log, "initial")?;
    assert_stdout_contains(&filtered_log, "tracked.txt")?;
    let stat_log = ctx.command(&["log", "--stat", "-n", "3"], repo.clone(), true)?;
    assert_stdout_contains(&stat_log, "tracked.txt")?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    let empty = ctx.command(
        &["commit", "-m", "empty", "--no-verify"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&empty, "nothing to commit")?;

    fs::rename(repo.join("tracked.txt"), repo.join("renamed.txt"))
        .context("rename tracked fixture")?;
    ctx.command(&["add", "-A"], repo.clone(), true)?;
    let rename_short = ctx.command(&["status", "--short"], repo.clone(), true)?;
    assert_stdout_contains(&rename_short, "R  tracked.txt -> renamed.txt")?;
    let rename_v2 = ctx.command(&["status", "--porcelain", "v2"], repo.clone(), true)?;
    assert_stdout_contains(&rename_v2, "2 R  ")?;
    assert_stdout_contains(&rename_v2, "R100")?;
    assert_stdout_contains(&rename_v2, "renamed.txt\ttracked.txt")?;
    let rename_v2_z = ctx.command(&["status", "--porcelain", "v2", "-z"], repo.clone(), true)?;
    assert_stdout_bytes_contains(&rename_v2_z, b"renamed.txt\0tracked.txt\0")?;
    assert_stdout_not_contains(&rename_v2_z, "renamed.txt\ttracked.txt")?;
    let rename_short_z = ctx.command(&["status", "-z", "-s"], repo.clone(), true)?;
    assert_stdout_bytes_contains(&rename_short_z, b"R  renamed.txt\0tracked.txt\0")?;
    let rename_json = ctx.command(&["--json", "status"], repo.clone(), true)?;
    assert_json_ok(&rename_json, "status")?;
    assert_status_rename_json(&rename_json, "tracked.txt", "renamed.txt", 100)?;
    ctx.command(
        &["commit", "-m", "rename tracked", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let follow_log = ctx.command(
        &["log", "--follow", "--oneline", "renamed.txt"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&follow_log, "rename tracked")?;
    assert_stdout_contains(&follow_log, "initial")?;
    let follow_name_status = ctx.command(
        &["log", "--follow", "--name-status", "renamed.txt"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&follow_name_status, "R100\ttracked.txt\trenamed.txt")?;
    assert_json_ok(
        &ctx.command(
            &["--json", "log", "--follow", "renamed.txt"],
            repo.clone(),
            true,
        )?,
        "log",
    )?;

    fs::create_dir_all(repo.join("scratch")).context("create scratch dir")?;
    fs::write(repo.join("scratch").join("note.txt"), "untracked\n")
        .context("write untracked scratch file")?;
    ctx.command(
        &["config", "status.showUntrackedFiles", "no"],
        repo.clone(),
        true,
    )?;
    let hidden_untracked = ctx.command(&["status", "--short"], repo.clone(), true)?;
    assert_stdout_not_contains(&hidden_untracked, "scratch")?;
    let override_untracked = ctx.command(
        &["status", "--short", "--untracked-files=all"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&override_untracked, "?? scratch/note.txt")?;
    ctx.command(&["config", "status.branch", "true"], repo.clone(), true)?;
    let branch_short = ctx.command(&["status", "--short"], repo.clone(), true)?;
    assert_stdout_contains(&branch_short, "## main")?;

    #[cfg(unix)]
    {
        fs::write(repo.join("type-target.txt"), "target\n").context("write type target")?;
        ctx.command(&["add", "type-target.txt"], repo.clone(), true)?;
        ctx.command(
            &["commit", "-m", "add type target", "--no-verify"],
            repo.clone(),
            true,
        )?;
        fs::remove_file(repo.join("type-target.txt")).context("remove type target")?;
        std::os::unix::fs::symlink("renamed.txt", repo.join("type-target.txt"))
            .context("create typechange symlink")?;
        let typechange_v2 = ctx.command(&["status", "--porcelain", "v2"], repo.clone(), true)?;
        assert_stdout_contains(&typechange_v2, "1  T")?;
        assert_stdout_contains(&typechange_v2, "type-target.txt")?;
    }
    Ok(())
}

fn assert_stdout_not_contains(output: &std::process::Output, unexpected: &str) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains(unexpected) {
        bail!("stdout unexpectedly contained {unexpected:?}: {stdout}");
    }
    Ok(())
}

fn assert_stdout_bytes_contains(output: &std::process::Output, expected: &[u8]) -> Result<()> {
    if output
        .stdout
        .windows(expected.len())
        .any(|window| window == expected)
    {
        return Ok(());
    }
    bail!(
        "stdout did not contain expected bytes {:?}: {:?}",
        expected,
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_status_rename_json(
    output: &std::process::Output,
    expected_from: &str,
    expected_to: &str,
    expected_score: u64,
) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("parse status JSON: {stdout}"))?;
    let renames = value["data"]["renames"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("status JSON data.renames was not an array: {value}"))?;
    let found = renames.iter().any(|rename| {
        rename["from"].as_str() == Some(expected_from)
            && rename["to"].as_str() == Some(expected_to)
            && rename["score"].as_u64() == Some(expected_score)
    });
    if !found {
        bail!(
            "status JSON did not contain expected rename {expected_from}->{expected_to} score {expected_score}: {value}"
        );
    }
    Ok(())
}
