//! L2 (`--features test-network`): the `git://` fetch transport recovers from a
//! hung / black-holed peer within its configured timeout instead of blocking
//! forever, and a protocol stall never silently succeeds.
//!
//! These tests are self-contained — they bind a local listener that accepts the
//! connection but never responds — so they need no external service or secrets.

#![cfg(feature = "test-network")]

use std::time::Duration;

use libra::{
    git_protocol::ServiceType::UploadPack,
    internal::protocol::{ProtocolClient, git_client::GitClient},
};
use tokio::net::TcpListener;
use url::Url;

/// A peer that accepts the TCP connection but never sends a byte must be
/// recovered by the idle timeout, not left to hang the fetch.
#[tokio::test]
async fn git_discovery_recovers_from_a_silent_peer() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    // Accept the connection and hold it open without ever responding.
    let accept = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(stream);
        }
    });

    let url = Url::parse(&format!("git://127.0.0.1:{}/repo.git", addr.port())).unwrap();
    let client = GitClient::from_url(&url)
        .with_network_timeouts(Duration::from_secs(2), Duration::from_millis(300));

    let started = tokio::time::Instant::now();
    let result = client.discovery_reference(UploadPack).await;
    assert!(
        result.is_err(),
        "a silent peer must surface an error, not a successful empty discovery"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the idle read timeout must recover the fetch quickly, took {:?}",
        started.elapsed()
    );

    accept.abort();
}

/// A connect to a local port with no listener must fail fast (the connect is
/// bounded, not left hanging). This uses a refused connection rather than a
/// black-holed address so it is deterministic on every network.
#[tokio::test]
async fn git_connect_fails_fast_on_a_refused_port() {
    // Bind then immediately drop the listener so the port is free but unused;
    // a connect there is refused promptly by the OS.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        listener.local_addr().expect("addr").port()
        // listener dropped here — nothing accepts on `port`.
    };

    let url = Url::parse(&format!("git://127.0.0.1:{port}/repo.git")).unwrap();
    let client = GitClient::from_url(&url)
        .with_network_timeouts(Duration::from_secs(5), Duration::from_secs(60));

    let started = tokio::time::Instant::now();
    let result = client.discovery_reference(UploadPack).await;
    assert!(result.is_err(), "a refused connect must surface an error");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a refused connect must return promptly (well under the connect timeout), took {:?}",
        started.elapsed()
    );
}
