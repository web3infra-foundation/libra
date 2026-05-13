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
}
