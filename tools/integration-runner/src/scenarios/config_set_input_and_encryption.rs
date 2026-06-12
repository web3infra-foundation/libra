use super::prelude::*;

pub(crate) fn scenario_config_set_input_and_encryption(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "+refs/tags/*:refs/tags/*",
        ],
        repo.clone(),
        true,
    )?;
    let all = ctx.command(
        &["config", "get", "--all", "remote.origin.fetch"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&all, "+refs/heads/*:refs/remotes/origin/*")?;
    assert_stdout_contains(&all, "+refs/tags/*:refs/tags/*")?;
    ctx.command_with_stdin(
        &["config", "set", "--stdin", "custom.stdin"],
        repo.clone(),
        "stdin-value\n",
        true,
    )?;
    let stdin_value = ctx.command(&["config", "get", "custom.stdin"], repo.clone(), true)?;
    if stdout_trim(&stdin_value) != "stdin-value" {
        bail!(
            "stdin config value was not trimmed: {:?}",
            stdout_trim(&stdin_value)
        );
    }
    ctx.command(
        &["config", "set", "--encrypt", "custom.secret", "s3cr3t"],
        repo.clone(),
        true,
    )?;
    let masked = ctx.command(&["config", "get", "custom.secret"], repo.clone(), true)?;
    if stdout_trim(&masked).contains("s3cr3t") {
        bail!("encrypted config leaked plaintext without --reveal");
    }
    let revealed = ctx.command(
        &["config", "get", "--reveal", "custom.secret"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&revealed, "s3cr3t")?;
    ctx.command(
        &[
            "config",
            "set",
            "--plaintext",
            "custom.plain",
            "plain-value",
        ],
        repo.clone(),
        true,
    )?;
    let plain = ctx.command(&["config", "get", "custom.plain"], repo.clone(), true)?;
    assert_stdout_contains(&plain, "plain-value")?;
    assert_json_ok(
        &ctx.command(
            &["--json", "config", "get", "custom.plain"],
            repo.clone(),
            true,
        )?,
        "config",
    )?;
    let bad_combo = ctx.command(
        &[
            "config",
            "set",
            "--encrypt",
            "--plaintext",
            "custom.bad",
            "value",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_combo, "cannot")?;
    let bad_stdin = ctx.command(
        &["config", "set", "--stdin", "custom.bad", "value"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_stdin, "stdin")?;
    let bad_vault_plaintext = ctx.command(
        &[
            "config",
            "set",
            "--plaintext",
            "vault.env.TEST_SECRET",
            "value",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_vault_plaintext, "plaintext")?;
    Ok(())
}
