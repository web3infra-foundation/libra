use crate::utils::util;
use crate::{
    command::{get_target_commit, load_object},
    utils::object_ext::TreeExt,
};

use chrono::DateTime;
use clap::Parser;
use git_internal::diff::compute_diff;
use git_internal::hash::SHA1;
use git_internal::internal::object::{blob::Blob, commit::Commit, tree::Tree};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
pub struct BlameArgs {
    /// The file to blame
    #[clap(value_name = "FILE")]
    pub file: String,

    /// The commit to use for blame
    #[clap(value_name = "COMMIT", default_value = "HEAD")]
    pub commit: String,

    /// The line range to blame
    #[clap(short = 'L', value_name = "RANGE")]
    pub line_range: Option<String>,
}

struct LineBlame {
    line_number: usize,
    commit_id: SHA1,
    author: String,
    date: String,
    content: String,
}

pub async fn execute(args: BlameArgs) {
    // check if we're in a valid repository
    if !util::check_repo_exist() {
        return;
    }

    let commit_id = match get_target_commit(&args.commit).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Error: {}", e);
            return;
        }
    };

    let commit_obj = match load_object::<Commit>(&commit_id) {
        Ok(obj) => obj,
        Err(e) => {
            eprintln!("Failed to load commit: {}", e);
            return;
        }
    };

    // get the final file content (the version we're blaming)
    let target_lines = match get_file_lines(&commit_obj, &args.file) {
        Ok(lines) => lines,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };

    if target_lines.is_empty() {
        println!("File is empty");
        return;
    }

    // Initialize blame: assume all lines come from the target commit initially
    let mut blame_lines: Vec<LineBlame> = target_lines
        .iter()
        .enumerate()
        .map(|(idx, content)| LineBlame {
            line_number: idx + 1,
            commit_id: commit_id.clone(),
            author: commit_obj.author.name.clone(),
            date: commit_obj.author.timestamp.to_string(),
            content: content.clone(),
        })
        .collect();

    // walk backwards through commit history
    let mut current_commit = commit_obj;
    let mut current_lines = target_lines;

    loop {
        // checking if this commit has a parent
        let parent_id = match current_commit.parent_commit_ids.first() {
            Some(id) => id,
            None => {
                break; // no more parents, we're done
            }
        };

        // the parent commit
        let parent_commit = match load_object::<Commit>(parent_id) {
            Ok(obj) => obj,
            Err(_) => {
                break;
            }
        };

        // getting the file in the parent commit
        let parent_lines = match get_file_lines(&parent_commit, &args.file) {
            Ok(lines) => {
                if lines.is_empty() {
                    break;
                }
                lines
            }
            Err(_) => {
                break; // file was created in current commit
            }
        };

        // compute diff operations between parent and current
        let operations = compute_diff(&parent_lines, &current_lines);

        for op in operations {
            use git_internal::diff::DiffOperation;
            match op {
                // line was inserted in current commit - blame the current commit
                DiffOperation::Insert { line: _, .. } => {
                    // This line was added in current_commit, so it's already blamed correctly
                }

                // line unchanged - it came from parent, update blame
                DiffOperation::Equal { old_line, new_line } => {
                    // get the content from the final file (blame_lines)
                    let final_content = blame_lines.get(new_line - 1).map(|b| &b.content);
                    let parent_content = parent_lines.get(old_line - 1);

                    // only update if the final file content matches parent content
                    if final_content == parent_content && final_content.is_some() {
                        // content matches, line came from parent
                        if let Some(blame) = blame_lines.get_mut(new_line - 1) {
                            blame.commit_id = parent_id.clone();
                            blame.author = parent_commit.author.name.clone();
                            blame.date = parent_commit.author.timestamp.to_string();
                        }
                    }
                }
                // line was deleted - doesn't exist in final file, ignore
                DiffOperation::Delete { .. } => {
                    // deleted lines don't appear in the final file
                }
            }
        }
        // move to parent for next iteration
        current_commit = parent_commit;
        current_lines = parent_lines;
    }

    // line range if specified
    let filtered_lines = if let Some(ref range) = args.line_range {
        match parse_line_range(range, blame_lines.len()) {
            Ok((start, end)) => blame_lines
                .into_iter()
                .filter(|b| b.line_number >= start && b.line_number <= end)
                .collect(),
            Err(e) => {
                eprintln!("Error parsing line range: {}", e);
                return;
            }
        }
    } else {
        blame_lines
    };

    let mut output = String::new();

    for blame in &filtered_lines {
        let short_hash = blame
            .commit_id
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
        let author_short = if blame.author.len() > 15 {
            format!("{}...", &blame.author[..12])
        } else {
            format!("{:15}", blame.author)
        };

        let date_formatted = if let Ok(timestamp) = blame.date.parse::<i64>() {
            DateTime::from_timestamp(timestamp, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S %z")
                        .to_string()
                })
                .unwrap_or_else(|| blame.date.clone())
        } else {
            blame.date.clone()
        };

        output.push_str(&format!(
            "{} ({:19} {} {}) {}\n",
            short_hash, author_short, date_formatted, blame.line_number, blame.content
        ));
    }

    #[cfg(unix)]
    {
        let mut child = Command::new("less")
            .arg("-R") // Allow ANSI colors
            .arg("-F") // Quit if output fits on one screen
            .stdin(Stdio::piped())
            .spawn()
            .expect("Failed to spawn less");

        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(output.as_bytes())
                .expect("Failed to write to less");
        }

        child.wait().expect("Failed to wait for less");
    }

    #[cfg(not(unix))]
    {
        print!("{}", output);
    }
}
fn get_file_lines(commit: &Commit, file_path: &str) -> Result<Vec<String>, String> {
    let tree =
        load_object::<Tree>(&commit.tree_id).map_err(|e| format!("Failed to load tree: {}", e))?;

    let plain_items = tree.get_plain_items();
    let target_path = util::to_workdir_path(file_path);

    let blob_hash = plain_items
        .iter()
        .find(|(path, _)| path == &target_path)
        .map(|(_, hash)| hash)
        .ok_or_else(|| format!("File '{}' not found in commit", file_path))?;

    let blob = load_object::<Blob>(blob_hash).map_err(|e| format!("Failed to load blob: {}", e))?;

    let content = String::from_utf8_lossy(&blob.data);
    Ok(content.lines().map(|s| s.to_string()).collect())
}

/// Parse line range from string like "10", "10,20", "10,+5"
fn parse_line_range(range_str: &str, total_lines: usize) -> Result<(usize, usize), String> {
    let parts: Vec<&str> = range_str.split(',').collect();

    match parts.len() {
        1 => {
            // Single line: "10"
            let line = parts[0]
                .parse::<usize>()
                .map_err(|_| format!("Invalid line number: {}", parts[0]))?;
            if line == 0 || line > total_lines {
                return Err(format!("Line {} is out of range (1-{})", line, total_lines));
            }
            Ok((line, line))
        }
        2 => {
            let start = parts[0]
                .parse::<usize>()
                .map_err(|_| format!("Invalid start line: {}", parts[0]))?;

            // Check if second part is relative (+N) or absolute
            let end = if parts[1].starts_with('+') {
                let offset = parts[1][1..]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid offset: {}", parts[1]))?;
                start + offset - 1
            } else {
                parts[1]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid end line: {}", parts[1]))?
            };

            if start == 0 || start > total_lines || end == 0 || end > total_lines || start > end {
                return Err(format!(
                    "Invalid range {},{} (total lines: {})",
                    start, end, total_lines
                ));
            }
            Ok((start, end))
        }
        _ => Err("Invalid range format. Use: LINE or START,END or START,+COUNT".to_string()),
    }
}
