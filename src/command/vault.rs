//! Vault key management commands for generating and exporting GPG/SSH public keys.

use clap::{Parser, Subcommand};

use crate::{
    internal::{config::Config, vault},
    utils::util,
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
    let root_dir = util::storage_path();
    if !vault::vault_exists(&root_dir) {
        eprintln!("fatal: vault is not initialized in this repository; run `libra init --vault`");
        return;
    }

    match args.command {
        VaultCommand::GenerateGpgKey { name, email } => {
            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    eprintln!("fatal: vault unseal key not found");
                    return;
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

            match vault::generate_pgp_key(&root_dir, &unseal_key, &user_name, &user_email).await {
                Ok(public_key) => {
                    Config::insert("vault", None, "signing", "true").await;
                    println!("{public_key}");
                }
                Err(e) => eprintln!("fatal: {e}"),
            }
        }
        VaultCommand::GenerateSshKey { name } => {
            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    eprintln!("fatal: vault unseal key not found");
                    return;
                }
            };

            let user_name = match name {
                Some(v) => v,
                None => Config::get("user", None, "name")
                    .await
                    .unwrap_or_else(|| "Libra User".to_string()),
            };

            match vault::generate_ssh_key(&root_dir, &unseal_key, &user_name).await {
                Ok(public_key) => println!("{public_key}"),
                Err(e) => eprintln!("fatal: {e}"),
            }
        }
        VaultCommand::GpgPublicKey => {
            if let Some(public_key) = Config::get("vault", None, "gpg_pubkey").await {
                println!("{public_key}");
                return;
            }

            let unseal_key = match vault::load_unseal_key().await {
                Some(k) => k,
                None => {
                    eprintln!("fatal: vault unseal key not found");
                    return;
                }
            };

            match vault::get_gpg_public_key(&root_dir, &unseal_key).await {
                Ok(public_key) => println!("{public_key}"),
                Err(e) => eprintln!("fatal: {e}"),
            }
        }
        VaultCommand::SshPublicKey => match vault::get_ssh_public_key().await {
            Some(public_key) => println!("{public_key}"),
            None => eprintln!("fatal: no SSH public key found; run `libra vault generate-ssh-key`"),
        },
    }
}
