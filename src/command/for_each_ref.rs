//! Implements `for-each-ref` to enumerate refs with filtering and formatting.

use std::{collections::HashMap, str::FromStr};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tag::Tag as GitTag, types::ObjectType},
};
use serde::Serialize;

use crate::{
    command::load_object,
    common_utils::parse_commit_msg,
    internal::{
        branch::Branch, config::ConfigKv, head::Head, log::formatter::format_timestamp_with, tag,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

/// `--help` examples for for-each-ref
pub const FOR_EACH_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra for-each-ref                  List all refs with commit info
    libra for-each-ref --heads          List only branches (refs/heads/)
    libra for-each-ref --tags           List only tags (refs/tags/)
    libra for-each-ref --remotes        List only remote-tracking refs
    libra for-each-ref --all            List all refs (default)
    libra for-each-ref --format='%(refname) %(objectname)'  Custom format
    libra for-each-ref --format='%(refname:short) %(objectname:short)'  Short ref/object forms
    libra for-each-ref --sort=refname   Sort by ref name
    libra for-each-ref --sort=version:refname   Version-aware sort (v1.9 before v1.10)
    libra for-each-ref --sort=-committerdate    Most recently committed refs first
    libra for-each-ref --sort=objectsize --format='%(objectsize) %(refname)'  Sort by object size
    libra for-each-ref --tags --format='%(refname:short) -> %(*objectname:short)'  Show each tag's dereferenced target
    libra for-each-ref --tags --format='%(refname:short) %(*objecttype) %(*objectsize)'  Dereferenced target type and size
    libra for-each-ref --shell --format='%(refname)'  Shell-quote each field for eval
    libra for-each-ref --points-at HEAD List refs that point at HEAD
    libra for-each-ref --merged=main    List refs already merged into main
    libra for-each-ref --no-merged=main List refs not yet merged into main
    libra for-each-ref --exclude=wip refs/heads/  Skip refs whose name matches the pattern
    libra for-each-ref --count=10       Limit output to 10 refs";

#[derive(Parser, Debug)]
#[command(after_help = FOR_EACH_REF_EXAMPLES)]
pub struct ForEachRefArgs {
    /// Show only branches (refs/heads/)
    #[clap(long)]
    pub heads: bool,

    /// Show only tags (refs/tags/)
    #[clap(long)]
    pub tags: bool,

    /// Show only remote-tracking refs (refs/remotes/)
    #[clap(long)]
    pub remotes: bool,

    /// Show all refs (default)
    #[clap(long)]
    pub all: bool,

    /// Custom output format with %(atoms)
    #[clap(long, value_name = "FORMAT")]
    pub format: Option<String>,

    /// Sort output by key: `refname`, `objectname`, `version:refname`
    /// (alias `v:refname`), `objectsize`, `*objectname` (an annotated tag's
    /// dereferenced object), or the date keys `committerdate` / `authordate` /
    /// `creatordate`; prefix with `-` to reverse.
    #[clap(long, value_name = "KEY")]
    pub sort: Option<String>,

    /// Limit output to N refs
    #[clap(long, value_name = "COUNT")]
    pub count: Option<usize>,

    /// Show only refs that point at OBJECT
    #[clap(long, value_name = "OBJECT")]
    pub points_at: Option<String>,

    /// Show only refs whose commit contains COMMIT (i.e. COMMIT is an ancestor).
    #[clap(long, value_name = "COMMIT")]
    pub contains: Option<String>,

    /// Show only refs whose commit does NOT contain COMMIT.
    #[clap(long = "no-contains", value_name = "COMMIT")]
    pub no_contains: Option<String>,

    /// Show only refs whose commit is merged into COMMIT (reachable from COMMIT).
    #[clap(long, value_name = "COMMIT")]
    pub merged: Option<String>,

    /// Show only refs whose commit is NOT merged into COMMIT.
    #[clap(long = "no-merged", value_name = "COMMIT")]
    pub no_merged: Option<String>,

    /// Do not list refs matching PATTERN (repeatable; applied after the
    /// positional include patterns).
    #[clap(long = "exclude", value_name = "PATTERN")]
    pub exclude: Vec<String>,

    /// Quote each interpolated field for `eval` in `sh` (single-quote escaping).
    #[clap(long = "shell", conflicts_with_all = ["perl", "python", "tcl"])]
    pub shell: bool,

    /// Quote each interpolated field as a Perl string literal.
    #[clap(long = "perl", conflicts_with_all = ["python", "tcl"])]
    pub perl: bool,

    /// Quote each interpolated field as a Python string literal.
    #[clap(long = "python", conflicts_with_all = ["tcl"])]
    pub python: bool,

    /// Quote each interpolated field as a Tcl string literal.
    #[clap(long = "tcl")]
    pub tcl: bool,

    /// Refname patterns to match
    #[clap(value_name = "PATTERN")]
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefEntry {
    refname: String,
    objectname: String,
    objecttype: String,
    #[serde(skip_serializing)]
    points_at: Vec<String>,
}

pub async fn execute(args: ForEachRefArgs) -> CliResult<()> {
    execute_safe(args, &OutputConfig::default()).await
}

pub async fn execute_safe(args: ForEachRefArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_for_each_ref(&args).await?;
    // Resolve the current HEAD branch so `%(HEAD)` can mark it with `*`.
    let head_refname = match Head::current().await {
        Head::Branch(name) => Some(format!("refs/heads/{name}")),
        Head::Detached(_) => None,
    };
    // Resolve each branch's upstream tracking ref for `%(upstream)`.
    let upstreams = resolve_upstreams(&result).await;
    // Resolve each branch's push tracking ref for `%(push)`.
    let pushes = resolve_pushes(&result).await;
    render_output(
        &result,
        &args,
        output,
        head_refname.as_deref(),
        &upstreams,
        &pushes,
    )?;
    Ok(())
}

/// Map each `refs/heads/<branch>` entry to its upstream tracking ref
/// (`refs/remotes/<remote>/<branch>`), computed from `branch.<name>.remote` and
/// `branch.<name>.merge`. Branches without a configured upstream are omitted.
/// This is the standard tracking computation (default fetch refspec); custom
/// refspec mappings are not modeled.
async fn resolve_upstreams(entries: &[RefEntry]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in entries {
        let Some(branch) = entry.refname.strip_prefix("refs/heads/") else {
            continue;
        };
        let remote = ConfigKv::get(&format!("branch.{branch}.remote"))
            .await
            .ok()
            .flatten()
            .map(|e| e.value);
        let merge = ConfigKv::get(&format!("branch.{branch}.merge"))
            .await
            .ok()
            .flatten()
            .map(|e| e.value);
        if let (Some(remote), Some(merge)) = (remote, merge) {
            let merge_short = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
            map.insert(
                entry.refname.clone(),
                format!("refs/remotes/{remote}/{merge_short}"),
            );
        }
    }
    map
}

/// Resolve a Git config variable case-insensitively in its variable name (the
/// segment after `prefix`) — Git config variable names are case-insensitive, so
/// both the documented camelCase spelling (e.g. `pushRemote`) and the lowercase
/// form emitted by `git config --list` / imports (`pushremote`) resolve to the
/// same logical variable. See [`ConfigKv::get_var_case_insensitive`] for the
/// single-row-vs-anomaly semantics.
async fn config_var(prefix: &str, variable: &str) -> Option<String> {
    ConfigKv::get_var_case_insensitive(prefix, variable)
        .await
        .ok()
        .flatten()
        .map(|entry| entry.value)
}

/// Map each `refs/heads/<branch>` entry to its push tracking ref for `%(push)`.
/// The push remote follows Git's precedence — `branch.<name>.pushRemote`, then
/// `remote.pushDefault`, then `branch.<name>.remote` — combined with
/// `branch.<name>.merge` to form `refs/remotes/<push-remote>/<branch>`. Like
/// `resolve_upstreams`, this is a config-derived computation (the standard refspec);
/// it does not check that the tracking ref exists and does not model custom refspecs.
async fn resolve_pushes(entries: &[RefEntry]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let push_default = config_var("remote.", "pushDefault").await;
    for entry in entries {
        let Some(branch) = entry.refname.strip_prefix("refs/heads/") else {
            continue;
        };
        let mut push_remote = config_var(&format!("branch.{branch}."), "pushRemote")
            .await
            .or_else(|| push_default.clone());
        if push_remote.is_none() {
            push_remote = ConfigKv::get(&format!("branch.{branch}.remote"))
                .await
                .ok()
                .flatten()
                .map(|e| e.value);
        }
        let merge = ConfigKv::get(&format!("branch.{branch}.merge"))
            .await
            .ok()
            .flatten()
            .map(|e| e.value);
        if let (Some(push_remote), Some(merge)) = (push_remote, merge) {
            let merge_short = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
            map.insert(
                entry.refname.clone(),
                format!("refs/remotes/{push_remote}/{merge_short}"),
            );
        }
    }
    map
}

async fn run_for_each_ref(_args: &ForEachRefArgs) -> CliResult<Vec<RefEntry>> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let show_all = _args.all || (!_args.heads && !_args.tags && !_args.remotes);
    let mut entries = Vec::new();

    if show_all || _args.heads {
        let branches = Branch::list_branches_result(None)
            .await
            .map_err(branch_error)?;
        for branch in branches {
            entries.push(direct_ref_entry(
                format!("refs/heads/{}", branch.name),
                branch.commit.to_string(),
                "commit",
            ));
        }
    }

    if show_all || _args.remotes {
        let remotes = ConfigKv::all_remote_configs().await.map_err(|source| {
            CliError::fatal(format!("failed to list remotes: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for remote in remotes {
            let branches = Branch::list_branches_result(Some(&remote.name))
                .await
                .map_err(branch_error)?;
            for branch in branches {
                let refname = if branch.name.starts_with("refs/remotes/") {
                    branch.name
                } else {
                    format!("refs/remotes/{}/{}", remote.name, branch.name)
                };
                entries.push(direct_ref_entry(
                    refname,
                    branch.commit.to_string(),
                    "commit",
                ));
            }
        }
    }

    if show_all || _args.tags {
        let tags = tag::list().await.map_err(|source| {
            CliError::fatal(format!("failed to list tags: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for t in tags {
            entries.push(tag_ref_entry(&t));
        }
    }

    if let Some(object_ref) = _args.points_at.as_deref() {
        let target = resolve_points_at_target(object_ref).await?;
        entries.retain(|entry| entry.points_at.iter().any(|hash| hash == &target));
    }

    if let Some(commit_ref) = _args.contains.as_deref() {
        let target = resolve_commit_target(commit_ref).await?;
        entries = retain_refs_containing(entries, &target, true).await?;
    }
    if let Some(commit_ref) = _args.no_contains.as_deref() {
        let target = resolve_commit_target(commit_ref).await?;
        entries = retain_refs_containing(entries, &target, false).await?;
    }

    if let Some(commit_ref) = _args.merged.as_deref() {
        let target = resolve_commit_target(commit_ref).await?;
        entries = retain_refs_merged_into(entries, &target, true).await?;
    }
    if let Some(commit_ref) = _args.no_merged.as_deref() {
        let target = resolve_commit_target(commit_ref).await?;
        entries = retain_refs_merged_into(entries, &target, false).await?;
    }

    if !_args.patterns.is_empty() {
        entries.retain(|entry| {
            _args
                .patterns
                .iter()
                .any(|pattern| matches_ref_pattern(&entry.refname, pattern))
        });
    }

    // `--exclude <pattern>` drops refs matching any exclude pattern (applied
    // after the include patterns, matching Git).
    if !_args.exclude.is_empty() {
        entries.retain(|entry| {
            !_args
                .exclude
                .iter()
                .any(|pattern| matches_ref_pattern(&entry.refname, pattern))
        });
    }

    // Date-based sort keys (`committerdate` / `authordate` / `creatordate`)
    // resolve each ref's timestamp by loading its object (peeling tags), so they
    // are handled separately; all other keys go through the plain key sorter.
    // Date and object-size keys require loading each ref's object, so they are
    // handled separately; all other keys go through the plain key sorter.
    let sort = _args.sort.as_deref();
    if let Some((date_key, reverse)) = sort.and_then(parse_date_sort_key) {
        sort_entries_by_date(&mut entries, date_key, reverse);
    } else if let Some(reverse) = sort.and_then(parse_objectsize_sort_key) {
        sort_entries_by_objectsize(&mut entries, reverse)?;
    } else if let Some(reverse) = sort.and_then(parse_deref_objectname_sort_key) {
        sort_entries_by_deref_objectname(&mut entries, reverse)?;
    } else {
        sort_entries(&mut entries, sort)?;
    }
    if let Some(count) = _args.count {
        entries.truncate(count);
    }
    Ok(entries)
}

/// Keep (or, when `want` is false, drop) refs whose commit has `target` as an
/// ancestor — i.e. the ref "contains" `target` (`--contains`/`--no-contains`).
/// A ref's commit is its peeled commit id (see [`peeled_commit`]); reachability
/// reuses `log::get_reachable_commits`, so this walks history once per ref.
async fn retain_refs_containing(
    entries: Vec<RefEntry>,
    target: &str,
    want: bool,
) -> CliResult<Vec<RefEntry>> {
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let contains = match peeled_commit(&entry) {
            Some(commit) => crate::command::log::get_reachable_commits(commit.clone(), None)
                .await?
                .iter()
                .any(|reachable| reachable.id.to_string().as_str() == target),
            None => false,
        };
        if contains == want {
            kept.push(entry);
        }
    }
    Ok(kept)
}

/// Keep (or, when `want` is false, drop) refs whose commit is reachable from
/// `target` — i.e. the ref is already merged into `target`
/// (`--merged`/`--no-merged`). Unlike [`retain_refs_containing`], the set of
/// commits reachable from `target` is computed once and each ref's first peeled
/// commit is tested for membership.
async fn retain_refs_merged_into(
    entries: Vec<RefEntry>,
    target: &str,
    want: bool,
) -> CliResult<Vec<RefEntry>> {
    let reachable: std::collections::HashSet<String> =
        crate::command::log::get_reachable_commits(target.to_string(), None)
            .await?
            .iter()
            .map(|commit| commit.id.to_string())
            .collect();

    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let merged = match peeled_commit(&entry) {
            Some(commit) => reachable.contains(commit),
            None => false,
        };
        if merged == want {
            kept.push(entry);
        }
    }
    Ok(kept)
}

/// The commit id a ref ultimately resolves to for reachability filters
/// (`--contains` / `--merged`). Direct refs and lightweight tags carry a single
/// id; annotated tags carry `[tag_id, peeled_target]`, so the peeled target (the
/// last element) is the commit to test. Returns `None` for refs that peel to a
/// non-commit object (tree/blob), which never satisfy a commit-reachability
/// filter.
fn peeled_commit(entry: &RefEntry) -> Option<&String> {
    entry.points_at.last()
}

fn direct_ref_entry(refname: String, objectname: String, objecttype: &str) -> RefEntry {
    RefEntry {
        refname,
        points_at: vec![objectname.clone()],
        objectname,
        objecttype: objecttype.to_string(),
    }
}

fn tag_ref_entry(tag: &tag::Tag) -> RefEntry {
    let (objectname, objecttype, points_at) = tag_object_info(&tag.object);
    RefEntry {
        refname: format!("refs/tags/{}", tag.name),
        objectname,
        objecttype,
        points_at,
    }
}

fn tag_object_info(object: &tag::TagObject) -> (String, String, Vec<String>) {
    match object {
        tag::TagObject::Commit(commit) => {
            let id = commit.id.to_string();
            (id.clone(), "commit".to_string(), vec![id])
        }
        tag::TagObject::Tag(tag) => (
            tag.id.to_string(),
            "tag".to_string(),
            vec![tag.id.to_string(), tag.object_hash.to_string()],
        ),
        tag::TagObject::Tree(tree) => {
            let id = tree.id.to_string();
            (id.clone(), "tree".to_string(), vec![id])
        }
        tag::TagObject::Blob(blob) => {
            let id = blob.id.to_string();
            (id.clone(), "blob".to_string(), vec![id])
        }
    }
}

/// Resolve the COMMIT argument of a reachability filter (`--contains` /
/// `--no-contains` / `--merged` / `--no-merged`) to a commit id, peeling
/// annotated tag names and tag objects to their underlying commit. Unlike
/// [`resolve_points_at_target`] — which keeps the raw tag object so
/// `--points-at` can match tag refs — this follows tags to a commit via
/// `util::get_commit_base`, so the reachability walk always starts from a
/// commit (matching Git's commit-ish resolution for these filters).
async fn resolve_commit_target(commit_ref: &str) -> CliResult<String> {
    // Fully-qualified refs name a namespace explicitly and must resolve only
    // within it — a same-named ref in another namespace must not shadow it.
    if let Some(tag_name) = commit_ref.strip_prefix("refs/tags/") {
        // Tag namespace: peel annotated tags to their commit.
        return match tag::find_tag_and_commit(tag_name).await {
            Ok(Some((_object, commit))) => Ok(commit.id.to_string()),
            Ok(None) => Err(invalid_object_name(commit_ref)),
            Err(source) => Err(CliError::fatal(format!(
                "failed to resolve tag '{commit_ref}': {source}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)),
        };
    }
    if let Some(branch_name) = commit_ref.strip_prefix("refs/heads/") {
        // Local-branch namespace: the branch store is keyed by short names, so
        // strip the prefix and look it up directly without falling back to tags.
        return match Branch::find_branch_result(branch_name, None).await {
            Ok(Some(branch)) => Ok(branch.commit.to_string()),
            Ok(None) => Err(invalid_object_name(commit_ref)),
            Err(source) => Err(branch_error(source)),
        };
    }
    if let Some(remote_path) = commit_ref.strip_prefix("refs/remotes/") {
        // Remote-tracking namespace: resolve only against remote-tracking
        // branches, trying each `<remote>/<branch>` split (multi-segment
        // remotes). All lookups are scoped to `Some(remote)`, with no
        // local-branch/tag/hash fallback — so a local branch literally named
        // `refs/remotes/<...>` cannot shadow the remote ref. Fetched refs are
        // stored under the full `refs/remotes/<remote>/<branch>` name (see
        // `fetch.rs`/`remote.rs`); an older/short form stores just the branch
        // name, so try the full ref first, then the short branch.
        for (remote, branch_name) in util::remote_tracking_candidates(remote_path) {
            for key in [commit_ref, branch_name] {
                match Branch::find_branch_result(key, Some(remote)).await {
                    Ok(Some(branch)) => return Ok(branch.commit.to_string()),
                    Ok(None) => {}
                    Err(source) => return Err(branch_error(source)),
                }
            }
        }
        return Err(invalid_object_name(commit_ref));
    }

    // Everything else — HEAD, short branch/tag/remote names, and commit hashes —
    // goes through the general commit-ish resolver, which peels annotated tags
    // and honors Git's resolution precedence for short names.
    match util::get_commit_base(commit_ref).await {
        Ok(hash) => Ok(hash.to_string()),
        Err(_) => Err(invalid_object_name(commit_ref)),
    }
}

/// Build the standard "Not a valid object name" error for an unresolvable
/// reachability target.
fn invalid_object_name(commit_ref: &str) -> CliError {
    CliError::fatal(format!("Not a valid object name {commit_ref}"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
}

async fn resolve_points_at_target(object_ref: &str) -> CliResult<String> {
    let tag_name = object_ref.strip_prefix("refs/tags/").unwrap_or(object_ref);
    if let Some(tag_ref) = tag::find_tag_ref(tag_name).await.map_err(|source| {
        CliError::fatal(format!("failed to resolve tag '{object_ref}': {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let target = tag_ref.target.ok_or_else(|| {
            CliError::fatal(format!("tag '{object_ref}' is missing target object"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        ObjectHash::from_str(&target).map_err(|source| {
            CliError::fatal(format!(
                "tag '{object_ref}' has invalid target object '{target}': {source}"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        return Ok(target);
    }

    if let Ok(hash) = util::get_commit_base(object_ref).await {
        return Ok(hash.to_string());
    }
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return Ok(hash.to_string());
    }

    Err(
        CliError::fatal(format!("Not a valid object name {object_ref}"))
            .with_stable_code(StableErrorCode::CliInvalidTarget),
    )
}

fn branch_error(source: crate::internal::branch::BranchStoreError) -> CliError {
    CliError::fatal(format!("failed to list branches: {source}"))
        .with_stable_code(StableErrorCode::IoReadFailed)
}

fn matches_ref_pattern(refname: &str, pattern: &str) -> bool {
    refname == pattern || refname.ends_with(pattern) || refname.contains(pattern)
}

/// A date-based `--sort` key. `committerdate`/`authordate` use the (peeled)
/// commit's committer/author date; `creatordate` uses the annotated tag's
/// tagger date, falling back to the commit's committer date for everything else
/// (commits and lightweight tags), matching Git.
#[derive(Clone, Copy)]
enum DateSortKey {
    Committer,
    Author,
    Creator,
}

/// Recognise a date-based sort key, returning the key and whether a leading `-`
/// requested a reversed (descending) order. Non-date keys return `None` so they
/// fall through to [`sort_entries`].
fn parse_date_sort_key(sort: &str) -> Option<(DateSortKey, bool)> {
    let (reverse, name) = match sort.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, sort),
    };
    let key = match name {
        "committerdate" => DateSortKey::Committer,
        "authordate" => DateSortKey::Author,
        "creatordate" => DateSortKey::Creator,
        _ => return None,
    };
    Some((key, reverse))
}

/// Recognise the `objectsize` sort key, returning whether a leading `-` requested
/// reversed (descending) order. Non-matching keys return `None`.
fn parse_objectsize_sort_key(sort: &str) -> Option<bool> {
    match sort.strip_prefix('-') {
        Some("objectsize") => Some(true),
        _ if sort == "objectsize" => Some(false),
        _ => None,
    }
}

/// The byte size of the object a ref points at directly (the tag object for an
/// annotated tag, the commit for a branch) — Git's `%(objectsize)`.
/// `ClientStorage::get` returns the decompressed object content, whose length is
/// the size Git reports. A missing/unreadable object is a real corruption and is
/// surfaced as an error (rather than silently reported as size 0).
fn ref_object_size(entry: &RefEntry) -> CliResult<i64> {
    let hash = ObjectHash::from_str(&entry.objectname).map_err(|source| {
        CliError::fatal(format!(
            "ref '{}' has an invalid object id '{}': {source}",
            entry.refname, entry.objectname
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let data = util::objects_storage().get(&hash).map_err(|source| {
        CliError::fatal(format!(
            "failed to read object {} for ref '{}': {source}",
            entry.objectname, entry.refname
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    Ok(data.len() as i64)
}

/// Sort entries by their object's byte size (`objectsize`); ties break by refname
/// ascending, matching Git's final ordering key.
fn sort_entries_by_objectsize(entries: &mut [RefEntry], reverse: bool) -> CliResult<()> {
    let mut sizes: Vec<i64> = Vec::with_capacity(entries.len());
    for entry in entries.iter() {
        sizes.push(ref_object_size(entry)?);
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let primary = sizes[a].cmp(&sizes[b]);
        let primary = if reverse { primary.reverse() } else { primary };
        primary.then_with(|| entries[a].refname.cmp(&entries[b].refname))
    });
    let reordered: Vec<RefEntry> = order.into_iter().map(|i| entries[i].clone()).collect();
    entries.clone_from_slice(&reordered);
    Ok(())
}

/// Recognise the `*objectname` (dereferenced object name) sort key, returning
/// whether a leading `-` requested reversed order. Non-matching keys return
/// `None`.
fn parse_deref_objectname_sort_key(sort: &str) -> Option<bool> {
    match sort.strip_prefix('-') {
        Some("*objectname") => Some(true),
        _ if sort == "*objectname" => Some(false),
        _ => None,
    }
}

/// Sort entries by `*objectname` (the object an annotated tag dereferences to,
/// empty for non-tag refs); ties break by refname ascending, matching Git's
/// final ordering key. Empty values sort together (lexicographically first).
fn sort_entries_by_deref_objectname(entries: &mut [RefEntry], reverse: bool) -> CliResult<()> {
    let mut derefs: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries.iter() {
        derefs.push(ref_deref_objectname(entry)?);
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let primary = derefs[a].cmp(&derefs[b]);
        let primary = if reverse { primary.reverse() } else { primary };
        primary.then_with(|| entries[a].refname.cmp(&entries[b].refname))
    });
    let reordered: Vec<RefEntry> = order.into_iter().map(|i| entries[i].clone()).collect();
    entries.clone_from_slice(&reordered);
    Ok(())
}

/// Sort entries by a date key. The timestamp for each ref is resolved by loading
/// its object (peeling annotated tags to their commit); ties break by refname
/// ascending, matching Git's final ordering key.
fn sort_entries_by_date(entries: &mut [RefEntry], key: DateSortKey, reverse: bool) {
    let mut times: Vec<i64> = Vec::with_capacity(entries.len());
    for entry in entries.iter() {
        times.push(ref_sort_timestamp(entry, key));
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let primary = times[a].cmp(&times[b]);
        let primary = if reverse { primary.reverse() } else { primary };
        primary.then_with(|| entries[a].refname.cmp(&entries[b].refname))
    });
    let reordered: Vec<RefEntry> = order.into_iter().map(|i| entries[i].clone()).collect();
    entries.clone_from_slice(&reordered);
}

/// Resolve the timestamp a ref contributes for a date sort key (`0` when the
/// object cannot be loaded or carries no such date — e.g. a tag pointing at a
/// tree/blob).
fn ref_sort_timestamp(entry: &RefEntry, key: DateSortKey) -> i64 {
    // `creatordate` of an annotated tag is its OWN tagger date (not the peeled
    // commit's). `entry.objecttype` is the object's actual type, determined when
    // the ref was listed, so loading it as a tag here is sound.
    if matches!(key, DateSortKey::Creator) && entry.objecttype == "tag" {
        return ObjectHash::from_str(&entry.objectname)
            .ok()
            .and_then(|hash| load_object::<GitTag>(&hash).ok())
            .map(|tag| tag.tagger.timestamp as i64)
            .unwrap_or(0);
    }
    match ref_commit(entry) {
        Some(commit) => match key {
            DateSortKey::Author => commit.author.timestamp as i64,
            // Committer and creatordate (for commits / lightweight tags).
            _ => commit.committer.timestamp as i64,
        },
        None => 0,
    }
}

/// Maximum number of annotated-tag dereferences when peeling to a commit (the
/// terminal commit itself does not count against this); guards against tag
/// cycles and pathological chains.
pub const MAX_TAG_PEEL_DEPTH: usize = 16;

/// Resolve the commit a ref ultimately points to, dereferencing annotated tags
/// (tag → tag → … → commit). Returns `None` when the chain resolves to a
/// tree/blob or cannot be loaded.
fn ref_commit(entry: &RefEntry) -> Option<Commit> {
    let hash = ObjectHash::from_str(&entry.objectname).ok()?;
    peel_to_commit(hash)
}

/// Peel an object to the commit it ultimately names, following annotated-tag
/// targets. The object database's **actual** stored type is consulted (via
/// `get_object_type`) before every typed load — never a tag's declared `type`
/// line — so a corrupt or mismatched object is never handed to a typed parser
/// that assumes the wrong kind (the `from_bytes` parsers are not defensive
/// against the wrong object type). Allows up to [`MAX_TAG_PEEL_DEPTH`] tag
/// dereferences plus the terminal commit (so a chain of exactly that many tags
/// still resolves) before giving up; returns `None` for a chain ending at a
/// tree/blob or an unreadable object.
fn peel_to_commit(start: ObjectHash) -> Option<Commit> {
    let storage = util::objects_storage();
    let mut current = start;
    // `..=` so the terminal commit can be checked after the deepest allowed tag.
    for _ in 0..=MAX_TAG_PEEL_DEPTH {
        match storage.get_object_type(&current).ok()? {
            ObjectType::Commit => return load_object::<Commit>(&current).ok(),
            ObjectType::Tag => current = load_object::<GitTag>(&current).ok()?.object_hash,
            _ => return None,
        }
    }
    None
}

/// Git's `%(*objectname)`: the object an annotated tag dereferences to (empty
/// string for non-tag refs). Only annotated tags dereference; the value is the
/// tag's recorded target object id, following nested tags via the tag objects'
/// own `object_type`/`object_hash` (no need to read the target object itself,
/// matching Git, which reports the recorded id). A tag whose chain cannot be
/// resolved is a corruption and is surfaced as an error rather than rendered
/// empty (which would be indistinguishable from a legitimate non-tag ref).
fn ref_deref_objectname(entry: &RefEntry) -> CliResult<String> {
    Ok(ref_deref_target(entry)?
        .map(|(hash, _)| hash.to_string())
        .unwrap_or_default())
}

/// Git's `%(*objecttype)`: the type of the object an annotated tag dereferences
/// to (empty for non-tag refs). Read from the tag's recorded `object_type` (the
/// final non-tag tag in a nested chain), so no target read is needed.
fn ref_deref_objecttype(entry: &RefEntry) -> CliResult<String> {
    Ok(ref_deref_target(entry)?
        .map(|(_, object_type)| object_type_name(object_type).to_string())
        .unwrap_or_default())
}

/// Git's `%(*objectsize)`: the byte size of the object an annotated tag
/// dereferences to (`None` → empty for non-tag refs). Unlike `*objecttype`, the
/// size is not recorded in the tag, so the dereferenced object is read; a
/// missing/unreadable target is surfaced as an error rather than a silent 0.
fn ref_deref_objectsize(entry: &RefEntry) -> CliResult<Option<i64>> {
    let Some((target, _)) = ref_deref_target(entry)? else {
        return Ok(None);
    };
    let data = util::objects_storage().get(&target).map_err(|source| {
        CliError::fatal(format!(
            "failed to read the object {target} dereferenced from ref '{}': {source}",
            entry.refname
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    Ok(Some(data.len() as i64))
}

/// Shared helper for the `*`-dereference atoms: for an annotated-tag ref,
/// `Ok(Some((target object id, target type)))`; `Ok(None)` for any non-tag ref
/// (branches, lightweight tags), matching Git, whose `*` atoms are empty unless
/// the ref points at a tag object. A tag ref whose chain cannot be peeled (a
/// missing/corrupt object id, or an unreadable intermediate tag) returns `Err`
/// so the failure is surfaced rather than silently collapsing to the non-tag
/// empty case.
fn ref_deref_target(entry: &RefEntry) -> CliResult<Option<(ObjectHash, ObjectType)>> {
    if entry.objecttype != "tag" {
        return Ok(None);
    }
    let start = ObjectHash::from_str(&entry.objectname).map_err(|source| {
        CliError::fatal(format!(
            "ref '{}' has an invalid object id '{}': {source}",
            entry.refname, entry.objectname
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    Ok(Some(peel_tag_to_target(start, &entry.refname)?))
}

/// Follow an annotated-tag object to the first non-tag object it points at,
/// returning that object's id and type. Uses each tag's recorded `object_type`/
/// `object_hash` (not the target's stored type) and is bounded by
/// [`MAX_TAG_PEEL_DEPTH`]. An unreadable tag in the chain, or a chain deeper than
/// the bound (e.g. a cycle), is surfaced as an error.
fn peel_tag_to_target(tag_hash: ObjectHash, refname: &str) -> CliResult<(ObjectHash, ObjectType)> {
    let mut current = tag_hash;
    for _ in 0..=MAX_TAG_PEEL_DEPTH {
        let tag = load_object::<GitTag>(&current).map_err(|source| {
            CliError::fatal(format!(
                "failed to read tag object {current} while dereferencing ref '{refname}': {source}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        if tag.object_type == ObjectType::Tag {
            current = tag.object_hash;
        } else {
            return Ok((tag.object_hash, tag.object_type));
        }
    }
    Err(CliError::fatal(format!(
        "ref '{refname}' has a tag chain deeper than {MAX_TAG_PEEL_DEPTH} (possible cycle)"
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt))
}

/// The Git object-type name for an [`ObjectType`], matching the strings used for
/// `%(objecttype)`. A tag only ever dereferences to one of the four canonical
/// loose object types; any other (pack-internal delta) variant is not a valid
/// stored object type and degrades to an empty string.
fn object_type_name(object_type: ObjectType) -> &'static str {
    match object_type {
        ObjectType::Commit => "commit",
        ObjectType::Tree => "tree",
        ObjectType::Blob => "blob",
        ObjectType::Tag => "tag",
        _ => "",
    }
}

fn sort_entries(entries: &mut [RefEntry], sort: Option<&str>) -> CliResult<()> {
    match sort.unwrap_or("refname") {
        "refname" => entries.sort_by(|a, b| a.refname.cmp(&b.refname)),
        "-refname" => entries.sort_by(|a, b| b.refname.cmp(&a.refname)),
        "objectname" => entries.sort_by(|a, b| a.objectname.cmp(&b.objectname)),
        "-objectname" => entries.sort_by(|a, b| b.objectname.cmp(&a.objectname)),
        // `version:refname` (and the `v:refname` alias) order embedded numbers
        // numerically, so `v1.9` sorts before `v1.10`. Shared comparator.
        "version:refname" | "v:refname" => {
            entries.sort_by(|a, b| util::version_refname_cmp(&a.refname, &b.refname))
        }
        "-version:refname" | "-v:refname" => {
            entries.sort_by(|a, b| util::version_refname_cmp(&b.refname, &a.refname))
        }
        other => {
            return Err(CliError::command_usage(format!(
                "unsupported for-each-ref sort key '{other}'"
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments));
        }
    }
    Ok(())
}

/// Output quoting style (`--shell` / `--perl` / `--python` / `--tcl`): each
/// interpolated field value is wrapped as a string literal of the target
/// language so the output can be `eval`-ed/sourced. Literal text in the format
/// (and the default `<oid> <refname>` separators) is left unquoted.
#[derive(Clone, Copy)]
enum QuoteStyle {
    Shell,
    Perl,
    Python,
    Tcl,
}

/// Resolve the active quoting style from the mutually-exclusive flags (clap
/// already rejects more than one).
fn resolve_quote_style(args: &ForEachRefArgs) -> Option<QuoteStyle> {
    if args.shell {
        Some(QuoteStyle::Shell)
    } else if args.perl {
        Some(QuoteStyle::Perl)
    } else if args.python {
        Some(QuoteStyle::Python)
    } else if args.tcl {
        Some(QuoteStyle::Tcl)
    } else {
        None
    }
}

/// Quote `value` as a string literal in the given style, matching `git
/// for-each-ref`'s `--shell`/`--perl`/`--python`/`--tcl` output.
fn quote_value(value: &str, style: QuoteStyle) -> String {
    match style {
        // Single-quote; both `'` and `!` close the quote, emit a backslash-escaped
        // char, and reopen (e.g. `'` → `'\''`, `!` → `'\!'`), matching git's
        // `sq_quote_buf`. Other bytes (incl. newlines) are kept verbatim.
        QuoteStyle::Shell => {
            let mut out = String::with_capacity(value.len() + 2);
            out.push('\'');
            for ch in value.chars() {
                if ch == '\'' || ch == '!' {
                    out.push_str("'\\");
                    out.push(ch);
                    out.push('\'');
                } else {
                    out.push(ch);
                }
            }
            out.push('\'');
            out
        }
        // Single-quote; escape backslash first, then the single-quote (git's
        // `perl_quote_buf`). Newlines stay literal.
        QuoteStyle::Perl => {
            format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
        }
        // Like Perl, but also convert a newline to a literal `\n` so the result
        // stays a single-line Python literal (git's `python_quote_buf`).
        QuoteStyle::Python => {
            let escaped = value
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('\n', "\\n");
            format!("'{escaped}'")
        }
        // Double-quote; backslash-escape the Tcl specials and name the control
        // characters, matching Git's `tcl_quote_buf`.
        QuoteStyle::Tcl => {
            let mut out = String::with_capacity(value.len() + 2);
            out.push('"');
            for ch in value.chars() {
                match ch {
                    '[' | ']' | '{' | '}' | '$' | '\\' | '"' => {
                        out.push('\\');
                        out.push(ch);
                    }
                    '\u{c}' => out.push_str("\\f"),
                    '\r' => out.push_str("\\r"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\u{b}' => out.push_str("\\v"),
                    _ => out.push(ch),
                }
            }
            out.push('"');
            out
        }
    }
}

/// Push an interpolated field value, quoting it when a `--shell`/etc. style is
/// active (literal format text bypasses this and is pushed directly).
fn push_field(out: &mut String, value: &str, quote: Option<QuoteStyle>) {
    match quote {
        Some(style) => out.push_str(&quote_value(value, style)),
        None => out.push_str(value),
    }
}

fn render_output(
    entries: &[RefEntry],
    args: &ForEachRefArgs,
    output: &OutputConfig,
    head_refname: Option<&str>,
    upstreams: &HashMap<String, String>,
    pushes: &HashMap<String, String>,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("for-each-ref", &entries.to_vec(), output);
    }
    if output.quiet {
        return Ok(());
    }

    let quote = resolve_quote_style(args);
    for entry in entries {
        if let Some(format) = &args.format {
            println!(
                "{}",
                render_format(format, entry, head_refname, upstreams, pushes, quote)?
            );
        } else if let Some(style) = quote {
            // The default format is the two fields `<objectname> <refname>`; each
            // is quoted independently, the separating space is literal.
            println!(
                "{} {}",
                quote_value(&entry.objectname, style),
                quote_value(&entry.refname, style)
            );
        } else {
            println!("{} {}", entry.objectname, entry.refname);
        }
    }
    Ok(())
}

fn render_format(
    format: &str,
    entry: &RefEntry,
    head_refname: Option<&str>,
    upstreams: &HashMap<String, String>,
    pushes: &HashMap<String, String>,
    quote: Option<QuoteStyle>,
) -> CliResult<String> {
    // `:short` modifiers: the short ref name (namespace prefix stripped) and the
    // 7-char abbreviated object id. Substituted before the bare atoms (the
    // strings are distinct, so order is not load-bearing, only for clarity).
    let refname_short = short_refname(&entry.refname);
    let objectname_short: String = entry.objectname.chars().take(7).collect();
    // `%(objectsize)`: the byte size of the ref's object (computed lazily only
    // when the atom is present, to avoid an extra object read per ref).
    let objectsize = if format.contains("%(objectsize)") {
        ref_object_size(entry)?.to_string()
    } else {
        String::new()
    };
    // `%(*objectname)` / `%(*objectname:short)`: the object an annotated tag
    // dereferences to (empty for non-tag refs); computed lazily.
    let deref_objectname = if format.contains("%(*objectname") {
        ref_deref_objectname(entry)?
    } else {
        String::new()
    };
    let deref_objectname_short: String = deref_objectname.chars().take(7).collect();
    // `%(*objecttype)` / `%(*objectsize)`: the type / byte size of the object an
    // annotated tag dereferences to (empty for non-tag refs); computed lazily.
    let deref_objecttype = if format.contains("%(*objecttype)") {
        ref_deref_objecttype(entry)?
    } else {
        String::new()
    };
    let deref_objectsize = if format.contains("%(*objectsize)") {
        ref_deref_objectsize(entry)?
            .map(|size| size.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    // `%(HEAD)`: `*` for the currently checked-out branch, a space otherwise.
    let head_marker = if head_refname == Some(entry.refname.as_str()) {
        "*"
    } else {
        " "
    };
    // `%(upstream)`: the tracking ref (empty when none); `:short` strips the
    // `refs/remotes/` prefix.
    let upstream = upstreams
        .get(&entry.refname)
        .map(String::as_str)
        .unwrap_or("");
    let upstream_short = upstream.strip_prefix("refs/remotes/").unwrap_or(upstream);
    // `%(push)`: the push-tracking ref (empty when none); `:short` strips the
    // `refs/remotes/` prefix.
    let push = pushes.get(&entry.refname).map(String::as_str).unwrap_or("");
    let push_short = push.strip_prefix("refs/remotes/").unwrap_or(push);
    // Commit-field atoms (`%(subject)`, author/committer name+email) require
    // loading the ref's object. Load it once, only when at least one such atom
    // is present, to avoid extra object reads.
    const COMMIT_FIELD_ATOMS: [&str; 14] = [
        "%(subject)",
        "%(contents)",
        "%(contents:subject)",
        "%(body)",
        "%(contents:body)",
        "%(authorname)",
        "%(authoremail)",
        "%(authordate)",
        "%(committername)",
        "%(committeremail)",
        "%(committerdate)",
        "%(taggername)",
        "%(taggeremail)",
        "%(taggerdate)",
    ];
    let fields = if COMMIT_FIELD_ATOMS.iter().any(|a| format.contains(a)) {
        commit_fields_for(entry)
    } else {
        CommitFields::default()
    };
    // Atom name (inside `%(...)`) -> value. Single-pass substitution below
    // writes each value literally, so a value containing `%(` is never
    // re-parsed as an atom and never trips the unknown-atom check.
    let atoms: [(&str, &str); 29] = [
        ("objectsize", objectsize.as_str()),
        ("*objectname:short", deref_objectname_short.as_str()),
        ("*objectname", deref_objectname.as_str()),
        ("*objecttype", deref_objecttype.as_str()),
        ("*objectsize", deref_objectsize.as_str()),
        ("HEAD", head_marker),
        ("upstream:short", upstream_short),
        ("upstream", upstream),
        ("push:short", push_short),
        ("push", push),
        ("subject", fields.subject.as_str()),
        ("contents:subject", fields.subject.as_str()),
        ("contents:body", fields.body.as_str()),
        ("contents", fields.contents.as_str()),
        ("body", fields.body.as_str()),
        ("authorname", fields.author_name.as_str()),
        ("authoremail", fields.author_email.as_str()),
        ("authordate", fields.author_date.as_str()),
        ("committername", fields.committer_name.as_str()),
        ("committeremail", fields.committer_email.as_str()),
        ("committerdate", fields.committer_date.as_str()),
        ("taggername", fields.tagger_name.as_str()),
        ("taggeremail", fields.tagger_email.as_str()),
        ("taggerdate", fields.tagger_date.as_str()),
        ("refname:short", refname_short.as_str()),
        ("objectname:short", objectname_short.as_str()),
        ("refname", entry.refname.as_str()),
        ("objectname", entry.objectname.as_str()),
        ("objecttype", entry.objecttype.as_str()),
    ];
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(pos) = rest.find("%(") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let Some(end) = after.find(')') else {
            return Err(unsupported_atom_error());
        };
        let token = &after[2..end];
        // Parameterized atoms (`%(refname:lstrip=N)` / `%(refname:rstrip=N)` and
        // `%(objectname:short=N)`) are handled first; everything else is an exact
        // atom-name match.
        // Each interpolated field value is quoted (when a `--shell`/etc. style is
        // active); the literal format text between atoms is pushed verbatim above.
        if let Some(value) = refname_strip_atom(token, &entry.refname) {
            push_field(&mut out, &value, quote);
        } else if let Some(n) = token
            .strip_prefix("objectname:short=")
            .and_then(|s| s.parse::<usize>().ok())
        {
            push_field(
                &mut out,
                &entry.objectname.chars().take(n).collect::<String>(),
                quote,
            );
        } else {
            match atoms.iter().find(|(name, _)| *name == token) {
                Some((_, value)) => push_field(&mut out, value, quote),
                None => return Err(unsupported_atom_error()),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn unsupported_atom_error() -> CliError {
    CliError::command_usage("unsupported for-each-ref format atom")
        .with_stable_code(StableErrorCode::CliInvalidArguments)
}

/// Commit-field atom values for one ref. `author_*`/`committer_*` are populated
/// only for refs pointing directly at a commit (empty for annotated tags, which
/// carry a tagger rather than an author, and for trees/blobs); `subject` is the
/// first message line of a commit or annotated-tag object. The `*_email` values
/// include the surrounding angle brackets, matching Git.
#[derive(Default)]
struct CommitFields {
    subject: String,
    /// Full message (`%(contents)`): gpgsig-stripped for commits, the raw
    /// message for annotated tags.
    contents: String,
    /// Message body (`%(body)`): everything after the first blank line.
    body: String,
    author_name: String,
    author_email: String,
    committer_name: String,
    committer_email: String,
    author_date: String,
    committer_date: String,
    tagger_name: String,
    tagger_email: String,
    tagger_date: String,
}

/// Load the ref's object (once) and extract its commit-field atom values.
fn commit_fields_for(entry: &RefEntry) -> CommitFields {
    let Ok(hash) = ObjectHash::from_str(&entry.objectname) else {
        return CommitFields::default();
    };
    match entry.objecttype.as_str() {
        "commit" => match load_object::<Commit>(&hash) {
            // Strip a leading `gpgsig`/`gpgsig-sha256` header before the subject.
            Ok(c) => {
                let contents = parse_commit_msg(&c.message).0.to_string();
                CommitFields {
                    subject: first_subject_line(&contents),
                    body: message_body(&contents),
                    contents,
                    author_name: c.author.name.clone(),
                    author_email: format!("<{}>", c.author.email),
                    committer_name: c.committer.name.clone(),
                    committer_email: format!("<{}>", c.committer.email),
                    author_date: format_timestamp_with(c.author.timestamp as i64, ""),
                    committer_date: format_timestamp_with(c.committer.timestamp as i64, ""),
                    ..CommitFields::default()
                }
            }
            Err(_) => CommitFields::default(),
        },
        // Annotated tags have a message (subject) and a tagger, but no
        // author/committer.
        "tag" => match load_object::<GitTag>(&hash) {
            Ok(t) => CommitFields {
                subject: first_subject_line(&t.message),
                body: message_body(&t.message),
                contents: t.message.clone(),
                tagger_name: t.tagger.name.clone(),
                tagger_email: format!("<{}>", t.tagger.email),
                tagger_date: format_timestamp_with(t.tagger.timestamp as i64, ""),
                ..CommitFields::default()
            },
            Err(_) => CommitFields::default(),
        },
        _ => CommitFields::default(),
    }
}

/// First non-empty line of a commit/tag message (messages can carry leading
/// newlines from the header separator).
fn first_subject_line(message: &str) -> String {
    message
        .trim_start_matches('\n')
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Message body for `%(body)`: everything after the first blank line that
/// separates the subject from the body (empty when there is no body), matching
/// `git for-each-ref`.
fn message_body(message: &str) -> String {
    message
        .trim_start_matches('\n')
        .split_once("\n\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// The `:short` form of a ref name: strip the well-known namespace prefix
/// (`refs/heads/`, `refs/tags/`, `refs/remotes/`), falling back to stripping a
/// leading `refs/`, otherwise the name unchanged.
fn short_refname(refname: &str) -> String {
    for prefix in ["refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(rest) = refname.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    refname.strip_prefix("refs/").unwrap_or(refname).to_string()
}

/// Handle `%(refname:lstrip=N)` / `%(refname:rstrip=N)`, returning the stripped
/// ref name. `N > 0` removes that many leading (lstrip) or trailing (rstrip)
/// slash-separated components; `N < 0` keeps the last `|N|` (lstrip) or first
/// `|N|` (rstrip) components. Returns `None` for any other token (including a
/// non-integer N), so the caller treats it as an unknown atom.
fn refname_strip_atom(token: &str, refname: &str) -> Option<String> {
    let (from_left, num) = if let Some(n) = token.strip_prefix("refname:lstrip=") {
        (true, n)
    } else if let Some(n) = token.strip_prefix("refname:rstrip=") {
        (false, n)
    } else {
        return None;
    };
    let n: i64 = num.parse().ok()?;
    let comps: Vec<&str> = refname.split('/').collect();
    let len = comps.len() as i64;
    let kept: &[&str] = match (from_left, n >= 0) {
        // lstrip=N: drop N leading components
        (true, true) => comps.get(n.min(len) as usize..).unwrap_or(&[]),
        // lstrip=-N: keep the last N components
        (true, false) => &comps[(len - (-n).min(len)) as usize..],
        // rstrip=N: drop N trailing components
        (false, true) => &comps[..(len - n.min(len)) as usize],
        // rstrip=-N: keep the first N components
        (false, false) => comps.get(..(-n).min(len) as usize).unwrap_or(&comps),
    };
    Some(kept.join("/"))
}
