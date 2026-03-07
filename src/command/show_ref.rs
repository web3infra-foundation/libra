//! Implements `show-ref` to list all refs (branches, tags) with their object IDs.

use clap::Parser;

use crate::{
    internal::{branch::Branch, head::Head, tag},
    utils::error::{CliError, CliResult},
};

#[derive(Parser, Debug)]
pub struct ShowRefArgs {
    /// Show only branches (refs/heads/)
    #[clap(long)]
    pub heads: bool,

    /// Show only tags (refs/tags/)
    #[clap(long)]
    pub tags: bool,

    /// Include HEAD in the output
    #[clap(long = "head")]
    pub head: bool,

    /// Only show the object hash, not the reference name
    #[clap(short = 's', long = "hash")]
    pub hash: bool,

    /// Filter refs by pattern (substring match on the ref name)
    pub pattern: Vec<String>,
}

pub async fn execute(args: ShowRefArgs) -> Result<(), String> {
    run_show_ref(args).await
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Lists all refs (branches, tags) with their object IDs.
pub async fn execute_safe(args: ShowRefArgs) -> CliResult<()> {
    run_show_ref(args).await.map_err(CliError::failure)
}

async fn run_show_ref(args: ShowRefArgs) -> Result<(), String> {
    // When neither --heads nor --tags is specified, show both
    let show_heads = args.heads || !args.tags;
    let show_tags = args.tags || !args.heads;

    let mut entries: Vec<(String, String)> = Vec::new(); // (hash, refname)

    // Include HEAD if --head is specified
    if args.head
        && let Some(hash) = Head::current_commit().await
    {
        entries.push((hash.to_string(), "HEAD".to_string()));
    }

    // Collect local branches: refs/heads/<name>
    if show_heads {
        let branches = Branch::list_branches(None).await;
        for branch in branches {
            entries.push((
                branch.commit.to_string(),
                format!("refs/heads/{}", branch.name),
            ));
        }

        // TODO: collect remote-tracking branches
    }

    // Collect tags: refs/tags/<name>
    if show_tags {
        let tag_list = tag::list().await.map_err(|e| e.to_string())?;
        for t in tag_list {
            // For annotated tags use the tag object hash; for lightweight use the commit hash.
            let hash = match &t.object {
                tag::TagObject::Commit(c) => c.id.to_string(),
                tag::TagObject::Tag(tg) => tg.id.to_string(),
                tag::TagObject::Blob(b) => b.id.to_string(),
                tag::TagObject::Tree(tr) => tr.id.to_string(),
            };
            entries.push((hash, format!("refs/tags/{}", t.name)));
        }
    }

    // Apply pattern filter if any patterns were given
    if !args.pattern.is_empty() {
        entries.retain(|(_, refname)| {
            refname == "HEAD" || args.pattern.iter().any(|p| refname.contains(p.as_str()))
        });
    }

    if entries.is_empty() {
        return Err("no matching refs found".to_string());
    }

    // Print entries
    for (hash, refname) in &entries {
        if args.hash {
            println!("{}", hash);
        } else {
            println!("{} {}", hash, refname);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::ShowRefArgs;

    #[test]
    fn test_show_ref_args_default() {
        let args = ShowRefArgs::try_parse_from(["show-ref"]).unwrap();
        assert!(!args.heads);
        assert!(!args.tags);
        assert!(!args.head);
        assert!(!args.hash);
        assert!(args.pattern.is_empty());
    }

    #[test]
    fn test_show_ref_args_heads_only() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--heads"]).unwrap();
        assert!(args.heads);
        assert!(!args.tags);
    }

    #[test]
    fn test_show_ref_args_tags_only() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--tags"]).unwrap();
        assert!(!args.heads);
        assert!(args.tags);
    }

    #[test]
    fn test_show_ref_args_with_pattern() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--heads", "main"]).unwrap();
        assert!(args.heads);
        assert_eq!(args.pattern, vec!["main".to_string()]);
    }

    #[test]
    fn test_show_ref_args_hash_flag() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--hash"]).unwrap();
        assert!(args.hash);
    }
}
