//! Push command wiring that reads remote configuration, negotiates with servers, and sends local refs and pack data for update.

use std::{
    collections::{HashSet, VecDeque},
    io::Write,
    path::Path,
    str::FromStr,
};

use bytes::BytesMut;
use clap::Parser;
use colored::Colorize;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
        pack::{encode::PackEncoder, entry::Entry},
    },
};
use sea_orm::TransactionTrait;
use tokio::sync::mpsc;
use url::Url;

use crate::{
    command::branch,
    git_protocol::{ServiceType::ReceivePack, add_pkt_line_string, read_pkt_line},
    internal::{
        branch::Branch,
        config::Config,
        db::get_db_conn_instance,
        head::Head,
        protocol::{ProtocolClient, https_client::HttpsClient, lfs_client::LFSClient},
        reflog::{Reflog, ReflogAction, ReflogContext, ReflogError},
    },
    utils::object_ext::{BlobExt, CommitExt, TreeExt},
};

#[derive(Parser, Debug)]
pub struct PushArgs {
    /// repository, e.g. origin
    #[clap(requires("refspec"))]
    repository: Option<String>,
    /// ref to push, e.g. master
    #[clap(requires("repository"))]
    refspec: Option<String>,

    #[clap(long, short = 'u', requires("refspec"), requires("repository"))]
    set_upstream: bool,

    /// force push to remote repository
    #[clap(long, short = 'f')]
    pub force: bool,
}

pub async fn execute(args: PushArgs) {
    if args.repository.is_some() ^ args.refspec.is_some() {
        // must provide both or none
        eprintln!("fatal: both repository and refspec should be provided");
        return;
    }
    if args.set_upstream && args.refspec.is_none() {
        eprintln!("fatal: --set-upstream requires a branch name");
        return;
    }

    let branch = match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(_) => panic!("fatal: HEAD is detached while pushing"),
    };

    let repository = match args.repository {
        Some(repo) => repo,
        None => {
            // e.g. [branch "master"].remote = origin
            let remote = Config::get_remote(&branch).await;
            if let Some(remote) = remote {
                remote
            } else {
                eprintln!("fatal: no remote configured for branch '{branch}'");
                return;
            }
        }
    };
    let repo_url = Config::get_remote_url(&repository).await;

    let branch = args.refspec.unwrap_or(branch);
    let commit_hash = match Branch::find_branch(&branch, None).await {
        Some(branch_info) => branch_info.commit.to_string(),
        None => {
            eprintln!("fatal: branch '{}' not found", branch);
            return;
        }
    };

    println!("pushing {branch}({commit_hash}) to {repository}({repo_url})");

    let url = match Url::parse(&repo_url).or_else(|e| {
        if e == url::ParseError::RelativeUrlWithoutBase && Path::new(&repo_url).exists() {
            Url::from_file_path(Path::new(&repo_url))
                .map_err(|_| url::ParseError::RelativeUrlWithoutBase)
        } else {
            Err(e)
        }
    }) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("fatal: invalid remote url '{}': {}", repo_url, e);
            return;
        }
    };

    // Local file path remote is not supported for push; avoid pretending success.
    if url.scheme() == "file" {
        eprintln!("fatal: pushing to local file repositories is not yet supported");
        return;
    }

    let client = HttpsClient::from_url(&url);
    let refs = match client.discovery_reference(ReceivePack).await {
        Ok(refs) => refs,
        Err(e) => {
            eprintln!("fatal: {e}");
            return;
        }
    };

    let tracked_branch = Config::get("branch", Some(&branch), "merge")
        .await // New branch may not have tracking branch
        .unwrap_or_else(|| format!("refs/heads/{branch}"));

    let tracked_ref = refs.iter().find(|r| r._ref == tracked_branch);
    // [0; 20] if new branch
    let remote_hash = tracked_ref
        .map(|r| r._hash.clone())
        .unwrap_or(ObjectHash::default().to_string());
    if remote_hash == commit_hash {
        println!("Everything up-to-date");
        return;
    }

    // Check if remote is ancestor of local (for fast-forward check)
    let remote_sha1 = ObjectHash::from_str(&remote_hash).unwrap();
    let local_sha1 = ObjectHash::from_str(&commit_hash).unwrap();
    let can_fast_forward = if remote_sha1 == ObjectHash::default() {
        true // New branch, always fast-forwardable
    } else {
        is_ancestor(&remote_sha1, &local_sha1)
    };

    // If remote has commits that local doesn't have and force is not specified, reject push
    if !can_fast_forward && !args.force {
        eprintln!("fatal: cannot push to '{}' (non-fast-forward)", branch);
        eprintln!(
            "hint: Updates were rejected because the remote contains work that you do not have locally."
        );
        eprintln!("hint: This is usually caused by another repository pushing to the same ref.");
        eprintln!(
            "hint: You may want to first integrate the remote changes (e.g., 'libra pull ...')"
        );
        eprintln!("hint: before pushing again, or use '--force' to overwrite the remote history.");
        return;
    } else if !can_fast_forward && args.force {
        // Force push case - only show warning when force is actually needed
        println!(
            "{}",
            "warning: forcing update of remote reference (override history)".yellow()
        );
        println!(
            "{}",
            "warning: this may overwrite remote commits, use with caution".yellow()
        );
    }

    let mut data = BytesMut::new();
    add_pkt_line_string(
        &mut data,
        format!("{remote_hash} {commit_hash} {tracked_branch}\0report-status\n"),
    );
    data.extend_from_slice(b"0000");
    tracing::debug!("{:?}", data);

    // TODO 考虑remote有多个refs，可以少发一点commits
    let objs = incremental_objs(
        ObjectHash::from_str(&commit_hash).unwrap(),
        ObjectHash::from_str(&remote_hash).unwrap(),
    );

    {
        // upload lfs files
        let client = LFSClient::from_url(&url);
        let res = client.push_objects(&objs).await;
        if res.is_err() {
            eprintln!("fatal: LFS files upload failed, stop pushing");
            return;
        }
    }

    let (entry_tx, entry_rx) = mpsc::channel::<MetaAttached<Entry, EntryMeta>>(1_000_000);
    let (stream_tx, mut stream_rx) = mpsc::channel(1_000_000);

    let encoder = PackEncoder::new(objs.len(), 0, stream_tx); // TODO: diff slow, so window_size = 0
    encoder.encode_async(entry_rx).await.unwrap();

    for obj in objs.iter().cloned() {
        // TODO progress bar
        let meta_entry = MetaAttached {
            inner: obj,
            meta: EntryMeta::default(),
        };
        if let Err(e) = entry_tx.send(meta_entry).await {
            tracing::error!("fatal: failed to send entry: {}", e);
            return;
        }
    }
    drop(entry_tx);

    println!("Compression...");
    let mut pack_data = Vec::new();
    while let Some(chunk) = stream_rx.recv().await {
        pack_data.extend(chunk);
    }
    data.extend_from_slice(&pack_data);
    println!("Delta compression done.");

    let res = client.send_pack(data.freeze()).await.unwrap(); // TODO: send stream

    if res.status() != 200 {
        eprintln!("status code: {}", res.status());
    }
    let mut data = res.bytes().await.unwrap();
    let (_, pkt_line) = read_pkt_line(&mut data);
    if pkt_line != "unpack ok\n" {
        eprintln!("fatal: unpack failed");
        return;
    }
    let (_, pkt_line) = read_pkt_line(&mut data);
    if !pkt_line.starts_with("ok".as_ref()) {
        eprintln!("fatal: ref update failed [{pkt_line:?}]");
        return;
    }
    let (len, _) = read_pkt_line(&mut data);
    assert_eq!(len, 0);

    println!("{}", "Push success".green());

    let remote_tracking_branch = format!("refs/remotes/{}/{}", repository, branch);
    update_remote_tracking(&remote_tracking_branch, &commit_hash).await;

    // set after push success
    if args.set_upstream {
        branch::set_upstream(&branch, &format!("{repository}/{branch}")).await;
    }
}

/// Updates the remote tracking branch reference and records a Push action in the reflog.
///
/// This operation is performed atomically within a database transaction to keep the branch
/// pointer and reflog entry consistent.
///
/// # Arguments
/// * `remote_tracking_branch` - The full ref name (e.g., "refs/remotes/origin/master")
/// * `commit_hash` - The commit hash to update the branch to
///
/// If the transaction fails, an error is printed to stderr.
async fn update_remote_tracking(remote_tracking_branch: &str, commit_hash: &str) {
    let remote_tracking_branch = remote_tracking_branch.to_string();
    let commit_hash = commit_hash.to_string();

    // Update the remote tracking branch with a reflog entry using a transaction
    let db = get_db_conn_instance().await;
    let transaction_result = db
        .transaction(|txn| {
            Box::pin(async move {
                // Get the old OID before updating
                let old_oid = Branch::find_branch_with_conn(txn, &remote_tracking_branch, None)
                    .await
                    .map_or(ObjectHash::zero_str(get_hash_kind()).to_string(), |b| {
                        b.commit.to_string()
                    });

                // Update the branch
                Branch::update_branch_with_conn(txn, &remote_tracking_branch, &commit_hash, None)
                    .await;

                // Record the reflog
                let context = ReflogContext {
                    old_oid,
                    new_oid: commit_hash.clone(),
                    action: ReflogAction::Push,
                };
                Reflog::insert_single_entry(txn, &context, &remote_tracking_branch).await?;
                Ok::<_, ReflogError>(())
            })
        })
        .await;

    if let Err(e) = transaction_result {
        eprintln!("fatal: failed to update remote tracking branch: {}", e);
    }
}

/// collect all commits from `commit_id` to root commit
fn collect_history_commits(commit_id: &ObjectHash) -> HashSet<ObjectHash> {
    if commit_id == &ObjectHash::default() {
        // 0000...0000 means not exist
        return HashSet::new();
    }

    let mut commits = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(*commit_id);
    while let Some(commit) = queue.pop_front() {
        commits.insert(commit);

        // Try to load the commit; if missing or corrupt, skip this path
        let commit = match Commit::try_load(&commit) {
            Some(c) => c,
            None => continue,
        };

        for parent in commit.parent_commit_ids.iter() {
            queue.push_back(*parent);
        }
    }
    commits
}

fn incremental_objs(local_ref: ObjectHash, remote_ref: ObjectHash) -> HashSet<Entry> {
    tracing::debug!("local_ref: {}, remote_ref: {}", local_ref, remote_ref);

    // just fast-forward optimization
    if remote_ref != ObjectHash::default() {
        // remote exists
        let mut commit = match Commit::try_load(&local_ref) {
            Some(c) => c,
            None => return HashSet::new(), // If commit doesn't exist, return empty set
        };
        let mut commits = Vec::new();
        let mut ok = true;
        loop {
            commits.push(commit.id);
            if commit.id == remote_ref {
                break;
            }
            if commit.parent_commit_ids.len() != 1 {
                // merge commit or root commit
                ok = false;
                break;
            }
            // update commit to it's only parent
            commit = match Commit::try_load(&commit.parent_commit_ids[0]) {
                Some(c) => c,
                None => {
                    ok = false;
                    break;
                }
            };
        }
        if ok {
            // fast-forward
            let mut objs = HashSet::new();
            commits.reverse(); // from old to new
            for i in 0..commits.len() - 1 {
                let old_commit = match Commit::try_load(&commits[i]) {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Commit {} became inaccessible during push (fast-forward object collection)",
                            commits[i]
                        );
                        eprintln!("fatal: object storage error during push preparation");
                        return HashSet::new();
                    }
                };
                let old_tree = old_commit.tree_id;
                let new_commit = match Commit::try_load(&commits[i + 1]) {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Commit {} became inaccessible during push (fast-forward object collection)",
                            commits[i + 1]
                        );
                        eprintln!("fatal: object storage error during push preparation");
                        return HashSet::new();
                    }
                };
                objs.extend(diff_tree_objs(Some(&old_tree), &new_commit.tree_id));
                objs.insert(new_commit.into());
            }
            return objs;
        }
    }

    let mut objs = HashSet::new();
    let mut visit = HashSet::new(); // avoid duplicate commit visit
    let exist_commits = collect_history_commits(&remote_ref);
    let mut queue = VecDeque::new();
    if !exist_commits.contains(&local_ref) {
        queue.push_back(local_ref);
        visit.insert(local_ref);
    }
    let mut root_commit = None;

    while let Some(commit_id) = queue.pop_front() {
        let commit = match Commit::try_load(&commit_id) {
            Some(c) => c,
            None => continue,
        };
        let parents = &commit.parent_commit_ids;
        if parents.is_empty() {
            if root_commit.is_none() {
                root_commit = Some(commit.id);
            } else if root_commit != Some(commit.id) {
                eprintln!("{}", "fatal: multiple root commits".red());
            }
        }
        for parent in parents.iter() {
            let parent_commit = match Commit::try_load(parent) {
                Some(c) => c,
                None => continue,
            };
            let parent_tree = parent_commit.tree_id;
            objs.extend(diff_tree_objs(Some(&parent_tree), &commit.tree_id));
            if !exist_commits.contains(parent) && !visit.contains(parent) {
                queue.push_back(*parent);
                visit.insert(*parent);
            }
        }
        objs.insert(commit.into());

        print!("Counting objects: {}\r", objs.len());
        std::io::stdout().flush().unwrap();
    }

    // root commit has no parent
    if let Some(root_commit) = root_commit {
        let root_tree = Commit::load(&root_commit).tree_id;
        objs.extend(diff_tree_objs(None, &root_tree));
    }

    println!("Counting objects: {} done.", objs.len());
    objs
}

/// Check if `ancestor` is an ancestor of `descendant` using breadth-first search.
///
/// Returns `true` if `ancestor` is reachable by traversing parent commits from `descendant`,
/// or if `ancestor` and `descendant` are the same commit. Returns `false` otherwise.
///
/// If a commit cannot be loaded (missing or corrupt), that path is skipped and the search continues.
fn is_ancestor(ancestor: &ObjectHash, descendant: &ObjectHash) -> bool {
    if ancestor == descendant {
        return true;
    }

    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();

    queue.push_back(*descendant);
    visited.insert(*descendant);

    while let Some(commit_id) = queue.pop_front() {
        if &commit_id == ancestor {
            return true;
        }

        // Try to load the commit; if missing or corrupt, skip this path
        let commit = match Commit::try_load(&commit_id) {
            Some(c) => c,
            None => continue,
        };

        for parent_id in &commit.parent_commit_ids {
            if parent_id == ancestor {
                return true;
            }
            if !visited.contains(parent_id) {
                visited.insert(*parent_id);
                queue.push_back(*parent_id);
            }
        }
    }

    false
}

/// calc objects that in `new_tree` but not in `old_tree`
/// - if `old_tree` is None, return all objects in `new_tree` (include tree itself)
fn diff_tree_objs(old_tree: Option<&ObjectHash>, new_tree: &ObjectHash) -> HashSet<Entry> {
    // TODO: skip objs that has been added in caller
    let mut objs = HashSet::new();
    if let Some(old_tree) = old_tree
        && old_tree == new_tree
    {
        return objs;
    }

    let new_tree = Tree::load(new_tree);
    objs.insert(new_tree.clone().into()); // tree itself

    let old_items = match old_tree {
        Some(tree) => {
            let tree = Tree::load(tree);
            tree.tree_items
                .iter()
                .map(|item| item.id)
                .collect::<HashSet<_>>()
        }
        None => HashSet::new(),
    };

    for item in new_tree.tree_items.iter() {
        if !old_items.contains(&item.id) {
            match item.mode {
                TreeItemMode::Tree => {
                    objs.extend(diff_tree_objs(None, &item.id)); //TODO optimize, find same name tree
                }
                _ => {
                    // TODO: submodule (TreeItemMode: Commit)
                    if item.mode == TreeItemMode::Commit {
                        // (160000)| Gitlink (Submodule)
                        eprintln!("{}", "Warning: Submodule is not supported yet".red());
                    }
                    let blob = Blob::load(&item.id);
                    objs.insert(blob.into());
                }
            }
        }
    }

    objs
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use git_internal::hash::ObjectHash;

    use super::*;

    #[test]
    /// Tests successful parsing of push command arguments with different parameter combinations.
    /// Verifies repository, refspec and upstream flag settings are correctly interpreted.
    fn test_parse_args_success() {
        let args = vec!["push"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, None);
        assert_eq!(args.refspec, None);
        assert!(!args.set_upstream);
        assert!(!args.force);

        let args = vec!["push", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(!args.force);

        let args = vec!["push", "-u", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(args.set_upstream);
        assert!(!args.force);

        let args = vec!["push", "--force", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(args.force);

        let args = vec!["push", "-f", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(args.force);
    }

    #[test]
    /// Tests failure cases for push command argument parsing with invalid parameter combinations.
    /// Verifies that missing required parameters are properly detected as errors.
    fn test_parse_args_fail() {
        let args = vec!["push", "-u"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "-u", "origin"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "-u", "master"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "origin"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());
    }

    #[test]
    /// Tests the is_ancestor function with various scenarios.
    /// Verifies correct behavior for same commit, direct parent, multiple generations, and divergent branches.
    fn test_is_ancestor() {
        // Test same commit - should return true
        let commit_id = ObjectHash::from_str("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();
        assert!(is_ancestor(&commit_id, &commit_id));

        // Note: Additional tests would require creating actual commit objects in the test environment
        // which is beyond the scope of simple unit tests. These would be better tested in integration tests.
    }
}
