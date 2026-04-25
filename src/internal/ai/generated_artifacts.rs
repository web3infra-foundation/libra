use std::path::{Component, Path};

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

pub(crate) fn relative_path_contains_generated_build_dir(path: &Path) -> bool {
    let mut parent = None;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
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

    #[test]
    fn does_not_treat_rust_src_bin_as_generated_output() {
        assert!(!relative_path_contains_generated_build_dir(Path::new(
            "src/bin/main.rs"
        )));
        assert!(!is_generated_build_dir_path(Path::new("src/bin")));
    }
}
