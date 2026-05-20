//! Stub network-allowlist proxy interface for sandbox Phase 7
//! (`docs/improvement/sandbox.md` §7.4).
//!
//! When the sandbox network three-state migration lands a complete
//! `Allowlist { services }` mode, AI shell tools that need outbound
//! network access will route through a local-only HTTP CONNECT proxy
//! that filters connections by SNI / Host header against the
//! allowlist. The full proxy (built on `hyper` + `hickory-resolver`)
//! is a follow-up batch; this module ships the **trait surface plus
//! two reference implementations** so:
//!
//! 1. The sandbox runtime can route through a `&'static dyn
//!    NetworkProxy` without knowing whether the active proxy is the
//!    real one or a stub.
//! 2. Tests and the `Denied`-mode fallback can use [`NoopProxy`]
//!    (deny everything) without depending on the network crate
//!    stack.
//! 3. The `BestEffort` enforcement path can default to
//!    [`LoopbackOnlyProxy`] — allow 127.0.0.1 / `::1` only, deny
//!    everything else — which approximates the v1 "loopback-only"
//!    contract while the real proxy is being built.
//!
//! Both implementations are zero-sized so the sandbox runtime can
//! borrow them through `&'static dyn NetworkProxy` without any heap
//! allocation.

use std::net::IpAddr;

use crate::internal::ai::sandbox::policy::NetworkProtocol;

/// One outbound connection request handed to a [`NetworkProxy`]
/// before the sandbox runtime forwards it.
///
/// Kept as plain owned strings / numbers so the type can cross the
/// proxy boundary without forcing every caller to depend on `url` /
/// `hyper`. The Phase 7.4 real proxy will rebuild whatever wire-type
/// it needs from these fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkRequest {
    /// Destination hostname (FQDN or IP literal). Never empty —
    /// callers that have only an `IpAddr` should stringify it before
    /// constructing the request so the proxy can apply hostname
    /// allowlist rules consistently.
    pub host: String,
    /// Destination port. Always set — the proxy needs a concrete
    /// port to apply per-port allowlist rules from
    /// [`crate::internal::ai::sandbox::policy::NetworkService`].
    pub port: u16,
    /// Wire protocol selector; matches the
    /// [`NetworkProtocol`] enum so the proxy can route TCP vs UDP
    /// without a second translation step.
    pub protocol: NetworkProtocol,
}

/// Decision returned by [`NetworkProxy::evaluate`].
///
/// Modeled as an explicit enum rather than a `bool` so the future
/// audit pipeline can pin per-decision reason codes without changing
/// the trait signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkDecision {
    /// The proxy allows the connection to proceed.
    Allow,
    /// The proxy denies the connection. The string is a short
    /// human-readable reason that the sandbox audit path will copy
    /// into the `ToolInvocation[E]` evidence record.
    Deny(String),
}

impl NetworkDecision {
    /// `true` when the decision allows the connection.
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// `true` when the decision denies the connection.
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny(_))
    }
}

/// Pluggable network filter that sits between the sandbox runtime
/// and the outbound transport.
///
/// Phase 7.4's real implementation will combine SNI / Host-header
/// matching with the per-`NetworkService` rules from the policy
/// layer; the stubs in this module short-circuit to a fixed answer
/// so the rest of the system can be wired without depending on the
/// network crate stack.
///
/// Implementors must be `Send + Sync` so the sandbox runtime can
/// share a single instance across worker threads via
/// `&'static dyn NetworkProxy`.
pub trait NetworkProxy: Send + Sync {
    /// Stable identifier for this proxy implementation. Used by
    /// `libra sandbox status` to surface which proxy is wired up;
    /// also serialised into the audit record so downstream readers
    /// can correlate decisions with the active backend.
    fn backend_name(&self) -> &'static str;

    /// Apply the proxy's allowlist policy to the supplied request.
    ///
    /// Returning [`NetworkDecision::Deny`] does NOT mean the proxy
    /// drops the connection on the wire — the caller (sandbox
    /// runtime) is responsible for treating the deny as a tool
    /// failure and surfacing the reason. The trait is pure-function
    /// so it stays testable without any I/O.
    fn evaluate(&self, request: &NetworkRequest) -> NetworkDecision;
}

/// Proxy implementation that denies every connection.
///
/// Used by the `Denied` mode of the upcoming
/// `NetworkAccess::{Denied, Allowlist, Full}` migration (sandbox.md
/// §7.1) — when the user has configured "no network", routing
/// through a `NoopProxy` keeps the sandbox runtime's dispatch path
/// uniform across the three modes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopProxy;

impl NetworkProxy for NoopProxy {
    fn backend_name(&self) -> &'static str {
        "noop"
    }

    fn evaluate(&self, _request: &NetworkRequest) -> NetworkDecision {
        NetworkDecision::Deny(
            "NetworkAccess::Denied — no network access is permitted in this sandbox".to_string(),
        )
    }
}

/// Proxy implementation that allows only loopback destinations
/// (127.0.0.1 / ::1 / `localhost`).
///
/// Used as the v1 fallback for `Allowlist` mode when the full proxy
/// hasn't started, and as the `BestEffort` enforcement default. Any
/// non-loopback destination is denied with a reason naming the
/// observed host so audit consumers can see exactly which target was
/// rejected.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoopbackOnlyProxy;

impl NetworkProxy for LoopbackOnlyProxy {
    fn backend_name(&self) -> &'static str {
        "loopback-only"
    }

    fn evaluate(&self, request: &NetworkRequest) -> NetworkDecision {
        if is_loopback_host(&request.host) {
            NetworkDecision::Allow
        } else {
            NetworkDecision::Deny(format!(
                "LoopbackOnlyProxy: outbound connection to {}:{} ({:?}) is not loopback",
                request.host, request.port, request.protocol,
            ))
        }
    }
}

/// `true` when `host` is the literal string `"localhost"` (case-
/// insensitive) or parses as a loopback `IpAddr`.
///
/// Kept as a free function so the same predicate can be reused by the
/// Phase 7.4 full proxy's allowlist-fallback path.
pub fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|addr| addr.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(host: &str, port: u16) -> NetworkRequest {
        NetworkRequest {
            host: host.to_string(),
            port,
            protocol: NetworkProtocol::Tcp,
        }
    }

    /// `is_loopback_host` must accept the literal `"localhost"` (any
    /// casing), `127.0.0.1`, and `::1`. Everything else — including
    /// the lookalike `localhost.evil.com` — is rejected.
    #[test]
    fn is_loopback_host_accepts_localhost_and_ipv4_v6_loopback_addresses() {
        for accepted in ["localhost", "LOCALHOST", "127.0.0.1", "127.0.0.2", "::1"] {
            assert!(
                is_loopback_host(accepted),
                "expected '{accepted}' to be loopback",
            );
        }
        for rejected in [
            "example.com",
            "192.168.1.1",
            "10.0.0.1",
            "::2",
            "localhost.evil.com",
            "",
        ] {
            assert!(
                !is_loopback_host(rejected),
                "expected '{rejected}' to NOT be loopback",
            );
        }
    }

    /// `NoopProxy::evaluate` must deny every request regardless of
    /// host / port / protocol. Pin the rejection so a future
    /// "smart" NoopProxy that secretly allows some hosts fails this
    /// test.
    #[test]
    fn noop_proxy_denies_every_request() {
        let proxy = NoopProxy;
        assert_eq!(proxy.backend_name(), "noop");
        for (host, port) in [
            ("example.com", 443),
            ("127.0.0.1", 22),
            ("localhost", 80),
            ("registry.npmjs.org", 443),
        ] {
            let req = request(host, port);
            let decision = proxy.evaluate(&req);
            assert!(decision.is_deny(), "noop proxy must deny {host}:{port}");
            assert!(!decision.is_allow());
        }
    }

    /// `LoopbackOnlyProxy::evaluate` must allow loopback destinations
    /// (`127.0.0.1`, `::1`, `localhost`) and deny everything else.
    /// The denial reason must include the rejected host so audit
    /// records can pinpoint the offending target.
    #[test]
    fn loopback_only_proxy_allows_loopback_and_denies_remote_hosts() {
        let proxy = LoopbackOnlyProxy;
        assert_eq!(proxy.backend_name(), "loopback-only");

        for host in ["localhost", "127.0.0.1", "::1"] {
            let decision = proxy.evaluate(&request(host, 8080));
            assert!(
                decision.is_allow(),
                "loopback-only proxy must allow {host} (got {decision:?})",
            );
        }

        for host in ["example.com", "192.168.1.1", "registry.npmjs.org"] {
            let decision = proxy.evaluate(&request(host, 443));
            match decision {
                NetworkDecision::Deny(ref reason) => {
                    assert!(
                        reason.contains(host),
                        "deny reason must name the rejected host; got {reason}",
                    );
                }
                NetworkDecision::Allow => {
                    panic!("loopback-only proxy must deny non-loopback {host}");
                }
            }
        }
    }

    /// The proxy trait must be object-safe so the sandbox runtime can
    /// borrow either implementation through a `&'static dyn
    /// NetworkProxy`. This test exercises the dyn-safe path
    /// explicitly so a future trait change that adds a non-object-safe
    /// method (e.g. a generic) fails to compile here rather than at
    /// the sandbox-runtime callsite.
    #[test]
    fn network_proxy_is_object_safe_and_dispatches_through_dyn_reference() {
        let proxies: &[&'static dyn NetworkProxy] = &[&NoopProxy, &LoopbackOnlyProxy];
        let req = request("localhost", 8080);
        let mut allowed = 0;
        let mut denied = 0;
        for proxy in proxies {
            match proxy.evaluate(&req) {
                NetworkDecision::Allow => allowed += 1,
                NetworkDecision::Deny(_) => denied += 1,
            }
        }
        // Exactly the loopback-only proxy should accept the loopback
        // request; the noop proxy denies it. The two-element split
        // proves the dyn dispatch hit both implementations.
        assert_eq!(allowed, 1);
        assert_eq!(denied, 1);
    }
}
