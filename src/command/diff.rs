use std::{
    fmt,
    io::{self, Write},
    path::PathBuf,
};

use clap::Parser;
use colored::Colorize;
use git_internal::Diff;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::Index,
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
        pack::utils::calculate_object_hash,
    },
};
use similar;
use std::io::IsTerminal;

use crate::{
    command::{get_target_commit, load_object},
    internal::head::Head,
    utils::{
        ignore::{self, IgnorePolicy},
        object_ext::TreeExt,
        path, util,
    },
};

use crate::utils::util::to_workdir_path;
#[cfg(unix)]
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
pub struct DiffArgs {
    /// Old commit, default is HEAD
    #[clap(long, value_name = "COMMIT")]
    pub old: Option<String>,

    /// New commit, default is working directory
    #[clap(long, value_name = "COMMIT")]
    #[clap(requires = "old", group = "op_new")]
    pub new: Option<String>,

    /// Use stage as new commit. This option is conflict with --new.
    #[clap(long)]
    #[clap(group = "op_new")]
    pub staged: bool,

    #[clap(help = "Files to compare")]
    pathspec: Vec<String>,

    // TODO: If algorithm support gets added to git-internal
    /// choose the exact diff algorithm default value is histogram
    /// support myers and myersMinimal
    #[clap(long, default_value = "histogram", value_parser=["histogram", "myers", "myersMinimal"])]
    pub algorithm: Option<String>,

    // Print the result to file
    #[clap(long, value_name = "FILENAME")]
    pub output: Option<String>,
}

pub async fn execute(args: DiffArgs) {
    if !util::check_repo_exist() {
        return;
    }
    tracing::debug!("diff args: {:?}", args);
    let index = Index::load(path::index()).unwrap();

    let mut w = match args.output {
        Some(ref path) => {
            let file = std::fs::File::create(path)
                .map_err(|e| {
                    eprintln!("fatal: could not open to file '{path}' for writing: {e}");
                })
                .unwrap();
            Some(file)
        }
        None => None,
    };

    let old_blobs = match &args.old {
        // explicit --old <commit>
        Some(source) => match get_target_commit(source).await {
            Ok(commit_hash) => get_commit_blobs(&commit_hash).await,
            Err(e) => {
                eprintln!("fatal: {e}, can't use as diff old source");
                return;
            }
        },

        // no --old
        None => {
            if args.staged {
                // git diff --staged  => old = HEAD
                match Head::current_commit().await {
                    Some(commit_hash) => get_commit_blobs(&commit_hash).await,
                    None => {
                        println!("No commits yet - nothing to compare");
                        return;
                    }
                }
            } else {
                // default git diff => old = INDEX
                let files = index.tracked_files();
                get_files_blobs(&files, &index, IgnorePolicy::Respect)
            }
        }
    };

    let new_blobs = match args.new {
        Some(ref source) => match get_target_commit(source).await {
            Ok(commit_hash) => get_commit_blobs(&commit_hash).await,
            Err(e) => {
                eprintln!("fatal: {e}, can't use as diff new source");
                return;
            }
        },
        None => {
            let files = if args.staged {
                // use staged as new commit
                index.tracked_files()
            } else {
                // use working directory as new commit
                // NOTE: git didn't show diff for untracked files, but we do
                util::list_workdir_files().unwrap()
            };
            get_files_blobs(&files, &index, IgnorePolicy::Respect)
        }
    };

    // use pathspec to filter files
    let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();

    let read_content = |file: &PathBuf, hash: &ObjectHash| {
        // read content from blob or file
        match load_object::<Blob>(hash) {
            Ok(blob) => blob.data,
            Err(_) => {
                let file = to_workdir_path(file);
                std::fs::read(&file)
                    .map_err(|e| {
                        eprintln!("fatal: could not read file '{}': {}", file.display(), e);
                    })
                    .unwrap()
            }
        }
    };

    // Get diff output as string using the unified diff function
    let diff_output = Diff::diff(
        old_blobs,
        new_blobs,
        // args.algorithm.unwrap_or_default(),
        paths,
        read_content,
    );

    let results: Vec<String> = diff_output.iter().map(|i| i.data.clone()).collect();

    // Handle output - libra processes the string according to its needs
    match w {
        Some(ref mut file) => {
            file.write_all(results.join("").as_bytes()).unwrap();
        }
        None => {
            let output = if io::stdout().is_terminal() {
                colorize_diff(&results.join(""))
            } else {
                results.join("")
            };
            #[cfg(unix)]
            {
                let mut child = Command::new("less")
                    .arg("-R")
                    .arg("-F")
                    .stdin(Stdio::piped())
                    .spawn()
                    .expect("failed to execute process");
                let stdin = child.stdin.as_mut().unwrap();
                stdin.write_all(output.as_bytes()).unwrap();
                child.wait().unwrap();
            }
            #[cfg(not(unix))]
            {
                io::stdout().write_all(output.as_bytes()).unwrap();
            }
        }
    }
}

async fn get_commit_blobs(commit_hash: &ObjectHash) -> Vec<(PathBuf, ObjectHash)> {
    let commit = load_object::<Commit>(commit_hash).unwrap();
    let tree = load_object::<Tree>(&commit.tree_id).unwrap();
    tree.get_plain_items()
}

/// diff needs to print hashes even if the files have not been staged yet.
/// This helper maps workdir paths to blob ids while applying the shared ignore policy.
fn get_files_blobs(
    files: &[PathBuf],
    index: &Index,
    policy: IgnorePolicy,
) -> Vec<(PathBuf, ObjectHash)> {
    files
        .iter()
        .filter(|path| !ignore::should_ignore(path, policy, index))
        .map(|p| {
            let path = util::workdir_to_absolute(p);
            let data = std::fs::read(&path).unwrap();
            (p.to_owned(), calculate_object_hash(ObjectType::Blob, &data))
        })
        .collect()
}

fn colorize_diff(diff_text: &str) -> String {
    let mut output = String::with_capacity(diff_text.len() + 500);

    for line in diff_text.lines() {
        let colored_line = if line.starts_with("diff --git") {
            line.bold().to_string()
        } else if line.starts_with("@@") {
            line.cyan().to_string()
        } else if line.starts_with('-') && !line.starts_with("---") {
            line.red().to_string()
        } else if line.starts_with('+') && !line.starts_with("+++") {
            line.green().to_string()
        } else {
            line.to_string()
        };

        output.push_str(&colored_line);
        output.push('\n');
    }
    output
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),
            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}

#[allow(dead_code)]
fn similar_diff_result(old: &str, new: &str, w: &mut dyn io::Write) {
    let diff = similar::TextDiff::from_lines(old, new);
    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            println!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    similar::ChangeTag::Delete => "-",
                    similar::ChangeTag::Insert => "+",
                    similar::ChangeTag::Equal => " ",
                };
                write!(
                    w,
                    "{}{} |{}",
                    Line(change.old_index()),
                    Line(change.new_index()),
                    sign
                )
                .unwrap();
                write!(w, "{}", change.value()).unwrap();
                if change.missing_newline() {
                    writeln!(w).unwrap();
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::utils::test;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    use super::*;
    #[test]
    /// Tests command line argument parsing for the diff command with various parameter combinations.
    /// Verifies parameter requirements, conflicts and default values are handled correctly.
    fn test_args() {
        {
            let args = DiffArgs::try_parse_from(["diff", "--old", "old", "--new", "new", "paths"]);
            assert!(args.is_ok());
            let args = args.unwrap();
            assert_eq!(args.old, Some("old".to_string()));
            assert_eq!(args.new, Some("new".to_string()));
            assert_eq!(args.pathspec, vec!["paths".to_string()]);
        }
        {
            // --staged didn't require --old
            let args =
                DiffArgs::try_parse_from(["diff", "--staged", "pathspec", "--output", "output"]);
            let args = args.unwrap();
            assert_eq!(args.old, None);
            assert!(args.staged);
        }
        {
            // --staged conflicts with --new
            let args = DiffArgs::try_parse_from([
                "diff", "--old", "old", "--new", "new", "--staged", "paths",
            ]);
            assert!(args.is_err());
            assert!(args.err().unwrap().kind() == clap::error::ErrorKind::ArgumentConflict);
        }
        {
            // --new requires --old
            let args = DiffArgs::try_parse_from([
                "diff", "--new", "new", "pathspec", "--output", "output",
            ]);
            assert!(args.is_err());
            assert!(args.err().unwrap().kind() == clap::error::ErrorKind::MissingRequiredArgument);
        }
        #[ignore]
        {
            // --algorithm arg
            let args = DiffArgs::try_parse_from([
                "diff",
                "--old",
                "old",
                "--new",
                "new",
                "--algorithm",
                "myers",
                "target paths",
            ])
            .unwrap();
            assert_eq!(args.algorithm, Some("myers".to_string()));
        }
        #[ignore]
        {
            // --algorithm arg with default value
            let args = DiffArgs::try_parse_from(["diff", "--old", "old", "target paths"]).unwrap();
            assert_eq!(args.algorithm, Some("histogram".to_string()));
        }
    }

    #[test]
    /// Tests the functionality of the `similar_diff_result` function.
    /// Verifies that it correctly generates a diff between two text inputs.
    fn test_similar_diff_result() {
        let old = "Hello World\nThis is the second line.\nThis is the third.";
        let new = "Hallo Welt\nThis is the second line.\nThis is life.\nMoar and more";
        let mut buf = Vec::new();
        similar_diff_result(old, new, &mut buf);
        let result = String::from_utf8(buf).unwrap();
        println!("{result}");
    }

    #[tokio::test]
    #[serial]
    /// Tests that the get_files_blobs function properly respects .libraignore patterns.
    /// Verifies ignored files are correctly excluded from the blob collection process.
    async fn test_get_files_blob_gitignore() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let mut gitignore_file = fs::File::create(".libraignore").unwrap();
        gitignore_file.write_all(b"should_ignore").unwrap();

        fs::File::create("should_ignore").unwrap();
        fs::File::create("not_ignore").unwrap();

        let index = Index::load(path::index()).unwrap();
        let blob = get_files_blobs(
            &[PathBuf::from("should_ignore"), PathBuf::from("not_ignore")],
            &index,
            IgnorePolicy::Respect,
        );
        assert_eq!(blob.len(), 1);
        assert_eq!(blob[0].0, PathBuf::from("not_ignore"));
    }
}
