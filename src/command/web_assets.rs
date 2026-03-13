//! Embedded web UI assets produced by the Next.js static export (`web/out/`).
//!
//! `rust-embed` embeds these files into the binary (using an `include_bytes!`-like
//! mechanism) so the web UI is available at runtime without accessing the
//! filesystem. See the `rust-embed` crate documentation for configuration options.

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/out/"]
pub struct WebAssets;
