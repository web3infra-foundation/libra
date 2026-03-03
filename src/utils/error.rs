//! User-facing error output utilities.
//!
//! Provides the [`cli_error!`] macro that conditionally includes internal
//! error details based on the build profile:
//!
//! - **Debug** builds (`cargo build`): includes the `Debug` representation
//!   of the error so developers can see the full cause chain.
//! - **Release** builds (`cargo build --release`): shows only the
//!   user-friendly message, with no internal implementation details.
//!
//! # Usage
//!
//! ```ignore
//! // Pattern A – error *is* the message (Display vs Debug):
//! //   Release → "fatal: the repository is already initialized at '...'"
//! //   Debug   → "fatal: Io(Custom { kind: AlreadyExists, … })"
//! cli_error!("fatal" => e);
//!
//! // Pattern B – fixed message with hidden error cause (error first):
//! //   Release → "fatal: failed to load commit"
//! //   Debug   → "fatal: failed to load commit: InvalidCommitObject"
//! cli_error!(e, "fatal: failed to load commit");
//! cli_error!(e, "fatal: invalid url '{}'", url);
//! ```

/// Print a user-facing error to stderr with build-profile–aware detail.
///
/// See the [module-level documentation](self) for examples.
#[macro_export]
macro_rules! cli_error {
    // ── Pattern A ──────────────────────────────────────────────────
    // The error's own message IS the user-facing text.
    //   Release  →  eprintln!("{prefix}: {e}")       (Display)
    //   Debug    →  eprintln!("{prefix}: {e:?}")      (Debug)
    //
    // NOTE: In debug builds the output uses `{:?}` (Debug), not `{}`
    // (Display), so you will see the Rust struct representation rather
    // than the human-readable message. This is intentional — it surfaces
    // the full cause chain for developers.
    ($prefix:expr => $err:expr) => {{
        #[cfg(debug_assertions)]
        eprintln!("{}: {:?}", $prefix, $err);
        #[cfg(not(debug_assertions))]
        eprintln!("{}: {}", $prefix, $err);
    }};

    // ── Pattern B ──────────────────────────────────────────────────
    // Error cause first, then a human-readable message.
    // The cause is appended only in debug builds.
    //   Release  →  eprintln!(msg, args…)
    //   Debug    →  eprint!(msg, args…) + eprintln!(": {:?}", err)
    ($err:expr, $($arg:tt)+) => {{
        #[cfg(debug_assertions)]
        {
            eprint!($($arg)+);
            eprintln!(": {:?}", $err);
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = &$err;
            eprintln!($($arg)+);
        }
    }};
}

#[cfg(test)]
mod tests {
    use std::io;

    /// Verify Pattern A compiles with a std::io::Error (Display + Debug).
    #[test]
    fn pattern_a_compiles_with_io_error() {
        let err = io::Error::new(io::ErrorKind::NotFound, "gone");
        cli_error!("fatal" => err);
    }

    /// Verify Pattern B compiles with a plain message.
    #[test]
    fn pattern_b_compiles_with_message() {
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "no access");
        cli_error!(err, "fatal: failed to open file");
    }

    /// Verify Pattern B compiles with format arguments.
    #[test]
    fn pattern_b_compiles_with_format_args() {
        let err = io::Error::new(io::ErrorKind::InvalidInput, "bad path");
        let path = "/tmp/test";
        cli_error!(err, "fatal: cannot read '{}'", path);
    }

    /// Verify Pattern B compiles with non-error types (e.g. String).
    #[test]
    fn pattern_b_compiles_with_string() {
        let detail = String::from("ng refs/heads/main non-fast-forward");
        cli_error!(detail, "fatal: ref update failed");
    }
}
