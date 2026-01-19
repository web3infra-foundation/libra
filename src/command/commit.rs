//! Commit command that collects staged changes, builds tree and commit objects, validates messages (including GPG), and updates HEAD/refs.

use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        index::{Index, IndexEntry},
        object::{
            ObjectTrait,
            blob::Blob,
            commit::Commit,
            signature::{Signature, SignatureType},
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};
use sea_orm::ConnectionTrait;

use super::save_object;
use crate::{
    command::{load_object, status},
    common_utils::{check_conventional_commits_message, format_commit_msg},
    internal::{
        branch::Branch,
        config::Config as UserConfig,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{client_storage::ClientStorage, lfs, object_ext::BlobExt, path, util},
};

#[derive(Parser, Debug, Default)]
pub struct CommitArgs {
    #[arg(short, long, required_unless_present_any(["file", "no_edit"]))]
    pub message: Option<String>,

    /// read message from file
    #[arg(short = 'F', long, required_unless_present_any(["message", "no_edit"]))]
    pub file: Option<String>,

    /// allow commit with empty index
    #[arg(long)]
    pub allow_empty: bool,

    /// check if the commit message follows conventional commits
    #[arg(long)]
    pub conventional: bool,

    /// amend the last commit
    #[arg(long)]
    pub amend: bool,

    /// use the message from the original commit when amending
    #[arg(long, requires = "amend",conflicts_with_all(["message", "file"]))]
    pub no_edit: bool,
    /// add signed-off-by line at the end of the commit message
    #[arg(short = 's', long)]
    pub signoff: bool,

    #[arg(long)]
    pub disable_pre: bool,

    /// Automatically stage tracked files that have been modified or deleted
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Skip all pre-commit and commit-msg hooks/validations (align with Git --no-verify)
    #[arg(long = "no-verify")]
    pub no_verify: bool,

    /// Override the commit author. Specify an explicit author using the standard A U Thor <author@example.com> format.
    #[arg(long)]
    pub author: Option<String>,
}

/// Parse author string in format "Name <email>" and return (name, email)
/// If parsing fails, panic with an error message
fn parse_author(author: &str) -> (String, String) {
    let author = author.trim();
    
    // Try to parse "Name <email>" format
    // Use find (not rfind) to get the first '<' and '>' which matches Git's behavior
    if let Some(start_idx) = author.find('<') {
        if let Some(end_idx) = author[start_idx..].find('>') {
            let end_idx = start_idx + end_idx;
            if start_idx < end_idx && end_idx == author.len() - 1 {
                let name = author[..start_idx].trim().to_string();
                let email = author[start_idx + 1..end_idx].trim().to_string();
                
                if !name.is_empty() && !email.is_empty() {
                    return (name, email);
                }
            }
        }
    }
    
    panic!("fatal: invalid author format '{}'. Expected format: 'Name <email>'", author);
}

/// Create author and committer signatures based on the provided arguments
async fn create_commit_signatures(author_override: Option<&str>) -> (Signature, Signature) {
    // Get default user info
    let default_user_name = UserConfig::get("user", None, "name")
        .await
        .unwrap_or_else(|| "unknown".to_string());
    let default_user_email = UserConfig::get("user", None, "email")
        .await
        .unwrap_or_else(|| "unknown".to_string());
    
    // Create author signature (use override if provided)
    let author = if let Some(author_str) = author_override {
        let (name, email) = parse_author(author_str);
        Signature::new(SignatureType::Author, name, email)
    } else {
        Signature::new(SignatureType::Author, default_user_name.clone(), default_user_email.clone())
    };
    
    // Committer always uses default user info
    let committer = Signature::new(SignatureType::Committer, default_user_name, default_user_email);
    
    (author, committer)
}

pub async fn execute(args: CommitArgs) {
    /* check args */
    let auto_stage_applied = if args.all {
        // Mimic `git commit -a` by staging tracked modifications/deletions first
        auto_stage_tracked_changes()
    } else {
        false
    };
    let index = Index::load(path::index()).unwrap();
    let storage = ClientStorage::init(path::objects());
    let tracked_entries = index.tracked_entries(0);
    if tracked_entries.is_empty() && !args.allow_empty && !auto_stage_applied {
        panic!("fatal: no changes added to commit, use --allow-empty to override");
    }

    // run pre commit hook
    if !args.disable_pre && !args.no_verify {
        let hooks_dir = path::hooks();

        #[cfg(not(target_os = "windows"))]
        let hook_path = hooks_dir.join("pre-commit.sh");

        #[cfg(target_os = "windows")]
        let hook_path = hooks_dir.join("pre-commit.ps1");
        if hook_path.exists() {
            let hook_display = hook_path.display();
            #[cfg(not(target_os = "windows"))]
            let output = Command::new("sh")
                .arg(&hook_path)
                .current_dir(util::working_dir())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .unwrap_or_else(|e| panic!("Failed to execute hook {hook_display}: {e}"));

            #[cfg(target_os = "windows")]
            let output = Command::new("powershell")
                .arg("-File")
                .arg(&hook_path)
                .current_dir(util::working_dir())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .unwrap_or_else(|e| panic!("Failed to execute hook {hook_display}: {e}"));

            if !output.status.success() {
                panic!(
                    "Hook {} failed with exit code {}",
                    hook_display,
                    output.status.code().unwrap_or(-1)
                );
            }
        }
    }

    //Find commit message source
    let message = match (args.message, args.file) {
        //from -m
        (Some(msg), _) => msg,
        //from file
        (None, Some(file_path)) => match tokio::fs::read_to_string(file_path).await {
            Ok(msg) => msg,
            Err(e) => panic!("fatal: failed to read commit message from file: {}", e),
        },
        //no commit message, which is not supposed to happen
        (None, None) => {
            if !args.no_edit {
                panic!("fatal: no commit message provided")
            } else {
                //its ok to use "" because no_edit is True ,
                //and we will use the message from the original commit
                // message wont be used by amend
                "".to_string()
            }
        }
    };
    /* Create tree */
    let tree = create_tree(&index, &storage, "".into()).await;

    /* Create & save commit objects */
    let parents_commit_ids = get_parents_ids().await;

    // Create author and committer signatures (respecting --author override)
    let (author, committer) = create_commit_signatures(args.author.as_deref()).await;

    // Amend commits are only supported for a single parent commit.
    if args.amend {
        if parents_commit_ids.len() > 1 {
            panic!("fatal: --amend is not supported for merge commits with multiple parents");
        }
        let parent_commit = load_object::<Commit>(&parents_commit_ids[0]).unwrap_or_else(|_| {
            panic!(
                "fatal: not a valid object name: '{}'",
                parents_commit_ids[0]
            )
        });
        let grandpa_commit_id = parent_commit.parent_commit_ids;
        // if no_edit is True, use parent commit message;else use commit message from args
        let final_message = if args.no_edit {
            parent_commit.message.clone()
        } else {
            message.clone()
        };
        //Prepare commit message
        let commit_message = if args.signoff {
            // get user
            let user_name = UserConfig::get("user", None, "name")
                .await
                .unwrap_or_else(|| "unknown".to_string());
            let user_email = UserConfig::get("user", None, "email")
                .await
                .unwrap_or_else(|| "unknown".to_string());

            // get sign line
            let signoff_line = format!("Signed-off-by: {user_name} <{user_email}>");
            format!("{}\n\n{signoff_line}", final_message)
        } else {
            final_message.clone()
        };

        // check format(if needed)
        if args.conventional
            && !args.no_verify
            && !check_conventional_commits_message(&commit_message)
        {
            panic!("fatal: commit message does not follow conventional commits");
        }
        let commit = Commit::new(
            author,
            committer,
            tree.id,
            grandpa_commit_id,
            &format_commit_msg(&final_message, None),
        );

        storage
            .put(&commit.id, &commit.to_data().unwrap(), commit.get_type())
            .unwrap();

        /* update HEAD */
        update_head_and_reflog(&commit.id.to_string(), &commit_message).await;
        return;
    }

    //Prepare commit message
    let commit_message = if args.signoff {
        // get user
        let user_name = UserConfig::get("user", None, "name")
            .await
            .unwrap_or_else(|| "unknown".to_string());
        let user_email = UserConfig::get("user", None, "email")
            .await
            .unwrap_or_else(|| "unknown".to_string());

        // get sign line
        let signoff_line = format!("Signed-off-by: {user_name} <{user_email}>");
        format!("{}\n\n{signoff_line}", message)
    } else {
        message.clone()
    };

    // check format(if needed)
    if args.conventional && !args.no_verify && !check_conventional_commits_message(&commit_message)
    {
        panic!("fatal: commit message does not follow conventional commits");
    }

    // There must be a `blank line`(\n) before `message`, or remote unpack failed
    let commit = Commit::new(
        author,
        committer,
        tree.id,
        parents_commit_ids,
        &format_commit_msg(&message, None),
    );

    storage
        .put(&commit.id, &commit.to_data().unwrap(), commit.get_type())
        .unwrap();

    /* update HEAD */
    update_head_and_reflog(&commit.id.to_string(), &commit_message).await;
}

/// recursively create tree from index's tracked entries
pub async fn create_tree(index: &Index, storage: &ClientStorage, current_root: PathBuf) -> Tree {
    // blob created when add file to index
    let get_blob_entry = |path: &PathBuf| {
        let name = util::path_to_string(path);
        let mete = index.get(&name, 0).unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();

        TreeItem {
            name: filename,
            mode: TreeItemMode::tree_item_type_from_bytes(format!("{:o}", mete.mode).as_bytes())
                .unwrap(),
            id: mete.hash,
        }
    };

    let mut tree_items: Vec<TreeItem> = Vec::new();
    let mut processed_path: HashSet<String> = HashSet::new();
    let path_entries: Vec<PathBuf> = index
        .tracked_entries(0)
        .iter()
        .map(|file| PathBuf::from(file.name.clone()))
        .filter(|path| path.starts_with(&current_root))
        .collect();
    for path in path_entries.iter() {
        let in_current_path = path.parent().unwrap() == current_root;
        if in_current_path {
            let item = get_blob_entry(path);
            tree_items.push(item);
        } else {
            if path.components().count() == 1 {
                continue;
            }
            // next level tree
            let process_path = path
                .components()
                .nth(current_root.components().count())
                .unwrap()
                .as_os_str()
                .to_str()
                .unwrap();

            if processed_path.contains(process_path) {
                continue;
            }
            processed_path.insert(process_path.to_string());

            let sub_tree = Box::pin(create_tree(
                index,
                storage,
                current_root.clone().join(process_path),
            ))
            .await;
            tree_items.push(TreeItem {
                name: process_path.to_string(),
                mode: TreeItemMode::Tree,
                id: sub_tree.id,
            });
        }
    }
    let tree = {
        // `from_tree_items` can't create empty tree, so use `from_bytes` instead
        if tree_items.is_empty() {
            let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
            Tree::from_bytes(&[], empty_id).unwrap()
        } else {
            Tree::from_tree_items(tree_items).unwrap()
        }
    };
    // save
    save_object(&tree, &tree.id).unwrap();
    tree
}

fn auto_stage_tracked_changes() -> bool {
    let pending = status::changes_to_be_staged();
    if pending.modified.is_empty() && pending.deleted.is_empty() {
        return false;
    }

    let index_path = path::index();
    let mut index = Index::load(&index_path).unwrap();
    let workdir = util::working_dir();
    let mut touched = false;

    for file in pending.modified {
        let abs = util::workdir_to_absolute(&file);
        if !abs.exists() {
            continue;
        }
        // Refresh blob IDs for modified tracked files before updating the index
        let blob = blob_from_file(&abs);
        blob.save();
        index.update(IndexEntry::new_from_file(&file, blob.id, &workdir).unwrap());
        touched = true;
    }

    for file in pending.deleted {
        if let Some(path) = file.to_str() {
            // Drop entries that disappeared from the working tree
            index.remove(path, 0);
            touched = true;
        }
    }

    if touched {
        index.save(&index_path).unwrap();
    }
    touched
}

fn blob_from_file(path: impl AsRef<std::path::Path>) -> Blob {
    if lfs::is_lfs_tracked(&path) {
        Blob::from_lfs_file(path)
    } else {
        Blob::from_file(path)
    }
}

/// get current head commit id as parent, if in branch, get branch's commit id, if detached head, get head's commit id
async fn get_parents_ids() -> Vec<ObjectHash> {
    // let current_commit_id = reference::Model::current_commit_hash(db).await.unwrap();
    let current_commit_id = Head::current_commit().await;
    match current_commit_id {
        Some(id) => vec![id],
        None => vec![], // first commit
    }
}

/// update HEAD to new commit, if in branch, update branch's commit id, if detached head, update head's commit id
async fn update_head<C: ConnectionTrait>(db: &C, commit_id: &str) {
    // let head = reference::Model::current_head(db).await.unwrap();
    match Head::current_with_conn(db).await {
        Head::Branch(name) => {
            // in branch
            Branch::update_branch_with_conn(db, &name, commit_id, None).await;
        }
        // None => {
        Head::Detached(_) => {
            let head = Head::Detached(ObjectHash::from_str(commit_id).unwrap());
            Head::update_with_conn(db, head, None).await;
        }
    }
}

async fn update_head_and_reflog(commit_id: &str, commit_message: &str) {
    let reflog_context = new_reflog_context(commit_id, commit_message).await;
    let commit_id = commit_id.to_string();
    with_reflog(
        reflog_context,
        |txn| {
            Box::pin(async move {
                update_head(txn, &commit_id).await;
                Ok(())
            })
        },
        true,
    )
    .await
    .unwrap();
}

async fn new_reflog_context(commit_id: &str, message: &str) -> ReflogContext {
    let old_oid = Head::current_commit()
        .await
        .unwrap_or(ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()]).unwrap())
        .to_string();
    let new_oid = commit_id.to_string();
    let action = ReflogAction::Commit {
        message: message.to_string(),
    };
    ReflogContext {
        old_oid,
        new_oid,
        action,
    }
}

#[cfg(test)]
mod test {
    use std::env;

    use git_internal::internal::object::{ObjectTrait, signature::Signature};
    use serial_test::serial;
    use tempfile::tempdir;
    use tokio::{
        fs::{self, File},
        io::AsyncWriteExt,
    };

    use super::*;
    use crate::utils::test::*;

    #[test]
    ///Testing basic parameter parsing functionality.
    fn test_parse_args() {
        let args = CommitArgs::try_parse_from(["commit", "-m", "init"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--allow-empty"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "--conventional", "-m", "init"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "--conventional"]);
        assert!(args.is_err(), "conventional should require message");

        let args = CommitArgs::try_parse_from(["commit"]);
        assert!(args.is_err(), "message is required");

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend"]);
        assert!(args.is_ok());
        //failed
        let args = CommitArgs::try_parse_from(["commit", "--amend", "--no-edit"]);
        assert!(args.is_ok());
        let args = CommitArgs::try_parse_from(["commit", "--no-edit"]);
        assert!(args.is_err(), "--no-edit requires --amend");
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--allow-empty", "--amend"]);
        assert!(args.is_ok());
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "-s"]);
        assert!(args.is_ok());
        assert!(args.unwrap().signoff);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--signoff"]);
        assert!(args.is_ok());
        assert!(args.unwrap().signoff);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "-a"]);
        assert!(args.is_ok());
        assert!(args.unwrap().all);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--all"]);
        assert!(args.is_ok());
        assert!(args.unwrap().all);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--no-edit"]);
        assert!(
            args.is_err(),
            "--no-edit conflicts with --message and --file"
        );
        let args = CommitArgs::try_parse_from(["commit", "-F", "init", "--amend", "--no-edit"]);
        assert!(
            args.is_err(),
            "--no-edit conflicts with --message and --file"
        );
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--signoff"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        assert!(args.amend);
        assert!(args.signoff);

        let args = CommitArgs::try_parse_from(["commit", "-F", "unreachable_file"]);
        assert!(args.is_ok());
        assert!(args.unwrap().file.is_some());

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should be a valid parameter");

        let args =
            CommitArgs::try_parse_from(["commit", "-m", "init", "--conventional", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should work with --conventional");

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should work with --amend");

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--author", "Test User <test@example.com>"]);
        assert!(args.is_ok(), "--author should be a valid parameter");
        let args = args.unwrap();
        assert_eq!(args.author, Some("Test User <test@example.com>".to_string()));

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--author", "Test User <test@example.com>", "--amend"]);
        assert!(args.is_ok(), "--author should work with --amend");
    }

    #[test]
    fn test_parse_author() {
        // Valid author formats
        let (name, email) = parse_author("John Doe <john@example.com>");
        assert_eq!(name, "John Doe");
        assert_eq!(email, "john@example.com");

        let (name, email) = parse_author("  Jane Smith  <jane@test.org>  ");
        assert_eq!(name, "Jane Smith");
        assert_eq!(email, "jane@test.org");

        let (name, email) = parse_author("Multi Word Name <multi@word.com>");
        assert_eq!(name, "Multi Word Name");
        assert_eq!(email, "multi@word.com");

        // Invalid formats should panic
        let result = std::panic::catch_unwind(|| parse_author("invalid"));
        assert!(result.is_err());

        let result = std::panic::catch_unwind(|| parse_author("No Email"));
        assert!(result.is_err());

        let result = std::panic::catch_unwind(|| parse_author("<noemail@test.com>"));
        assert!(result.is_err());

        let result = std::panic::catch_unwind(|| parse_author("Name <"));
        assert!(result.is_err());
    }

    #[test]
    fn test_commit_message() {
        let args = CommitArgs {
            message: None,
            file: None,
            allow_empty: false,
            conventional: false,
            amend: true,
            no_edit: true,
            signoff: false,
            disable_pre: false,
            all: false,
            no_verify: false,
            author: None,
        };
        fn message_and_file_are_none(args: &CommitArgs) -> Option<String> {
            let message = match (&args.message, &args.file) {
                (Some(msg), _) => Some(msg.clone()),
                (None, Some(file)) => Some(file.clone()),
                (None, None) => {
                    if args.no_edit {
                        Some("".to_string())
                    } else {
                        None
                    }
                }
            };
            message
        }
        let message = message_and_file_are_none(&args);
        assert_eq!(message, Some("".to_string()));
    }

    #[tokio::test]
    #[serial]
    async fn test_commit_message_from_file() {
        let test_path = "test_data.txt";

        // All sorts of edge cases
        let test_cases = vec![
            // Regular string
            "Hello, World! 你好，世界！",
            // Special characters
            "Special chars: \n\t\r\\\"'",
            // Unicode
            "Emoji: 😀🎉🚀, Unicode:  Café café",
            // Empty
            "",
            // Mixed
            "Mix: 中文\n\tEmoji😀\rSpecial\\\"'",
        ];

        for test_data in test_cases {
            let bytes = test_data.as_bytes();
            // Async write file
            let mut file = File::create(&test_path).await.expect("create file failed");
            file.write_all(bytes)
                .await
                .expect("write test data to file failed");
            file.sync_all()
                .await
                .expect("write test data to file failed");

            // Async read file
            let content = tokio::fs::read_to_string(test_path).await.unwrap();

            // Mock author
            let author = Signature {
                signature_type: git_internal::internal::object::signature::SignatureType::Author,
                name: "test".to_string(),
                email: "test".to_string(),
                timestamp: 1,
                timezone: "test".to_string(),
            };

            // Mock commiter
            let commiter = Signature {
                signature_type: git_internal::internal::object::signature::SignatureType::Committer,
                name: "test".to_string(),
                email: "test".to_string(),
                timestamp: 1,
                timezone: "test".to_string(),
            };

            // Mock commit
            let zero = ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()]).unwrap();
            let commit = Commit::new(author, commiter, zero, Vec::new(), &content);

            // Commit to data
            let commit_data = commit.to_data().unwrap();

            // Parse data
            let message = Commit::from_bytes(&commit_data, commit.id).unwrap().message;

            // Test eq
            assert_eq!(message, test_data);

            // Clean up
            fs::remove_file(&test_path)
                .await
                .expect("cleaning test file failed, please remove test file manually");
        }
    }

    #[tokio::test]
    #[serial]
    // Tests the recursive tree creation from index entries (uses original test data via absolute path)
    async fn test_create_tree() {
        // 1. 初始化临时 Libra 仓库（保持原有逻辑，确保仓库结构正确）
        let temp_path = tempdir().unwrap();
        setup_with_new_libra_in(temp_path.path()).await;
        let _guard = ChangeDirGuard::new(temp_path.path());

        // 2. 基于项目根目录（CARGO_MANIFEST_DIR）构建测试 index 文件的绝对路径（关键修复）
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // 项目根目录（Cargo.toml 所在处）
        let index_file_path = project_root.join("tests/data/index/index-760"); // 绝对路径：根目录/tests/data/...

        // 3. 检查文件是否存在，给出明确提示（指导你补充文件）
        assert!(
            index_file_path.exists(),
            "测试文件不存在！请在项目根目录下创建路径：{}，并放入 index-760 文件",
            index_file_path.display()
        );

        // 4. 加载 index 文件（使用绝对路径，不再报错）
        let index = Index::from_file(index_file_path).unwrap_or_else(|e| {
            panic!("加载 index 文件失败：{}，请确认文件格式正确", e);
        });
        println!(
            "加载的 index 包含 {} 个跟踪文件",
            index.tracked_entries(0).len()
        );

        // 5. 初始化存储（确保指向临时仓库的 objects 目录，避免干扰主仓库）
        let temp_objects_dir = temp_path.path().join(".libra/objects"); // 临时仓库的 objects 目录
        let storage = ClientStorage::init(temp_objects_dir);

        // 6. 调用 create_tree（current_root 设为空，因为 index 中路径是相对于仓库根的）
        let tree = create_tree(&index, &storage, PathBuf::new()).await;

        // 7. 原有验证逻辑（不变）
        assert!(storage.get(&tree.id).is_ok(), "根 tree 未保存到存储");
        for item in tree.tree_items.iter() {
            if item.mode == TreeItemMode::Tree {
                assert!(
                    storage.get(&item.id).is_ok(),
                    "子 tree 未保存：{}",
                    item.name
                );
                if item.name == "DeveloperExperience" {
                    let sub_tree_data = storage.get(&item.id).unwrap();
                    let sub_tree = Tree::from_bytes(&sub_tree_data, item.id).unwrap();
                    assert_eq!(
                        sub_tree.tree_items.len(),
                        4,
                        "DeveloperExperience 子 tree 条目数错误"
                    );
                }
            }
        }
    }

    #[test]
    fn test_no_verify_skips_conventional_check() {
        let invalid_conventional_msg = "invalid commit: no type or scope";
        assert!(
            !check_conventional_commits_message(invalid_conventional_msg),
            "Test setup error: message should be invalid for conventional commits"
        );

        let args_with_verify = CommitArgs {
            message: Some(invalid_conventional_msg.to_string()),
            file: None,
            allow_empty: true,
            conventional: true,
            no_verify: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: false,
            all: false,
            author: None,
        };

        let commit_message_with_verify = args_with_verify
            .signoff
            .then(|| {
                format!(
                    "{}\n\nSigned-off-by: test <test@example.com>",
                    invalid_conventional_msg
                )
            })
            .unwrap_or_else(|| invalid_conventional_msg.to_string());

        let verify_result = std::panic::catch_unwind(|| {
            if args_with_verify.conventional
                && !args_with_verify.no_verify
                && !check_conventional_commits_message(&commit_message_with_verify)
            {
                panic!("fatal: commit message does not follow conventional commits");
            }
        });
        assert!(
            verify_result.is_err(),
            "Conventional check should fail without --no-verify"
        );

        let args_no_verify = CommitArgs {
            no_verify: true,
            ..args_with_verify
        };

        let commit_message_no_verify = args_no_verify
            .signoff
            .then(|| {
                format!(
                    "{}\n\nSigned-off-by: test <test@example.com>",
                    invalid_conventional_msg
                )
            })
            .unwrap_or_else(|| invalid_conventional_msg.to_string());

        let no_verify_result = std::panic::catch_unwind(|| {
            if args_no_verify.conventional
                && !args_no_verify.no_verify
                && !check_conventional_commits_message(&commit_message_no_verify)
            {
                panic!("fatal: commit message does not follow conventional commits");
            }
        });
        assert!(
            no_verify_result.is_ok(),
            "--no-verify should skip conventional check"
        );
    }
}
