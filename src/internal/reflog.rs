use crate::internal::config;
use crate::internal::db::{DbConnection, get_db_conn_instance};
use crate::internal::head::Head;
use crate::internal::model::reflog::Model;
use git_internal::hash::SHA1;
use std::fmt::{Debug, Display, Formatter};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

pub const HEAD: &str = "HEAD";

#[derive(Debug)]
pub struct ReflogContext {
    pub old_oid: String,
    pub new_oid: String,
    pub action: ReflogAction,
}

#[derive(Debug)]
pub enum ReflogError {
    IoError(io::Error),
    TursoError(turso::Error),
}

impl Display for ReflogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::TursoError(e) => write!(f, "Turso error: {e}"),
        }
    }
}

impl From<io::Error> for ReflogError {
    fn from(err: io::Error) -> Self {
        ReflogError::IoError(err)
    }
}

impl From<turso::Error> for ReflogError {
    fn from(err: turso::Error) -> Self {
        ReflogError::TursoError(err)
    }
}

impl Display for ReflogContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.action {
            ReflogAction::Commit { message } => write!(
                f,
                "{}",
                message.lines().next().unwrap_or("(no commit message)")
            ),
            ReflogAction::Switch { from, to } => write!(f, "moving from {from} to {to}"),
            ReflogAction::Checkout { from, to } => write!(f, "moving from {from} to {to}"),
            ReflogAction::Reset { target } => write!(f, "moving to {target}"),
            ReflogAction::Merge { branch, policy } => write!(f, "merge {branch}:{policy}"),
            ReflogAction::CherryPick { source_message } => write!(
                f,
                "{}",
                source_message
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or("(no commit message)")
            ),
            ReflogAction::Fetch => write!(f, "fast-forward"),
            ReflogAction::Pull => write!(f, "fast-forward"),
            ReflogAction::Rebase { state, details } => write!(f, "({state}) {details}"),
            ReflogAction::Clone { from } => write!(f, "from {from}"),
        }
    }
}

#[derive(Debug)]
pub enum ReflogAction {
    Commit { message: String },
    Reset { target: String },
    Checkout { from: String, to: String },
    Switch { from: String, to: String },
    Merge { branch: String, policy: String },
    CherryPick { source_message: String },
    Rebase { state: String, details: String },
    Fetch,
    Pull,
    Clone { from: String },
}

#[derive(Copy, Clone)]
pub enum ReflogActionKind {
    Commit,
    Reset,
    Checkout,
    Switch,
    Merge,
    CherryPick,
    Rebase,
    Fetch,
    Pull,
    Clone,
}

impl Display for ReflogActionKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Commit => write!(f, "commit"),
            Self::Reset => write!(f, "reset"),
            Self::Checkout => write!(f, "checkout"),
            Self::Switch => write!(f, "switch"),
            Self::Merge => write!(f, "merge"),
            Self::CherryPick => write!(f, "cherry-pick"),
            Self::Rebase => write!(f, "rebase"),
            Self::Fetch => write!(f, "fetch"),
            Self::Pull => write!(f, "pull"),
            Self::Clone => write!(f, "clone"),
        }
    }
}

impl ReflogAction {
    fn kind(&self) -> ReflogActionKind {
        match self {
            Self::Commit { .. } => ReflogActionKind::Commit,
            Self::Reset { .. } => ReflogActionKind::Reset,
            Self::Switch { .. } => ReflogActionKind::Switch,
            Self::Merge { .. } => ReflogActionKind::Merge,
            Self::Pull => ReflogActionKind::Pull,
            Self::Clone { .. } => ReflogActionKind::Clone,
            Self::CherryPick { .. } => ReflogActionKind::CherryPick,
            Self::Rebase { .. } => ReflogActionKind::Rebase,
            Self::Checkout { .. } => ReflogActionKind::Checkout,
            Self::Fetch => ReflogActionKind::Fetch,
        }
    }
}

pub struct Reflog;

impl Reflog {
    pub async fn insert_single_entry(
        db: &DbConnection,
        context: &ReflogContext,
        ref_to_log: &str,
    ) -> Result<(), ReflogError> {
        // considering that there are many commands that have not yet used user configs,
        // we just set default user info.
        let name = config::Config::get_with_conn(db, "user", None, "name")
            .await
            .map(|m| m.value)
            .unwrap_or("mega".to_string());
        let email = config::Config::get_with_conn(db, "user", None, "email")
            .await
            .map(|m| m.value)
            .unwrap_or("admin@mega.org".to_string());
        let message = context.to_string();

        let sql = "INSERT INTO reflog (ref_name, old_oid, new_oid, action, committer_name, committer_email, timestamp, message) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
        db.execute(
            sql,
            turso::params![
                ref_to_log,
                context.old_oid.clone(),
                context.new_oid.clone(),
                context.action.kind().to_string(),
                name,
                email,
                timestamp_seconds(),
                message
            ],
        )
        .await?;
        Ok(())
    }

    /// insert a reflog record.
    /// see `ReflogContext`
    pub async fn insert(
        db: &DbConnection,
        context: ReflogContext,
        insert_ref: bool,
    ) -> Result<(), ReflogError> {
        ensure_reflog_table_exists(db).await?;
        let head = Head::current_with_conn(db).await;

        Self::insert_single_entry(db, &context, HEAD).await?;

        if let Head::Branch(branch_name) = head
            && insert_ref
        {
            let full_branch_ref = format!("refs/heads/{branch_name}");
            Self::insert_single_entry(db, &context, &full_branch_ref).await?;
        }
        Ok(())
    }

    pub async fn find_all(db: &DbConnection, ref_name: &str) -> Result<Vec<Model>, ReflogError> {
        let sql = "SELECT * FROM reflog WHERE ref_name = ?1 ORDER BY timestamp DESC";
        let mut rows = db.query(sql, turso::params![ref_name]).await?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(Model::from_row(&row).unwrap());
        }
        Ok(result)
    }

    pub async fn find_one(db: &DbConnection, ref_name: &str) -> Result<Option<Model>, ReflogError> {
        let sql = "SELECT * FROM reflog WHERE ref_name = ?1 ORDER BY timestamp DESC LIMIT 1";
        let mut rows = db.query(sql, turso::params![ref_name]).await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(Model::from_row(&row).unwrap()))
        } else {
            Ok(None)
        }
    }
}

fn timestamp_seconds() -> i64 {
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH).expect("Time went backwards");
    since_the_epoch.as_secs() as i64
}

/// Executes a database operation within a transaction and records a reflog entry upon success.
///
/// This function acts as a safe, atomic wrapper for any operation that needs to be
/// recorded in the reflog. It ensures that the core operation and the creation of its
/// corresponding reflog entry either both succeed and are committed, or both fail and
/// are rolled back. This prevents inconsistent states where an action is performed
/// but not logged.
///
/// # Example
///
/// ```rust,ignore
/// // 1. First, prepare the context for the reflog entry.
/// let reflog_context = ReflogContext {
///     old_oid: "previous_commit_hash".to_string(),
///     new_oid: "new_commit_hash".to_string(),
///     action: ReflogAction::Commit {
///         message: message.to_string(),
///     }
/// };
///
/// // 2. Define the core database operation as an async closure.
/// let core_operation = |db: &DbConnection| Box::pin(async move {
///     // This is where you move the branch pointer, update HEAD, etc.
///     // IMPORTANT: Use `_with_conn` variants of your helper functions.
///     Branch::update_branch_with_conn(db, "main", "new_commit_hash", None).await;
///     Head::update_with_conn(db, Head::Branch("main".to_string()), None).await;
///
///     Ok(())
/// }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), std::io::Error>> + Send + '_>>;
///
/// // 3. Execute the wrapper.
/// match with_reflog(reflog_context, core_operation, true).await {
///     Ok(_) => println!("Commit and reflog recorded successfully."),
///     Err(e) => eprintln!("Operation failed: {:?}", e),
/// }
/// ```
pub async fn with_reflog<F>(
    context: ReflogContext,
    operation: F,
    insert_ref: bool,
) -> Result<(), ReflogError>
where
    F: for<'a> FnOnce(
            &'a DbConnection,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), io::Error>> + Send + 'a>,
        > + Send
        + 'static,
{
    let db = get_db_conn_instance().await;
    db.transaction(|txn_db| {
        Box::pin(async move {
            operation(txn_db).await?;
            Reflog::insert(txn_db, context, insert_ref)
                .await
                .map_err(|e| io::Error::other(format!("Reflog insert error: {e}")))?;
            Ok(())
        })
    })
    .await
    .map_err(ReflogError::IoError)
}

/// Check whether the current libra repo have a `reflog` table
async fn reflog_table_exists(db_conn: &DbConnection) -> Result<bool, ReflogError> {
    let sql = "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1";
    let mut rows = db_conn.query(sql, turso::params!["reflog"]).await?;

    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0).unwrap();
        if count == 0 {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Ensures that the 'reflog' table and its associated indexes exist in the database.
/// If they do not exist, they will be created.
async fn ensure_reflog_table_exists(db: &DbConnection) -> Result<(), ReflogError> {
    if reflog_table_exists(db).await? {
        return Ok(());
    }

    println!("Warning: The current libra repo does not have a `reflog` table, creating one...");

    let create_table_sql = r#"
        CREATE TABLE IF NOT EXISTS `reflog` (
            `id`              INTEGER PRIMARY KEY AUTOINCREMENT,
            `ref_name`        TEXT NOT NULL,
            `old_oid`         TEXT NOT NULL,
            `new_oid`         TEXT NOT NULL,
            `committer_name`  TEXT NOT NULL,
            `committer_email` TEXT NOT NULL,
            `timestamp`       INTEGER NOT NULL,
            `action`          TEXT NOT NULL,
            `message`         TEXT NOT NULL
        );
    "#;
    db.execute(create_table_sql, turso::params![]).await?;

    let create_index_sql = r#"
        CREATE INDEX IF NOT EXISTS idx_ref_name_timestamp ON `reflog`(`ref_name`, `timestamp`);
    "#;
    db.execute(create_index_sql, turso::params![]).await?;

    Ok(())
}

pub fn zero_sha1() -> SHA1 {
    SHA1::from_bytes(&[0; 20])
}
