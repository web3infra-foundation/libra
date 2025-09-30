use bytes::{Buf, BufMut};
use core::fmt;
use std::str::FromStr;

use git_internal::errors::GitError;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ServiceType {
    UploadPack,
    ReceivePack,
}

impl fmt::Display for ServiceType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ServiceType::UploadPack => write!(f, "git-upload-pack"),
            ServiceType::ReceivePack => write!(f, "git-receive-pack"),
        }
    }
}

impl FromStr for ServiceType {
    type Err = GitError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "git-upload-pack" => Ok(ServiceType::UploadPack),
            "git-receive-pack" => Ok(ServiceType::ReceivePack),
            _ => Err(GitError::InvalidArgument(format!("Invalid service name: {}", s))),
        }
    }
}

// Define the constants that were used in the smart protocol functions
pub const PKT_LINE_END_MARKER: &[u8; 4] = b"0000";

// Function to read a single pkt-format line
use bytes::Bytes;

/// Read a single pkt-format line from the `bytes` buffer and return the line length and line bytes.
pub fn read_pkt_line(bytes: &mut Bytes) -> (usize, Bytes) {
    if bytes.is_empty() {
        return (0, Bytes::new());
    }
    let pkt_length = bytes.copy_to_bytes(4);
    let pkt_length = usize::from_str_radix(core::str::from_utf8(&pkt_length).unwrap(), 16)
        .unwrap_or_else(|_| panic!("{:?} is not a valid digit?", pkt_length));
    if pkt_length == 0 {
        return (0, Bytes::new());
    }
    // this operation will change the original bytes
    let pkt_line = bytes.copy_to_bytes(pkt_length - 4);
    tracing::debug!("pkt line: {:?}", pkt_line);

    (pkt_length, pkt_line)
}

use bytes::BytesMut;

pub fn add_pkt_line_string(pkt_line_stream: &mut BytesMut, buf_str: String) {
    let buf_str_length = buf_str.len() + 4;
    pkt_line_stream.put(Bytes::from(format!("{:04x}", buf_str_length)));
    pkt_line_stream.put(buf_str.as_bytes());
}