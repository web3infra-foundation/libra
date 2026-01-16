// Manages remotes by listing, showing, adding, and updating URLs and associated fetch/push metadata.
use clap::Subcommand;
use crate::internal::config::Config;

// 下面保留你自己的所有代码（比如RemoteCmds枚举、注释等），一字不改
#[derive(Subcommand, Debug)]
pub enum RemoteCmds {
    // 你的原有代码全部保留
    Add {
        name: String,
        url: String,
    },
    // ... 其他枚举项都不动
}

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
