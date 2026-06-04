//! Protocol abstraction for Git transport with shared advertisement parsing and traits implemented by HTTPS, local, and LFS clients.

use std::cell::RefCell;

use bytes::{Bytes, BytesMut};
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash},
};
use url::Url;

use crate::{
    git_protocol::{ServiceType, add_pkt_line_string, read_pkt_line},
    internal::branch::Branch,
};

pub mod clone_support; // local-object reuse helpers for `clone --reference`/`--shared`/`--local`
pub mod git_client; // to support git server protocol (git://) over TCP
pub mod https_client;
pub mod lfs_client;
pub mod local_client;
pub mod ssh_client; // to support SSH transport (ssh:// and git@host:path)

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

impl DiscoveredReference {
    pub fn hash(&self) -> &str {
        &self._hash
    }

    pub fn name(&self) -> &str {
        &self._ref
    }
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

/// Advanced shallow-clone negotiation parameters carried alongside the existing
/// `shallow` boundary set. Models the three Git upload-pack deepen requests:
///
/// - [`depth`](Self::depth) → `deepen <n>` (`--depth N` / `--deepen N`)
/// - [`deepen_since`](Self::deepen_since) → `deepen-since <unix>` (`--shallow-since`)
/// - [`deepen_not`](Self::deepen_not) → one `deepen-not <ref>` per entry (`--shallow-exclude`)
///
/// Git accepts `--depth` combined with `--shallow-since`/`--shallow-exclude`, so
/// these are layered into the request rather than treated as mutually exclusive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShallowOptions {
    /// Truncate history to this many commits from each tip (`deepen <n>`).
    pub depth: Option<usize>,
    /// Restrict shallow history to commits newer than this Unix timestamp
    /// (`deepen-since <unix-seconds>`).
    pub deepen_since: Option<i64>,
    /// Exclude history reachable from these refs/revisions (`deepen-not <ref>`).
    pub deepen_not: Vec<String>,
}

impl ShallowOptions {
    /// Convenience constructor for a plain `--depth` request.
    pub fn from_depth(depth: Option<usize>) -> Self {
        Self {
            depth,
            ..Self::default()
        }
    }

    /// Whether any shallow-shaping request is present.
    pub fn is_requested(&self) -> bool {
        self.depth.is_some() || self.deepen_since.is_some() || !self.deepen_not.is_empty()
    }
}

pub fn generate_upload_pack_content(
    have: &[String],
    want: &[String],
    shallow: &[String],
    options: &ShallowOptions,
) -> Bytes {
    let mut buf = BytesMut::new();
    let mut write_first_line = false;

    let mut capability = vec!["side-band-64k", "multi_ack_detailed"];
    if get_wire_hash_kind() == HashKind::Sha256 {
        capability.push("object-format=sha256");
    }
    // `deepen-since`/`deepen-not` commands are only honored by upload-pack when
    // the corresponding capability is advertised on the first `want` line; a
    // plain `deepen <n>` does not need one.
    if options.deepen_since.is_some() {
        capability.push("deepen-since");
    }
    if !options.deepen_not.is_empty() {
        capability.push("deepen-not");
    }
    let capability = capability.join(" ");
    for w in want {
        if !write_first_line {
            add_pkt_line_string(
                &mut buf,
                format!(
                    "want {w} {capability} agent=libra/{}\n",
                    env!("CARGO_PKG_VERSION")
                )
                .to_string(),
            );
            write_first_line = true;
        } else {
            add_pkt_line_string(&mut buf, format!("want {w}\n").to_string());
        }
    }

    for oid in shallow {
        add_pkt_line_string(&mut buf, format!("shallow {oid}\n"));
    }

    // Git's upload-pack rejects `deepen` combined with `deepen-since`/`deepen-not`
    // ("deepen and deepen-since (or deepen-not) cannot be used together"), so the
    // time/ref-based requests take precedence over a plain commit-count depth when
    // both are supplied. This keeps `--depth` + `--shallow-since`/`--shallow-exclude`
    // accepted at the CLI while still producing a protocol-valid request.
    if options.deepen_since.is_some() || !options.deepen_not.is_empty() {
        if let Some(since) = options.deepen_since {
            add_pkt_line_string(&mut buf, format!("deepen-since {since}\n").to_string());
        }
        for reference in &options.deepen_not {
            add_pkt_line_string(&mut buf, format!("deepen-not {reference}\n").to_string());
        }
    } else if let Some(d) = options.depth {
        add_pkt_line_string(&mut buf, format!("deepen {d}\n").to_string());
    }

    buf.extend(b"0000");
    for h in have {
        add_pkt_line_string(&mut buf, format!("have {h}\n").to_string());
    }

    add_pkt_line_string(&mut buf, "done\n".to_string());

    buf.freeze()
}

impl From<Branch> for DiscoveredReference {
    fn from(branch: Branch) -> Self {
        let _ref = if branch.name.starts_with("refs/") {
            branch.name.clone()
        } else {
            match branch.remote {
                Some(remote) => format!("refs/remotes/{}/{}", remote, branch.name),
                None => format!("refs/heads/{}", branch.name),
            }
        };
        DiscoveredReference {
            _hash: branch.commit.to_string(),
            _ref,
        }
    }
}

#[cfg(test)]
mod test {
    use super::{ShallowOptions, generate_upload_pack_content};

    fn render(want: &[String], shallow: &[String], opts: &ShallowOptions) -> String {
        let bytes = generate_upload_pack_content(&[], want, shallow, opts);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn upload_pack_emits_shallow_and_depth_frames() {
        let want = vec!["a".repeat(40)];
        let shallow = vec!["b".repeat(40)];
        let content = render(&want, &shallow, &ShallowOptions::from_depth(Some(5)));
        assert!(content.contains(&format!("shallow {}", "b".repeat(40))));
        assert!(content.contains("deepen 5\n"));
        assert!(!content.contains("deepen-since"));
        assert!(!content.contains("deepen-not"));
    }

    #[test]
    fn upload_pack_emits_deepen_since_and_not_frames() {
        let want = vec!["a".repeat(40)];
        let opts = ShallowOptions {
            // `depth` is intentionally also set: git-upload-pack rejects `deepen`
            // alongside `deepen-since`/`deepen-not`, so the time/ref requests must
            // take precedence and the plain `deepen` line must be suppressed.
            depth: Some(2),
            deepen_since: Some(1_704_067_200),
            deepen_not: vec!["refs/tags/v1".to_string(), "refs/heads/legacy".to_string()],
        };
        let content = render(&want, &[], &opts);
        assert!(
            !content.contains("deepen 2\n"),
            "plain deepen must be suppressed when deepen-since/deepen-not present"
        );
        assert!(content.contains("deepen-since 1704067200\n"));
        assert!(content.contains("deepen-not refs/tags/v1\n"));
        assert!(content.contains("deepen-not refs/heads/legacy\n"));
    }

    #[test]
    fn upload_pack_omits_deepen_frames_when_unset() {
        let want = vec!["a".repeat(40)];
        let content = render(&want, &[], &ShallowOptions::default());
        assert!(!content.contains("deepen"));
        assert!(content.contains("done\n"));
        assert!(!ShallowOptions::default().is_requested());
    }
}
