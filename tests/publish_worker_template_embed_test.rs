//! Integration test: the embedded Worker template (`worker/`) ships
//! the Phase 6/7 source-only files and excludes generated /
//! secret-bearing directories.
//!
//! The test exercises:
//!
//!   1) `WorkerTemplate::iter()` returns at least the manifest entries.
//!   2) The embed contains no path matching any deny fragment.
//!   3) Specific Worker entrypoints (Next route handlers, Tailwind
//!      stylesheet, Wrangler config, D1 migration) are present and
//!      non-empty.
//!   4) The `publish_worker_template_embed_test` doubles as a
//!      regression check for the cargo `package` rules — every entry
//!      we read from the embed is also visible on disk under
//!      `worker/` (modulo embed path normalisation), so a future
//!      cargo manifest tweak can't strip files silently.

use std::{collections::BTreeSet, path::PathBuf};

use libra::internal::publish::worker_template::{
    EMBED_DENY_SEGMENTS, MANIFEST, WorkerTemplate, embed_path_is_allowed,
};

#[test]
fn embed_contains_every_manifest_entry() {
    let embed_paths: BTreeSet<String> = WorkerTemplate::iter().map(|s| s.to_string()).collect();
    for entry in MANIFEST {
        assert!(
            embed_paths.contains(entry.path),
            "MANIFEST entry {:?} is not present in the WorkerTemplate embed; \
             refresh `worker/{}` and rebuild, or update the include rules in \
             `src/internal/publish/worker_template.rs`",
            entry.path,
            entry.path,
        );
    }
}

#[test]
fn embed_does_not_carry_generated_or_secret_paths() {
    // Sanity-check the pure helper first so a regression in segment
    // matching shows up before we crawl the embed. The publish
    // scaffold ships exactly one example env file
    // (`.dev.vars.example`); every other `.env*` and `.dev.vars*`
    // pattern must be denied per Codex pass-1 P1.
    assert!(embed_path_is_allowed(".dev.vars.example"));
    assert!(embed_path_is_allowed("app/page.tsx"));
    assert!(!embed_path_is_allowed(".env.example"));
    assert!(!embed_path_is_allowed(".env"));
    assert!(!embed_path_is_allowed(".env.local"));
    assert!(!embed_path_is_allowed(".env.production"));
    assert!(!embed_path_is_allowed(".dev.vars"));
    assert!(!embed_path_is_allowed(".dev.vars.local"));
    assert!(!embed_path_is_allowed("app/.dev.vars"));
    assert!(!embed_path_is_allowed("app/.env"));
    assert!(!embed_path_is_allowed("node_modules/foo/package.json"));

    // Codex pass-3 P2 + pass-4 P1: credential-bearing filenames
    // must be denied wherever they appear in the worker tree, and
    // SSH key prefixes must match suffixed variants too.
    assert!(!embed_path_is_allowed("id_rsa"));
    assert!(!embed_path_is_allowed("config/id_rsa.pub"));
    assert!(!embed_path_is_allowed("keys/id_ed25519"));
    assert!(!embed_path_is_allowed("keys/id_ed25519_work"));
    assert!(!embed_path_is_allowed("keys/id_rsa_personal"));
    assert!(!embed_path_is_allowed("keys/id_ecdsa-2024"));
    assert!(!embed_path_is_allowed("keys/id_dsa.bak"));
    assert!(!embed_path_is_allowed("server.pem"));
    assert!(!embed_path_is_allowed("server.PEM"));
    assert!(!embed_path_is_allowed("api.key"));
    // Codex pass-5 P1: every segment containing `token` / `secret` /
    // `credential` is denied unless the whole segment is on the
    // design-system allowlist (`tokens.css`, `tokens.ts`, …).
    // `tokens.json` is denied because a `.json` file with a token
    // keyword is more often a credential dump than a design asset.
    assert!(!embed_path_is_allowed("Cloudflare-Token.json"));
    assert!(!embed_path_is_allowed("auth_token.txt"));
    assert!(!embed_path_is_allowed("api.token.json"));
    assert!(!embed_path_is_allowed("our-secret.txt"));
    assert!(!embed_path_is_allowed("auth_secret.json"));
    assert!(!embed_path_is_allowed("app.secret.config"));
    assert!(!embed_path_is_allowed("aws-credentials.json"));
    assert!(!embed_path_is_allowed("nested/dir/has-secret.json"));
    assert!(!embed_path_is_allowed("tokens.json"));
    assert!(!embed_path_is_allowed("token.txt"));
    assert!(!embed_path_is_allowed("secrets.json"));
    assert!(!embed_path_is_allowed("auth_tokens.css"));
    // Sanity-check that genuinely safe paths pass.
    assert!(embed_path_is_allowed("app/page.tsx"));
    assert!(embed_path_is_allowed("lib/server/d1.ts"));
    // Design-system allowlist: stylesheet / TypeScript / JavaScript
    // variants of the canonical design tokens are explicitly
    // allowed despite carrying the `token` keyword.
    assert!(embed_path_is_allowed("design/tokens.css"));
    assert!(embed_path_is_allowed("design/tokens.ts"));
    assert!(embed_path_is_allowed("design/tokens.scss"));
    assert!(embed_path_is_allowed("design/tokens.tsx"));
    assert!(embed_path_is_allowed("design/tokens.js"));
    assert!(embed_path_is_allowed("design/tokens.mjs"));
    assert!(embed_path_is_allowed("design/design-tokens.css"));
    assert!(embed_path_is_allowed("design/design-tokens.ts"));

    for path in WorkerTemplate::iter() {
        assert!(
            embed_path_is_allowed(path.as_ref()),
            "Worker template embed carries a denied path {path:?}; \
             allowed denylist segments: {EMBED_DENY_SEGMENTS:?}; \
             check rust-embed include/exclude rules and worker/.gitignore"
        );
    }
}

/// Every entry in `EMBED_DENY_SEGMENTS` must actually be rejected by
/// `embed_path_is_allowed` — as a bare segment, mid-path, and as a
/// leaf. The existing test only spot-checks `.env` / `.dev.vars` /
/// `node_modules` explicitly; the other segments (`.next`,
/// `.open-next`, `.wrangler`, `.turbo`, `.next-types`, `test-results`,
/// `playwright-report`, `cloudflare-env.d.ts`, `.DS_Store`,
/// `_design_reference`, `_legacy_not_for_v1`) were never individually
/// pinned. This data-driven test guarantees the const and the deny
/// logic stay in sync and auto-covers any future segment added to the
/// const — a build artifact or design-reference dir slipping into the
/// published Worker bundle is exactly what this denylist prevents.
///
/// Note: `.env` and `.dev.vars` additionally have dedicated
/// `starts_with` fallback guards in `embed_path_is_allowed`, so for
/// those two this test pins *that they are denied*, not specifically
/// the exact-match arm. The other 12 segments have no fallback and are
/// pinned here for the first time.
#[test]
fn every_embed_deny_segment_is_rejected() {
    for seg in EMBED_DENY_SEGMENTS {
        assert!(
            !embed_path_is_allowed(seg),
            "deny segment `{seg}` must be rejected as a bare path",
        );
        assert!(
            !embed_path_is_allowed(&format!("app/{seg}/page.tsx")),
            "deny segment `{seg}` must be rejected mid-path",
        );
        assert!(
            !embed_path_is_allowed(&format!("src/{seg}")),
            "deny segment `{seg}` must be rejected as a leaf segment",
        );
    }
}

#[test]
fn embed_contains_critical_worker_entrypoints() {
    let embed_paths: BTreeSet<String> = WorkerTemplate::iter().map(|s| s.to_string()).collect();
    for required in [
        "wrangler.jsonc",
        "package.json",
        "tsconfig.json",
        "next.config.ts",
        "open-next.config.ts",
        "migrations/0001_publish.sql",
        "migrations/0002_publish_digest_check.sql",
        "app/layout.tsx",
        "app/page.tsx",
        "app/globals.css",
        "app/fonts/inter-latin-variable.woff2",
        "app/fonts/jetbrains-mono-latin-variable.woff2",
        "app/api/sites/[slug]/route.ts",
        "app/api/sites/[slug]/refs/route.ts",
        "app/api/sites/[slug]/tree/route.ts",
        "app/api/sites/[slug]/file/route.ts",
        "app/api/sites/[slug]/status/route.ts",
        "app/api/sites/[slug]/ai/versions/route.ts",
        "app/api/sites/[slug]/ai/objects/route.ts",
        "app/api/sites/[slug]/ai/objects/[type]/[id]/route.ts",
        "lib/server/cloudflare.ts",
        "lib/server/d1.ts",
        "lib/server/r2.ts",
        "lib/server/access.ts",
        "lib/server/wire.ts",
        "lib/wire-types.ts",
        "lib/client/api.ts",
    ] {
        assert!(
            embed_paths.contains(required),
            "expected Worker template entrypoint {required:?} is missing from embed",
        );
        let bytes = WorkerTemplate::get(required)
            .unwrap_or_else(|| panic!("file {required:?} present in iter() but get() failed"));
        assert!(
            !bytes.data.is_empty(),
            "Worker template entrypoint {required:?} is empty; the source file \
             should be non-empty so callers can detect template corruption"
        );
    }
}

#[test]
fn embed_only_references_files_present_on_disk() {
    // CARGO_MANIFEST_DIR points at the crate root in `cargo test`. We
    // treat that as the authoritative source for the worker/ tree.
    // Every file in the embed must exist on disk under worker/; this
    // catches the case where rust-embed picks up a stale build cache
    // entry that no longer matches the source tree.
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let worker_root = crate_root.join("worker");
    for path in WorkerTemplate::iter() {
        let absolute = worker_root.join(path.as_ref());
        assert!(
            absolute.exists(),
            "Worker template embed references {path:?} but {:?} does not \
             exist on disk; rebuild the workspace or update the include rules",
            absolute
        );
    }
}
