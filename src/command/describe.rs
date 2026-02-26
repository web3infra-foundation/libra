//! Implementation of `describe` command, which finds the most recent tag reachable from a commit.
use std::{collections::{HashMap, HashSet, VecDeque}, str::FromStr};

use clap::Parser;
use git_internal::{hash::ObjectHash, internal::object::{commit::Commit, types::ObjectType}};

use crate::{
    command::load_object,
    internal::{
        head::Head,
        tag::{self, TagObject},
    },
    utils::util,
};

#[derive(Parser, Debug)]
pub struct DescribeArgs {

    // The commit object name, Defaults to HEAD.
    pub commit: Option<String>,

    // Instead of only using annotated tags, use any tag found in refs/tags namespace.
    #[clap(long)]
    pub tags: bool,

    // Instead of using the default 7 hexadecimal digits as the abbreviated object name, use <n> digits.
    #[clap(long)]
    pub abbrev: Option<usize>,
}

// Entry in tag lookup map
struct TagInfo {
    name: String,
    #[allow(dead_code)]
    is_annotated: bool,
}

pub async fn execute(args: DescribeArgs) -> Result<(), String> {
    // 检查是否在 libra 仓库中
    if !util::check_repo_exist() {
        return Err("fatal: not a libra repository".to_string());
    }

    // 1. 确定起始提交
    let start_hash_str = if let Some(c) = args.commit {
        c
    } else {
        Head::current_commit()
            .await
            .ok_or("fatal: no commit at HEAD")?
            .to_string()
    };
    let start_hash = ObjectHash::from_str(&start_hash_str)
        .map_err(|_| format!("fatal: Not a valid object name {}", start_hash_str))?;

    // 2. 加载所有标签并构建映射表：提交哈希 -> 标签信息
    let all_tags = tag::list().await.map_err(|e| format!("fatal: {}", e))?;
    let mut tag_map: HashMap<ObjectHash, TagInfo> = HashMap::new();

    for t in all_tags {
        let is_annotated = t.object.get_type() == ObjectType::Tag;
        
        // 只有当是附注标签，或者指定了 --tags 参数时，才包含该标签
        if is_annotated || args.tags {
            let target_commit_hash = match t.object {
                TagObject::Commit(c) => c.id,
                TagObject::Tag(tg) => tg.object_hash,
                _ => continue,
            };
            
            // 如果多个标签指向同一个提交，附注标签通常具有更高的优先级
            // 这里使用 entry.or_insert 逻辑，优先保留先发现的标签
            tag_map.entry(target_commit_hash).or_insert(TagInfo {
                name: t.name,
                is_annotated,
            });
        }
    }

    // 3. 使用广度优先搜索 (BFS) 寻找最近的标签（以确保找到最短距离）
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();

    // 队列存储格式：(当前提交哈希, 距离起点的提交数)
    queue.push_back((start_hash, 0));
    visited.insert(start_hash);

    while let Some((curr_hash, dist)) = queue.pop_front() {
        // 检查当前提交是否有对应的标签
        if let Some(tag_info) = tag_map.get(&curr_hash) {
            let output = format_describe_result(
                &tag_info.name,
                dist,
                &start_hash_str,
                args.abbrev.unwrap_or(7),
            );
            println!("{}", output);
            return Ok(());
        }

        // 加载提交对象以获取其父提交
        let commit = load_object::<Commit>(&curr_hash)
            .map_err(|_| format!("fatal: failed to load commit {}", curr_hash))?;

        for parent_id_str in commit.parent_commit_ids {
            let parent_hash = ObjectHash::from_str(&parent_id_str.to_string()).unwrap();
            if !visited.contains(&parent_hash) {
                visited.insert(parent_hash);
                queue.push_back((parent_hash, dist + 1));
            }
        }
    }

    // 如果遍历完整个历史记录都没有找到标签
    Err("fatal: No names found, cannot describe anything.".to_string())
}

// 根据 Git 的 describe 规则格式化输出字符串
fn format_describe_result(tag_name: &str, dist: usize, full_sha: &str, abbrev: usize) -> String {
    if dist == 0 {
        // 如果当前提交就是标签指向的提交，直接返回标签名
        tag_name.to_string()
    } else {
        // 截取哈希缩写
        let short_sha = if abbrev >= full_sha.len() {
            full_sha
        } else {
            &full_sha[..abbrev]
        };
        // 格式：{标签名}-{距离}-{哈希缩写}
        format!("{}-{}-g{}", tag_name, dist, short_sha)
    }
}