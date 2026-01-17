// Manages remotes by listing, showing, adding, and updating URLs and associated fetch/push metadata.
use clap::Subcommand;
use crate::internal::config::Config;

#[derive(Subcommand, Debug)]
pub enum RemoteCmds {
    // Add a remote
    Add {
        // The name of the remote
        name: String,
        // The URL of the remote
        url: String,
    },
    // Remove a remote
    Remove {
        // The name of the remote
        name: String,
    },
    // Rename a remote
    Rename {
        // The current name of the remote
        old: String,
        // The new name of the remote
        new: String,
    },
    // List remotes
    #[command(name = "-v")]
    List,
    // Show current remote repository
    Show,
    // Print URLs for the given remote
    #[command(name = "get-url")]
    GetUrl {
        // The name of the remote
        name: String,
        // Show push URL instead of fetch URL
        #[arg(short, long)]
        push: bool,
    },
}

pub async fn execute(command: RemoteCmds) {
    match command {
        RemoteCmds::Add { name, url } => {
            Config::insert("remote", Some(&name), "url", &url).await;
        }
        RemoteCmds::Remove { name } => {
            if let Err(e) = Config::remove_remote(&name).await {
                eprintln!("{}", e);
            }
        }
        RemoteCmds::Rename { old, new } => {
            if let Err(e) = Config::rename_remote(&old, &new).await {
                eprintln!("{}", e);
            }
        }
        RemoteCmds::List => {
            let remotes = Config::all_remote_configs().await;
            for remote in remotes {
                show_remote_verbose(&remote.name).await;
            }
        }
        RemoteCmds::Show => {
            let remotes = Config::all_remote_configs().await;
            for remote in remotes {
                println!("{}", remote.name);
            }
        }
        RemoteCmds::GetUrl { push, name } => {
            if Config::remote_config(&name).await.is_none() {
                eprintln!("fatal: No such remote: {}", name);
                return;
            }
            
            let config_key = if push {
                "pushurl"
            } else {
                "url"
            };
            
            if let Some(url) = Config::get("remote", Some(&name), config_key).await {
                println!("{}", url);
            }
        }
    }
}


async fn show_remote_verbose(remote_name: &str) {
    if let Some(url) = Config::get("remote", Some(remote_name), "url").await {
        println!("{}: {}", remote_name, url);
    }
}
