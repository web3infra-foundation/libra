//! Runtime HTTP proxy for `NetworkAccess::Allowlist`.
//!
//! The policy layer decides whether a destination is allowed. This module owns
//! the short-lived local transport used by sandboxed commands: clients connect
//! to a loopback HTTP proxy, the proxy evaluates CONNECT / Host targets against
//! [`AllowlistProxy`], and only allowed TCP connections are forwarded.

use std::{net::IpAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{Duration, timeout},
};
use url::Url;

use super::{
    NetworkDecision, NetworkProtocol, NetworkProxy, NetworkRequest, NetworkService,
    evidence::{SandboxEvidenceEvent, SandboxEvidenceSink, TracingSandboxEvidenceSink},
    proxy::AllowlistProxy,
};

const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_HEADER_TIMEOUT_SECS: u64 = 10;
const UPSTREAM_CONNECT_TIMEOUT_SECS: u64 = 10;

#[derive(Debug)]
pub struct RunningAllowlistProxy {
    listen_addr: std::net::SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl RunningAllowlistProxy {
    pub fn local_http_proxy_url(&self) -> String {
        format!("http://{}", self.listen_addr)
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for RunningAllowlistProxy {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub async fn spawn_allowlist_http_proxy(
    services: Vec<NetworkService>,
) -> Result<RunningAllowlistProxy, String> {
    spawn_allowlist_http_proxy_with_evidence(services, None).await
}

pub async fn spawn_allowlist_http_proxy_with_evidence(
    services: Vec<NetworkService>,
    evidence_sink: Option<Arc<dyn SandboxEvidenceSink>>,
) -> Result<RunningAllowlistProxy, String> {
    let listener = TcpListener::bind((IpAddr::from([127, 0, 0, 1]), 0))
        .await
        .map_err(|error| format!("failed to bind sandbox allowlist proxy: {error}"))?;
    let listen_addr = listener
        .local_addr()
        .map_err(|error| format!("failed to inspect sandbox allowlist proxy address: {error}"))?;
    let proxy = Arc::new(AllowlistProxy::new(&services));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(run_proxy(listener, proxy, evidence_sink, shutdown_rx));

    Ok(RunningAllowlistProxy {
        listen_addr,
        shutdown: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn run_proxy(
    listener: TcpListener,
    proxy: Arc<AllowlistProxy>,
    evidence_sink: Option<Arc<dyn SandboxEvidenceSink>>,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => accepted,
        };

        let (stream, _) = match accepted {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!(error = %error, "sandbox allowlist proxy accept failed");
                continue;
            }
        };

        let proxy = Arc::clone(&proxy);
        let evidence_sink = evidence_sink.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, proxy, evidence_sink).await {
                tracing::debug!(error = %error, "sandbox allowlist proxy connection failed");
            }
        });
    }
}

async fn handle_connection(
    mut client: TcpStream,
    proxy: Arc<AllowlistProxy>,
    evidence_sink: Option<Arc<dyn SandboxEvidenceSink>>,
) -> Result<(), String> {
    let (request, buffered_request) = read_proxy_request(&mut client).await?;
    let decision = proxy.evaluate(&NetworkRequest {
        host: request.host.clone(),
        port: request.port,
        protocol: NetworkProtocol::Tcp,
    });

    if let NetworkDecision::Deny(reason) = decision {
        record_denied_request(
            evidence_sink.as_deref(),
            proxy.backend_name(),
            &request,
            &reason,
        );
        write_response(
            &mut client,
            403,
            "Forbidden",
            &format!("sandbox network deny: {reason}\n"),
        )
        .await?;
        return Ok(());
    }

    let mut upstream = timeout(
        Duration::from_secs(UPSTREAM_CONNECT_TIMEOUT_SECS),
        TcpStream::connect((request.host.as_str(), request.port)),
    )
    .await
    .map_err(|_| format!("timed out connecting to {}:{}", request.host, request.port))?
    .map_err(|error| {
        format!(
            "failed to connect to allowed upstream {}:{}: {error}",
            request.host, request.port
        )
    })?;

    if request.is_connect {
        client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .map_err(|error| format!("failed to acknowledge CONNECT tunnel: {error}"))?;
    } else {
        upstream
            .write_all(&buffered_request)
            .await
            .map_err(|error| format!("failed to forward HTTP request: {error}"))?;
    }

    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|error| format!("proxy forwarding failed: {error}"))?;
    Ok(())
}

fn record_denied_request(
    evidence_sink: Option<&dyn SandboxEvidenceSink>,
    proxy_backend: &str,
    request: &ParsedProxyRequest,
    reason: &str,
) {
    let event = SandboxEvidenceEvent::NetworkRequestDenied {
        proxy_backend: proxy_backend.to_string(),
        host: request.host.clone(),
        port: request.port,
        protocol: NetworkProtocol::Tcp,
        reason: reason.to_string(),
    };
    if let Some(sink) = evidence_sink {
        sink.record(event);
    } else {
        TracingSandboxEvidenceSink.record(event);
    }
}

async fn read_proxy_request(
    client: &mut TcpStream,
) -> Result<(ParsedProxyRequest, Vec<u8>), String> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        if bytes.len() > MAX_REQUEST_HEADER_BYTES {
            return Err("proxy request header exceeded maximum size".to_string());
        }

        let read = timeout(
            Duration::from_secs(REQUEST_HEADER_TIMEOUT_SECS),
            client.read(&mut chunk),
        )
        .await
        .map_err(|_| "timed out reading proxy request header".to_string())?
        .map_err(|error| format!("failed to read proxy request header: {error}"))?;
        if read == 0 {
            return Err("client closed before sending proxy request header".to_string());
        }

        bytes.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = find_header_end(&bytes) {
            let request = parse_proxy_request_head(&bytes[..header_end])?;
            return Ok((request, bytes));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedProxyRequest {
    host: String,
    port: u16,
    is_connect: bool,
}

fn parse_proxy_request_head(head: &[u8]) -> Result<ParsedProxyRequest, String> {
    let head = std::str::from_utf8(head)
        .map_err(|error| format!("proxy request header is not utf-8: {error}"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing proxy request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "missing proxy request method".to_string())?;
    let target = parts
        .next()
        .ok_or_else(|| "missing proxy request target".to_string())?;
    let is_connect = method.eq_ignore_ascii_case("CONNECT");

    if is_connect {
        let (host, port) = parse_host_port(target)?;
        return Ok(ParsedProxyRequest {
            host,
            port,
            is_connect,
        });
    }

    if let Ok(url) = Url::parse(target)
        && let Some(host) = url.host_str()
    {
        let port = url
            .port()
            .or_else(|| default_port_for_scheme(url.scheme()))
            .ok_or_else(|| format!("unsupported proxy URL scheme '{}'", url.scheme()))?;
        return Ok(ParsedProxyRequest {
            host: host.to_string(),
            port,
            is_connect,
        });
    }

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("Host") {
            let (host, port) = parse_host_header(value)?;
            return Ok(ParsedProxyRequest {
                host,
                port,
                is_connect,
            });
        }
    }

    Err("proxy request did not include an absolute URL or Host header".to_string())
}

fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn parse_host_header(value: &str) -> Result<(String, u16), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("empty Host header".to_string());
    }
    parse_host_port(value).or_else(|_| Ok((strip_ipv6_brackets(value).to_string(), 80)))
}

fn parse_host_port(value: &str) -> Result<(String, u16), String> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| "invalid bracketed IPv6 host".to_string())?;
        let port = tail
            .strip_prefix(':')
            .ok_or_else(|| "missing port after bracketed IPv6 host".to_string())?
            .parse::<u16>()
            .map_err(|_| "invalid port after bracketed IPv6 host".to_string())?;
        return Ok((host.to_string(), port));
    }

    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| "missing host port separator".to_string())?;
    if host.contains(':') {
        return Err("IPv6 host with port must use brackets".to_string());
    }
    if host.is_empty() {
        return Err("empty host".to_string());
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("invalid port '{port}'"))?;
    Ok((host.to_string(), port))
}

fn strip_ipv6_brackets(value: &str) -> &str {
    value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value)
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

async fn write_response(
    stream: &mut TcpStream,
    status_code: u16,
    status_text: &str,
    body: &str,
) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status_code} {status_text}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|error| format!("failed to write proxy response: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::*;
    use crate::internal::ai::sandbox::evidence::{
        InMemorySandboxEvidenceSink, SandboxEvidenceEvent,
    };

    #[test]
    fn parse_connect_target_extracts_host_port() {
        let parsed = parse_proxy_request_head(
            b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org:443\r\n\r\n",
        )
        .expect("CONNECT request should parse");

        assert_eq!(
            parsed,
            ParsedProxyRequest {
                host: "registry.npmjs.org".to_string(),
                port: 443,
                is_connect: true,
            }
        );
    }

    #[test]
    fn parse_http_host_header_extracts_host_port() {
        let parsed = parse_proxy_request_head(b"GET / HTTP/1.1\r\nHost: example.com:8080\r\n\r\n")
            .expect("HTTP request should parse");

        assert_eq!(
            parsed,
            ParsedProxyRequest {
                host: "example.com".to_string(),
                port: 8080,
                is_connect: false,
            }
        );
    }

    #[tokio::test]
    async fn allowlist_proxy_runtime_denies_non_matching_connect_target() {
        let proxy = spawn_allowlist_http_proxy(vec![NetworkService {
            host: "allowed.example".to_string(),
            ports: vec![443],
            protocol: Some(NetworkProtocol::Tcp),
        }])
        .await
        .expect("proxy should start");

        let proxy_url = proxy.local_http_proxy_url();
        let proxy_addr = proxy_url
            .strip_prefix("http://")
            .expect("local proxy URL should be http")
            .to_string();
        let mut client = TcpStream::connect(proxy_addr)
            .await
            .expect("connect to local proxy");
        client
            .write_all(b"CONNECT denied.example:443 HTTP/1.1\r\nHost: denied.example:443\r\n\r\n")
            .await
            .expect("write CONNECT request");

        let mut response = vec![0_u8; 512];
        let read = client.read(&mut response).await.expect("read proxy denial");
        let response = String::from_utf8_lossy(&response[..read]);
        assert!(response.contains("403 Forbidden"), "{response}");
        assert!(response.contains("sandbox network deny"), "{response}");

        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn allowlist_proxy_runtime_records_evidence_on_denied_connect_target() {
        let sink = Arc::new(InMemorySandboxEvidenceSink::new());
        let proxy = spawn_allowlist_http_proxy_with_evidence(
            vec![NetworkService {
                host: "allowed.example".to_string(),
                ports: vec![443],
                protocol: Some(NetworkProtocol::Tcp),
            }],
            Some(sink.clone()),
        )
        .await
        .expect("proxy should start");

        let proxy_url = proxy.local_http_proxy_url();
        let proxy_addr = proxy_url
            .strip_prefix("http://")
            .expect("local proxy URL should be http")
            .to_string();
        let mut client = TcpStream::connect(proxy_addr)
            .await
            .expect("connect to local proxy");
        client
            .write_all(b"CONNECT denied.example:443 HTTP/1.1\r\nHost: denied.example:443\r\n\r\n")
            .await
            .expect("write CONNECT request");

        let mut response = vec![0_u8; 512];
        let read = client.read(&mut response).await.expect("read proxy denial");
        let response = String::from_utf8_lossy(&response[..read]);
        assert!(response.contains("403 Forbidden"), "{response}");

        let events = sink.events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            SandboxEvidenceEvent::NetworkRequestDenied {
                proxy_backend,
                host,
                port,
                protocol,
                reason,
            } => {
                assert_eq!(proxy_backend, "allowlist");
                assert_eq!(host, "denied.example");
                assert_eq!(*port, 443);
                assert_eq!(*protocol, NetworkProtocol::Tcp);
                assert!(reason.contains("not in allowlist"), "{reason}");
            }
            other => panic!("expected network request denial evidence, got {other:?}"),
        }

        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn allowlist_proxy_runtime_forwards_matching_connect_target() {
        let upstream = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind upstream");
        let upstream_port = upstream.local_addr().expect("upstream addr").port();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.expect("accept upstream");
            let mut request = [0_u8; 4];
            stream
                .read_exact(&mut request)
                .await
                .expect("read tunneled request");
            assert_eq!(&request, b"ping");
            stream
                .write_all(b"pong")
                .await
                .expect("write tunneled response");
        });

        let proxy = spawn_allowlist_http_proxy(vec![NetworkService {
            host: "127.0.0.1".to_string(),
            ports: vec![upstream_port],
            protocol: Some(NetworkProtocol::Tcp),
        }])
        .await
        .expect("proxy should start");
        let proxy_addr = proxy
            .local_http_proxy_url()
            .strip_prefix("http://")
            .expect("local proxy URL should be http")
            .to_string();
        let mut client = TcpStream::connect(proxy_addr)
            .await
            .expect("connect to local proxy");
        client
            .write_all(
                format!(
                    "CONNECT 127.0.0.1:{upstream_port} HTTP/1.1\r\nHost: 127.0.0.1:{upstream_port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("write CONNECT request");

        let mut response = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            client
                .read_exact(&mut byte)
                .await
                .expect("read CONNECT ack");
            response.push(byte[0]);
            if response.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let response = String::from_utf8_lossy(&response);
        assert!(
            response.contains("200 Connection established"),
            "{response}"
        );

        client.write_all(b"ping").await.expect("write tunnel body");
        let mut tunnel_response = [0_u8; 4];
        client
            .read_exact(&mut tunnel_response)
            .await
            .expect("read tunnel response");
        assert_eq!(&tunnel_response, b"pong");

        upstream_task.await.expect("upstream task should finish");
        proxy.shutdown().await;
    }
}
