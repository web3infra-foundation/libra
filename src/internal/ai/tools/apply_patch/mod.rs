//! Apply patch module using Codex-style format.
//!
//! This module provides a patch system that is more AI-friendly than standard unified diff:
//! - Uses `*** Begin Patch` / `*** End Patch` format
//! - Supports relative paths (resolved against working directory)
//! - Supports Add/Delete/Update/Move operations
//! - Supports multiple files in a single patch
//! - Uses fuzzy matching (seek_sequence) for tolerance

mod core;
mod parser;
mod seek_sequence;

pub use core::{
    AffectedPaths, ApplyPatchError, ApplyResult, FileDiff, apply_hunks, apply_patch, format_summary,
};

pub use parser::{ApplyPatchArgs, Hunk, UpdateFileChunk, parse_patch};
