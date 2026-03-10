//! Vault key management commands for generating and exporting GPG/SSH public keys.

use clap::{Parser, Subcommand};

use crate::{
    internal::{config::Config, vault},
    utils::{
        error::{CliError, CliResult},
        util,
    },
};

#[derive(Parser, Debug)]
#[command(about = "Manage vault-backed signing and SSH keys")]
pub struct VaultArgs {
    #[command(subcommand)]
    pub command: VaultCommand,
}

#[derive(Subcommand, Debug)]
pub enum VaultCommand {
    /// Generate a GPG key in vault for commit signing
    GenerateGpgKey {
        /// User name for the generated key (defaults to `user.name`)
        #[arg(long)]
        name: Option<String>,
        /// User email for the generated key (defaults to `user.email`)
        #[arg(long)]
        email: Option<String>,
    },
    /// Generate an SSH key in vault for Git transport
    GenerateSshKey {
        /// User name for SSH key id (defaults to `user.name`)
        #[arg(long)]
        name: Option<String>,
    },
    /// Print the vault GPG public key
    GpgPublicKey,
    /// Print the vault SSH public key
    SshPublicKey,
}

pub async fn execute(args: VaultArgs) {
    if let Err(e) = execute_safe(args).await {
        eprintln!("{}", e.render());
    }
}

pub async fn execute_safe(args: VaultArgs) -> CliResult<()> {
    let root_dir = util::storage_path();
    if !vault::vault_exists(&root_dir) {
        return Err(CliError::fatal(
            "vault is not initialized in this repository; run `libra init --vault`",
        ));
    }

    match args.command {
        VaultCommand::GenerateGpgKey { name, email } => {
            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    return Err(CliError::fatal("vault unseal key not found"));
                }
            };

            let user_name = match name {
                Some(v) => v,
                None => Config::get("user", None, "name")
                    .await
                    .unwrap_or_else(|| "Libra User".to_string()),
            };
            let user_email = match email {
                Some(v) => v,
                None => Config::get("user", None, "email")
                    .await
                    .unwrap_or_else(|| "user@libra.local".to_string()),
            };

            let public_key =
                vault::generate_pgp_key(&root_dir, &unseal_key, &user_name, &user_email)
                    .await
                    .map_err(|e| CliError::fatal(e.to_string()))?;
            Config::insert("vault", None, "signing", "true").await;
            println!("{public_key}");
        }
        VaultCommand::GenerateSshKey { name } => {
            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    return Err(CliError::fatal("vault unseal key not found"));
                }
            };

            let user_name = match name {
                Some(v) => v,
                None => Config::get("user", None, "name")
                    .await
                    .unwrap_or_else(|| "Libra User".to_string()),
            };

            let public_key = vault::generate_ssh_key(&root_dir, &unseal_key, &user_name)
                .await
                .map_err(|e| CliError::fatal(e.to_string()))?;
            println!("{public_key}");
        }
        VaultCommand::GpgPublicKey => {
            if let Some(public_key) = Config::get("vault", None, "gpg_pubkey").await {
                println!("{public_key}");
                return Ok(());
            }

            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    return Err(CliError::fatal("vault unseal key not found"));
                }
            };

            let public_key = vault::get_gpg_public_key(&root_dir, &unseal_key)
                .await
                .map_err(|e| CliError::fatal(e.to_string()))?;
            println!("{public_key}");
        }
        VaultCommand::SshPublicKey => {
            let public_key = vault::get_ssh_public_key().await.ok_or_else(|| {
                CliError::fatal("no SSH public key found; run `libra vault generate-ssh-key`")
            })?;
            println!("{public_key}");
        }
    }

    Ok(())
}
