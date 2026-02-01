//! Shared test utilities and re-exports for the command integration test suite.

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use libra::{
    command::{
        add::{self, AddArgs},
        branch::{BranchArgs, execute},
        calc_file_blob_hash,
        commit::{self, CommitArgs},
        get_target_commit,
        init::{InitArgs, init},
        load_object,
        log::{LogArgs, get_reachable_commits},
        remove::{self, RemoveArgs},
        save_object,
        status::{changes_to_be_committed, changes_to_be_staged},
        switch::{self, SwitchArgs},
    },
    common_utils::format_commit_msg,
    internal::{branch::Branch, head::Head},
    utils::test::{self, ChangeDirGuard},
};
use serial_test::serial;
use tempfile::tempdir;
mod add_test;
mod blame_test;
mod branch_test;
mod checkout_test;
mod cherry_pick_test;
mod clone_test;
mod commit_test;
mod config_test;
mod diff_test;
mod fetch_test;
mod index_pack_test;
mod init_test;
mod lfs_test;
mod log_test;
mod merge_test;
mod open_test;
mod pull_test;
mod push_test;
mod rebase_test;
mod reflog_test;
mod remote_test;
mod remove_test;
mod reset_test;
mod restore_test;
mod revert_test;
mod show_test;
mod status_test;
mod switch_test;
mod tag_test;
