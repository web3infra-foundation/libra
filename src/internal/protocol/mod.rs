//! Protocol abstraction for Git transport with shared advertisement parsing and traits implemented by HTTPS, local, and LFS clients.

use std::cell::RefCell;

use bytes::{Bytes, BytesMut};
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash},
};
use url::Url;

use crate::git_protocol::{ServiceType, add_pkt_line_string, read_pkt_line};

pub mod git_client; // to support git server protocol (git://) over TCP
pub mod https_client;
pub mod lfs_client;
pub mod local_client;

#[allow(dead_code)] // todo: unimplemented
pub trait ProtocolClient {
    /// create client from url
    fn from_url(url: &Url) -> Self;
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredReference {
    pub(crate) _hash: String,
    pub(crate) _ref: String,
}

pub type DiscRef = DiscoveredReference;

pub type FetchStream = futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>;

thread_local! {
    static WIRE_HASH_KIND: RefCell<HashKind> = RefCell::new(HashKind::default());
}

pub fn set_wire_hash_kind(kind: HashKind) {
    WIRE_HASH_KIND.with(|k| {
        *k.borrow_mut() = kind;
    });
}

pub fn get_wire_hash_kind() -> HashKind {
    WIRE_HASH_KIND.with(|k| *k.borrow())
}

/// Result of reference discovery containing refs, capabilities, and hash kind.
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    pub refs: Vec<DiscRef>,
    pub capabilities: Vec<String>,
    pub hash_kind: HashKind,
}

/// Parse discovered references from Git protocol advertisement response.
pub fn parse_discovered_references(
    mut response_content: Bytes,
    service: ServiceType,
) -> Result<DiscoveryResult, GitError> {
    let mut ref_list = Vec::new(); // refs
    let mut capabilities = Vec::new(); // capabilities
    let mut saw_header = false; // header seen or not
    let mut processed_first_ref = false;
    let mut hash_kind = HashKind::Sha1;
    // Closure to parse hash kind based on length
    let parse_hash_kind = |hash: &str| match hash.len() {
        40 => Ok(HashKind::Sha1),
        64 => Ok(HashKind::Sha256),
        _ => Err(GitError::NetworkError(format!(
            "Invalid hash length {}, expected 40 or 64",
            hash.len()
        ))),
    };

    loop {
        let (bytes_take, pkt_line) = read_pkt_line(&mut response_content);
        if bytes_take == 0 {
            if response_content.is_empty() {
                break;
            } else {
                continue;
            }
        }

        if !saw_header && pkt_line.starts_with(b"# service=") {
            let header = String::from_utf8(pkt_line.to_vec()).map_err(|e| {
                GitError::NetworkError(format!("Invalid UTF-8 in response header: {}", e))
            })?;
            tracing::debug!("discovery header: {header:?}");
            saw_header = true;
            continue;
        }
        saw_header = true;

        let pkt_line = String::from_utf8(pkt_line.to_vec())
            .map_err(|e| GitError::NetworkError(format!("Invalid UTF-8 in response: {}", e)))?;
        let (hash, rest) = pkt_line.split_once(' ').ok_or_else(|| {
            GitError::NetworkError("Invalid reference format, missing object id".to_string())
        })?;
        let detected_kind = parse_hash_kind(hash)?;
        if !processed_first_ref {
            hash_kind = detected_kind;
        } else if detected_kind != hash_kind {
            return Err(GitError::NetworkError(format!(
                "Hash kind mismatch: expected {hash_kind}, got length {}",
                hash.len()
            )));
        }

        let rest = rest.trim();

        if !processed_first_ref {
            let (reference, caps) = match rest.split_once('\0') {
                Some((r, c)) => (r, c),
                None => (rest, ""),
            };
            if !caps.is_empty() {
                capabilities = caps
                    .split(' ')
                    .filter(|cap| !cap.is_empty())
                    .map(|cap| cap.to_string())
                    .collect();
                if let Some(format_cap) = capabilities
                    .iter()
                    .find(|cap| cap.starts_with("object-format="))
                {
                    let format_kind = match format_cap.as_str() {
                        "object-format=sha1" => HashKind::Sha1,
                        "object-format=sha256" => HashKind::Sha256,
                        other => {
                            return Err(GitError::NetworkError(format!(
                                "Unsupported object format capability: {other}"
                            )));
                        }
                    };
                    if format_kind != detected_kind {
                        return Err(GitError::NetworkError(format!(
                            "Object format mismatch: advertised {format_kind}, got hash length {}",
                            hash.len()
                        )));
                    }
                    hash_kind = format_kind;
                }
            }

            if hash == ObjectHash::zero_str(hash_kind) {
                tracing::debug!(
                    "discovery for {:?} returned zero hash, treating as empty repository",
                    service
                );
                break;
            }

            if reference != "capabilities^{}" {
                ref_list.push(DiscoveredReference {
                    _hash: hash.to_string(),
                    _ref: reference.to_string(),
                });
            }
            if !caps.is_empty() {
                let caps = caps.split(' ').collect::<Vec<&str>>();
                tracing::debug!("capability declarations: {:?}", caps);
            }
            processed_first_ref = true;
        } else {
            ref_list.push(DiscoveredReference {
                _hash: hash.to_string(),
                _ref: rest.to_string(),
            });
        }
    }

    Ok(DiscoveryResult {
        refs: ref_list,
        capabilities,
        hash_kind,
    })
}

pub fn generate_upload_pack_content(
    have: &[String],
    want: &[String],
    depth: Option<usize>,
) -> Bytes {
    let mut buf = BytesMut::new();
    let mut write_first_line = false;

    let mut capability = vec!["side-band-64k", "ofs-delta", "multi_ack_detailed"];
    if get_wire_hash_kind() == HashKind::Sha256 {
        capability.push("object-format=sha256");
    }
    let capability = capability.join(" ");
    for w in want {
        if !write_first_line {
            add_pkt_line_string(
                &mut buf,
                format!("want {w} {capability} agent=libra/0.1.0\n").to_string(),
            );
            write_first_line = true;
        } else {
            add_pkt_line_string(&mut buf, format!("want {w}\n").to_string());
        }
    }

    // Add deepen line if depth is specified
    if let Some(d) = depth {
        add_pkt_line_string(&mut buf, format!("deepen {d}\n").to_string());
    }

    buf.extend(b"0000");
    for h in have {
        add_pkt_line_string(&mut buf, format!("have {h}\n").to_string());
    }

    add_pkt_line_string(&mut buf, "done\n".to_string());

    buf.freeze()
}

#[cfg(test)]
mod test {}
