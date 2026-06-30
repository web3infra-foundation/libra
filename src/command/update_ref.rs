//! `libra update-ref` — update or delete a ref safely, a focused subset of
//! `git update-ref`. The ref read, the ref write/delete, and the reflog entry
//! all happen inside a single SQLite transaction so a compare-and-swap failure
//! rolls everything back atomically.
//!
//! Scope (v1): operates on `refs/heads/<branch>` only — the branch-tip case
//! Libra's `reference` table models cleanly. `HEAD`, `refs/tags/*`,
//! `refs/remotes/*`, and arbitrary ref namespaces are rejected with guidance
//! (use `symbolic-ref` / `switch` / `tag`), since they are not directly
//! representable here.

use std::str::FromStr;

use clap::Parser;
use git_internal::hash::{ObjectHash, get_hash_kind};
use sea_orm::{TransactionError, TransactionTrait};
use serde::Serialize;

use crate::{
    internal::{
        branch::Branch,
        db::get_db_conn_instance,
        reflog::{Reflog, ReflogAction, ReflogContext},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

const HEADS_PREFIX: &str = "refs/heads/";

/// `--help` examples (cross-cutting EXAMPLES contract, `_general.md`).
pub const UPDATE_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra update-ref refs/heads/main <newoid>            Point a branch at a commit
    libra update-ref refs/heads/main <newoid> <oldoid>   Compare-and-swap update
    libra update-ref refs/heads/topic <oid> 0000000...   Create only if absent
    libra update-ref -d refs/heads/old                   Delete a branch ref
    libra update-ref -d refs/heads/old <oldoid>          Delete only if it matches";

/// Update, create, or delete a `refs/heads/<branch>` ref with an optional
/// compare-and-swap against its current value.
#[derive(Parser, Debug)]
#[command(after_help = UPDATE_REF_EXAMPLES)]
pub struct UpdateRefArgs {
    /// Delete the ref instead of updating it.
    #[clap(short = 'd', long = "delete")]
    pub delete: bool,

    /// Reflog reason recorded with the update (Git's `-m`).
    #[clap(short = 'm', value_name = "REASON")]
    pub message: Option<String>,

    /// The ref to update, e.g. `refs/heads/main`.
    #[clap(value_name = "REF")]
    pub ref_name: String,

    /// The new object id (omit with `-d`; with `-d` this position is the
    /// optional old value to verify before deleting).
    #[clap(value_name = "NEWVALUE")]
    pub value: Option<String>,

    /// The expected current object id for a compare-and-swap (`0{40}`/`0{64}`
    /// means "must not already exist"). Only valid without `-d`.
    #[clap(value_name = "OLDVALUE")]
    pub old_value: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpdateRefOutput {
    #[serde(rename = "ref")]
    ref_name: String,
    old: Option<String>,
    new: Option<String>,
    deleted: bool,
}

/// Transaction-internal error, mapped to a 128 `CliError` by the caller.
#[derive(Debug, thiserror::Error)]
enum UpdateRefTxError {
    #[error("cannot lock ref '{ref_name}': is at {actual} but expected {expected}")]
    CasMismatch {
        ref_name: String,
        expected: String,
        actual: String,
    },
    #[error("cannot create ref '{ref_name}': it already exists at {actual}")]
    MustNotExist { ref_name: String, actual: String },
    #[error("cannot delete ref '{ref_name}': it does not exist")]
    DoesNotExist { ref_name: String },
    #[error("ref storage error: {0}")]
    Storage(String),
}

pub async fn execute(args: UpdateRefArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

/// Safe entry point. Validates inputs, then performs the read/CAS/write+reflog
/// inside a single transaction. All failures exit 128 (matching Git's fatals).
pub async fn execute_safe(args: UpdateRefArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let fatal = |message: String| {
        CliError::fatal(message)
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    };

    // Only `refs/heads/<branch>` is representable in v1.
    let branch = parse_heads_ref(&args.ref_name).map_err(fatal)?;
    if !util::is_valid_refname(&args.ref_name) {
        return Err(fatal(format!("invalid ref name '{}'", args.ref_name)));
    }

    let hash_kind = get_hash_kind();
    let zero = ObjectHash::zero_str(hash_kind);

    // Disambiguate positionals: `-d <ref> [<old>]` vs `<ref> <new> [<old>]`.
    let (new_oid, old_spec) = if args.delete {
        if args.old_value.is_some() {
            return Err(fatal(
                "too many arguments: `update-ref -d <ref> [<oldvalue>]` takes at most one value"
                    .to_string(),
            ));
        }
        (None, args.value.clone())
    } else {
        let Some(new_value) = args.value.clone() else {
            return Err(fatal(format!(
                "missing new value for '{}' (use -d to delete)",
                args.ref_name
            )));
        };
        let new_hash = parse_object_id(&new_value, &zero).map_err(fatal)?;
        // Git's update-ref refuses to point a ref at an object that is not in
        // the store; do the same so we never create a dangling ref.
        if util::objects_storage().get(&new_hash).is_err() {
            return Err(CliError::fatal(format!(
                "cannot update '{}': object {new_value} does not exist in the repository",
                args.ref_name
            ))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidTarget));
        }
        (Some(new_hash.to_string()), args.old_value.clone())
    };

    // Parse the optional compare-and-swap operand.
    let old_spec = match old_spec {
        Some(value) => Some(parse_old_value(&value, &zero).map_err(fatal)?),
        None => None,
    };

    let reflog_reason = args.message.clone().unwrap_or_default();
    let full_ref = args.ref_name.clone();
    let branch_name = branch.to_string();
    let delete = args.delete;

    let db = get_db_conn_instance().await;
    let outcome = db
        .transaction(move |txn| {
            Box::pin(async move {
                let current = Branch::find_branch_result_with_conn(txn, &branch_name, None)
                    .await
                    .map_err(|error| UpdateRefTxError::Storage(error.to_string()))?
                    .map(|b| b.commit.to_string());

                // Compare-and-swap precondition.
                if let Some(expected) = &old_spec {
                    match (expected, &current) {
                        // `0{40}` => the ref must not exist.
                        (OldValue::MustNotExist, Some(actual)) => {
                            return Err(UpdateRefTxError::MustNotExist {
                                ref_name: full_ref.clone(),
                                actual: actual.clone(),
                            });
                        }
                        (OldValue::MustNotExist, None) => {}
                        (OldValue::Exact(want), actual)
                            if actual.as_deref() != Some(want.as_str()) =>
                        {
                            return Err(UpdateRefTxError::CasMismatch {
                                ref_name: full_ref.clone(),
                                expected: want.clone(),
                                actual: actual.clone().unwrap_or_else(|| zero.clone()),
                            });
                        }
                        (OldValue::Exact(_), _) => {}
                    }
                }

                if delete {
                    let Some(old) = current.clone() else {
                        return Err(UpdateRefTxError::DoesNotExist {
                            ref_name: full_ref.clone(),
                        });
                    };
                    Branch::delete_branch_result_with_conn(txn, &branch_name, None)
                        .await
                        .map_err(|error| UpdateRefTxError::Storage(error.to_string()))?;
                    write_reflog(txn, &full_ref, &old, &zero, &reflog_reason).await?;
                    Ok(UpdateRefOutcome {
                        old: Some(old),
                        new: None,
                    })
                } else {
                    // INVARIANT: in the non-delete branch the positional
                    // disambiguation above always set `new_oid` to `Some`.
                    let new = new_oid.expect("new value validated for non-delete");
                    Branch::update_branch_with_conn(txn, &branch_name, &new, None)
                        .await
                        .map_err(|error| UpdateRefTxError::Storage(error.to_string()))?;
                    let old = current.clone().unwrap_or_else(|| zero.clone());
                    write_reflog(txn, &full_ref, &old, &new, &reflog_reason).await?;
                    Ok::<_, UpdateRefTxError>(UpdateRefOutcome {
                        old: current,
                        new: Some(new),
                    })
                }
            })
        })
        .await
        .map_err(|error| {
            let message = match error {
                TransactionError::Connection(error) => error.to_string(),
                TransactionError::Transaction(error) => error.to_string(),
            };
            CliError::fatal(message)
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::RepoStateInvalid)
        })?;

    if output.is_json() {
        emit_json_data(
            "update-ref",
            &UpdateRefOutput {
                ref_name: args.ref_name,
                old: outcome.old,
                new: outcome.new,
                deleted: args.delete,
            },
            output,
        )
    } else {
        Ok(())
    }
}

struct UpdateRefOutcome {
    old: Option<String>,
    new: Option<String>,
}

/// The parsed compare-and-swap operand.
#[derive(Clone)]
enum OldValue {
    /// `0{40}` / `0{64}`: the ref must not already exist.
    MustNotExist,
    /// An exact object id the ref must currently point to.
    Exact(String),
}

/// Write a single `update-ref` reflog entry (never leaks the user's CAS operand).
async fn write_reflog<C: sea_orm::ConnectionTrait>(
    txn: &C,
    full_ref: &str,
    old: &str,
    new: &str,
    reason: &str,
) -> Result<(), UpdateRefTxError> {
    let context = ReflogContext {
        old_oid: old.to_string(),
        new_oid: new.to_string(),
        action: ReflogAction::UpdateRef {
            message: reason.to_string(),
        },
    };
    Reflog::insert_single_entry(txn, &context, full_ref)
        .await
        .map_err(|error| UpdateRefTxError::Storage(error.to_string()))
}

/// Require a `refs/heads/<branch>` ref and return the short branch name.
fn parse_heads_ref(ref_name: &str) -> Result<&str, String> {
    if ref_name == "HEAD" {
        return Err(
            "update-ref cannot operate on HEAD; use `symbolic-ref` or `switch` instead".to_string(),
        );
    }
    if let Some(branch) = ref_name.strip_prefix(HEADS_PREFIX) {
        if branch.is_empty() {
            return Err("missing branch name after refs/heads/".to_string());
        }
        return Ok(branch);
    }
    Err(format!(
        "unsupported ref '{ref_name}': update-ref supports refs/heads/<branch> only \
         (use `tag` for refs/tags/*, `symbolic-ref`/`switch` for HEAD)"
    ))
}

/// Parse a new-value object id, rejecting symbolic-ref syntax, the null id, and
/// hash-format mismatches. Returns the parsed [`ObjectHash`] so the caller can
/// check that the object exists.
fn parse_object_id(value: &str, zero: &str) -> Result<ObjectHash, String> {
    if value.starts_with("ref:") {
        return Err(
            "symbolic refs are not supported by update-ref; use `symbolic-ref`".to_string(),
        );
    }
    if value == zero {
        return Err("refusing to point a ref at the null object id; use -d to delete".to_string());
    }
    validate_oid(value)
}

/// Parse a compare-and-swap operand (`0{40}` => must-not-exist).
fn parse_old_value(value: &str, zero: &str) -> Result<OldValue, String> {
    if value == zero {
        return Ok(OldValue::MustNotExist);
    }
    validate_oid(value)?;
    Ok(OldValue::Exact(value.to_string()))
}

/// Validate that `value` is a full object id matching the repository hash kind,
/// returning the parsed hash.
fn validate_oid(value: &str) -> Result<ObjectHash, String> {
    let expected_len = get_hash_kind().hex_len();
    if value.len() != expected_len {
        return Err(format!(
            "'{value}' is not a valid object id for this repository (expected {expected_len} hex chars)"
        ));
    }
    ObjectHash::from_str(value).map_err(|_| {
        format!("'{value}' is not a valid object id for this repository (expected {expected_len} hex chars)")
    })
}
