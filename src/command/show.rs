//! Show command that resolves object IDs and prints commit, tree, blob, or ref details with formatting suitable for diffable objects.

use std::{path::PathBuf, str::FromStr};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};

use crate::{
    command::{
        load_object,
        log::{ChangeType, generate_diff, get_changed_files_for_commit},
    },
    common_utils::parse_commit_msg,
    internal::tag,
    utils::{client_storage::ClientStorage, object_ext::TreeExt, path, util},
};

/// 显示各种类型的对象
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// 对象名称（提交、标签等）或 <对象>:<路径>。默认为 HEAD
    #[clap(value_name = "OBJECT")]
    pub object: Option<String>,

    /// 不显示 diff 输出（仅显示提交信息）
    #[clap(long, short = 's')]
    pub no_patch: bool,

    /// --pretty=oneline 的简写
    #[clap(long)]
    pub oneline: bool,

    /// 仅显示改变文件的文件名
    #[clap(long)]
    pub name_only: bool,

    /// 显示差异统计信息（文件更改摘要）
    #[clap(long)]
    pub stat: bool,

    /// 限制输出的路径
    #[clap(value_name = "PATHS", num_args = 0..)]
    pub pathspec: Vec<String>,
}

/// 执行 show 命令
pub async fn execute(args: ShowArgs) {
    let object_ref = args.object.as_deref().unwrap_or("HEAD");

    // 检查是否是 <提交>:<路径> 语法
    if let Some((rev, path)) = object_ref.split_once(':') {
        show_commit_file(rev, path, &args).await;
        return;
    }

    // 首先尝试作为引用解析（分支/标签/HEAD）
    if let Ok(commit_hash) = util::get_commit_base(object_ref).await {
        show_commit(&commit_hash, &args).await;
        return;
    }

    // 尝试解析为直接的哈希值
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        show_object_by_hash(&hash, &args).await;
        return;
    }

    eprintln!("fatal: bad revision '{}'", object_ref);
    std::process::exit(1);
}

/// 通过哈希值显示对象（自动检测类型）
fn show_object_by_hash<'a>(
    hash: &'a ObjectHash,
    args: &'a ShowArgs,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
        let storage = ClientStorage::init(path::objects());

        let obj_type = match storage.get_object_type(hash) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("fatal: could not read object {}: {}", hash, e);
                std::process::exit(1);
            }
        };

        match obj_type {
            ObjectType::Commit => show_commit(hash, args).await,
            ObjectType::Tag => show_tag_by_hash(hash, args).await,
            ObjectType::Tree => show_tree(hash).await,
            ObjectType::Blob => show_blob(hash).await,
            _ => {
                eprintln!("fatal: unsupported object type for {}", hash);
                std::process::exit(1);
            }
        }
    })
}

/// 显示提交及其详细信息和差异
async fn show_commit(commit_hash: &ObjectHash, args: &ShowArgs) {
    // 加载提交对象
    let commit = match load_object::<Commit>(commit_hash) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: could not load commit {}: {}", commit_hash, e);
            std::process::exit(1);
        }
    };

    // 显示提交信息
    display_commit_info(&commit, args);

    // 如果未禁用，显示差异或文件列表
    if !args.no_patch {
        let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();

        if args.stat {
            // 显示差异统计
            show_diffstat(&commit, paths.clone()).await;
        } else if args.name_only {
            // 仅显示改变的文件名
            let changed_files = get_changed_files_for_commit(&commit, &paths).await;
            if !changed_files.is_empty() {
                println!();
                for file in changed_files {
                    println!("{}", file.path.display());
                }
            }
        } else {
            // 显示完整差异
            let diff_output = generate_diff(&commit, paths).await;
            if !diff_output.is_empty() {
                println!();
                print!("{}", diff_output);
            }
        }
    }
}

/// 显示标签对象
async fn show_tag_by_hash(hash: &ObjectHash, args: &ShowArgs) {
    match tag::load_object_trait(hash).await {
        Ok(tag::TagObject::Tag(tag_obj)) => {
            // 显示标签信息
            println!("{} {}", "tag".yellow(), tag_obj.tag_name);
            println!(
                "Tagger: {} <{}>",
                tag_obj.tagger.name.trim(),
                tag_obj.tagger.email.trim()
            );

            let date = chrono::DateTime::from_timestamp(tag_obj.tagger.timestamp as i64, 0)
                .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            println!("Date:   {}", date.to_rfc2822());
            println!();
            println!("{}", tag_obj.message.trim());
            println!();

            // 显示标签指向的对象
            show_object_by_hash(&tag_obj.object_hash, args).await;
        }
        Ok(tag::TagObject::Commit(commit)) => {
            // 指向提交的轻量级标签
            show_commit(&commit.id, args).await;
        }
        Ok(_) => {
            eprintln!("fatal: tag points to unsupported object type");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("fatal: {}", e);
            std::process::exit(1);
        }
    }
}

/// 显示树对象
async fn show_tree(hash: &ObjectHash) {
    let tree = match load_object::<Tree>(hash) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fatal: could not load tree {}: {}", hash, e);
            std::process::exit(1);
        }
    };

    println!("{} {}\n", "tree".yellow(), hash);

    for item in &tree.tree_items {
        println!(
            "{:06o} {:?} {}\t{}",
            item.mode as u32, item.mode, item.id, item.name
        );
    }
}

/// 显示二进制对象
async fn show_blob(hash: &ObjectHash) {
    let blob = match load_object::<Blob>(hash) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("fatal: could not load blob {}: {}", hash, e);
            std::process::exit(1);
        }
    };

    // 尝试作为文本显示，否则显示二进制信息
    match String::from_utf8(blob.data.clone()) {
        Ok(text) => print!("{}", text),
        Err(_) => {
            println!("Binary file (size: {} bytes)", blob.data.len());
        }
    }
}

/// 显示提交中的特定文件
async fn show_commit_file(rev: &str, file_path: &str, _args: &ShowArgs) {
    // 将修订版本解析为提交
    let commit_hash = match util::get_commit_base(rev).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("fatal: {}", e);
            std::process::exit(1);
        }
    };

    let commit = match load_object::<Commit>(&commit_hash) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: could not load commit: {}", e);
            std::process::exit(1);
        }
    };

    // 获取树
    let tree = match load_object::<Tree>(&commit.tree_id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fatal: could not load tree: {}", e);
            std::process::exit(1);
        }
    };

    // 在树中查找文件
    let items = tree.get_plain_items();
    let target_path = PathBuf::from(file_path);

    if let Some((_, blob_hash)) = items.iter().find(|(path, _)| path == &target_path) {
        show_blob(blob_hash).await;
    } else {
        eprintln!("fatal: path '{}' does not exist in '{}'", file_path, rev);
        std::process::exit(1);
    }
}

/// 根据格式选项显示提交信息
fn display_commit_info(commit: &Commit, args: &ShowArgs) {
    if args.oneline {
        // 单行格式：短哈希 + 消息
        let short_hash = &commit.id.to_string()[..7];
        let (msg, _) = parse_commit_msg(&commit.message);
        let first_line = msg.lines().next().unwrap_or("");
        println!("{} {}", short_hash.yellow(), first_line);
    } else {
        // 完整格式
        println!("{} {}", "commit".yellow(), commit.id.to_string().yellow());
        println!(
            "Author: {} <{}>",
            commit.author.name.trim(),
            commit.author.email.trim()
        );

        // 格式化时间戳
        let date = chrono::DateTime::from_timestamp(commit.committer.timestamp as i64, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        println!("Date:   {}", date.to_rfc2822());

        // 显示消息
        let (msg, _) = parse_commit_msg(&commit.message);
        for line in msg.lines() {
            println!("    {}", line);
        }
    }
}

/// 显示差异统计（更改摘要）
async fn show_diffstat(commit: &Commit, paths: Vec<PathBuf>) {
    let changed_files = get_changed_files_for_commit(commit, &paths).await;

    if changed_files.is_empty() {
        return;
    }

    println!();

    // 统计更改
    let mut additions = 0;
    let mut deletions = 0;

    for change in &changed_files {
        match change.status {
            ChangeType::Added => additions += 1,
            ChangeType::Deleted => deletions += 1,
            ChangeType::Modified => {
                additions += 1;
                deletions += 1;
            }
        }
        let status = match change.status {
            ChangeType::Added => "A",
            ChangeType::Modified => "M",
            ChangeType::Deleted => "D",
        };
        println!("{}  {}", status, change.path.display());
    }

    println!(
        "\n{} file{} changed, {} insertion{}(+), {} deletion{}(-)",
        changed_files.len(),
        if changed_files.len() != 1 { "s" } else { "" },
        additions,
        if additions != 1 { "s" } else { "" },
        deletions,
        if deletions != 1 { "s" } else { "" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        // 测试默认值（HEAD）
        let args = ShowArgs::try_parse_from(["show"]).unwrap();
        assert_eq!(args.object, None);
        assert!(!args.no_patch);
        assert!(!args.oneline);

        // 测试提交哈希
        let args = ShowArgs::try_parse_from(["show", "abc123"]).unwrap();
        assert_eq!(args.object, Some("abc123".to_string()));

        // 测试 --no-patch
        let args = ShowArgs::try_parse_from(["show", "--no-patch"]).unwrap();
        assert!(args.no_patch);

        // 测试 --oneline
        let args = ShowArgs::try_parse_from(["show", "--oneline"]).unwrap();
        assert!(args.oneline);

        // 测试 --name-only
        let args = ShowArgs::try_parse_from(["show", "--name-only"]).unwrap();
        assert!(args.name_only);

        // 测试 --stat
        let args = ShowArgs::try_parse_from(["show", "--stat"]).unwrap();
        assert!(args.stat);

        // 测试 <提交>:<路径> 语法
        let args = ShowArgs::try_parse_from(["show", "HEAD:test.txt"]).unwrap();
        assert_eq!(args.object, Some("HEAD:test.txt".to_string()));
    }
}
