//! Embedded Worker template (`worker/`).
//!
//! Phase 6/7 requires the Libra binary to ship the Worker template
//! source-only — no `node_modules/`, no generated `.next/`, no
//! `.open-next/`, no `.wrangler/`, no `.env*`. `libra publish init`
//! materialises the template into the user's repo at `<repo-root>/worker/`
//! without depending on a Libra source checkout.
//!
//! `rust-embed` is configured with `include` and `exclude` glob lists so
//! that the file set baked into the binary stays in lockstep with what
//! `cargo package` ships. The `publish_worker_template_embed_test`
//! integration test asserts the inclusion/exclusion invariants on a
//! cargo-managed copy of the source tree.

use rust_embed::Embed;

/// Embed of the source-only Worker template files.
///
/// Inclusion is allow-list-driven: every directory we want to ship has
/// an explicit `include` entry. Generated and secret-bearing patterns
/// are pinned in `exclude`, mirroring `worker/.gitignore` and the
/// publish.md "Worker 分发与打包" section.
#[derive(Embed)]
#[folder = "worker/"]
#[include = "app/**/*"]
#[include = "components/**/*"]
#[include = "lib/**/*"]
#[include = "public/**/*"]
#[include = "migrations/**/*"]
#[include = "tests/**/*"]
#[include = "package.json"]
#[include = "pnpm-lock.yaml"]
#[include = "tsconfig.json"]
#[include = "next.config.ts"]
#[include = "open-next.config.ts"]
#[include = "wrangler.jsonc"]
#[include = "next-env.d.ts"]
#[include = "env.d.ts"]
#[include = "eslint.config.mjs"]
#[include = "postcss.config.mjs"]
#[include = "vitest.config.ts"]
#[include = ".dev.vars.example"]
#[include = ".gitignore"]
#[exclude = "node_modules/*"]
#[exclude = ".next/*"]
#[exclude = ".open-next/*"]
#[exclude = ".wrangler/*"]
#[exclude = ".turbo/*"]
#[exclude = "*.tsbuildinfo"]
#[exclude = ".env"]
#[exclude = ".env.*"]
#[exclude = ".dev.vars"]
#[exclude = ".dev.vars.*.local"]
#[exclude = "cloudflare-env.d.ts"]
#[exclude = "**/.DS_Store"]
#[exclude = ".DS_Store"]
#[exclude = "_design_reference/*"]
#[exclude = "_legacy_not_for_v1/*"]
pub struct WorkerTemplate;

/// Render policy for a template file.
///
/// `libra publish init` consults this when materialising the template
/// in the user's repo. Currently every file in the embed is
/// `ManagedTemplate`; future Phase 7 work introduces `RenderedConfig`
/// (for `wrangler.jsonc` variable substitution) and `UserOwned`
/// (files we never overwrite once present).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPolicy {
    /// File is owned by the template; reapplied on `publish init`
    /// unless the user has modified it since the previous run.
    ManagedTemplate,
    /// File is rendered with template-variable substitution; the
    /// rendered output is treated as managed.
    RenderedConfig,
    /// File is created on first init, then never touched.
    UserOwned,
}

/// Static manifest entry covering one template file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestEntry {
    pub path: &'static str,
    pub render_policy: RenderPolicy,
}

/// Static manifest — pure metadata that does NOT pull file bytes
/// into the binary; the actual contents come from `WorkerTemplate`.
///
/// The list documents which files exist in the template and their
/// render policy. The Phase 6 embed test asserts the manifest path
/// set is a subset of `WorkerTemplate.iter()`.
pub const MANIFEST: &[ManifestEntry] = &[
    ManifestEntry {
        path: "package.json",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "pnpm-lock.yaml",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "tsconfig.json",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "next.config.ts",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "open-next.config.ts",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "wrangler.jsonc",
        render_policy: RenderPolicy::RenderedConfig,
    },
    ManifestEntry {
        path: "next-env.d.ts",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "env.d.ts",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "eslint.config.mjs",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "postcss.config.mjs",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "vitest.config.ts",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: ".gitignore",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: ".dev.vars.example",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "migrations/0001_publish.sql",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "app/layout.tsx",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "app/page.tsx",
        render_policy: RenderPolicy::ManagedTemplate,
    },
    ManifestEntry {
        path: "app/globals.css",
        render_policy: RenderPolicy::ManagedTemplate,
    },
];

/// Path-segment denylist used by the embed test.
///
/// Each entry is a directory name or exact file basename that MUST
/// NOT appear inside the embedded `worker/` template. The check is
/// segment-aware: it splits the embedded relative path on `/` and
/// compares each segment exactly, so e.g. `.dev.vars.example` (an
/// allowed file we ship) does not collide with `.dev.vars` (a real
/// secrets file we exclude).
pub const EMBED_DENY_SEGMENTS: &[&str] = &[
    "node_modules",
    ".next",
    ".open-next",
    ".wrangler",
    ".turbo",
    ".env",
    ".dev.vars",
    "cloudflare-env.d.ts",
    ".DS_Store",
    "_design_reference",
    "_legacy_not_for_v1",
];

/// Returns true when `relative_path` contains any path segment that
/// the embed must not ship. Exposed to keep the segment-comparison
/// logic in lockstep between the test and any future runtime check.
pub fn embed_path_is_allowed(relative_path: &str) -> bool {
    for segment in relative_path.split('/') {
        for deny in EMBED_DENY_SEGMENTS {
            if segment == *deny {
                return false;
            }
            // Block any `.env*` variant that isn't an explicit example
            // file. `.env.example` and `.dev.vars.example` are
            // template scaffolds we ship intentionally; everything
            // else under those name prefixes is treated as secret.
            if (deny == &".env" || deny == &".dev.vars")
                && segment.starts_with(deny)
                && segment != *deny
                && !segment.ends_with(".example")
            {
                return false;
            }
        }
    }
    true
}
