//! Build script: runs `pnpm run build` inside `web/` to produce the static
//! export that `rust-embed` embeds into the binary.
//!
//! When built under Buck2 (`BUCK_SCRATCH_PATH` is set), the frontend build is
//! skipped because Buck2's `filegroup` already includes the pre-built `web/out/`
//! directory and its sandboxed PATH does not contain Node.js tooling.

use std::{env, path::Path, process::Command};

fn main() {
    let manifest_dir = match env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!(
                "build.rs: failed to read CARGO_MANIFEST_DIR environment variable: {err}. \
                 This is required to locate the `web/` frontend directory."
            );
            std::process::exit(1);
        }
    };
    let web_dir = Path::new(&manifest_dir).join("web");

    // Re-run this build script when any web source file changes.
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/public");
    println!("cargo:rerun-if-changed=web/next.config.ts");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/pnpm-lock.yaml");
    println!("cargo:rerun-if-changed=web/tsconfig.json");
    println!("cargo:rerun-if-changed=web/tailwind.config.ts");

    // Under Buck2 the sandbox already contains web/out/ from the filegroup,
    // and Node.js tooling is not available. Skip the frontend build.
    if env::var_os("BUCK_SCRATCH_PATH").is_some() {
        let web_out_index = web_dir.join("out").join("index.html");
        if !web_out_index.exists() {
            panic!(
                "BUCK_SCRATCH_PATH is set, but `{}` does not exist.\n\
                 Buck2 is expected to materialize `web/out/` (e.g. via a filegroup).\n\
                 Ensure the Buck2 rule exports the pre-built frontend into `web/out/` \
                 before running this build.",
                web_out_index.display()
            );
        }
        return;
    }

    // Install dependencies if node_modules is missing (e.g. fresh clone).
    if !web_dir.join("node_modules").exists() {
        let install = Command::new("pnpm")
            .arg("install")
            .arg("--frozen-lockfile")
            .current_dir(&web_dir)
            .status()
            .expect("failed to execute `pnpm install` — is pnpm installed?");

        if !install.success() {
            panic!("`pnpm install` failed (exit code {:?})", install.code());
        }
    }

    let status = Command::new("pnpm")
        .arg("run")
        .arg("build")
        .current_dir(&web_dir)
        .status()
        .expect("failed to execute `pnpm run build` — is pnpm installed?");

    if !status.success() {
        panic!("frontend build failed (exit code {:?})", status.code());
    }
}
