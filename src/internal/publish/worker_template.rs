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
// Codex pass-3 P2: credential / secret filenames must never enter
// the binary. The `embed_path_is_allowed` runtime helper enforces
// the same set against any path the embed iterator produces, so
// these globs and the helper stay in lockstep.
#[exclude = "**/*.pem"]
#[exclude = "**/*.key"]
#[exclude = "**/id_rsa*"]
#[exclude = "**/id_dsa*"]
#[exclude = "**/id_ecdsa*"]
#[exclude = "**/id_ed25519*"]
// Codex pass-6 P2: rust-embed gives `#[exclude]` priority over
// `#[include]`, so a broad `**/*token*` glob would strip the
// design-system allowlist (`tokens.css`, `tokens.ts`, …) before
// `WorkerTemplate::iter()` ever runs. We keep the **bounded**
// credential excludes here (which do not match design-system
// filenames because they require a separator before the keyword)
// and let the runtime `embed_path_is_allowed` helper enforce the
// final policy via the
// `embed_does_not_carry_generated_or_secret_paths` test. The
// runtime helper is more permissive only for the explicit design-
// system allowlist; everything else stays denied at both layers.
#[exclude = "**/*_token*"]
#[exclude = "**/*-token*"]
#[exclude = "**/*.token*"]
#[exclude = "**/*_secret*"]
#[exclude = "**/*-secret*"]
#[exclude = "**/*.secret*"]
#[exclude = "**/*credential*"]
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

/// Substrings denied wherever they appear in a path segment.
///
/// Codex pass-3 P2 made these strict per the publish.md spec; pass-4
/// P3 relaxed them to allow design-system token assets; pass-5 P1
/// re-tightens them: every segment containing `token`, `secret`, or
/// `credential` is denied UNLESS the entire segment matches one of
/// `EMBED_DENY_FILENAME_DESIGN_ALLOWLIST` (e.g. `tokens.css`,
/// `tokens.ts`). This catches `tokens.json`, `token.txt`,
/// `secrets.json`, etc. while still letting the design package's
/// canonical asset filenames through.
pub const EMBED_DENY_FILENAME_FRAGMENTS: &[&str] = &[
    "token",
    "secret",
    "credential",
];

/// Allowlist of design-system filenames that contain `token` /
/// `secret` substrings but are NOT credentials. Whole-segment
/// match (case-insensitive). Add entries here only after
/// confirming the file is a design-system / styling asset and
/// would never carry secrets.
///
/// Codex pass-5 P1: `tokens.json` is intentionally NOT on this
/// list because a `.json` file with a `token` keyword is more
/// often a credential dump (CI tokens, API tokens) than a
/// design asset. Style-sheet / TypeScript / JavaScript variants
/// of the design tokens stay allowed.
pub const EMBED_DENY_FILENAME_DESIGN_ALLOWLIST: &[&str] = &[
    "tokens.css",
    "tokens.scss",
    "tokens.ts",
    "tokens.tsx",
    "tokens.js",
    "tokens.mjs",
    "design-tokens.css",
    "design-tokens.ts",
];

/// Substrings whose presence at the END of a segment marks it as a
/// credential file. Compared case-insensitively.
pub const EMBED_DENY_FILENAME_SUFFIXES: &[&str] = &[".pem", ".key"];

/// Prefix-matched credential filenames.
///
/// Codex pass-4 P1: an earlier draft used exact-match only (`id_rsa`,
/// `id_rsa.pub`, …). Real SSH keys are commonly named `id_rsa_work`,
/// `id_ed25519_personal`, `id_ecdsa-2024`, etc. Switch to a
/// case-insensitive prefix match so any segment that *starts with*
/// one of these names is denied.
pub const EMBED_DENY_FILENAME_PREFIXES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];

/// Returns true when `relative_path` contains any path segment that
/// the embed must not ship.
///
/// Codex pass-1 P1 (closed): an earlier draft allow-listed any
/// `.env*.example` file alongside `.dev.vars.example`. The publish
/// scaffold only ships ONE example file: `.dev.vars.example`. Any
/// `.env*` file (including `.env.example`) MUST be rejected so a
/// stray template variant in the source tree never lands in a
/// downstream user's repo.
///
/// Codex pass-3 P2 (closed): the credential allowlist was extended
/// to cover `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, and any
/// segment containing `token`, `secret`, or `credential` (case-
/// insensitive). The regression tests pin both the env-file and
/// credential asymmetry.
pub fn embed_path_is_allowed(relative_path: &str) -> bool {
    for segment in relative_path.split('/') {
        for deny in EMBED_DENY_SEGMENTS {
            if segment == *deny {
                return false;
            }
        }
        // Reject every `.env*` segment outright. `.dev.vars.example`
        // is the single scaffold we ship; everything else under the
        // `.env` and `.dev.vars` prefixes is treated as secret.
        if segment.starts_with(".env") {
            return false;
        }
        if segment.starts_with(".dev.vars") && segment != ".dev.vars.example" {
            return false;
        }
        // Credential-name patterns. `to_ascii_lowercase` is fine
        // because we're matching ASCII keywords; non-ASCII filenames
        // pass through this branch and are then re-checked against
        // the deny segments above.
        //
        // Codex pass-5 P1: deny ANY segment that contains
        // `token` / `secret` / `credential` UNLESS the whole segment
        // matches an allowlisted design-system filename. The
        // allowlist is whole-segment so `auth_tokens.css`,
        // `tokens.json` and similar variants stay denied.
        let lower = segment.to_ascii_lowercase();
        let mut matched_fragment = false;
        for needle in EMBED_DENY_FILENAME_FRAGMENTS {
            if lower.contains(needle) {
                matched_fragment = true;
                break;
            }
        }
        if matched_fragment {
            let allowed = EMBED_DENY_FILENAME_DESIGN_ALLOWLIST
                .iter()
                .any(|allowed| lower == *allowed);
            if !allowed {
                return false;
            }
        }
        for suffix in EMBED_DENY_FILENAME_SUFFIXES {
            if lower.ends_with(suffix) {
                return false;
            }
        }
        for prefix in EMBED_DENY_FILENAME_PREFIXES {
            if lower == *prefix || lower.starts_with(&format!("{prefix}.")) || lower.starts_with(&format!("{prefix}_")) || lower.starts_with(&format!("{prefix}-")) {
                return false;
            }
        }
    }
    true
}
