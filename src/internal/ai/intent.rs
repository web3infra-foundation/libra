//! Storage-side glue trait implementations for the [`Intent`] workflow object.
//!
//! The Libra storage layer ([`crate::utils::storage_ext::Identifiable`])
//! abstracts over any object that can be persisted under a stable
//! `(object_type, object_id)` pair. Most workflow types provide this glue in
//! the corresponding domain module; this file does the same for `Intent`,
//! re-exporting its header-derived identity to the rest of the codebase
//! without forcing storage code to know the `git-internal` shape.

use git_internal::internal::object::intent::Intent;

use crate::utils::storage_ext::Identifiable;

// Wire `Intent` into the generic storage abstraction by surfacing the
// identity already encoded in its header.
impl Identifiable for Intent {
    /// Stable, content-derived identifier of this intent (UUID-style).
    ///
    /// Functional scope:
    /// - Defers entirely to the intent header, so storage and history
    ///   layers always see the same id that `git-internal` itself uses.
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }

    /// Workflow object type tag, used as the directory name on the AI
    /// history branch (e.g. `intent/`, `task/`, `plan/`).
    ///
    /// Functional scope:
    /// - Returns the type string recorded in the intent header verbatim.
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}
