//! Build script: runs `pnpm run build` inside `web/` to produce the static
//! export that `rust-embed` embeds into the binary.
//!

use std::{env, fs, path::Path, process::Command};

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

    // Re-run this build script when relevant environment variables change.
    println!("cargo:rerun-if-env-changed=LIBRA_PNPM");
    println!("cargo:rerun-if-env-changed=LIBRA_SKIP_WEB_BUILD");

    if should_skip_web_build() {
        ensure_stub_web_out(&web_dir);
        return;
    }

    // Normal Cargo build: build the frontend directly inside web/.
    run_pnpm_build(&web_dir);
}

fn pnpm_executable() -> String {
    if let Ok(value) = env::var("LIBRA_PNPM") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if cfg!(windows) {
        "pnpm.cmd".to_string()
    } else {
        "pnpm".to_string()
    }
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
    let pnpm = pnpm_executable();

    // Install dependencies if node_modules is missing (e.g. fresh clone).
    if !web_dir.join("node_modules").exists() {
        let install = Command::new(&pnpm)
            .arg("install")
            .arg("--frozen-lockfile")
            .current_dir(web_dir)
            .status()
            .expect("failed to execute `pnpm install` — is pnpm installed?");

        if !install.success() {
            panic!("`pnpm install` failed (exit code {:?})", install.code());
        }
    }

    let status = Command::new(&pnpm)
        .arg("run")
        .arg("build")
        .current_dir(web_dir)
        .status()
        .expect("failed to execute `pnpm run build` — is pnpm installed?");

    if !status.success() {
        panic!("frontend build failed (exit code {:?})", status.code());
    }
}
