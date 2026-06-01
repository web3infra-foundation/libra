//! CEX-S2-13 完成判定 (2) semantic conflict detection over a `MergeCandidate`'s
//! aggregated sub-agent patches.
//!
//! Two sub-agent patches conflict when they cannot be cleanly merged without a
//! human decision. This module implements the three documented detection rules
//! and emits the CEX-S2-13-frozen [`Conflict`] records (the value-filling job
//! that CEX-S2-15 owns over the schema CEX-S2-13 froze in [`super::decision`]):
//!
//! 1. **Overlapping hunk** — two patches modify intersecting line ranges of the
//!    same file (`overlapping_hunk`).
//! 2. **Same symbol** — two patches modify the same named symbol in the same
//!    file (`same_symbol`).
//! 3. **Non-mergeable file cross-edit** — two patches both edit the same file
//!    of a class that does not auto-merge (lockfiles, dependency manifests,
//!    tests, CI/pipeline config), regardless of hunk overlap
//!    (`non_mergeable_cross_edit`).
//!
//! The function is **pure**. The future ValidatorEngine normalises each
//! git-internal `PatchSet` — `diffy` hunks plus a tree-sitter symbol scan of the
//! changed regions — into the [`PatchTouch`] shape; detection has no I/O and
//! does not mutate the frozen schema. Output order is deterministic: patches are
//! compared pairwise in index order, files in first-patch order, and the rules
//! are emitted overlapping-hunk → same-symbol → cross-edit.

use std::collections::BTreeSet;

use super::{AgentPatchSetId, decision::Conflict};

/// Inclusive 1-based line range of one modified hunk in a file's new image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkRange {
    pub start: u32,
    pub end: u32,
}

impl HunkRange {
    /// Two ranges overlap when neither lies entirely before the other
    /// (touching endpoints count as overlap: `1-3` overlaps `3-5`).
    fn overlaps(&self, other: &HunkRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// One file edited by a single patch: the modified hunk ranges and the symbols
/// it touches. Assumed normalised — at most one entry per path per patch, no
/// duplicate symbols.
#[derive(Clone, Debug)]
pub struct PatchFileEdit {
    pub path: String,
    pub hunks: Vec<HunkRange>,
    pub symbols: Vec<String>,
}

/// Every file one sub-agent [`AgentPatchSet`](super::AgentPatchSet) edits.
#[derive(Clone, Debug)]
pub struct PatchTouch {
    pub patchset_id: AgentPatchSetId,
    pub files: Vec<PatchFileEdit>,
}

/// Detect semantic merge conflicts across a candidate's patches
/// (CEX-S2-13 完成判定 (2)). Returns one [`Conflict`] per distinct finding;
/// an empty slice (or a single patch) never conflicts.
pub fn detect_conflicts(patches: &[PatchTouch]) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    for (i, left) in patches.iter().enumerate() {
        for right in &patches[i + 1..] {
            detect_pair(left, right, &mut conflicts);
        }
    }
    conflicts
}

/// Emit the conflicts between one ordered pair of patches.
fn detect_pair(left: &PatchTouch, right: &PatchTouch, out: &mut Vec<Conflict>) {
    for lf in &left.files {
        for rf in &right.files {
            if lf.path != rf.path {
                continue;
            }

            // Rule 1: overlapping hunk.
            if let Some((lh, rh)) = first_overlap(&lf.hunks, &rf.hunks) {
                out.push(Conflict {
                    kind: "overlapping_hunk".to_string(),
                    path: lf.path.clone(),
                    detail: Some(format!(
                        "lines {}-{} overlap {}-{}",
                        lh.start, lh.end, rh.start, rh.end
                    )),
                });
            }

            // Rule 2: same symbol (in first-patch symbol order).
            let right_symbols: BTreeSet<&str> = rf.symbols.iter().map(String::as_str).collect();
            for symbol in &lf.symbols {
                if right_symbols.contains(symbol.as_str()) {
                    out.push(Conflict {
                        kind: "same_symbol".to_string(),
                        path: lf.path.clone(),
                        detail: Some(symbol.clone()),
                    });
                }
            }

            // Rule 3: both patches edit the same non-mergeable file.
            if let Some(class) = non_mergeable_class(&lf.path) {
                out.push(Conflict {
                    kind: "non_mergeable_cross_edit".to_string(),
                    path: lf.path.clone(),
                    detail: Some(class.to_string()),
                });
            }
        }
    }
}

/// The first overlapping `(left, right)` hunk pair, scanning left hunks then
/// right hunks, or `None` if no hunks intersect.
fn first_overlap(left: &[HunkRange], right: &[HunkRange]) -> Option<(HunkRange, HunkRange)> {
    for lh in left {
        for rh in right {
            if lh.overlaps(rh) {
                return Some((*lh, *rh));
            }
        }
    }
    None
}

/// Classify a path into a non-auto-mergeable file class, or `None` for an
/// ordinary source file. Conservative and extensible: lockfiles and dependency
/// manifests are matched by exact basename, tests by the common path/basename
/// conventions. New ecosystems are added by extending the matchers below.
fn non_mergeable_class(path: &str) -> Option<&'static str> {
    let basename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if is_lockfile(basename) {
        Some("lockfile")
    } else if is_dependency_manifest(basename) {
        Some("manifest")
    } else if is_test_path(path, basename) {
        Some("test")
    } else if is_ci_config(path, basename) {
        // CI / pipeline config: concurrent edits routinely collide on the same
        // workflow steps (Step 2.5 narrative "测试/配置/lockfile 交叉修改").
        Some("config")
    } else {
        None
    }
}

/// CI / pipeline configuration whose concurrent edits routinely collide. Matched
/// by the well-known basenames and the `.github/workflows/` directory; kept
/// conservative so ordinary app config (e.g. an arbitrary `config.toml`) is not
/// swept in.
fn is_ci_config(path: &str, basename: &str) -> bool {
    let in_github_workflows = path
        .split(['/', '\\'])
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == ".github" && w[1] == "workflows");
    in_github_workflows
        || matches!(
            basename,
            ".gitlab-ci.yml"
                | ".gitlab-ci.yaml"
                | ".travis.yml"
                | "azure-pipelines.yml"
                | "Jenkinsfile"
                | ".circleci"
        )
        || path.contains(".circleci/")
}

/// Generated dependency lockfiles — these never merge cleanly.
fn is_lockfile(basename: &str) -> bool {
    matches!(
        basename,
        "Cargo.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "go.sum"
            | "poetry.lock"
            | "Pipfile.lock"
            | "Gemfile.lock"
            | "composer.lock"
    )
}

/// Dependency manifests whose concurrent edits routinely collide.
fn is_dependency_manifest(basename: &str) -> bool {
    matches!(
        basename,
        "Cargo.toml"
            | "package.json"
            | "go.mod"
            | "pyproject.toml"
            | "requirements.txt"
            | "Gemfile"
            | "pom.xml"
            | "build.gradle"
    )
}

/// Test sources, matched by the common directory and basename conventions.
fn is_test_path(path: &str, basename: &str) -> bool {
    let in_test_dir = path
        .split(['/', '\\'])
        .any(|segment| matches!(segment, "tests" | "test" | "__tests__" | "spec" | "specs"));
    let suffix_marked = basename.ends_with("_test.rs")
        || basename.ends_with("_test.go")
        || basename.ends_with("_spec.rb")
        || basename.starts_with("test_");
    // JavaScript / TypeScript tests: any `.test.` / `.spec.` infix in front of
    // a JS/TS extension (covers ts, tsx, js, jsx, mjs, cjs).
    let js_ts_marked = (basename.contains(".test.") || basename.contains(".spec."))
        && [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
            .iter()
            .any(|ext| basename.ends_with(ext));
    in_test_dir || suffix_marked || js_ts_marked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(start: u32, end: u32) -> HunkRange {
        HunkRange { start, end }
    }

    fn patch(files: Vec<PatchFileEdit>) -> PatchTouch {
        PatchTouch {
            patchset_id: AgentPatchSetId::new(),
            files,
        }
    }

    fn edit(path: &str, hunks: Vec<HunkRange>, symbols: &[&str]) -> PatchFileEdit {
        PatchFileEdit {
            path: path.to_string(),
            hunks,
            symbols: symbols.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// No conflict for an empty set or a single patch.
    #[test]
    fn empty_or_single_patch_never_conflicts() {
        assert!(detect_conflicts(&[]).is_empty());
        let single = patch(vec![edit("src/a.rs", vec![hunk(1, 10)], &["foo"])]);
        assert!(detect_conflicts(std::slice::from_ref(&single)).is_empty());
    }

    /// Disjoint hunks on the same ordinary file do not conflict.
    #[test]
    fn disjoint_hunks_same_file_do_not_conflict() {
        let a = patch(vec![edit("src/a.rs", vec![hunk(1, 5)], &[])]);
        let b = patch(vec![edit("src/a.rs", vec![hunk(10, 20)], &[])]);
        assert!(detect_conflicts(&[a, b]).is_empty());
    }

    /// Edits to different files do not conflict.
    #[test]
    fn different_files_do_not_conflict() {
        let a = patch(vec![edit("src/a.rs", vec![hunk(1, 5)], &["foo"])]);
        let b = patch(vec![edit("src/b.rs", vec![hunk(1, 5)], &["foo"])]);
        assert!(detect_conflicts(&[a, b]).is_empty());
    }

    /// Rule 1: overlapping (including endpoint-touching) hunks on the same file
    /// produce one `overlapping_hunk` conflict with the overlapping ranges.
    #[test]
    fn overlapping_hunks_conflict() {
        let a = patch(vec![edit("src/a.rs", vec![hunk(1, 3)], &[])]);
        let b = patch(vec![edit("src/a.rs", vec![hunk(3, 8)], &[])]);
        let conflicts = detect_conflicts(&[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, "overlapping_hunk");
        assert_eq!(conflicts[0].path, "src/a.rs");
        assert_eq!(
            conflicts[0].detail.as_deref(),
            Some("lines 1-3 overlap 3-8")
        );
    }

    /// Rule 2: the same symbol modified by both patches yields a `same_symbol`
    /// conflict per shared symbol, in first-patch order; unshared symbols are
    /// ignored.
    #[test]
    fn same_symbol_conflicts_per_shared_symbol() {
        let a = patch(vec![edit(
            "src/a.rs",
            vec![hunk(1, 5)],
            &["foo", "bar", "baz"],
        )]);
        let b = patch(vec![edit(
            "src/a.rs",
            vec![hunk(40, 50)],
            &["bar", "foo", "qux"],
        )]);
        let conflicts = detect_conflicts(&[a, b]);
        // Disjoint hunks (no rule 1); shared symbols foo & bar in first-patch order.
        assert_eq!(
            conflicts
                .iter()
                .map(|c| (c.kind.as_str(), c.detail.as_deref()))
                .collect::<Vec<_>>(),
            vec![("same_symbol", Some("foo")), ("same_symbol", Some("bar")),],
        );
        assert!(conflicts.iter().all(|c| c.path == "src/a.rs"));
    }

    /// Rule 3: two patches editing the same lockfile conflict even with
    /// disjoint hunks and no shared symbols.
    #[test]
    fn lockfile_cross_edit_conflicts_without_overlap() {
        let a = patch(vec![edit("Cargo.lock", vec![hunk(1, 2)], &[])]);
        let b = patch(vec![edit("Cargo.lock", vec![hunk(80, 90)], &[])]);
        let conflicts = detect_conflicts(&[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, "non_mergeable_cross_edit");
        assert_eq!(conflicts[0].path, "Cargo.lock");
        assert_eq!(conflicts[0].detail.as_deref(), Some("lockfile"));
    }

    /// Rule 3 end-to-end for the test-file class: two patches editing the same
    /// test source conflict with `detail = "test"`, even with disjoint hunks.
    #[test]
    fn test_file_cross_edit_conflicts() {
        let a = patch(vec![edit("src/foo_test.rs", vec![hunk(1, 4)], &[])]);
        let b = patch(vec![edit("src/foo_test.rs", vec![hunk(20, 30)], &[])]);
        let conflicts = detect_conflicts(&[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, "non_mergeable_cross_edit");
        assert_eq!(conflicts[0].path, "src/foo_test.rs");
        assert_eq!(conflicts[0].detail.as_deref(), Some("test"));
    }

    /// A non-mergeable file edited with overlapping hunks reports both the
    /// overlapping-hunk and the cross-edit conflict (distinct concerns).
    #[test]
    fn non_mergeable_overlap_reports_both_rules() {
        let a = patch(vec![edit("package.json", vec![hunk(1, 5)], &[])]);
        let b = patch(vec![edit("package.json", vec![hunk(4, 9)], &[])]);
        let conflicts = detect_conflicts(&[a, b]);
        let kinds: Vec<&str> = conflicts.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(kinds, vec!["overlapping_hunk", "non_mergeable_cross_edit"]);
    }

    /// The non-mergeable classifier recognises lockfiles, manifests and tests
    /// across directories, and leaves ordinary source files unclassified.
    #[test]
    fn non_mergeable_class_matrix() {
        assert_eq!(non_mergeable_class("a/b/Cargo.lock"), Some("lockfile"));
        assert_eq!(non_mergeable_class("web/pnpm-lock.yaml"), Some("lockfile"));
        assert_eq!(non_mergeable_class("Cargo.toml"), Some("manifest"));
        assert_eq!(non_mergeable_class("web/package.json"), Some("manifest"));
        assert_eq!(non_mergeable_class("tests/foo.rs"), Some("test"));
        assert_eq!(non_mergeable_class("src/foo_test.rs"), Some("test"));
        assert_eq!(non_mergeable_class("web/x.test.tsx"), Some("test"));
        assert_eq!(non_mergeable_class("web/x.spec.js"), Some("test"));
        assert_eq!(non_mergeable_class("web/x.test.jsx"), Some("test"));
        assert_eq!(non_mergeable_class("web/x.spec.jsx"), Some("test"));
        assert_eq!(non_mergeable_class("a/__tests__/x.ts"), Some("test"));
        // CI / pipeline config (Step 2.5 narrative "配置...交叉修改").
        assert_eq!(
            non_mergeable_class(".github/workflows/ci.yml"),
            Some("config"),
        );
        assert_eq!(
            non_mergeable_class("repo/.github/workflows/release.yaml"),
            Some("config"),
        );
        assert_eq!(non_mergeable_class(".gitlab-ci.yml"), Some("config"));
        assert_eq!(non_mergeable_class("Jenkinsfile"), Some("config"));
        assert_eq!(non_mergeable_class(".circleci/config.yml"), Some("config"));
        assert_eq!(non_mergeable_class("src/testing.rs"), None);
        assert_eq!(non_mergeable_class("src/internal/foo.rs"), None);
        assert_eq!(non_mergeable_class("README.md"), None);
        // Ordinary app config is NOT swept in — only CI/pipeline config.
        assert_eq!(non_mergeable_class("src/config.toml"), None);
        assert_eq!(non_mergeable_class("settings.yaml"), None);
    }

    /// Conflicts are aggregated across every patch pair, deterministically.
    #[test]
    fn three_patches_compare_pairwise() {
        let a = patch(vec![edit("src/a.rs", vec![hunk(1, 5)], &["foo"])]);
        let b = patch(vec![edit("src/a.rs", vec![hunk(3, 9)], &["foo"])]);
        let c = patch(vec![edit("src/a.rs", vec![hunk(100, 110)], &["foo"])]);
        let conflicts = detect_conflicts(&[a, b, c]);
        // (a,b): overlap + same symbol; (a,c): same symbol; (b,c): same symbol.
        assert_eq!(
            conflicts
                .iter()
                .map(|c| c.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "overlapping_hunk",
                "same_symbol",
                "same_symbol",
                "same_symbol",
            ],
        );
    }
}
