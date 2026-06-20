use super::prelude::*;

pub(crate) fn scenario_config_key_generation(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("keygen-repo");
    ctx.command(&["init", "keygen-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "set", "user.name", "Keygen User"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "keygen@example.invalid"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:owner/repo.git",
        ],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "generate-ssh-key", "--remote", "origin"],
        repo.clone(),
        true,
    )?;
    let ssh_pub = ctx.command(
        &["config", "get", "vault.ssh.origin.pubkey"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&ssh_pub, "ssh-")?;
    let gpg_pub = ctx.command(&["config", "get", "vault.gpg.pubkey"], repo.clone(), true)?;
    assert_stdout_contains(&gpg_pub, "BEGIN PGP PUBLIC KEY BLOCK")?;
    let signing = ctx.command(&["config", "get", "vault.signing"], repo.clone(), true)?;
    assert_stdout_contains(&signing, "true")?;
    assert_json_ok(
        &ctx.command(
            &["--json", "config", "get", "vault.signing"],
            repo.clone(),
            true,
        )?,
        "config",
    )?;
    let duplicate_signing = ctx.command(
        &[
            "config",
            "generate-gpg-key",
            "--name",
            "Signing User",
            "--email",
            "signing@example.invalid",
            "--usage",
            "signing",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&duplicate_signing, "already exists")?;
    let duplicate_encrypt = ctx.command(
        &[
            "config",
            "generate-gpg-key",
            "--name",
            "Encrypt User",
            "--email",
            "encrypt@example.invalid",
            "--usage",
            "encrypt",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&duplicate_encrypt, "already exists")?;
    let vault_list = ctx.command(&["config", "list", "--vault"], repo.clone(), true)?;
    assert_not_contains(&vault_list, "PRIVATE KEY")?;
    let global_ssh = ctx.command(
        &[
            "config",
            "--global",
            "generate-ssh-key",
            "--remote",
            "origin",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&global_ssh, "global")?;
    let bad_remote = ctx.command(
        &["config", "generate-ssh-key", "--remote", "bad.name"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_remote, "remote")?;
    let missing_remote = ctx.command(
        &["config", "generate-ssh-key", "--remote", "no-such-remote"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing_remote, "remote")?;
    let bad_usage = ctx.command(
        &["config", "generate-gpg-key", "--usage", "archive"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_usage, "usage")?;
    Ok(())
}
