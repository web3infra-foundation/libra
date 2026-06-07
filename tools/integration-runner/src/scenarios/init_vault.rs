use super::prelude::*;

pub(crate) fn scenario_init_vault(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    ctx.command(
        &["init", "--vault", "true", "vault-repo"],
        ctx.run_dir.clone(),
        true,
    )?;
    let vault_repo = ctx.run_dir.join("vault-repo");
    ensure_file(vault_repo.join(".libra/vault.db"))?;
    let signing = ctx.command(
        &["config", "get", "vault.signing"],
        vault_repo.clone(),
        true,
    )?;
    assert_stdout_contains(&signing, "true")?;
    let json_signing = ctx.command(
        &["--json", "config", "get", "vault.signing"],
        vault_repo.clone(),
        true,
    )?;
    assert_json_ok(&json_signing, "config")?;
    ctx.command(&["fsck"], vault_repo, true)?;

    ctx.command(
        &["init", "--vault", "false", "no-vault-repo"],
        ctx.run_dir.clone(),
        true,
    )?;
    let no_vault_repo = ctx.run_dir.join("no-vault-repo");
    if no_vault_repo.join(".libra/vault.db").exists() {
        bail!("--vault false created .libra/vault.db");
    }
    let signing = ctx.command(
        &["config", "get", "vault.signing"],
        no_vault_repo.clone(),
        true,
    )?;
    assert_stdout_contains(&signing, "false")?;
    let json_signing = ctx.command(
        &["--json", "config", "get", "vault.signing"],
        no_vault_repo.clone(),
        true,
    )?;
    assert_json_ok(&json_signing, "config")?;
    ctx.command(&["fsck"], no_vault_repo, true)?;
    Ok(())
}
