//! Manages tags by resolving target commits, creating lightweight or annotated tag objects, storing refs, and listing existing tags.

use clap::Parser;
use git_internal::internal::object::types::ObjectType;
use sea_orm::sqlx::types::chrono;

use crate::internal::{tag, tag::TagObject};

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
    if args.list || args.n_lines.is_some() {
        let show_lines = args.n_lines.unwrap_or(0);
        list_tags(show_lines).await;
        return;
    }

    if let Some(name) = args.name {
        if args.delete {
            delete_tag(&name).await;
        } else if args.message.is_some() {
            create_tag(&name, args.message, args.force).await;
        } else {
            create_tag(&name, None, args.force).await;
            show_tag(&name).await;
        }
    } else {
        list_tags(0).await;
    }
}

async fn create_tag(tag_name: &str, message: Option<String>, force: bool) {
    match tag::create(tag_name, message, force).await {
        Ok(_) => (),
        Err(e) => eprintln!("fatal: {}", e),
    }
}

async fn list_tags(show_lines: usize) {
    match render_tags(show_lines).await {
        Ok(s) => print!("{}", s),
        Err(e) => eprintln!("fatal: {}", e),
    }
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

async fn delete_tag(tag_name: &str) {
    match tag::delete(tag_name).await {
        Ok(_) => println!("Deleted tag '{}'", tag_name),
        Err(e) => eprintln!("fatal: {}", e),
    }
}

async fn show_tag(tag_name: &str) {
    match tag::find_tag_and_commit(tag_name).await {
        Ok(Some((object, commit))) => {
            if object.get_type() == ObjectType::Tag {
                // Access the tag data directly from the object if it is a Tag variant.
                if let tag::TagObject::Tag(tag_object) = &object {
                    println!("tag {}", tag_object.tag_name);
                    println!("Tagger: {}", tag_object.tagger.to_string().trim());
                    println!("\n{}", tag_object.message);
                } else {
                    eprintln!("fatal: object is not a Tag variant");
                    return;
                }
            }

            println!("\ncommit {}", commit.id);
            println!("Author: {}", commit.author.to_string().trim());
            let commit_date =
                chrono::DateTime::from_timestamp(commit.committer.timestamp as i64, 0)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            println!("Date:   {}", commit_date.to_rfc2822());
            println!("\n    {}", commit.message.trim());
        }
        Ok(None) => eprintln!("fatal: tag '{}' not found", tag_name),
        Err(e) => eprintln!("fatal: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git_internal::internal::object::types::ObjectType;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{cli::parse_async, internal::tag};

    async fn setup_repo_with_commit() -> tempfile::TempDir {
        let temp_dir = tempdir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();
        parse_async(Some(&["libra", "init"])).await.unwrap();
        fs::write("test.txt", "hello").unwrap();
        parse_async(Some(&["libra", "add", "test.txt"]))
            .await
            .unwrap();
        parse_async(Some(&["libra", "commit", "-m", "Initial commit"]))
            .await
            .unwrap();
        temp_dir
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag() {
        let _temp_dir = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-light");
        assert_eq!(tags[0].object.get_type(), ObjectType::Commit);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag_force() {
        let _temp_dir = setup_repo_with_commit().await;
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
        let _temp_dir = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Release v1.0".to_string()), false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-annotated");
        assert_eq!(tags[0].object.get_type(), ObjectType::Tag);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_annotated_tag_force() {
        let _temp_dir = setup_repo_with_commit().await;
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
        let _temp_dir = setup_repo_with_commit().await;
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
        let _temp_dir = setup_repo_with_commit().await;
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
        let _temp_dir = setup_repo_with_commit().await;
        create_tag("v1.0", None, false).await;
        delete_tag("v1.0").await;
        let tags = tag::list().await.unwrap();
        assert!(tags.is_empty());
    }
}
