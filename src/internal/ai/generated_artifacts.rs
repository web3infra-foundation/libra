//! Heuristics for identifying machine-generated build artifact directories.
//!
//! AI agents that snapshot a workspace must avoid mistaking compiler output
//! for source. This module centralises the whitelist of directory names the
//! Libra runtime considers "generated" — they are excluded from workspace
//! diffs, agent context windows, and patch sets so the agent never proposes
//! edits inside `target/`, `.gradle/`, `bazel-bin/`, etc.
//!
//! The recognition rules are deliberately path-component aware: e.g. `bin` is
//! a build output only when its parent is *not* `src` (Rust convention places
//! binary entry points at `src/bin/`). All checks are pure path inspection —
//! no filesystem access — so they remain cheap to evaluate inside walk
//! filters.

use std::path::{Component, Path};

/// Returns `true` when the directory at `path` itself looks like a generated
/// build output.
///
/// Functional scope:
/// - Inspects the leaf component of `path` and the *parent* component name to
///   apply context-sensitive rules (notably the `bin` vs `src/bin`
///   distinction).
///
/// Boundary conditions:
/// - Returns `false` when `path` has no UTF-8 file name (non-UTF-8 file names
///   on Windows/Linux are rare but possible; the safe default is to treat them
///   as ordinary directories).
/// - Only the immediate parent name is considered; deeper context such as
///   "bin sitting under crates/foo/src" is approximated by the single-step
///   parent check.
pub(crate) fn is_generated_build_dir_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str());

    is_generated_build_dir_name(name, parent)
}

/// Returns `true` when *any* component of a relative path lies under a
/// generated build directory.
///
/// Functional scope:
/// - Walks the components left-to-right, threading the previous normal-name
///   component as `parent` so each candidate is evaluated with the same
///   contextual rule used by [`is_generated_build_dir_path`].
/// - Skips non-`Normal` components (e.g. `..`, `.`, root, prefix) without
///   advancing `parent`. Hitting an unprintable (non-UTF-8) component resets
///   `parent` so the next normal component is judged in isolation rather than
///   being paired with a stale name.
///
/// Boundary conditions:
/// - Designed for relative paths produced by workspace walks. Absolute paths
///   work but the leading root/prefix components are simply ignored.
/// - Returns `false` for empty paths.
pub(crate) fn relative_path_contains_generated_build_dir(path: &Path) -> bool {
    let mut parent = None;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
            // A non-UTF-8 component blocks contextual reasoning — treat it as
            // an unknown placeholder and re-anchor the parent slot.
            parent = None;
            continue;
        };

        if is_generated_build_dir_name(name, parent) {
            return true;
        }
        parent = Some(name);
    }

    false
}

/// Core rule table for generated-output directory recognition.
///
/// Functional scope:
/// - Matches a fixed set of well-known build directory names across major
///   compiled-language ecosystems (Rust, Go, JVM/Maven/Gradle, .NET, Swift,
///   Zig, CMake, Bazel).
/// - Adds two contextual rules:
///   * `bin` is a build output only when the parent directory is not `src`
///     (Rust's `src/bin/` is source, not output).
///   * Any directory whose name starts with `cmake-build-` (CLion's various
///     configurations) or `bazel-` (Bazel's symlink farm) is treated as
///     generated.
///
/// Boundary conditions:
/// - Comparisons are case-sensitive: a project that names a directory `Build`
///   (capitalised) will not be filtered. This matches conventional naming on
///   Linux/macOS where the standard tool output is lowercase.
/// - The list is intentionally conservative — adding a name here causes silent
///   exclusion from snapshots, so changes should be considered carefully.
fn is_generated_build_dir_name(name: &str, parent: Option<&str>) -> bool {
    matches!(
        name,
        ".build"
            | ".gradle"
            | ".zig-cache"
            | "CMakeFiles"
            | "build"
            | "obj"
            | "out"
            | "target"
            | "zig-out"
    ) || (name == "bin" && parent != Some("src"))
        || name.starts_with("cmake-build-")
        || name.starts_with("bazel-")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: every recognised generated-output path from the major
    /// ecosystems should be identified, including nested ones (e.g.
    /// `rust/target/debug`) and Bazel's flat sibling layout.
    #[test]
    fn detects_common_compiled_language_build_dirs() {
        for path in [
            "target",
            "rust/target/debug",
            "java/build/classes",
            "dotnet/bin/Debug",
            "dotnet/obj",
            "swift/.build/debug",
            "zig/.zig-cache",
            "zig/zig-out/bin",
            "cpp/cmake-build-debug",
            "cpp/CMakeFiles/app.dir",
            "bazel-bin",
            "bazel-out",
            "bazel-testlogs",
        ] {
            assert!(
                relative_path_contains_generated_build_dir(Path::new(path)),
                "{path} should be treated as generated build output"
            );
        }
    }

    /// Scenario: Rust's `src/bin/` convention must not collide with the
    /// generic `bin` build-output rule — both the leaf path and the parent
    /// directory must remain visible.
    #[test]
    fn does_not_treat_rust_src_bin_as_generated_output() {
        assert!(!relative_path_contains_generated_build_dir(Path::new(
            "src/bin/main.rs"
        )));
        assert!(!is_generated_build_dir_path(Path::new("src/bin")));
    }

    /// INVARIANT: each well-known build-output directory name in the
    /// `matches!` table must independently classify as generated when
    /// used as a leaf path. A silent drop from the table would let
    /// the agent see compiler output as ordinary source.
    #[test]
    fn is_generated_build_dir_path_recognises_every_listed_name() {
        for name in [
            ".build",
            ".gradle",
            ".zig-cache",
            "CMakeFiles",
            "build",
            "obj",
            "out",
            "target",
            "zig-out",
        ] {
            assert!(
                is_generated_build_dir_path(Path::new(name)),
                "{name} must be recognised as a generated build dir leaf"
            );
        }
    }

    /// INVARIANT: comparisons are case-sensitive. A `Build/` directory
    /// (capitalised) is the project's own — it must not be silently
    /// excluded from snapshots just because it shares letters with
    /// the lowercase `build` rule.
    #[test]
    fn is_generated_build_dir_name_is_case_sensitive() {
        assert!(is_generated_build_dir_path(Path::new("build")));
        assert!(!is_generated_build_dir_path(Path::new("Build")));
        assert!(!is_generated_build_dir_path(Path::new("BUILD")));
        assert!(!is_generated_build_dir_path(Path::new("Target")));
    }

    /// INVARIANT: bare `bin` is generated unless its parent is `src/`.
    /// This is the asymmetric rule the doc-comment documents — every
    /// other parent (including no parent) must keep the leaf flagged.
    #[test]
    fn bare_bin_is_generated_unless_parent_is_src() {
        assert!(is_generated_build_dir_path(Path::new("bin")));
        assert!(is_generated_build_dir_path(Path::new("project/bin")));
        assert!(is_generated_build_dir_path(Path::new("dotnet/bin")));
        assert!(!is_generated_build_dir_path(Path::new("src/bin")));
        // `bin` nested deeper than `src/` does not get protection —
        // the rule only looks at the immediate parent.
        assert!(is_generated_build_dir_path(Path::new(
            "crates/src/inner/bin"
        )));
    }

    /// INVARIANT: any `cmake-build-*` directory (CLion config dirs)
    /// is generated; a prefix mismatch must not trigger the rule.
    #[test]
    fn cmake_build_prefix_matches_any_suffix() {
        for variant in ["cmake-build-debug", "cmake-build-release", "cmake-build-"] {
            assert!(
                is_generated_build_dir_path(Path::new(variant)),
                "{variant} must match the cmake-build- prefix rule"
            );
        }
        assert!(!is_generated_build_dir_path(Path::new("cmake")));
        assert!(!is_generated_build_dir_path(Path::new("cmake_build_debug")));
        assert!(!is_generated_build_dir_path(Path::new(
            "notcmake-build-debug"
        )));
    }

    /// INVARIANT: any `bazel-*` directory is generated.
    #[test]
    fn bazel_prefix_matches_any_suffix() {
        for variant in ["bazel-bin", "bazel-out", "bazel-testlogs", "bazel-"] {
            assert!(
                is_generated_build_dir_path(Path::new(variant)),
                "{variant} must match the bazel- prefix rule"
            );
        }
        assert!(!is_generated_build_dir_path(Path::new("bazel")));
        assert!(!is_generated_build_dir_path(Path::new("notbazel-bin")));
    }

    /// INVARIANT: paths with no file name (root, prefix-only) must
    /// return false rather than panic. The walk filter relies on
    /// this graceful response.
    #[test]
    fn is_generated_build_dir_path_returns_false_for_paths_without_file_name() {
        assert!(!is_generated_build_dir_path(Path::new("")));
        assert!(!is_generated_build_dir_path(Path::new("/")));
    }

    /// INVARIANT: `relative_path_contains_generated_build_dir` walks
    /// every Normal component, so a nested `target` deep inside the
    /// path must trigger even when the leaf is a regular file.
    #[test]
    fn relative_path_detects_target_in_middle_of_path() {
        assert!(relative_path_contains_generated_build_dir(Path::new(
            "crates/foo/target/debug/build.rs"
        )));
        assert!(relative_path_contains_generated_build_dir(Path::new(
            "a/b/c/.gradle/cache.lock"
        )));
    }

    /// INVARIANT: non-Normal components (`..`, `.`) are skipped
    /// without touching the `parent` slot — only an explicit Normal
    /// component or a non-UTF-8 component changes it. That means a
    /// `bin` reached only after non-Normal components inherits the
    /// last seen Normal name as its parent context.
    #[test]
    fn relative_path_treats_non_normal_components_as_pass_through() {
        assert!(relative_path_contains_generated_build_dir(Path::new(
            "./project/bin"
        )));
        assert!(relative_path_contains_generated_build_dir(Path::new(
            "../sibling/target"
        )));
        // `src/../bin`: components are Normal("src"), ParentDir,
        // Normal("bin"). The ParentDir is skipped without resetting
        // parent, so `bin` is judged with parent=Some("src") and
        // remains protected (treated as Rust's src/bin convention).
        // This documents — rather than corrects — the current
        // pass-through behaviour. Any change must update both the
        // implementation comment and this test.
        assert!(!relative_path_contains_generated_build_dir(Path::new(
            "src/../bin"
        )));
    }

    /// INVARIANT: returns false for an empty path because no Normal
    /// components are present.
    #[test]
    fn relative_path_contains_returns_false_for_empty_path() {
        assert!(!relative_path_contains_generated_build_dir(Path::new("")));
    }

    /// INVARIANT: known-good source layouts must never trigger the
    /// filter — regression here would silently strip the agent's view
    /// of regular project files.
    #[test]
    fn relative_path_does_not_flag_ordinary_source_layouts() {
        for path in [
            "src/main.rs",
            "src/internal/ai/automation/config.rs",
            "tests/command/init_test.rs",
            "docs/improvement/README.md",
        ] {
            assert!(
                !relative_path_contains_generated_build_dir(Path::new(path)),
                "{path} should remain visible"
            );
        }
    }
}
