//! Implementation of `cat-file` command for inspecting Git object content, type, and size.
//!
//! This is a low-level debugging tool analogous to `git cat-file`. It supports:
//! - `-t <object>`: print the object type
//! - `-s <object>`: print the object size (in bytes)
//! - `-p <object>`: pretty-print the object content
//! - `-e <object>`: check if the object exists (exit status only)

use std::str::FromStr;

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};

use crate::{
    command::load_object,
    utils::{client_storage::ClientStorage, path, util},
};

/// Provide content, type, or size information for repository objects.
#[derive(Parser, Debug)]
pub struct CatFileArgs {
    /// Print the object type
    #[clap(short = 't', group = "mode")]
    pub show_type: bool,

    /// Print the object size (in bytes)
    #[clap(short = 's', group = "mode")]
    pub show_size: bool,

    /// Pretty-print the object content
    #[clap(short = 'p', group = "mode")]
    pub pretty_print: bool,

    /// Check if the object exists (exit with zero status if it does)
    #[clap(short = 'e', group = "mode")]
    pub check_exist: bool,

    /// The object hash (full or abbreviated), branch name, tag, or HEAD
    #[clap(value_name = "OBJECT")]
    pub object: String,
}

pub async fn execute(args: CatFileArgs) {
    let hash = resolve_object(&args.object).await;

    if args.check_exist {
        check_object_exists(&hash);
        return;
    }

    let storage = ClientStorage::init(path::objects());
    let obj_type = match storage.get_object_type(&hash) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("fatal: Not a valid object name {}", args.object);
            std::process::exit(128);
        }
    };

    if args.show_type {
        println!("{}", obj_type);
    } else if args.show_size {
        print_object_size(&storage, &hash);
    } else if args.pretty_print {
        pretty_print_object(&hash, obj_type);
    } else {
        eprintln!("fatal: one of '-t', '-s', '-p', or '-e' is required");
        std::process::exit(129);
    }
}

/// Resolve a user-supplied object reference to an `ObjectHash`.
///
/// Supports branch names, tags, HEAD, and raw hex hashes.
async fn resolve_object(object_ref: &str) -> ObjectHash {
    // Try as a ref (branch/tag/HEAD) first
    if let Ok(hash) = util::get_commit_base(object_ref).await {
        return hash;
    }

    // Try as a raw hex hash
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return hash;
    }

    // Try abbreviated hash via storage search
    let storage = ClientStorage::init(path::objects());
    let results = storage.search(object_ref).await;
    if results.len() == 1 {
        return results[0];
    } else if results.len() > 1 {
        eprintln!(
            "fatal: ambiguous argument '{}': matched {} objects",
            object_ref,
            results.len()
        );
        std::process::exit(128);
    }

    eprintln!("fatal: Not a valid object name {}", object_ref);
    std::process::exit(128);
}

/// Exit with 0 if the object exists, 1 otherwise.
fn check_object_exists(hash: &ObjectHash) {
    let storage = ClientStorage::init(path::objects());
    if !storage.exist(hash) {
        std::process::exit(1);
    }
}

/// Print the size (in bytes) of the raw object data.
fn print_object_size(storage: &ClientStorage, hash: &ObjectHash) {
    match storage.get(hash) {
        Ok(data) => println!("{}", data.len()),
        Err(e) => {
            eprintln!("fatal: unable to read object {}: {}", hash, e);
            std::process::exit(128);
        }
    }
}

/// Pretty-print an object based on its type.
fn pretty_print_object(hash: &ObjectHash, obj_type: ObjectType) {
    match obj_type {
        ObjectType::Blob => print_blob(hash),
        ObjectType::Tree => print_tree(hash),
        ObjectType::Commit => print_commit(hash),
        ObjectType::Tag => print_tag(hash),
        _ => {
            eprintln!("fatal: unsupported object type {:?}", obj_type);
            std::process::exit(128);
        }
    }
}

/// Print a blob object's raw content.
fn print_blob(hash: &ObjectHash) {
    let blob: Blob = match load_object(hash) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("fatal: could not read blob {}: {}", hash, e);
            std::process::exit(128);
        }
    };
    match String::from_utf8(blob.data.clone()) {
        Ok(text) => print!("{}", text),
        Err(_) => {
            // Write raw binary to stdout
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&blob.data).unwrap_or_else(|e| {
                eprintln!("fatal: write error: {}", e);
                std::process::exit(128);
            });
        }
    }
}

/// Print a tree object in a human-readable format.
fn print_tree(hash: &ObjectHash) {
    let tree: Tree = match load_object(hash) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fatal: could not read tree {}: {}", hash, e);
            std::process::exit(128);
        }
    };
    for item in &tree.tree_items {
        let type_name = match item.mode {
            git_internal::internal::object::tree::TreeItemMode::Tree => "tree",
            _ => "blob",
        };
        println!(
            "{:06o} {} {}\t{}",
            item.mode as u32, type_name, item.id, item.name
        );
    }
}

/// Print a commit object in human-readable format.
fn print_commit(hash: &ObjectHash) {
    let commit: Commit = match load_object(hash) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: could not read commit {}: {}", hash, e);
            std::process::exit(128);
        }
    };
    println!("tree {}", commit.tree_id);
    for parent in &commit.parent_commit_ids {
        println!("parent {}", parent);
    }
    println!(
        "author {} <{}> {} +0000",
        commit.author.name.trim(),
        commit.author.email.trim(),
        commit.author.timestamp,
    );
    println!(
        "committer {} <{}> {} +0000",
        commit.committer.name.trim(),
        commit.committer.email.trim(),
        commit.committer.timestamp,
    );
    println!();
    let (msg, _) = crate::common_utils::parse_commit_msg(&commit.message);
    println!("{}", msg.trim());
}

/// Print a tag object in human-readable format.
fn print_tag(hash: &ObjectHash) {
    let storage = ClientStorage::init(path::objects());
    let data = match storage.get(hash) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("fatal: could not read tag {}: {}", hash, e);
            std::process::exit(128);
        }
    };
    // Tag objects are text-based, print raw content
    match String::from_utf8(data) {
        Ok(text) => print!("{}", text),
        Err(_) => {
            eprintln!("fatal: invalid tag object encoding for {}", hash);
            std::process::exit(128);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing_type() {
        let args = CatFileArgs::try_parse_from(["cat-file", "-t", "abc123"]).unwrap();
        assert!(args.show_type);
        assert!(!args.show_size);
        assert!(!args.pretty_print);
        assert!(!args.check_exist);
        assert_eq!(args.object, "abc123");
    }

    #[test]
    fn test_args_parsing_size() {
        let args = CatFileArgs::try_parse_from(["cat-file", "-s", "abc123"]).unwrap();
        assert!(args.show_size);
        assert!(!args.show_type);
    }

    #[test]
    fn test_args_parsing_pretty() {
        let args = CatFileArgs::try_parse_from(["cat-file", "-p", "HEAD"]).unwrap();
        assert!(args.pretty_print);
        assert_eq!(args.object, "HEAD");
    }

    #[test]
    fn test_args_parsing_exist() {
        let args = CatFileArgs::try_parse_from(["cat-file", "-e", "abc123"]).unwrap();
        assert!(args.check_exist);
    }

    #[test]
    fn test_args_mutual_exclusion() {
        // -t and -p should be mutually exclusive
        let result = CatFileArgs::try_parse_from(["cat-file", "-t", "-p", "abc123"]);
        assert!(result.is_err());
    }
}
