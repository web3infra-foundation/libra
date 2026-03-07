//! Manages tags by resolving target commits, creating lightweight or annotated tag objects, storing refs, and listing existing tags.

use clap::Parser;
use git_internal::internal::object::types::ObjectType;
use sea_orm::sqlx::types::chrono;

use crate::{
    internal::{tag, tag::TagObject},
    utils::error::{CliError, CliResult},
};

#[derive(Parser, Debug)]
#[command(about = "Create, list, delete, or verify a tag object")]
pub struct TagArgs {
    /// The name of the tag to create, show, or delete
    #[clap(required = false)]
    pub name: Option<String>,

    /// List all tags
    #[clap(short, long, group = "action")]
    pub list: bool,

    /// Delete a tag
    #[clap(short, long, group = "action")]
    pub delete: bool,

    /// Message for the annotated tag. If provided, creates an annotated tag.
    #[clap(short, long)]
    pub message: Option<String>,

    #[clap(short, long, group = "action")]
    pub force: bool,

    /// Number of annotation lines to display when listing tags (0 for tag names only)
    #[clap(short, long)]
    pub n_lines: Option<usize>,
}

pub async fn execute(args: TagArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Lists, creates, or deletes tags depending on the
/// provided arguments.
pub async fn execute_safe(args: TagArgs) -> CliResult<()> {
    if args.list || args.n_lines.is_some() {
        let show_lines = args.n_lines.unwrap_or(0);
        let rendered = render_tags(show_lines)
            .await
            .map_err(|e| CliError::fatal(e.to_string()))?;
        print!("{}", rendered);
        return Ok(());
    }

    if let Some(name) = args.name {
        if args.delete {
            delete_tag_safe(&name).await?;
        } else if args.message.is_some() {
            create_tag_safe(&name, args.message, args.force).await?;
        } else {
            create_tag_safe(&name, None, args.force).await?;
            show_tag_safe(&name).await?;
        }
    } else {
        let rendered = render_tags(0)
            .await
            .map_err(|e| CliError::fatal(e.to_string()))?;
        print!("{}", rendered);
    }
    Ok(())
}

#[cfg(test)]
async fn create_tag(tag_name: &str, message: Option<String>, force: bool) {
    if let Err(err) = create_tag_safe(tag_name, message, force).await {
        eprintln!("{}", err.render());
    }
}

async fn create_tag_safe(tag_name: &str, message: Option<String>, force: bool) -> CliResult<()> {
    tag::create(tag_name, message, force).await.map_err(|e| {
        let message = e.to_string();
        let message = message
            .strip_prefix("Tag ")
            .map(|rest| format!("tag {rest}"))
            .unwrap_or(message);
        CliError::fatal(message)
            .with_hint(format!("delete it first with 'libra tag -d {}'.", tag_name))
            .with_hint("or choose a different tag name.")
    })?;
    Ok(())
}

pub async fn render_tags(show_lines: usize) -> Result<String, anyhow::Error> {
    let tags = tag::list().await?;
    let mut output = String::new();
    let extract_message = |msg: &str| {
        msg.trim()
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .take(show_lines)
            .collect::<Vec<_>>()
            .join("\n")
    };

    for tag in tags {
        if show_lines == 0 {
            output.push_str(&format!("{}\n", tag.name));
            continue;
        }

        let show_message = match &tag.object {
            TagObject::Tag(git_internal) => extract_message(&git_internal.message),
            TagObject::Commit(commit) => extract_message(&commit.message),
            _ => String::new(),
        };

        let lines: Vec<&str> = show_message.lines().collect();

        if lines.is_empty() {
            // lightweight tag
            output.push_str(&format!("{:<20}\n", tag.name));
        } else {
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    // print first line
                    output.push_str(&format!("{:<20} {}\n", tag.name, line));
                } else {
                    // print subsequent lines: use empty string with 20 characters width alignment to match the indentation
                    output.push_str(&format!("{:<20} {}\n", "", line));
                }
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
async fn delete_tag(tag_name: &str) {
    if let Err(err) = delete_tag_safe(tag_name).await {
        eprintln!("{}", err.render());
    }
}

async fn delete_tag_safe(tag_name: &str) -> CliResult<()> {
    tag::delete(tag_name)
        .await
        .map_err(|e| CliError::fatal(e.to_string()))?;
    println!("Deleted tag '{}'", tag_name);
    Ok(())
}

async fn show_tag_safe(tag_name: &str) -> CliResult<()> {
    match tag::find_tag_and_commit(tag_name).await {
        Ok(Some((object, commit))) => {
            if object.get_type() == ObjectType::Tag {
                // Access the tag data directly from the object if it is a Tag variant.
                if let tag::TagObject::Tag(tag_object) = &object {
                    println!("tag {}", tag_object.tag_name);
                    println!("Tagger: {}", tag_object.tagger.to_string().trim());
                    println!("\n{}", tag_object.message);
                } else {
                    return Err(CliError::fatal("object is not a Tag variant"));
                }
            }

            println!("\ncommit {}", commit.id);
            println!("Author: {}", commit.author.to_string().trim());
            let commit_date =
                chrono::DateTime::from_timestamp(commit.committer.timestamp as i64, 0)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            println!("Date:   {}", commit_date.to_rfc2822());
            println!("\n    {}", commit.message.trim());
            Ok(())
        }
        Ok(None) => Err(CliError::fatal(format!("tag '{}' not found", tag_name))),
        Err(e) => Err(CliError::fatal(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git_internal::internal::object::types::ObjectType;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{cli::parse_async, internal::tag, utils::test::ChangeDirGuard};

    async fn setup_repo_with_commit() -> (tempfile::TempDir, ChangeDirGuard) {
        let temp_dir = tempdir().unwrap();
        let guard = ChangeDirGuard::new(temp_dir.path());
        parse_async(Some(&["libra", "init"])).await.unwrap();
        parse_async(Some(&["libra", "config", "user.name", "Tag Test User"]))
            .await
            .unwrap();
        parse_async(Some(&[
            "libra",
            "config",
            "user.email",
            "tag-test@example.com",
        ]))
        .await
        .unwrap();
        fs::write("test.txt", "hello").unwrap();
        parse_async(Some(&["libra", "add", "test.txt"]))
            .await
            .unwrap();
        parse_async(Some(&["libra", "commit", "-m", "Initial commit"]))
            .await
            .unwrap();
        (temp_dir, guard)
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-light");
        assert_eq!(tags[0].object.get_type(), ObjectType::Commit);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag_force() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        create_tag("v1.0-light", None, true).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-light");
        assert_eq!(tags[0].object.get_type(), ObjectType::Commit);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_annotated_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Release v1.0".to_string()), false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-annotated");
        assert_eq!(tags[0].object.get_type(), ObjectType::Tag);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_annotated_tag_force() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Release v1.0".to_string()), false).await;
        create_tag("v1.0-annotated", Some("Release v2.0".to_string()), true).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-annotated");
        assert_eq!(tags[0].object.get_type(), ObjectType::Tag);

        // Check message
        let result = tag::find_tag_and_commit("v1.0-annotated").await;
        assert!(result.is_ok());
        let (object, _) = result.unwrap().unwrap();
        if let tag::TagObject::Tag(tag_object) = object {
            assert_eq!(tag_object.message, "Release v2.0");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_show_lightweight_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        let result = tag::find_tag_and_commit("v1.0-light").await;
        assert!(result.is_ok());
        let (object, commit) = result.unwrap().unwrap();
        assert_eq!(object.get_type(), ObjectType::Commit);
        assert_eq!(commit.message.trim(), "Initial commit");
    }

    #[tokio::test]
    #[serial]
    async fn test_show_annotated_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Test message".to_string()), false).await;
        let result = tag::find_tag_and_commit("v1.0-annotated").await;
        assert!(result.is_ok());
        let (object, commit) = result.unwrap().unwrap();
        assert_eq!(object.get_type(), ObjectType::Tag);
        assert_eq!(commit.message.trim(), "Initial commit");

        // Verify tag object content directly from the TagObject enum
        if let tag::TagObject::Tag(tag_object) = object {
            assert_eq!(tag_object.message, "Test message");
        } else {
            panic!("Expected Tag object type");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_delete_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0", None, false).await;
        delete_tag("v1.0").await;
        let tags = tag::list().await.unwrap();
        assert!(tags.is_empty());
    }
}
