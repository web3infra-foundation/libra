use crate::internal::config::Config;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum RemoteCmds {
    /// Add a remote
    Add {
        /// The name of the remote
        name: String,
        /// The URL of the remote
        url: String,
    },
    /// Remove a remote
    Remove {
        /// The name of the remote
        name: String,
    },
    /// Rename a remote
    Rename {
        /// The current name of the remote
        old: String,
        /// The new name of the remote
        new: String,
    },
    /// List remotes
    #[command(name = "-v")]
    List,
    /// Show current remote repository
    Show,
    /// Print URLs for the given remote
    ///
    /// Examples:
    /// `libra remote get-url origin` - print the fetch URL (first)
    /// `libra remote get-url --push origin` - print push URLs
    /// `libra remote get-url --all origin` - print all configured URLs
    GetUrl {
        /// Print push URLs instead of fetch URL
        #[arg(long)]
        push: bool,
        /// Print all URLs
        #[arg(long)]
        all: bool,
        /// Remote name
        name: String,
    },
    /// Set or modify URLs for the given remote
    ///
    /// Examples:
    /// `libra remote set-url origin newurl` - replace first url
    /// `libra remote set-url --all origin newurl` - replace all urls
    /// `libra remote set-url --add origin newurl` - add a new url
    /// `libra remote set-url --delete origin urlpattern` - delete matching url(s)
    SetUrl {
        /// Add the new URL instead of replacing
        #[arg(long)]
        add: bool,
        /// Delete the URL instead of adding/replacing
        #[arg(long)]
        delete: bool,
        /// Operate on push URLs (pushurl) instead of fetch URLs (url)
        #[arg(long)]
        push: bool,
        /// Apply to all matching entries
        #[arg(long)]
        all: bool,
        /// Remote name
        name: String,
        /// URL value (or pattern for --delete)
        value: String,
    },
}

pub async fn execute(command: RemoteCmds) {
    match command {
        RemoteCmds::Add { name, url } => {
            Config::insert("remote", Some(&name), "url", &url).await;
        }
        RemoteCmds::Remove { name } => {
            if let Err(e) = Config::remove_remote(&name).await {
                eprintln!("{e}");
            }
        }
        RemoteCmds::Rename { old, new } => {
            if let Err(e) = Config::rename_remote(&old, &new).await {
                eprintln!("{e}");
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
        RemoteCmds::GetUrl { push, all, name } => {
            // If --push, prefer explicit pushurl entries; fall back to url if none.
            if push {
                let push_urls = Config::get_all("remote", Some(&name), "pushurl").await;
                if !push_urls.is_empty() {
                    if all {
                        for u in push_urls {
                            println!("{}", u);
                        }
                    } else if let Some(u) = push_urls.first() {
                        println!("{}", u);
                    }
                    return;
                }
                // fall through to read regular url if no pushurl configured
            }

            let urls = Config::get_all("remote", Some(&name), "url").await;
            if urls.is_empty() {
                eprintln!("fatal: no URL configured for remote '{name}'");
            } else if all || push {
                // --push with no pushurl falls back to printing all configured url entries
                for url in urls {
                    println!("{}", url);
                }
            } else if let Some(url) = urls.first() {
                println!("{}", url);
            }
        }
        RemoteCmds::SetUrl {
            add,
            delete,
            push,
            all,
            name,
            value,
        } => {
            // Determine which config key to operate on
            let key = if push { "pushurl" } else { "url" };

            if add {
                // Insert a new URL entry
                Config::insert("remote", Some(&name), key, &value).await;
                return;
            }

            if delete {
                // Delete matching entries; if --all then delete all matching, else delete first matching
                Config::remove_config("remote", Some(&name), key, Some(&value), all).await;
                return;
            }

            // Default: replace behavior
            if all {
                // Remove all existing entries for this key, then insert the new value once
                Config::remove_config("remote", Some(&name), key, None, true).await;
                Config::insert("remote", Some(&name), key, &value).await;
            } else {
                // Replace first existing entry: remove first occurrence then insert new value
                Config::remove_config("remote", Some(&name), key, None, false).await;
                Config::insert("remote", Some(&name), key, &value).await;
            }
        }
    }
}

async fn show_remote_verbose(remote: &str) {
    // There can be multiple URLs for a remote, like Gitee & GitHub
    let urls = Config::get_all("remote", Some(remote), "url").await;
    match urls.first() {
        Some(url) => {
            println!("{remote} {url} (fetch)");
        }
        None => {
            eprintln!("fatal: no URL configured for remote '{remote}'");
        }
    }
    for url in urls {
        println!("{remote} {url} (push)");
    }
}
