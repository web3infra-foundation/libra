//! Embedded web UI assets produced by the Next.js static export (`web/out/`).
//!
//! In debug builds `rust-embed` reads from disk at runtime (enabling rapid
//! iteration without recompiling), while release builds embed every file into
//! the binary via `include_bytes!`.

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/out/"]
pub struct WebAssets;
