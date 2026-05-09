//! Libra publish — read-only Cloudflare-backed publishing.
//!
//! Per `docs/improvement/publish.md`, the `publish` module is the
//! outward-facing counterpart to `cloud`: it reuses Git object backup
//! plumbing for raw artefacts but adds a publish-specific snapshot
//! pipeline that materialises code previews, tree manifests and the
//! full Libra AI object model into D1/R2 for browser consumption.
//!
//! The module is laid out in phases that match the design doc's
//! `Phase 0..8` plan:
//!
//! - `contract` — versioned wire types shared by the Rust CLI side
//!   and the Worker / fixture layer (Phase 0). The serde shapes here
//!   are the source of truth; the JSON fixtures under
//!   `tests/data/publish/` and the byte-equal `sql/publish/0001_publish.sql`
//!   mirror at `worker/migrations/0001_publish.sql` round-trip
//!   through these types in the Phase 0 contract tests.
//!
//! Later phases land additional submodules: `preflight`, `snapshot`,
//! `ai_export`, `worker_template`, etc. Each phase's submodule is
//! gated on its predecessors' contracts holding stable.

pub mod contract;
