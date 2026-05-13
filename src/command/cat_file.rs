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
//! - `--ai <id>`:            pretty-print an AI object by object ID
//! - `--ai-type <id>`:       print the AI object type (intent/task/run/…)
//! - `--ai-list <type>`:     list all AI objects of the given type
//! - `--ai-list-types`:      list all AI object types present in history

use std::{str::FromStr, sync::Arc};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::{
    command::load_object,
    common_utils::parse_commit_msg,
    internal::{ai::history::HistoryManager, db, model::reference},
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, exit_with_legacy_stderr},
        output::{OutputConfig, emit_json_data},
        path,
        storage::local::LocalStorage,
        util,
    },
};

const CAT_FILE_LONG_ABOUT: &str = "Inspect Git objects or Libra AI history objects.

Modes:
  - Git modes: use exactly one of -t/-s/-p/-e and provide OBJECT.
  - AI lookup modes: use exactly one of --ai/--ai-type with an AI object ID or TYPE:ID.
  - AI listing modes: use --ai-list <TYPE> or --ai-list-types.

Notes:
  - OBJECT is ignored for all --ai* modes.
  - --ai and --ai-type search the AI history branch and can resolve persisted session objects such as ai_session.
  - If the same ID exists under multiple AI types, pass TYPE:ID to disambiguate.
  - --ai on ai_session objects prints a unified session summary before full JSON.
  - --ai-list accepts built-in AI types as well as any type already present in the history branch.";

const CAT_FILE_AFTER_HELP: &str = "Examples:
  libra cat-file -p HEAD
  libra cat-file -t 40d352ee7190f92dcf7883b8a81f2c730fd8a860
  libra cat-file --ai-list intent
  libra cat-file --ai patchset:call_KjR3NB4cQaT5Rm1c7zXjsskQ
  libra cat-file --ai 5b878637-f852-4bff-adee-3354c42ae69f
  libra cat-file --ai-type debug-local-1772707227";

/// Provide content, type, or size information for repository objects (Git and AI).
#[derive(Parser, Debug)]
#[command(
    about = "Inspect Git objects or Libra AI history objects",
    long_about = CAT_FILE_LONG_ABOUT,
    after_help = CAT_FILE_AFTER_HELP,
)]
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
    /// Pretty-print an AI object by ID across all stored AI types, or disambiguate with TYPE:ID.
    #[clap(long = "ai", value_name = "ID", group = "mode")]
    pub ai_object: Option<String>,

    /// Print the type of an AI object by ID across all stored AI types, or disambiguate with TYPE:ID.
    #[clap(long = "ai-type", value_name = "ID", group = "mode")]
    pub ai_type: Option<String>,

    /// List all AI objects of the given type (for example intent, patchset, event, patchset_snapshot)
    #[clap(long = "ai-list", value_name = "TYPE", group = "mode")]
    pub ai_list: Option<String>,

    /// List all AI object types present in the history branch
    #[clap(long = "ai-list-types", group = "mode")]
    pub ai_list_types: bool,

    /// Git object hash or ref. Required only for -t/-s/-p/-e and ignored for all --ai* modes.
    #[clap(value_name = "OBJECT")]
    pub object: Option<String>,
}

/// Known AI object type names stored under the `libra/intent` orphan branch.
const AI_OBJECT_TYPES: &[&str] = &[
    "agent_message",
    "ai_session",
    "approval_request",
    "context_frame",
    "context_snapshot",
    "claude_decision_input",
    "claude_managed_evidence_input",
    "decision",
    "event",
    "evidence",
    "evidence_input",
    "intent",
    "intent_event",
    "intent_snapshot",
    "invocation",
    "patchset",
    "patchset_snapshot",
    "plan",
    "plan_snapshot",
    "plan_step_event",
    "plan_step_snapshot",
    "provider_session",
    "provenance",
    "provenance_snapshot",
    "reasoning",
    "task",
    "task_event",
    "task_snapshot",
    "tool_invocation",
    "tool_invocation_event",
    "run",
    "run_event",
    "run_snapshot",
    "run_usage",
    "snapshot",
];
const TAG_REF_PREFIX: &str = "refs/tags/";

fn is_known_ai_object_type(type_name: &str) -> bool {
    AI_OBJECT_TYPES.contains(&type_name)
}

fn canonical_ai_object_type(type_name: &str) -> &str {
    match type_name {
        "context_snapshot" => "snapshot",
        "tool_invocation" => "invocation",
        _ => type_name,
    }
}

fn split_typed_ai_selector(selector: &str) -> Option<(&str, &str)> {
    let (type_name, object_id) = selector.split_once(':')?;
    if object_id.is_empty() || !is_known_ai_object_type(type_name) {
        return None;
    }
    Some((canonical_ai_object_type(type_name), object_id))
}

async fn resolve_ai_object_with_history(
    hm: &HistoryManager,
    selector: &str,
) -> CliResult<(ObjectHash, String)> {
    if let Some((type_name, object_id)) = split_typed_ai_selector(selector) {
        return hm
            .get_object_hash(type_name, object_id)
            .await
            .map_err(|e| {
                CliError::fatal(format!(
                    "failed to look up AI object {}: {}",
                    redact_uuid(object_id),
                    e
                ))
            })?
            .map(|hash| (hash, type_name.to_string()))
            .ok_or_else(|| {
                CliError::fatal(format!(
                    "AI object not found: {}:{}",
                    type_name,
                    redact_uuid(object_id)
                ))
            });
    }

    let matches = hm.find_object_hashes(selector).await.map_err(|e| {
        CliError::fatal(format!(
            "failed to look up AI object {}: {}",
            redact_uuid(selector),
            e
        ))
    })?;

    match matches.len() {
        0 => Err(CliError::fatal(format!(
            "AI object not found: {}",
            redact_uuid(selector)
        ))),
        1 => Ok(matches[0].clone()),
        _ => {
            let mut kinds: Vec<String> = matches.into_iter().map(|(_, kind)| kind).collect();
            kinds.sort();
            Err(CliError::fatal(format!(
                "AI object ID {} is ambiguous across types: {}. Use TYPE:ID to disambiguate.",
                redact_uuid(selector),
                kinds.join(", ")
            )))
        }
    }
}

async fn resolve_ai_object(selector: &str) -> CliResult<(ObjectHash, String)> {
    let hm = build_history_manager().await?;
    resolve_ai_object_with_history(&hm, selector).await
}

async fn ensure_ai_listable_type(hm: &HistoryManager, type_name: &str) -> CliResult<()> {
    if is_known_ai_object_type(type_name) {
        return Ok(());
    }

    let existing_types = hm
        .list_object_types()
        .await
        .map_err(|e| CliError::fatal(format!("failed to list AI object types: {e}")))?;
    if existing_types.iter().any(|existing| existing == type_name) {
        return Ok(());
    }

    Err(CliError::fatal(format!(
        "unknown AI object type '{}'. Valid built-in types: {}",
        type_name,
        AI_OBJECT_TYPES.join(", ")
    )))
}

fn cat_file_exit(message: impl Into<String>) -> ! {
    exit_with_legacy_stderr(message)
}

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
        None => cat_file_exit("fatal: <object> is required for Git object modes"),
    };

    let storage = ClientStorage::init(path::objects());
    let hash = resolve_object(object_ref, &storage).await;

    if args.check_exist {
        check_object_exists(&hash, &storage);
        return;
    }

    let obj_type = match storage.get_object_type(&hash) {
        Ok(t) => t,
        Err(_) => cat_file_exit(format!("fatal: Not a valid object name {}", object_ref)),
    };

    if args.show_type {
        println!("{}", obj_type);
    } else if args.show_size {
        print_object_size(&storage, &hash);
    } else if args.pretty_print {
        pretty_print_object(&hash, obj_type);
    } else {
        cat_file_exit("fatal: one of '-t', '-s', '-p', '-e' or an --ai* flag is required");
    }
}

/// Thin wrapper for CLI dispatch. Internal errors are still handled via
/// `eprintln!` + `process::exit`.
///
/// # Known limitations
///
/// `execute()` handles errors internally and never propagates them, so the
/// safe path only delegates to it for plain human-output mode.
pub async fn execute_safe(args: CatFileArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    if args.check_exist && !output.is_json() {
        execute(args).await;
        return Ok(());
    }

    if !output.quiet && !output.is_json() {
        execute(args).await;
        return Ok(());
    }

    execute_with_output_contract(args, output).await
}

async fn execute_with_output_contract(args: CatFileArgs, output: &OutputConfig) -> CliResult<()> {
    if args.ai_list_types {
        let types = ai_list_types_data().await?;
        if output.is_json() {
            emit_json_data(
                "cat-file",
                &serde_json::json!({
                    "mode": "ai_list_types",
                    "types": types,
                }),
                output,
            )?;
        }
        return Ok(());
    }

    if let Some(type_name) = args.ai_list.as_deref() {
        let objects = ai_list_objects_data(type_name).await?;
        if output.is_json() {
            emit_json_data(
                "cat-file",
                &serde_json::json!({
                    "mode": "ai_list",
                    "object_type": type_name,
                    "entries": objects,
                    "total": objects.len(),
                }),
                output,
            )?;
        }
        return Ok(());
    }

    if let Some(uuid) = args.ai_object.as_deref() {
        let ai_object = ai_pretty_print_data(uuid).await?;
        if output.is_json() {
            emit_json_data("cat-file", &ai_object, output)?;
        }
        return Ok(());
    }

    if let Some(uuid) = args.ai_type.as_deref() {
        let object_type = ai_show_type_data(uuid).await?;
        if output.is_json() {
            emit_json_data(
                "cat-file",
                &serde_json::json!({
                    "mode": "ai_type",
                    "object_type": object_type,
                }),
                output,
            )?;
        }
        return Ok(());
    }

    let object_ref = args
        .object
        .as_deref()
        .ok_or_else(|| CliError::command_usage("<object> is required for Git object modes"))?;

    if args.check_exist {
        return Err(CliError::command_usage(
            "`cat-file -e` does not yet support --json or --machine output",
        ));
    }

    let storage = ClientStorage::init(path::objects());
    let hash = resolve_object_safe(object_ref, &storage).await?;
    let obj_type = storage
        .get_object_type(&hash)
        .map_err(|_| invalid_object_name_error(object_ref))?;

    if args.show_type {
        if output.is_json() {
            emit_json_data(
                "cat-file",
                &serde_json::json!({
                    "mode": "type",
                    "object": object_ref,
                    "hash": hash.to_string(),
                    "object_type": obj_type.to_string(),
                }),
                output,
            )?;
        }
        return Ok(());
    }

    if args.show_size {
        let data = storage
            .get(&hash)
            .map_err(|e| CliError::fatal(format!("unable to read object {hash}: {e}")))?;
        if output.is_json() {
            emit_json_data(
                "cat-file",
                &serde_json::json!({
                    "mode": "size",
                    "object": object_ref,
                    "hash": hash.to_string(),
                    "size": data.len(),
                }),
                output,
            )?;
        }
        return Ok(());
    }

    if args.pretty_print {
        return emit_pretty_print_json(object_ref, &hash, obj_type, output);
    }

    Err(CliError::command_usage(
        "one of '-t', '-s', '-p', '-e' or an --ai* flag is required",
    ))
}

fn invalid_object_name_error(object_ref: &str) -> CliError {
    CliError::from_legacy_string(format!("fatal: Not a valid object name {}", object_ref))
}

async fn resolve_object_safe(object_ref: &str, storage: &ClientStorage) -> CliResult<ObjectHash> {
    if let Some(hash) = resolve_tag_object_ref(object_ref).await {
        return Ok(hash);
    }

    if let Ok(hash) = util::get_commit_base(object_ref).await {
        return Ok(hash);
    }

    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return Ok(hash);
    }

    let results = storage.search_result(object_ref).await.map_err(|error| {
        CliError::fatal(format!(
            "failed to search objects while resolving '{object_ref}': {error}"
        ))
    })?;
    if results.len() == 1 {
        return Ok(results[0]);
    }
    if results.len() > 1 {
        return Err(CliError::fatal(format!(
            "ambiguous argument '{}': matched {} objects",
            object_ref,
            results.len()
        )));
    }

    Err(invalid_object_name_error(object_ref))
}

fn emit_pretty_print_json(
    object_ref: &str,
    hash: &ObjectHash,
    obj_type: ObjectType,
    output: &OutputConfig,
) -> CliResult<()> {
    match obj_type {
        ObjectType::Blob => {
            let blob = load_object::<Blob>(hash)
                .map_err(|e| CliError::fatal(format!("could not read blob {hash}: {e}")))?;
            if output.is_json() {
                let content = String::from_utf8(blob.data).map_err(|_| {
                    CliError::command_usage(
                        "`cat-file -p` does not yet support --json for binary blob content",
                    )
                })?;
                emit_json_data(
                    "cat-file",
                    &serde_json::json!({
                        "mode": "pretty",
                        "object": object_ref,
                        "hash": hash.to_string(),
                        "object_type": "blob",
                        "content": content,
                    }),
                    output,
                )?;
            }
            Ok(())
        }
        ObjectType::Tree => {
            let tree = load_object::<Tree>(hash)
                .map_err(|e| CliError::fatal(format!("could not read tree {hash}: {e}")))?;
            if output.is_json() {
                let entries: Vec<serde_json::Value> = tree
                    .tree_items
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "mode": format!("{:06o}", item.mode as u32),
                            "object_type": match item.mode {
                                git_internal::internal::object::tree::TreeItemMode::Tree => "tree",
                                _ => "blob",
                            },
                            "hash": item.id.to_string(),
                            "name": item.name,
                        })
                    })
                    .collect();
                emit_json_data(
                    "cat-file",
                    &serde_json::json!({
                        "mode": "pretty",
                        "object": object_ref,
                        "hash": hash.to_string(),
                        "object_type": "tree",
                        "entries": entries,
                    }),
                    output,
                )?;
            }
            Ok(())
        }
        ObjectType::Commit => {
            let commit = load_object::<Commit>(hash)
                .map_err(|e| CliError::fatal(format!("could not read commit {hash}: {e}")))?;
            if output.is_json() {
                let (message, _) = parse_commit_msg(&commit.message);
                emit_json_data(
                    "cat-file",
                    &serde_json::json!({
                        "mode": "pretty",
                        "object": object_ref,
                        "hash": hash.to_string(),
                        "object_type": "commit",
                        "tree": commit.tree_id.to_string(),
                        "parents": commit
                            .parent_commit_ids
                            .iter()
                            .map(|parent| parent.to_string())
                            .collect::<Vec<_>>(),
                        "author": {
                            "name": commit.author.name.trim(),
                            "email": commit.author.email.trim(),
                            "timestamp": commit.author.timestamp,
                            "timezone": commit.author.timezone,
                        },
                        "committer": {
                            "name": commit.committer.name.trim(),
                            "email": commit.committer.email.trim(),
                            "timestamp": commit.committer.timestamp,
                            "timezone": commit.committer.timezone,
                        },
                        "message": message.trim(),
                    }),
                    output,
                )?;
            }
            Ok(())
        }
        ObjectType::Tag => {
            let storage = ClientStorage::init(path::objects());
            let data = storage
                .get(hash)
                .map_err(|e| CliError::fatal(format!("could not read tag {hash}: {e}")))?;
            if output.is_json() {
                let content = String::from_utf8(data).map_err(|_| {
                    CliError::command_usage(
                        "`cat-file -p` does not yet support --json for non-UTF-8 tag content",
                    )
                })?;
                emit_json_data(
                    "cat-file",
                    &serde_json::json!({
                        "mode": "pretty",
                        "object": object_ref,
                        "hash": hash.to_string(),
                        "object_type": "tag",
                        "content": content,
                    }),
                    output,
                )?;
            }
            Ok(())
        }
        _ => Err(CliError::fatal(format!(
            "unsupported object type {:?}",
            obj_type
        ))),
    }
}

async fn ai_list_types_data() -> CliResult<Vec<serde_json::Value>> {
    let hm = build_history_manager().await?;
    let mut types = Vec::new();
    for type_name in hm
        .list_object_types()
        .await
        .map_err(|e| CliError::fatal(format!("failed to list AI object types: {e}")))?
    {
        let objects = hm
            .list_objects(&type_name)
            .await
            .map_err(|e| CliError::fatal(format!("failed to list {type_name} objects: {e}")))?;
        if !objects.is_empty() {
            types.push(serde_json::json!({
                "object_type": type_name,
                "count": objects.len(),
            }));
        }
    }
    Ok(types)
}

async fn ai_list_objects_data(type_name: &str) -> CliResult<Vec<serde_json::Value>> {
    let hm = build_history_manager().await?;
    ensure_ai_listable_type(&hm, type_name).await?;
    let canonical_type_name = canonical_ai_object_type(type_name);
    let objects = hm
        .list_objects(canonical_type_name)
        .await
        .map_err(|e| CliError::fatal(format!("failed to list {type_name} objects: {e}")))?;

    Ok(objects
        .into_iter()
        .map(|(id, hash)| {
            serde_json::json!({
                "id": id,
                "hash": hash.to_string(),
            })
        })
        .collect())
}

async fn ai_pretty_print_data(uuid: &str) -> CliResult<serde_json::Value> {
    let (hash, type_name) = resolve_ai_object(uuid).await?;

    let storage = ClientStorage::init(path::objects());
    let data = storage
        .get(&hash)
        .map_err(|e| CliError::fatal(format!("could not read AI object blob {hash}: {e}")))?;
    let parsed = serde_json::from_slice::<serde_json::Value>(&data)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&data).to_string()));

    let summary = if let serde_json::Value::Object(_) = &parsed {
        let lines = match type_name.as_str() {
            "ai_session" => ai_session_summary_lines(&parsed),
            "provider_session" => provider_session_summary_lines(&parsed),
            "evidence_input" => evidence_input_summary_lines(&parsed),
            _ => vec![],
        };
        serde_json::Value::Array(lines.into_iter().map(serde_json::Value::String).collect())
    } else {
        serde_json::Value::Array(vec![])
    };

    Ok(serde_json::json!({
        "mode": "ai_object",
        "object_type": type_name,
        "hash": hash.to_string(),
        "summary": summary,
        "value": parsed,
    }))
}

async fn ai_show_type_data(uuid: &str) -> CliResult<String> {
    resolve_ai_object(uuid)
        .await
        .map(|(_hash, type_name)| type_name)
}

/// Resolve a user-supplied object reference to an `ObjectHash`.
///
/// Supports branch names, tags, HEAD, and raw hex hashes.
async fn resolve_object(object_ref: &str, storage: &ClientStorage) -> ObjectHash {
    // Resolve tags without dereferencing annotated tag objects to commits.
    if let Some(hash) = resolve_tag_object_ref(object_ref).await {
        return hash;
    }

    // Try as a ref (branch/tag/HEAD) first
    if let Ok(hash) = util::get_commit_base(object_ref).await {
        return hash;
    }

    // Try as a raw hex hash
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return hash;
    }

    // Try abbreviated hash via storage search
    let results = match storage.search_result(object_ref).await {
        Ok(results) => results,
        Err(error) => cat_file_exit(format!(
            "fatal: failed to search objects while resolving '{}': {}",
            object_ref, error
        )),
    };
    if results.len() == 1 {
        return results[0];
    } else if results.len() > 1 {
        cat_file_exit(format!(
            "fatal: ambiguous argument '{}': matched {} objects",
            object_ref,
            results.len()
        ));
    }

    cat_file_exit(format!("fatal: Not a valid object name {}", object_ref));
}

fn normalize_tag_ref_name(object_ref: &str) -> String {
    if object_ref.starts_with(TAG_REF_PREFIX) {
        object_ref.to_string()
    } else {
        format!("{TAG_REF_PREFIX}{object_ref}")
    }
}

async fn resolve_tag_object_ref(object_ref: &str) -> Option<ObjectHash> {
    let full_ref_name = normalize_tag_ref_name(object_ref);
    let db_conn = db::get_db_conn_instance().await;
    let tag_ref = reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Tag))
        .filter(reference::Column::Name.eq(full_ref_name))
        .one(&db_conn)
        .await
        .ok()
        .flatten()?;

    let target_hash = tag_ref.commit?;
    ObjectHash::from_str(&target_hash).ok()
}

/// Exit with 0 if the object exists, 1 otherwise, without printing diagnostics.
fn check_object_exists(hash: &ObjectHash, storage: &ClientStorage) {
    if !storage.exist(hash) {
        std::process::exit(1);
    }
}

/// Print the size (in bytes) of the raw object data.
fn print_object_size(storage: &ClientStorage, hash: &ObjectHash) {
    match storage.get(hash) {
        Ok(data) => println!("{}", data.len()),
        Err(e) => cat_file_exit(format!("fatal: unable to read object {}: {}", hash, e)),
    }
}

/// Pretty-print an object based on its type.
fn pretty_print_object(hash: &ObjectHash, obj_type: ObjectType) {
    match obj_type {
        ObjectType::Blob => print_blob(hash),
        ObjectType::Tree => print_tree(hash),
        ObjectType::Commit => print_commit(hash),
        ObjectType::Tag => print_tag(hash),
        _ => cat_file_exit(format!("fatal: unsupported object type {:?}", obj_type)),
    }
}

/// Print a blob object's raw content.
fn print_blob(hash: &ObjectHash) {
    let blob: Blob = match std::panic::catch_unwind(|| load_object(hash)) {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => cat_file_exit(format!("fatal: could not read blob {}: {}", hash, e)),
        Err(_) => cat_file_exit(format!(
            "fatal: failed to load blob object {}: internal error (panic)",
            hash
        )),
    };
    match String::from_utf8(blob.data.clone()) {
        Ok(text) => print!("{}", text),
        Err(_) => {
            // Write raw binary to stdout
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&blob.data).unwrap_or_else(|e| {
                cat_file_exit(format!("fatal: write error: {}", e));
            });
        }
    }
}

/// Print a tree object in a human-readable format.
fn print_tree(hash: &ObjectHash) {
    let tree: Tree = match std::panic::catch_unwind(|| load_object(hash)) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => cat_file_exit(format!("fatal: could not read tree {}: {}", hash, e)),
        Err(_) => cat_file_exit(format!(
            "fatal: failed to load tree object {}: internal error (panic)",
            hash
        )),
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
    let commit: Commit = match std::panic::catch_unwind(|| load_object(hash)) {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => cat_file_exit(format!("fatal: could not read commit {}: {}", hash, e)),
        Err(_) => cat_file_exit(format!(
            "fatal: failed to load commit object {}: internal error (panic)",
            hash
        )),
    };
    println!("tree {}", commit.tree_id);
    for parent in &commit.parent_commit_ids {
        println!("parent {}", parent);
    }
    println!(
        "author {} <{}> {} {}",
        commit.author.name.trim(),
        commit.author.email.trim(),
        commit.author.timestamp,
        commit.author.timezone,
    );
    println!(
        "committer {} <{}> {} {}",
        commit.committer.name.trim(),
        commit.committer.email.trim(),
        commit.committer.timestamp,
        commit.committer.timezone,
    );
    println!();
    let (msg, _) = parse_commit_msg(&commit.message);
    println!("{}", msg.trim());
}

/// Print a tag object in human-readable format.
fn print_tag(hash: &ObjectHash) {
    let storage = ClientStorage::init(path::objects());
    let data = match storage.get(hash) {
        Ok(d) => d,
        Err(e) => cat_file_exit(format!("fatal: could not read tag {}: {}", hash, e)),
    };
    // Tag objects are text-based, print raw content
    match String::from_utf8(data) {
        Ok(text) => print!("{}", text),
        Err(_) => cat_file_exit(format!("fatal: invalid tag object encoding for {}", hash)),
    }
}

// ── AI object helpers ───────────────────────────────────────────────────

/// Build a `HistoryManager` from the current repo context.
async fn build_history_manager() -> CliResult<HistoryManager> {
    let repo_path = util::try_get_storage_path(None).map_err(|_| CliError::repo_not_found())?;
    let objects_dir = repo_path.join("objects");
    let db_path = repo_path.join(util::DATABASE);
    let db_conn = db::get_db_conn_instance_for_path(&db_path)
        .await
        .map_err(|e| {
            CliError::fatal(format!(
                "failed to open repository database '{}': {}",
                db_path.display(),
                e
            ))
        })?;
    let storage = Arc::new(LocalStorage::new(objects_dir));
    Ok(HistoryManager::new(storage, repo_path, Arc::new(db_conn)))
}

/// List all AI object types that have at least one entry in the history branch.
async fn ai_list_types() {
    let hm = match build_history_manager().await {
        Ok(hm) => hm,
        Err(err) => cat_file_exit(err.to_string()),
    };
    let types = match hm.list_object_types().await {
        Ok(types) => types,
        Err(e) => cat_file_exit(format!("fatal: failed to list AI object types: {}", e)),
    };
    for type_name in types {
        match hm.list_objects(&type_name).await {
            Ok(objects) if !objects.is_empty() => {
                println!("{}\t({} objects)", type_name, objects.len());
            }
            Ok(_) => {}
            Err(e) => cat_file_exit(format!(
                "fatal: failed to list {} objects: {}",
                type_name, e
            )),
        }
    }
}

/// List all AI objects of a specific type.
async fn ai_list_objects(type_name: &str) {
    let hm = match build_history_manager().await {
        Ok(hm) => hm,
        Err(err) => cat_file_exit(err.to_string()),
    };
    if let Err(err) = ensure_ai_listable_type(&hm, type_name).await {
        cat_file_exit(err.to_string());
    }
    let canonical_type_name = canonical_ai_object_type(type_name);
    let objects = match hm.list_objects(canonical_type_name).await {
        Ok(o) => o,
        Err(e) => cat_file_exit(format!(
            "fatal: failed to list {} objects: {}",
            type_name, e
        )),
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
    let (hash, type_name) = match resolve_ai_object(uuid).await {
        Ok(resolved) => resolved,
        Err(err) => cat_file_exit(err.to_string()),
    };

    // Read raw blob JSON
    let storage = ClientStorage::init(path::objects());
    let data = match storage.get(&hash) {
        Ok(d) => d,
        Err(e) => cat_file_exit(format!(
            "fatal: could not read AI object blob {}: {}",
            hash, e
        )),
    };

    // Try to pretty-print as JSON
    match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(value) => {
            println!("type: {}", type_name);
            println!("hash: {}", hash);
            if type_name == "ai_session" {
                print_ai_session_summary(&value);
            } else if type_name == "provider_session" {
                print_provider_session_summary(&value);
            } else if type_name == "evidence_input" {
                print_evidence_input_summary(&value);
            }
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

fn print_ai_session_summary(value: &serde_json::Value) {
    for line in ai_session_summary_lines(value) {
        println!("{line}");
    }
}

fn print_provider_session_summary(value: &serde_json::Value) {
    for line in provider_session_summary_lines(value) {
        println!("{line}");
    }
}

fn print_evidence_input_summary(value: &serde_json::Value) {
    for line in evidence_input_summary_lines(value) {
        println!("{line}");
    }
}

fn ai_session_summary_lines(value: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(schema) = value.get("schema").and_then(serde_json::Value::as_str) {
        lines.push(format!("schema: {schema}"));
    }
    if let Some(provider) = value.get("provider").and_then(serde_json::Value::as_str) {
        lines.push(format!("provider: {provider}"));
    }
    if let Some(ai_session_id) = value
        .get("ai_session_id")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("ai_session_id: {ai_session_id}"));
    }
    if let Some(provider_session_id) = value
        .get("provider_session_id")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("provider_session_id: {provider_session_id}"));
    }

    if let Some(state_machine) = value.get("state_machine") {
        if let Some(phase) = state_machine
            .get("phase")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(format!("phase: {phase}"));
        }
        if let Some(status) = state_machine
            .get("status")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(format!("status: {status}"));
        }
        if let Some(event_count) = state_machine
            .get("event_count")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!("event_count: {event_count}"));
        }
        if let Some(tool_use_count) = state_machine
            .get("tool_use_count")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!("tool_event_count: {tool_use_count}"));
        }
        if let Some(compaction_count) = state_machine
            .get("compaction_count")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!("compaction_count: {compaction_count}"));
        }
    }

    if let Some(summary) = value.get("summary")
        && let Some(message_count) = summary
            .get("message_count")
            .and_then(serde_json::Value::as_u64)
    {
        lines.push(format!("message_count: {message_count}"));
    }

    if let Some(transcript) = value.get("transcript") {
        if let Some(path) = transcript.get("path").and_then(serde_json::Value::as_str) {
            lines.push(format!("transcript_path: {path}"));
        }
        if let Some(raw_event_count) = transcript
            .get("raw_event_count")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!("transcript_raw_event_count: {raw_event_count}"));
        }
    }

    lines
}

fn provider_session_summary_lines(value: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(schema) = value.get("schema").and_then(serde_json::Value::as_str) {
        lines.push(format!("schema: {schema}"));
    }
    if let Some(provider) = value.get("provider").and_then(serde_json::Value::as_str) {
        lines.push(format!("provider: {provider}"));
    }
    if let Some(object_id) = value.get("objectId").and_then(serde_json::Value::as_str) {
        lines.push(format!("object_id: {object_id}"));
    }
    if let Some(provider_session_id) = value
        .get("providerSessionId")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("provider_session_id: {provider_session_id}"));
    }
    if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str) {
        lines.push(format!("summary: {summary}"));
    }
    if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
        lines.push(format!("cwd: {cwd}"));
    }
    if let Some(message_sync) = value.get("messageSync") {
        if let Some(message_count) = message_sync
            .get("messageCount")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!("message_count: {message_count}"));
        }
        if let Some(first_kind) = message_sync
            .get("firstMessageKind")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(format!("first_message_kind: {first_kind}"));
        }
        if let Some(last_kind) = message_sync
            .get("lastMessageKind")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(format!("last_message_kind: {last_kind}"));
        }
    }

    lines
}

fn evidence_input_summary_lines(value: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(schema) = value.get("schema").and_then(serde_json::Value::as_str) {
        lines.push(format!("schema: {schema}"));
    }
    if let Some(provider) = value.get("provider").and_then(serde_json::Value::as_str) {
        lines.push(format!("provider: {provider}"));
    }
    if let Some(object_id) = value.get("objectId").and_then(serde_json::Value::as_str) {
        lines.push(format!("object_id: {object_id}"));
    }
    if let Some(provider_session_id) = value
        .get("providerSessionId")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("provider_session_id: {provider_session_id}"));
    }
    if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str) {
        lines.push(format!("summary: {summary}"));
    }
    if let Some(message_count) = value
        .get("messageOverview")
        .and_then(|overview| overview.get("messageCount"))
        .and_then(serde_json::Value::as_u64)
    {
        lines.push(format!("message_count: {message_count}"));
    }
    if let Some(assistant_count) = value
        .get("contentOverview")
        .and_then(|overview| overview.get("assistantMessageCount"))
        .and_then(serde_json::Value::as_u64)
    {
        lines.push(format!("assistant_message_count: {assistant_count}"));
    }
    if let Some(tool_count) = value
        .get("contentOverview")
        .and_then(|overview| overview.get("observedTools"))
        .and_then(serde_json::Value::as_object)
        .map(|tools| tools.len())
    {
        lines.push(format!("observed_tool_count: {tool_count}"));
    }
    if let Some(has_structured_output) = value
        .get("runtimeSignals")
        .and_then(|signals| signals.get("hasStructuredOutput"))
        .and_then(serde_json::Value::as_bool)
    {
        lines.push(format!("has_structured_output: {has_structured_output}"));
    }

    lines
}

/// Print the AI object type for a UUID.
async fn ai_show_type(uuid: &str) {
    match resolve_ai_object(uuid).await {
        Ok((_hash, type_name)) => println!("{}", type_name),
        Err(err) => cat_file_exit(err.to_string()),
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
            "patchset:550e8400-e29b-41d4-a716-446655440000",
        ])
        .unwrap();
        assert_eq!(
            args.ai_object,
            Some("patchset:550e8400-e29b-41d4-a716-446655440000".to_string())
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

    #[test]
    fn test_help_mentions_mode_relationships() {
        use clap::CommandFactory;

        let mut command = CatFileArgs::command();
        let mut help = Vec::new();
        command.write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(help.contains("OBJECT is ignored for all --ai* modes"));
        assert!(help.contains("persisted session objects such as ai_session"));
        assert!(help.contains("TYPE:ID"));
        assert!(help.contains("--ai-type <ID>"));
    }

    #[test]
    fn test_split_typed_ai_selector_recognizes_known_type() {
        assert_eq!(
            split_typed_ai_selector("patchset:call_123"),
            Some(("patchset", "call_123"))
        );
        assert_eq!(split_typed_ai_selector("unknown:call_123"), None);
        assert_eq!(split_typed_ai_selector("plain-id"), None);
    }

    #[test]
    fn test_normalize_tag_ref_name_short() {
        assert_eq!(normalize_tag_ref_name("v1.0.0"), "refs/tags/v1.0.0");
    }

    #[test]
    fn test_normalize_tag_ref_name_full() {
        assert_eq!(
            normalize_tag_ref_name("refs/tags/v1.0.0"),
            "refs/tags/v1.0.0"
        );
    }

    #[test]
    fn test_ai_session_summary_reads_tool_counts_from_state_machine() {
        let value = serde_json::json!({
            "schema": "libra.ai_session.v2",
            "provider": "gemini",
            "state_machine": {
                "phase": "ended",
                "event_count": 4,
                "tool_use_count": 2,
                "compaction_count": 1
            },
            "summary": {
                "message_count": 3
            },
            "transcript": {
                "path": "/tmp/t.jsonl",
                "raw_event_count": 4
            }
        });

        let lines = ai_session_summary_lines(&value);
        assert!(lines.iter().any(|line| line == "tool_event_count: 2"));
        assert!(lines.iter().any(|line| line == "compaction_count: 1"));
        assert!(lines.iter().any(|line| line == "message_count: 3"));
    }
}
