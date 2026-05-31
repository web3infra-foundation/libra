//! Data structures for the Git LFS HTTP API.
//!
//! These types encode/decode the JSON payloads exchanged with an LFS server: batch
//! requests, transfer adapter selection, signed action URLs (download/upload/verify),
//! file locks, and chunked transfer metadata.
//!
//! All structs match the wire format defined by the LFS spec
//! (<https://github.com/git-lfs/git-lfs/blob/main/docs/api>) and rely on `serde` rename
//! attributes to bridge `snake_case` Rust identifiers with the API's `lowercase`
//! conventions. None of these types perform I/O; they are pure data carriers used by
//! [`crate::internal::protocol::lfs_client`] and [`crate::command::lfs`].
//!
//! Git LFS HTTP API 的数据结构。
//!
//! 这些类型编码/解码与 LFS 服务器交换的 JSON 有效负载：批处理请求、传输适配器选择、签名
//! 操作 URL（下载/上传/验证）、文件锁和分块传输元数据。
//!
//! 所有结构都与 LFS 规范定义的线路格式匹配（<https://github.com/git-lfs/git-lfs/blob/main/docs/api>），
//! 并依赖 `serde` 重命名属性来桥接 `snake_case` Rust 标识符与 API 的 `lowercase` 约定。
//! 这些类型都不执行 I/O；它们是 [`crate::internal::protocol::lfs_client`] 和 [`crate::command::lfs`]
//! 使用的纯数据载体。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Negotiated transfer adapter for a batch request.
///
/// The LFS server advertises which adapters it supports; clients echo back the one
/// they want to use. `BASIC` is the only adapter every server must implement.
///
/// 为批处理请求协商的传输适配器。
///
/// LFS 服务器声传它支持哪些适配器；客户端回应它想使用的。`BASIC` 是每个服务器必须实现的
/// 唯一适配器。
#[derive(Serialize, Deserialize, Debug, Default)]
pub enum TransferMode {
    /// Single-shot download/upload through the URL returned in `actions`.
    /// 通过在 `actions` 中返回的 URL 进行一次性下载/上传。
    #[default]
    #[serde(rename = "basic")]
    BASIC,
    /// Object split into discrete pieces, each with its own URL — typically used for
    /// objects larger than the configured chunk threshold.
    /// 对象分成离散的片段，每个片段都有自己的 URL — 通常用于大于配置的块阈值的对象。
    #[serde(rename = "multipart")]
    MULTIPART,
    /// Streaming uploads via TUS-like resumable PATCH semantics. Reserved by the spec
    /// but not yet implemented in Libra.
    /// 通过类似 TUS 的可恢复 PATCH 语义进行流式上传。由规范保留但尚未在 Libra 中实现。
    STREAMING,
}

/// Direction of an LFS batch request.
///
/// LFS 批处理请求的方向。
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub enum Operation {
    /// Server-to-client: fetch object content.
    /// 服务器到客户端：获取对象内容。
    #[serde(rename = "download")]
    Download,
    /// Client-to-server: push object content.
    /// 客户端到服务器：推送对象内容。
    #[serde(rename = "upload")]
    Upload,
}

/// Download operations MUST specify a download action, or an object error if the object cannot be downloaded for some reason.
/// Upload operations can specify an upload and a verify action.
/// The upload action describes how to upload the object. If the object has a verify action, the LFS client will hit this URL after a successful upload. Servers can use this for extra verification, if needed.
/// If a client requests to upload an object that the server already has, the server should omit the actions property completely. The client will then assume the server already has it.
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    #[serde(rename = "download")]
    Download,
    #[serde(rename = "upload")]
    Upload,
    #[serde(rename = "verify")]
    Verify,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct RequestObject {
    pub oid: String,
    pub size: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authorization: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Lock {
    pub id: String,
    pub path: String,
    pub locked_at: String,
    pub owner: Option<User>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct User {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BatchRequest {
    // Should be download or upload.
    pub operation: Operation,
    // An optional Array of String identifiers for transfer adapters that the client has configured.
    // If omitted, the basic transfer adapter MUST be assumed by the server.
    pub transfers: Vec<String>,
    pub objects: Vec<RequestObject>,
    pub hash_algo: String,
}

#[derive(Serialize, Deserialize)]
pub struct BatchResponse {
    pub transfer: TransferMode,
    pub objects: Vec<ResponseObject>,
    pub hash_algo: String,
}

#[derive(Serialize, Deserialize)]
pub struct FetchchunkResponse {
    pub oid: String,
    pub size: i64,
    pub chunks: Vec<ChunkDownloadObject>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Link {
    pub href: String,
    #[serde(default)] // Optional field
    pub header: HashMap<String, String>,
    pub expires_at: String,
}

impl Link {
    /// Build a [`Link`] for an LFS action URL with sensible defaults.
    ///
    /// Functional scope:
    /// - Sets the `Accept: application/vnd.git-lfs` header so downstream HTTP clients
    ///   negotiate the LFS media type without having to remember it.
    /// - Stamps `expires_at` 24 hours into the future (RFC 3339), the default LFS
    ///   action lifetime expected by Git LFS clients.
    ///
    /// Boundary conditions:
    /// - `href` is stored verbatim; callers are responsible for URL encoding.
    /// - The 24-hour expiry is not configurable here; servers that wish to issue
    ///   shorter-lived URLs should construct the struct manually.
    ///
    /// 使用合理的默认值为 LFS 操作 URL 构建 [`Link`]。
    ///
    /// 功能范围：
    /// - 设置 `Accept: application/vnd.git-lfs` 标头，以便下游 HTTP 客户端协商 LFS 媒体类型，
    ///   而无需记住它。
    /// - 将 `expires_at` 标记为未来 24 小时（RFC 3339），这是 Git LFS 客户端期望的默认 LFS 操作
    ///   生命周期。
    ///
    /// 边界条件：
    /// - `href` 按原样存储；调用者负责 URL 编码。
    /// - 24 小时过期在这里不可配置；希望发布寿命较短的 URL 的服务器应手动构建结构。
    pub fn new(href: &str) -> Self {
        let mut header = HashMap::new();
        header.insert("Accept".to_string(), "application/vnd.git-lfs".to_owned());

        Link {
            href: href.to_string(),
            header,
            expires_at: {
                use chrono::{DateTime, Duration, Utc};
                // INVARIANT: 86_400 is a small constant well inside chrono's
                // representable range for Duration seconds; `try_seconds`
                // only returns None for values that overflow i64 nanoseconds
                // when multiplied by 1_000_000_000.
                let expire_time: DateTime<Utc> = Utc::now()
                    + Duration::try_seconds(86400)
                        .expect("24h in seconds is representable as chrono::Duration");
                expire_time.to_rfc3339()
            },
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct ObjectError {
    pub code: i64,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct ResponseObject {
    pub oid: String,
    pub size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticated: Option<bool>,
    // Object containing the next actions for this object. Applicable actions depend on which operation is specified in the request.
    // How these properties are interpreted depends on which transfer adapter the client will be using.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<HashMap<Action, Link>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ObjectError>,
}

pub struct ResCondition {
    pub file_exist: bool,
    pub operation: Operation,
    pub use_tus: bool,
}

impl ResponseObject {
    /// Build a [`ResponseObject`] for the four `(file_exist, operation)` combinations
    /// defined by the LFS batch API.
    ///
    /// Functional scope, by `res_condition`:
    /// - `(file_exist=true, Upload)`: omit `actions` entirely so the client knows the
    ///   server already has the object and skips the upload — required by spec.
    /// - `(file_exist=true, Download)`: emit a single `Download` action pointing at
    ///   `download_url`.
    /// - `(file_exist=false, Upload)`: emit a single `Upload` action pointing at
    ///   `upload_url`. (TUS verification is wired up but currently disabled — see the
    ///   commented-out block in source.)
    /// - `(file_exist=false, Download)`: cannot serve the object; populate `error`
    ///   with HTTP-style code 404.
    ///
    /// Boundary conditions:
    /// - `meta.oid` and `meta.size` are echoed back verbatim so the LFS client can
    ///   correlate the response with its own request even when reordering occurs.
    /// - `authenticated` is always `Some(true)` because Libra only returns response
    ///   objects after the surrounding handler has authenticated the caller.
    ///
    /// 为 LFS 批 API 定义的四个 `(file_exist, operation)` 组合构建 [`ResponseObject`]。
    ///
    /// 按 `res_condition` 的功能范围：
    /// - `(file_exist=true, Upload)`：完全省略 `actions`，以便客户端知道服务器已经拥有对象
    ///   并跳过上传 — 规范要求。
    /// - `(file_exist=true, Download)`：发出指向 `download_url` 的单个 `Download` 操作。
    /// - `(file_exist=false, Upload)`：发出指向 `upload_url` 的单个 `Upload` 操作。
    ///   （TUS 验证已接线但当前已禁用 — 见源代码中的注释块。）
    /// - `(file_exist=false, Download)`：无法服务对象；用 HTTP 风格代码 404 填充 `error`。
    ///
    /// 边界条件：
    /// - `meta.oid` 和 `meta.size` 被逐字回显，以便 LFS 客户端即使在发生重新排序时也可以
    ///   将响应与自己的请求相关联。
    /// - `authenticated` 总是 `Some(true)`，因为 Libra 仅在周围处理器验证了调用者后才返回
    ///   响应对象。
    pub fn new(
        meta: &MetaObject,
        res_condition: ResCondition,
        download_url: &str,
        upload_url: &str,
    ) -> ResponseObject {
        let mut res = ResponseObject {
            oid: meta.oid.to_owned(),
            size: meta.size,
            authenticated: Some(true),
            actions: None,
            error: None,
        };

        let mut actions = HashMap::new();

        match res_condition {
            ResCondition {
                file_exist: true,
                operation: Operation::Upload,
                ..
            } => {
                //If a client requests to upload an object that the server already has, the server should omit the actions property completely.
                // The client will then assume the server already has it.
                tracing::debug!("File existing, leave actions empty")
            }
            ResCondition {
                file_exist: true,
                operation: Operation::Download,
                ..
            } => {
                actions.insert(Action::Download, Link::new(download_url));
                res.actions = Some(actions);
            }
            ResCondition {
                file_exist: false,
                operation: Operation::Upload,
                ..
            } => {
                actions.insert(Action::Upload, Link::new(upload_url));
                // if use_tus {
                //     actions.insert(
                //         Action::Verify,
                //         Link::new(&req_object.verify_link(hostname.to_string())),
                //     );
                // }
                res.actions = Some(actions);
            }
            ResCondition {
                file_exist: false,
                operation: Operation::Download,
                ..
            } => {
                let err = ObjectError {
                    code: 404,
                    message: "Not found".to_owned(),
                };
                res.error = Some(err)
            }
        }
        res
    }

    /// Construct a failure-only response for `object` carrying `err`.
    ///
    /// Used when the server cannot even compute a [`MetaObject`] (e.g. the OID is
    /// malformed or storage is unreachable), so the normal [`ResponseObject::new`]
    /// path cannot run.
    ///
    /// 为 `object` 构造仅失败的响应，携带 `err`。
    ///
    /// 当服务器甚至无法计算 [`MetaObject`] 时使用（例如，OID 格式错误或存储无法访问），
    /// 因此正常的 [`ResponseObject::new`] 路径无法运行。
    pub fn failed_with_err(object: &RequestObject, err: ObjectError) -> ResponseObject {
        ResponseObject {
            oid: object.oid.to_owned(),
            size: object.size,
            authenticated: None,
            actions: None,
            error: Some(err),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ChunkDownloadObject {
    pub sub_oid: String,
    pub offset: i64,
    pub size: i64,
    pub link: Link,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Ref {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct LockRequest {
    pub path: String,
    #[serde(rename(serialize = "ref", deserialize = "ref"))]
    pub refs: Ref,
}

#[derive(Serialize, Deserialize)]
pub struct LockResponse {
    pub lock: Lock,
    pub message: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct UnlockRequest {
    pub force: Option<bool>,
    #[serde(rename(serialize = "ref", deserialize = "ref"))]
    pub refs: Ref,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UnlockResponse {
    pub lock: Lock,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct LockList {
    pub locks: Vec<Lock>,
    pub next_cursor: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct VerifiableLockRequest {
    #[serde(rename(serialize = "ref", deserialize = "ref"))]
    pub refs: Ref,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct VerifiableLockList {
    pub ours: Vec<Lock>,
    pub theirs: Vec<Lock>,
    pub next_cursor: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LockListQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub cursor: String,
    #[serde(default)]
    pub limit: String,
    #[serde(default)]
    pub refspec: String,
}

// Define MetaObject as it's used in ResponseObject::new
#[derive(Debug, Clone)]
pub struct MetaObject {
    pub oid: String,
    pub size: i64,
    pub exist: bool,
    pub splited: bool,
}
