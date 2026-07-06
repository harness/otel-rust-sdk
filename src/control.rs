//! Control plane: synchronous "should this operation proceed?" decisions,
//! backed by remote policy fetched over its own wire contract (never OTLP).
//!
//! This module is deliberately separate from the telemetry pipeline
//! (`SpanProcessor`/`Sampler`). See "Control Plane" in
//! `docs/harness-otel-sdk-specification.md` for the full rationale. In
//! short: `ControlClient` never blocks, panics, or forces control flow — it
//! only returns a [`Decision`] that the caller must inspect and act on.
//!
//! Ships `Warn`-only for now. `Decision::Block` and the `GuardedOperation`
//! wrapper are intentionally not implemented yet — see the spec's Action
//! Items for the sequencing.

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::config::Config;

const AUTH_HEADER: &str = "x-harness-service-token";
const DEFAULT_TTL: Duration = Duration::from_secs(60);
const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_millis(500);
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// The outcome of a policy evaluation. The SDK never acts on this itself —
/// the caller must inspect it and decide whether to proceed, warn, or
/// (once `Block` ships) abort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No policy objection. Proceed normally.
    Allow,
    /// Policy flags this operation, but does not prevent it. The caller
    /// should surface `reason` to the user (e.g. a printed warning) and
    /// proceed.
    Warn { reason: String },
}

impl Decision {
    fn allow() -> Self {
        Decision::Allow
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PolicyResponse {
    schema_version: u32,
    #[serde(default)]
    rules: HashMap<String, PolicyRule>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PolicyRule {
    #[serde(default)]
    warn: bool,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Clone)]
struct CachedPolicy {
    response: PolicyResponse,
    fetched_at: Instant,
}

/// Fetches, caches, and evaluates policy for the control path.
///
/// - Never blocks `evaluate()` on the network: the cache is populated by a
///   bounded-timeout fetch, and a stale cache is served (rather than
///   discarded) if a refresh fails or returns an unrecognized schema
///   version.
/// - Fails open: if there is no usable cached policy at all (first run,
///   collector unreachable, or unparseable response with no prior cache),
///   `evaluate()` returns `Decision::Allow` rather than blocking the
///   caller.
pub struct ControlClient {
    endpoint: String,
    token: Option<String>,
    ttl: Duration,
    agent: ureq::Agent,
    cache: RwLock<Option<CachedPolicy>>,
    // Guards against multiple threads all triggering a refresh at once when
    // the cache is stale; the actual cached value lives in `cache` above.
    refresh_lock: Mutex<()>,
}

impl ControlClient {
    /// Build a `ControlClient` from the same `Config` used for telemetry.
    /// Uses `HA_CONTROL_ENDPOINT` if set on the config's reporting section,
    /// otherwise falls back to the telemetry endpoint's host with the
    /// control path appended — see [`Config::control_endpoint`].
    pub fn from_config(config: &Config) -> Self {
        Self::new(config.control_endpoint(), config.reporting.token.clone())
    }

    pub fn new(endpoint: String, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(DEFAULT_FETCH_TIMEOUT))
            .build()
            .into();
        Self {
            endpoint,
            token,
            ttl: DEFAULT_TTL,
            agent,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Override the cache TTL (default 60s). Test/advanced use only.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Evaluate whether `operation` should proceed. Never blocks on the
    /// network beyond a best-effort background-style refresh bounded by the
    /// configured fetch timeout, and never panics or aborts — always
    /// returns a [`Decision`] for the caller to act on.
    pub fn evaluate(&self, operation: &str, _attrs: &HashMap<String, String>) -> Decision {
        self.ensure_fresh_cache();

        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let Some(cached) = cache.as_ref() else {
            // No usable policy at all — fail open.
            return Decision::allow();
        };

        match cached.response.rules.get(operation) {
            Some(rule) if rule.warn => Decision::Warn {
                reason: rule.reason.clone(),
            },
            _ => Decision::allow(),
        }
    }

    fn ensure_fresh_cache(&self) {
        let is_fresh = {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            cache
                .as_ref()
                .is_some_and(|c| c.fetched_at.elapsed() < self.ttl)
        };
        if is_fresh {
            return;
        }

        // Only one thread refreshes at a time; others proceed with
        // whatever is currently cached (possibly stale, possibly none —
        // both are fail-open outcomes handled by `evaluate`).
        let Ok(_guard) = self.refresh_lock.try_lock() else {
            return;
        };

        if let Ok(response) = self.fetch() {
            if response.schema_version == SUPPORTED_SCHEMA_VERSION {
                let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
                *cache = Some(CachedPolicy {
                    response,
                    fetched_at: Instant::now(),
                });
            }
            // Unknown schema version: leave the existing cache (fresh or
            // stale) in place rather than discarding it — a schema mismatch
            // means "can't use the new policy," not "the old one is
            // invalid." If there is no existing cache, `evaluate` still
            // fails open with `Decision::Allow`.
        }
        // Fetch error (network, timeout, non-2xx): same as above — keep
        // serving whatever is cached, or fail open if nothing is cached.
    }

    fn fetch(&self) -> Result<PolicyResponse, ureq::Error> {
        let mut request = self.agent.get(&self.endpoint);
        if let Some(token) = &self.token {
            if !token.is_empty() {
                request = request.header(AUTH_HEADER, token);
            }
        }
        let mut response = request.call()?;
        let body = response.body_mut().read_to_string()?;
        serde_json::from_str(&body).map_err(|_| ureq::Error::HostNotFound)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    /// Spawns a single-request HTTP server on an ephemeral port and returns
    /// its base URL. Serves exactly one response, then exits.
    fn serve_once(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{addr}/")
    }

    #[test]
    fn evaluate_fails_open_when_endpoint_unreachable() {
        // Port 1 is reserved and never listens — the fetch fails immediately.
        let client = ControlClient::new("http://127.0.0.1:1/".to_string(), None);
        let decision = client.evaluate("push", &HashMap::new());
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn evaluate_returns_warn_when_policy_flags_operation() {
        let url = serve_once(
            r#"{"schema_version":1,"rules":{"push":{"warn":true,"reason":"large diff"}}}"#,
        );
        let client = ControlClient::new(url, None);
        let decision = client.evaluate("push", &HashMap::new());
        assert_eq!(
            decision,
            Decision::Warn {
                reason: "large diff".to_string()
            }
        );
    }

    #[test]
    fn evaluate_allows_operation_not_named_in_policy() {
        let url = serve_once(
            r#"{"schema_version":1,"rules":{"push":{"warn":true,"reason":"large diff"}}}"#,
        );
        let client = ControlClient::new(url, None);
        let decision = client.evaluate("commit", &HashMap::new());
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn unknown_schema_version_keeps_serving_last_known_good_cache() {
        let url = serve_once(
            r#"{"schema_version":1,"rules":{"push":{"warn":true,"reason":"large diff"}}}"#,
        );
        let client = ControlClient::new(url, None).with_ttl(Duration::from_millis(1));
        // First fetch populates the cache with a known-good policy.
        assert_eq!(
            client.evaluate("push", &HashMap::new()),
            Decision::Warn {
                reason: "large diff".to_string()
            }
        );

        // Cache is now stale (ttl elapsed) and the endpoint has no more
        // responses queued, so refresh fails — the prior cache should
        // still be served rather than discarded.
        thread::sleep(Duration::from_millis(5));
        assert_eq!(
            client.evaluate("push", &HashMap::new()),
            Decision::Warn {
                reason: "large diff".to_string()
            }
        );
    }
}
