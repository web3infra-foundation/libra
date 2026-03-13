//! Build script: runs `pnpm run build` inside `web/` to produce the static
//! export that `rust-embed` embeds into the binary.
//!
//! When built under Buck2 (`BUCK_SCRATCH_PATH` is set) and the pre-built
//! `web/out/` directory is already present in the sandbox (materialized by the
//! filegroup), the frontend build is skipped entirely.
//!
//! On a fresh clone, `web/out/` will not be present in the sandbox. In that
//! case the script locates the real project root (by stripping the
//! `buck-out/…` suffix from `CARGO_MANIFEST_DIR`), runs `pnpm run build`
//! there, and copies the resulting `web/out/` tree into the sandbox so that
//! `rust-embed`'s `#[folder = "web/out/"]` (which resolves relative to
//! `CARGO_MANIFEST_DIR`) can find the files.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

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

    if should_skip_web_build() {
        ensure_stub_web_out(&web_dir);
        return;
    }

    if env::var_os("BUCK_SCRATCH_PATH").is_some() {
        // Under Buck2: if web/out/ was already materialized into the sandbox
        // (because it existed in the source tree when the filegroup was
        // analysed), there is nothing to do.
        if web_dir.join("out").join("index.html").exists() {
            return;
        }

        // web/out/ is absent (fresh clone or first build).
        // CARGO_MANIFEST_DIR under Buck2 is a path inside buck-out/:
        //   <project-root>/buck-out/…/__libra-build-script-run__/cwd
        // Recover the real project root by truncating at "/buck-out/".
        let project_web_dir: PathBuf = manifest_dir
            .find("/buck-out/")
            .map(|idx| Path::new(&manifest_dir[..idx]).join("web"))
            .unwrap_or_else(|| web_dir.clone());

        // Build the frontend in the real project source directory.
        run_pnpm_build(&project_web_dir);

        // Copy the freshly-built web/out/ into the sandbox so that
        // rust-embed (which resolves #[folder = "web/out/"] relative to
        // CARGO_MANIFEST_DIR) can embed the assets during this compilation.
        if project_web_dir != web_dir {
            let src = project_web_dir.join("out");
            let dst = web_dir.join("out");
            copy_dir_all(&src, &dst).unwrap_or_else(|err| {
                panic!(
                    "failed to copy `{}` → `{}`: {err}",
                    src.display(),
                    dst.display()
                )
            });
        }
        return;
    }

    // Normal Cargo build: build the frontend directly inside web/.
    run_pnpm_build(&web_dir);
}

fn should_skip_web_build() -> bool {
    match env::var("LIBRA_SKIP_WEB_BUILD") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn ensure_stub_web_out(web_dir: &Path) {
    let out_dir = web_dir.join("out");
    if let Err(err) = fs::create_dir_all(&out_dir) {
        panic!(
            "failed to create fallback frontend output directory `{}`: {err}",
            out_dir.display()
        );
    }

    let index_html = out_dir.join("index.html");
    if !index_html.exists() {
        let fallback = "<!doctype html><html><body>libra web build skipped</body></html>";
        if let Err(err) = fs::write(&index_html, fallback.as_bytes()) {
            panic!(
                "failed to write fallback frontend file `{}`: {err}",
                index_html.display()
            );
        }
    }
}

/// Runs `pnpm install` (if needed) then `pnpm run build` inside `web_dir`.
fn run_pnpm_build(web_dir: &Path) {
    // Install dependencies if node_modules is missing (e.g. fresh clone).
    if !web_dir.join("node_modules").exists() {
        let install = Command::new("pnpm")
            .arg("install")
            .arg("--frozen-lockfile")
            .current_dir(web_dir)
            .status()
            .expect("failed to execute `pnpm install` — is pnpm installed?");

        if !install.success() {
            panic!("`pnpm install` failed (exit code {:?})", install.code());
        }
    }

    let status = Command::new("pnpm")
        .arg("run")
        .arg("build")
        .current_dir(web_dir)
        .status()
        .expect("failed to execute `pnpm run build` — is pnpm installed?");

    if !status.success() {
        panic!("frontend build failed (exit code {:?})", status.code());
    }
}

/// Recursively copies a directory tree from `src` to `dst`.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
