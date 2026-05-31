//! Packet-line helpers and service type parsing for the Git smart protocol, covering both
//! `upload-pack` (fetch/clone) and `receive-pack` (push) flows.
//!
//! The Git smart protocol frames every payload as a sequence of `pkt-line` records: a
//! 4-byte ASCII hex length header followed by `length - 4` bytes of payload. A length of
//! `0000` is a flush marker. This module exposes the minimum primitives required to read
//! and write these frames and to identify which side of the protocol a request targets.
//!
//! Git 智能协议的包行帮助程序和服务类型解析，涵盖 `upload-pack`（fetch/clone）和 `receive-pack`（push）流。
//!
//! Git 智能协议将每个有效负载分帧为 `pkt-line` 记录的序列：4 字节 ASCII 十六进制长度标头后跟 `length - 4` 字节的有效负载。
//! 长度 `0000` 是刷新标记。此模块公开读取和写入这些帧所需的最小基元，以及识别请求针对协议的哪一侧。

use core::fmt;
use std::str::FromStr;

use bytes::{Buf, BufMut};
use git_internal::errors::GitError;

/// Identifies the direction of a smart-protocol exchange.
///
/// Used by HTTP routers and SSH dispatchers to pick the correct backend handler.
///
/// 标识智能协议交换的方向。
///
/// 由 HTTP 路由器和 SSH 分发器使用以选择正确的后端处理器。
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ServiceType {
    /// Server-to-client transfer: clone, fetch, ls-remote.
    /// 服务器到客户端传输：clone、fetch、ls-remote。
    UploadPack,
    /// Client-to-server transfer: push.
    /// 客户端到服务器传输：push。
    ReceivePack,
}

impl fmt::Display for ServiceType {
    /// Render the variant as the on-the-wire service name expected by Git clients
    /// (e.g. the `service=` query parameter in `info/refs`).
    ///
    /// 将变体渲染为 Git 客户端期望的线上服务名称（例如 `info/refs` 中的 `service=` 查询参数）。
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ServiceType::UploadPack => write!(f, "git-upload-pack"),
            ServiceType::ReceivePack => write!(f, "git-receive-pack"),
        }
    }
}

impl FromStr for ServiceType {
    type Err = GitError;

    /// Parse a wire-format service name back into a `ServiceType`.
    ///
    /// Boundary conditions:
    /// - Comparison is case-sensitive — `"git-upload-pack"` and `"git-receive-pack"` are
    ///   the only accepted strings.
    /// - Any other input returns `GitError::InvalidArgument` with the offending value
    ///   embedded for debugging.
    ///
    /// 将线上格式的服务名称解析回 `ServiceType`。
    ///
    /// 边界条件：
    /// - 比较是区分大小写的 — `"git-upload-pack"` 和 `"git-receive-pack"` 是唯一接受的字符串。
    /// - 任何其他输入返回 `GitError::InvalidArgument`，将违规值嵌入以进行调试。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "git-upload-pack" => Ok(ServiceType::UploadPack),
            "git-receive-pack" => Ok(ServiceType::ReceivePack),
            _ => Err(GitError::InvalidArgument(format!(
                "Invalid service name: {}",
                s
            ))),
        }
    }
}

/// Flush packet (`0000`). Marks the end of a logical group of pkt-lines.
pub const PKT_LINE_END_MARKER: &[u8; 4] = b"0000";

use bytes::Bytes;

/// Consume a single pkt-line frame from the front of `bytes` and return its
/// `(declared_length, payload)`.
///
/// Functional scope:
/// - Reads the 4-byte ASCII hex header, decodes it as the total frame length, then
///   splits off `length - 4` bytes of payload.
/// - Mutates the input buffer in place: after a successful call, `bytes` advances past
///   the consumed frame.
///
/// Boundary conditions:
/// - Returns `(0, Bytes::new())` when `bytes` is empty so callers can use a
///   zero-length response as a stop condition.
/// - Returns `(0, Bytes::new())` when the decoded length is zero (the flush marker
///   `0000`); the leading 4 header bytes are still consumed.
/// - **Panics** when the 4-byte header is not valid UTF-8 hex. Callers must therefore
///   validate or trust the source — typically only network code that rejects malformed
///   frames upstream invokes this helper.
///
/// 从 `bytes` 的前面消费单个 pkt-line 帧并返回其 `(declared_length, payload)`。
///
/// 功能范围：
/// - 读取 4 字节 ASCII 十六进制标头，将其解码为总帧长度，然后分割出 `length - 4` 字节的有效负载。
/// - 就地改变输入缓冲区：在成功调用后，`bytes` 在消费的帧之后前进。
///
/// 边界条件：
/// - 当 `bytes` 为空时返回 `(0, Bytes::new())`，以便调用者可以使用零长度响应作为停止条件。
/// - 当解码的长度为零时（刷新标记 `0000`）返回 `(0, Bytes::new())`；前导 4 个标头字节仍被消费。
/// - **会崩溃**当 4 字节标头不是有效的 UTF-8 十六进制时。因此，调用者必须验证或信任源 —
///   通常只有拒绝上游格式错误帧的网络代码调用此帮助程序。
pub fn read_pkt_line(bytes: &mut Bytes) -> (usize, Bytes) {
    if bytes.is_empty() {
        return (0, Bytes::new());
    }
    let pkt_length_bytes = bytes.copy_to_bytes(4);
    // INVARIANT: the function's doc comment explicitly documents that
    // callers must validate the 4-byte header as UTF-8 hex. Network code
    // upstream rejects malformed frames before they reach this helper.
    let header_str = core::str::from_utf8(&pkt_length_bytes)
        .expect("pkt-line header must be 4 bytes of ASCII hex (caller contract)");
    let pkt_length = usize::from_str_radix(header_str, 16).unwrap_or_else(|_| {
        panic!("pkt-line header {pkt_length_bytes:?} is not valid hex (caller contract)")
    });
    if pkt_length == 0 {
        return (0, Bytes::new());
    }
    // Advance the buffer past the payload — the caller receives the payload slice and
    // any subsequent read continues from the next frame.
    let pkt_line = bytes.copy_to_bytes(pkt_length - 4);
    tracing::debug!("pkt line: {:?}", pkt_line);

    (pkt_length, pkt_line)
}

use bytes::BytesMut;

/// Append a UTF-8 string as a pkt-line to `pkt_line_stream`.
///
/// Functional scope:
/// - Writes the 4-byte ASCII hex length header (`buf_str.len() + 4`, including the
///   header itself) followed by the raw bytes of `buf_str`.
/// - Does **not** add a trailing newline; callers that need newline-terminated lines
///   (the common case for capability advertisements) must include the `\n` in
///   `buf_str`.
///
/// Boundary conditions:
/// - Maximum frame size is `0xffff` bytes (65,535) per the Git protocol spec; this
///   helper does not enforce that limit and will silently produce malformed frames if
///   given an oversized string. Callers must chunk longer payloads themselves.
///
/// 将 UTF-8 字符串作为 pkt-line 附加到 `pkt_line_stream`。
///
/// 功能范围：
/// - 写入 4 字节 ASCII 十六进制长度标头（`buf_str.len() + 4`，包括标头本身）后跟 `buf_str` 的原始字节。
/// - **不**添加尾部换行符；需要换行符终止的行的调用者（能力通告的常见情况）必须在 `buf_str` 中
///   包含 `\n`。
///
/// 边界条件：
/// - 根据 Git 协议规范，最大帧大小为 `0xffff` 字节（65,535）；此帮助程序不强制该限制，如果给定
///   过大的字符串，将默认产生格式错误的帧。调用者必须自己分块较长的有效负载。
pub fn add_pkt_line_string(pkt_line_stream: &mut BytesMut, buf_str: String) {
    let buf_str_length = buf_str.len() + 4;
    pkt_line_stream.put(Bytes::from(format!("{:04x}", buf_str_length)));
    pkt_line_stream.put(buf_str.as_bytes());
}
