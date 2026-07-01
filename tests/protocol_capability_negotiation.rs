//! L2 (`--features test-network`): the fetch client's `want`-line negotiates
//! exactly the capabilities Libra's pack decoder can honour — and none it
//! cannot — so a fetch never asks a server for a pack it could not decode.
//!
//! The wire body is asserted directly (the capability list a real server would
//! see); live end-to-end negotiation against a remote is covered by
//! `network_remotes_test`.

#![cfg(feature = "test-network")]

use libra::internal::protocol::generate_upload_pack_content;

#[test]
fn upload_pack_body_advertises_supported_capabilities_only() {
    let have: Vec<String> = Vec::new();
    let want = vec!["1".repeat(40)];
    let body = generate_upload_pack_content(&have, &want, &[], None);
    let text = String::from_utf8_lossy(&body);

    // Capabilities the decoder honours: sideband multiplexing, delta detail, and
    // in-pack offset deltas (git-internal resolves OffsetDelta).
    for capability in [
        "side-band-64k",
        "multi_ack_detailed",
        "ofs-delta",
        "include-tag",
    ] {
        assert!(
            text.contains(capability),
            "want line must advertise {capability}: {text}"
        );
    }
    // Identify the client to the server, as Git does.
    assert!(
        text.contains("agent=libra/"),
        "want line must send an agent string: {text}"
    );

    // Deliberately NOT advertised:
    // - `thin-pack` would delta against objects outside the pack, which the
    //   self-contained decoder cannot complete;
    // - `report-status` is a push (receive-pack) capability, not upload-pack.
    assert!(
        !text.contains("thin-pack"),
        "thin-pack is unsupported and must not be advertised: {text}"
    );
    assert!(
        !text.contains("report-status"),
        "report-status is push-only and must not be on an upload-pack want line: {text}"
    );
}

/// The SHA-256 negotiation adds `object-format=sha256`; a SHA-1 repository must
/// not (its absence means SHA-1, the wire default).
#[test]
fn upload_pack_body_is_sha1_by_default() {
    let have: Vec<String> = Vec::new();
    let want = vec!["1".repeat(40)];
    let body = generate_upload_pack_content(&have, &want, &[], None);
    let text = String::from_utf8_lossy(&body);
    // The default test process hash kind is SHA-1, so no object-format is sent.
    assert!(
        !text.contains("object-format=sha256"),
        "a SHA-1 fetch must not advertise object-format=sha256: {text}"
    );
}
