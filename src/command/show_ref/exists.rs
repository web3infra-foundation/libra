use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::{
    internal::{
        branch::{Branch, BranchStoreError},
        db::get_db_conn_instance,
        model::reference,
    },
    utils::error::{CliError, CliResult},
};

pub(super) async fn show_ref_exists(target: &str) -> CliResult<()> {
    if reference_exists(target).await? {
        Ok(())
    } else {
        Err(CliError::failure("reference does not exist").with_exit_code(2))
    }
}

async fn reference_exists(target: &str) -> CliResult<bool> {
    if target == "HEAD" {
        return local_head_exists().await;
    }

    if let Some(branch_name) = target.strip_prefix("refs/heads/") {
        return branch_exists(branch_name, None).await;
    }

    if let Some((remote, branch_name)) = remote_tracking_parts(target) {
        if branch_exists(branch_name, Some(remote)).await? {
            return Ok(true);
        }
        return branch_exists(target, Some(remote)).await;
    }

    if target.strip_prefix("refs/tags/").is_some() {
        return local_ref_row_exists(target, reference::ConfigKind::Tag).await;
    }

    Ok(false)
}

async fn branch_exists(branch_name: &str, remote: Option<&str>) -> CliResult<bool> {
    Branch::exists_result(branch_name, remote)
        .await
        .map_err(|error| branch_query_error(branch_name, error))
}

async fn local_head_exists() -> CliResult<bool> {
    let db = get_db_conn_instance().await;
    reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .map(|row| row.is_some())
        .map_err(|error| raw_query_error("HEAD", error))
}

async fn local_ref_row_exists(refname: &str, kind: reference::ConfigKind) -> CliResult<bool> {
    let db = get_db_conn_instance().await;
    reference::Entity::find()
        .filter(reference::Column::Name.eq(refname))
        .filter(reference::Column::Kind.eq(kind))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .map(|row| row.is_some())
        .map_err(|error| raw_query_error(refname, error))
}

pub(super) fn remote_tracking_parts(refname: &str) -> Option<(&str, &str)> {
    let rest = refname.strip_prefix("refs/remotes/")?;
    let (remote, branch_name) = rest.split_once('/')?;
    if remote.is_empty() || branch_name.is_empty() {
        None
    } else {
        Some((remote, branch_name))
    }
}

fn branch_query_error(branch_name: &str, error: BranchStoreError) -> CliError {
    CliError::failure(format!(
        "failed to query reference '{branch_name}': {error}"
    ))
    .with_exit_code(1)
}

fn raw_query_error(refname: &str, error: sea_orm::DbErr) -> CliError {
    CliError::failure(format!("failed to query reference '{refname}': {error}")).with_exit_code(1)
}
