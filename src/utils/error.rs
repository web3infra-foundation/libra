//! User-facing error output utilities.
//!
//! Provides the [`cli_error!`] macro for printing user-facing error messages
//! to stderr.
//!
//! - **Pattern A** always uses the `Display` representation so that users
//!   see the human-readable message from `thiserror` / `std::fmt::Display`.
//! - **Pattern B** appends the `Debug` representation of the underlying
//!   error cause in debug builds only, while always printing the
//!   human-readable message.
//!
//! # Usage
//!
//! ```ignore
//! // Pattern A – error's Display message IS the user-facing text:
//! //   "fatal: the repository is already initialized at '...'"
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
    // The error's own Display message IS the user-facing text.
    // Always uses `{}` (Display) so the output is human-readable.
    // Developers who need the full cause chain should use tracing or
    // RUST_LOG.
    ($prefix:expr => $err:expr) => {{
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
