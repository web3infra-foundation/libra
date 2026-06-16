use std::collections::HashSet;

use git_internal::{hash::ObjectHash, internal::object::commit::Commit};

use crate::{
    command::log,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        util::{self, CommitBaseError},
    },
};

pub(super) struct RevListSelection {
    pub(super) input: String,
    pub(super) inputs: Vec<String>,
    pub(super) commits: Vec<Commit>,
}

enum RevisionTerm<'a> {
    Include(&'a str),
    Exclude(&'a str),
    Range { start: &'a str, end: &'a str },
    Symmetric { left: &'a str, right: &'a str },
}

pub(super) async fn resolve_revision_selection(specs: &[String]) -> CliResult<RevListSelection> {
    let input_terms = normalized_inputs(specs);
    let mut included = Vec::<Commit>::new();
    let mut included_ids = HashSet::<String>::new();
    let mut excluded = HashSet::<String>::new();

    for input in &input_terms {
        match parse_revision_term(input) {
            RevisionTerm::Include(spec) => {
                include_reachable(spec, &mut included, &mut included_ids).await?
            }
            RevisionTerm::Exclude(spec) => exclude_reachable(spec, &mut excluded).await?,
            RevisionTerm::Range { start, end } => {
                include_reachable(end, &mut included, &mut included_ids).await?;
                exclude_reachable(start, &mut excluded).await?;
            }
            RevisionTerm::Symmetric { left, right } => {
                let left_commits = reachable_commits(left).await?;
                let right_commits = reachable_commits(right).await?;
                let left_ids = commit_id_set(&left_commits);
                let right_ids = commit_id_set(&right_commits);
                excluded.extend(left_ids.intersection(&right_ids).cloned());
                insert_commits(left_commits, &mut included, &mut included_ids);
                insert_commits(right_commits, &mut included, &mut included_ids);
            }
        }
    }

    let commits = included
        .into_iter()
        .filter(|commit| !excluded.contains(&commit.id.to_string()))
        .collect::<Vec<_>>();
    let input = input_terms.join(" ");

    Ok(RevListSelection {
        input,
        inputs: input_terms,
        commits,
    })
}

fn normalized_inputs(specs: &[String]) -> Vec<String> {
    if specs.is_empty() {
        vec!["HEAD".to_string()]
    } else {
        specs.to_vec()
    }
}

fn parse_revision_term(input: &str) -> RevisionTerm<'_> {
    if let Some(spec) = input.strip_prefix('^')
        && !spec.is_empty()
    {
        return RevisionTerm::Exclude(spec);
    }
    if let Some((left, right)) = split_range(input, "...") {
        return RevisionTerm::Symmetric {
            left: default_head(left),
            right: default_head(right),
        };
    }
    if let Some((start, end)) = split_range(input, "..") {
        return RevisionTerm::Range {
            start: default_head(start),
            end: default_head(end),
        };
    }
    RevisionTerm::Include(input)
}

fn split_range<'a>(input: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let index = input.find(separator)?;
    let left = &input[..index];
    let right = &input[index + separator.len()..];
    Some((left, right))
}

fn default_head(input: &str) -> &str {
    if input.is_empty() { "HEAD" } else { input }
}

async fn include_reachable(
    spec: &str,
    included: &mut Vec<Commit>,
    included_ids: &mut HashSet<String>,
) -> CliResult<()> {
    insert_commits(reachable_commits(spec).await?, included, included_ids);
    Ok(())
}

async fn exclude_reachable(spec: &str, excluded: &mut HashSet<String>) -> CliResult<()> {
    excluded.extend(
        reachable_commits(spec)
            .await?
            .into_iter()
            .map(|commit| commit.id.to_string()),
    );
    Ok(())
}

async fn reachable_commits(spec: &str) -> CliResult<Vec<Commit>> {
    let commit = resolve_commit(spec).await?;
    log::get_reachable_commits(commit.to_string(), None).await
}

async fn resolve_commit(spec: &str) -> CliResult<ObjectHash> {
    util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_list_target_error(spec, err))
}

fn insert_commits(
    commits: Vec<Commit>,
    included: &mut Vec<Commit>,
    included_ids: &mut HashSet<String>,
) {
    for commit in commits {
        if included_ids.insert(commit.id.to_string()) {
            included.push(commit);
        }
    }
}

fn commit_id_set(commits: &[Commit]) -> HashSet<String> {
    commits.iter().map(|commit| commit.id.to_string()).collect()
}

fn rev_list_target_error(spec: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::failure(format!(
            "not a valid object name: '{spec}' (HEAD does not point to a commit)"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("create a commit before resolving HEAD."),
        CommitBaseError::InvalidReference(detail) => {
            CliError::failure(format!("not a valid object name: '{spec}' ({detail})"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        }
        CommitBaseError::ReadFailure(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}
