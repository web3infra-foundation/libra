//! Implementation of `cat-file` command for inspecting Git and AI workflow objects.
//!
//! This is a low-level debugging tool analogous to `git cat-file`, extended to also
//! inspect AI process objects (Intent, Task, Run, Plan, PatchSet, Evidence, etc.)
//! stored on the `libra/intent` orphan branch.
//!
//! ## Git object modes
//! - `-t <object>`: print the object type
//! - `-s <object>`: print the object size (in bytes)
//! - `-p <object>`: pretty-print the object content
//! - `-e <object>`: check if the object exists (exit status only)
//!
//! ## AI object modes
//! - `--ai <uuid>`:          pretty-print an AI object by UUID
//! - `--ai-type <uuid>`:     print the AI object type (intent/task/run/…)
//! - `--ai-list <type>`:     list all AI objects of the given type
//! - `--ai-list-types`:      list all AI object types present in history

use std::{str::FromStr, sync::Arc};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};

use crate::{
    command::load_object,
    internal::{ai::history::HistoryManager, db},
    utils::{client_storage::ClientStorage, path, storage::local::LocalStorage, util},
};

/// Provide content, type, or size information for repository objects (Git and AI).
#[derive(Parser, Debug)]
pub struct CatFileArgs {
    // ── Git object modes ────────────────────────────────────────────────
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

    // ── AI object modes ─────────────────────────────────────────────────
    /// Pretty-print an AI object by UUID (intent, task, run, plan, …)
    #[clap(long = "ai", value_name = "UUID", group = "mode")]
    pub ai_object: Option<String>,

    /// Print the type of an AI object by UUID
    #[clap(long = "ai-type", value_name = "UUID", group = "mode")]
    pub ai_type: Option<String>,

    /// List all AI objects of the given type (intent|task|run|plan|patchset|evidence|invocation|provenance|decision|snapshot)
    #[clap(long = "ai-list", value_name = "TYPE", group = "mode")]
    pub ai_list: Option<String>,

    /// List all AI object types present in the history branch
    #[clap(long = "ai-list-types", group = "mode")]
    pub ai_list_types: bool,

    /// The object hash, ref, or UUID (required for Git modes, ignored for --ai-list/--ai-list-types)
    #[clap(value_name = "OBJECT")]
    pub object: Option<String>,
}

/// Known AI object type names stored under the `libra/intent` orphan branch.
const AI_OBJECT_TYPES: &[&str] = &[
    "intent",
    "task",
    "run",
    "plan",
    "patchset",
    "evidence",
    "invocation",
    "provenance",
    "decision",
    "snapshot",
];

pub async fn execute(args: CatFileArgs) {
    // ── AI modes (no positional object arg required) ────────────────────
    if args.ai_list_types {
        ai_list_types().await;
        return;
    }
    if let Some(ref type_name) = args.ai_list {
        ai_list_objects(type_name).await;
        return;
    }
    if let Some(ref uuid) = args.ai_object {
        ai_pretty_print(uuid).await;
        return;
    }
    if let Some(ref uuid) = args.ai_type {
        ai_show_type(uuid).await;
        return;
    }

    // ── Git modes (positional object arg required) ──────────────────────
    let object_ref = match args.object {
        Some(ref o) => o.as_str(),
        None => {
            eprintln!("fatal: <object> is required for Git object modes");
            std::process::exit(129);
        }
    };

    let hash = resolve_object(object_ref).await;

    if args.check_exist {
        check_object_exists(&hash);
        return;
    }

    let storage = ClientStorage::init(path::objects());
    let obj_type = match storage.get_object_type(&hash) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("fatal: Not a valid object name {}", object_ref);
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
        eprintln!("fatal: one of '-t', '-s', '-p', '-e' or an --ai* flag is required");
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

// ── AI object helpers ───────────────────────────────────────────────────

/// Build a `HistoryManager` from the current repo context.
async fn build_history_manager() -> HistoryManager {
    let objects_dir = path::objects();
    let storage = Arc::new(LocalStorage::new(objects_dir));
    let db_conn = Arc::new(db::get_db_conn_instance().await.clone());
    HistoryManager::new(storage, util::storage_path(), db_conn)
}

/// List all AI object types that have at least one entry in the history branch.
async fn ai_list_types() {
    let hm = build_history_manager().await;
    for &type_name in AI_OBJECT_TYPES {
        match hm.list_objects(type_name).await {
            Ok(objects) if !objects.is_empty() => {
                println!("{}\t({} objects)", type_name, objects.len());
            }
            _ => {}
        }
    }
}

/// List all AI objects of a specific type.
async fn ai_list_objects(type_name: &str) {
    if !AI_OBJECT_TYPES.contains(&type_name) {
        eprintln!(
            "fatal: unknown AI object type '{}'. Valid types: {}",
            type_name,
            AI_OBJECT_TYPES.join(", ")
        );
        std::process::exit(128);
    }

    let hm = build_history_manager().await;
    let objects = match hm.list_objects(type_name).await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("fatal: failed to list {} objects: {}", type_name, e);
            std::process::exit(128);
        }
    };

    if objects.is_empty() {
        println!("No {} objects found.", type_name);
        return;
    }

    for (id, hash) in &objects {
        println!("{}\t{}", id, hash);
    }
    println!("\nTotal: {} {} object(s)", objects.len(), type_name);
}

/// Redact UUID for safe logging (keep first 8 chars)
fn redact_uuid(uuid: &str) -> String {
    if uuid.chars().count() > 8 {
        format!("{}***", uuid.chars().take(8).collect::<String>())
    } else {
        "***".to_string()
    }
}

/// Pretty-print an AI object by UUID (auto-detects type).
async fn ai_pretty_print(uuid: &str) {
    let hm = build_history_manager().await;
    let (hash, type_name) = match hm.find_object_hash(uuid).await {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            eprintln!("fatal: AI object not found: {}", redact_uuid(uuid));
            std::process::exit(128);
        }
        Err(e) => {
            eprintln!(
                "fatal: failed to look up AI object {}: {}",
                redact_uuid(uuid),
                e
            );
            std::process::exit(128);
        }
    };

    // Read raw blob JSON
    let storage = ClientStorage::init(path::objects());
    let data = match storage.get(&hash) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("fatal: could not read AI object blob {}: {}", hash, e);
            std::process::exit(128);
        }
    };

    // Try to pretty-print as JSON
    match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(value) => {
            println!("type: {}", type_name);
            println!("hash: {}", hash);
            println!("---");
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&data).to_string())
            );
        }
        Err(_) => {
            // Not valid JSON — dump raw
            println!("type: {}", type_name);
            println!("hash: {}", hash);
            println!("---");
            print!("{}", String::from_utf8_lossy(&data));
        }
    }
}

/// Print the AI object type for a UUID.
async fn ai_show_type(uuid: &str) {
    let hm = build_history_manager().await;
    match hm.find_object_hash(uuid).await {
        Ok(Some((_hash, type_name))) => println!("{}", type_name),
        Ok(None) => {
            eprintln!("fatal: AI object not found: {}", redact_uuid(uuid));
            std::process::exit(128);
        }
        Err(e) => {
            eprintln!(
                "fatal: failed to look up AI object {}: {}",
                redact_uuid(uuid),
                e
            );
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
        assert_eq!(args.object, Some("abc123".to_string()));
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
        assert_eq!(args.object, Some("HEAD".to_string()));
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

    #[test]
    fn test_args_ai_object() {
        let args = CatFileArgs::try_parse_from([
            "cat-file",
            "--ai",
            "550e8400-e29b-41d4-a716-446655440000",
        ])
        .unwrap();
        assert_eq!(
            args.ai_object,
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
        assert!(!args.show_type);
    }

    #[test]
    fn test_args_ai_type() {
        let args = CatFileArgs::try_parse_from([
            "cat-file",
            "--ai-type",
            "550e8400-e29b-41d4-a716-446655440000",
        ])
        .unwrap();
        assert!(args.ai_type.is_some());
    }

    #[test]
    fn test_args_ai_list() {
        let args = CatFileArgs::try_parse_from(["cat-file", "--ai-list", "task"]).unwrap();
        assert_eq!(args.ai_list, Some("task".to_string()));
    }

    #[test]
    fn test_args_ai_list_types() {
        let args = CatFileArgs::try_parse_from(["cat-file", "--ai-list-types"]).unwrap();
        assert!(args.ai_list_types);
    }

    #[test]
    fn test_args_ai_and_git_mutual_exclusion() {
        // --ai and -t should be mutually exclusive
        let result = CatFileArgs::try_parse_from(["cat-file", "--ai", "some-uuid", "-t", "abc123"]);
        assert!(result.is_err());
    }
}
