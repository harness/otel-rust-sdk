# Harness OTel Rust SDK — Specification

**Date:** 2026-07-06  
**Author:** Sunil Gattupalle  
**Status:** Draft  
**Reference:** [harness/otel-python-sdk](https://github.com/harness/otel-python-sdk)

---

## Goal

Build a thin Rust OTel SDK crate (`harness-otel-sdk`) that mirrors the Python SDK's contract, so git-ai (and any future Rust services) ship telemetry to the same backend using the same protocol, auth, and conventions as the Python/JS SDKs.

---

## High-Level Architecture

**Consumers stay separate. `harness-otel-sdk` stays separate. They never call
into each other.** The dependency is one-directional: a consumer (git-ai, or
any other Rust service) depends on `harness-otel-sdk` as a library (calls
`tracer.in_span(...)`, and eventually `control.evaluate(...)`), and the SDK
never knows anything about any specific consumer. The consumer owns *when*
something happens and *what to do about a decision*; the SDK owns *how to
ship data* and *how to fetch/cache a decision*. That's the whole architecture
in one sentence — every other section in this document is detail supporting
that.

Concretely, this crate exposes two independent paths, both consumed the same
way (a library call from consumer code — never the reverse):

- **Data path** — telemetry export. The consumer creates spans describing
  what happened (`tracer.in_span("commit", ...)`) and the SDK batches,
  exports, and delivers them to the Harness OTel Collector over standard
  OTLP. Async, best-effort, fire-and-forget from the consumer's perspective.
  Everything in the "Transport", "Resource Attributes", and "Span Processing"
  sections below describes this path. This is the only path implemented
  today.
- **Control path** — policy/config decisions. The consumer asks the SDK a
  synchronous question ("should this operation proceed?") before acting, and
  the SDK answers from cached/fetched policy — but the SDK itself never
  blocks, aborts, or acts on the consumer's behalf; it only returns a
  decision the consumer must act on. Synchronous, in the critical path,
  higher-stakes than the data path (see the "Control Plane" section below for
  the full design). **Not implemented yet** — specified for future work only.

Both paths share the same crate, the same `Config`/auth model, and the same
"consumer calls library, library never calls back" direction — they are
siblings, not layers of one another, and neither is a "plugin" of the other
or of git-ai. See the "Control Plane" section for why blocking specifically
must not be modeled as a telemetry-pipeline plugin (e.g. a `SpanProcessor`).

---

## Design Principles

1. **Use standard `opentelemetry` crates** — no hand-rolled OTLP serialization
2. **Same wire contract as Python SDK** — standard OTLP HTTP/protobuf (or gRPC), same auth header, same resource attributes
3. **Config-driven** — endpoint, token, service name configurable via env vars and/or config file
4. **Minimal surface** — this is a library crate, not a framework. Expose a builder that returns a configured `TracerProvider`. Callers create spans themselves.
5. **No auto-instrumentation** — unlike the Python SDK, Rust doesn't have a runtime hook model. Consumers instrument manually.

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Consumer (git-ai, or any Rust binary)                  │
│                                                         │
│  let provider = HarnessOtel::builder()                  │
│      .service_name("git-ai")                            │
│      .endpoint("https://otel.harness.io")               │
│      .token(token)                                      │
│      .build()?;                                         │
│                                                         │
│  let tracer = provider.tracer("git-ai-telemetry");      │
│  tracer.in_span("session_summary", |cx| { ... });       │
└──────────────────────────┬──────────────────────────────┘
                           │ spans
                           ▼
┌─────────────────────────────────────────────────────────┐
│  harness-otel-sdk (this crate)                          │
│                                                         │
│  • Configures TracerProvider with:                      │
│    - Resource (service.name, sdk.name, sdk.language)    │
│    - BatchSpanProcessor                                 │
│    - OtlpHttpSpanExporter (standard crate)              │
│      with auth headers + compression                    │
│  • Validates config (rejects http://, missing token)    │
│  • Exposes shutdown() for graceful flush                │
└──────────────────────────┬──────────────────────────────┘
                           │ OTLP HTTP/protobuf
                           ▼
┌─────────────────────────────────────────────────────────┐
│  Harness OTel Collector / Backend                       │
└─────────────────────────────────────────────────────────┘
```

---

## Configuration

### Sources (precedence: env > constructor > file > defaults)

| Parameter | Env Var | Default | Required |
|-----------|---------|---------|----------|
| Service name | `HA_SERVICE_NAME` | `"otel-sdk"` | No |
| Endpoint | `HA_REPORTING_ENDPOINT` | `http://localhost:5442` | Yes (for production) |
| Token | `HA_REPORTING_TOKEN` | None | Yes (for production) |
| Protocol | `HA_REPORTING_PROTOCOL` | `otlp_http` | No |
| Compression | `HA_REPORTING_COMPRESSION` | `none` | No |
| TLS enforce | `HA_REPORTING_SECURE` | `true` | No |
| Resource attributes | `HA_RESOURCE_ATTRIBUTES` | `{}` | No |
| Span attributes | `HA_SPAN_ATTRIBUTES` | `{}` | No |

### Config File (optional)

YAML format, path via `HA_CONFIG_FILE` env var:

```yaml
service_name: git-ai
reporting:
  endpoint: https://otel.harness.io
  token: ${HA_REPORTING_TOKEN}
  protocol: otlp_http       # otlp_http | otlp_grpc
  compression: gzip         # none | gzip
  secure: true
resource_attributes:
  deployment.environment: production
```

### Validation Rules

- **Endpoint MUST be `https://`** unless `secure: false` is explicitly set (rejects `http://` by default — addresses the security finding in git-ai)
- **Token MUST be non-empty** for non-localhost endpoints
- Invalid config → return `Err`, never silently degrade

---

## Authentication

Matches the Python SDK exactly:

```
Header: x-harness-service-token: <token>
```

No JWT, no OAuth, no account-ID derivation. The backend resolves identity from the token.

This is the ONLY auth mechanism. The current git-ai code that derives `account_id` from the token and passes it as a query param is non-standard — remove it when migrating to this SDK.

---

## Transport

### Protocol

Standard OTLP HTTP/protobuf via the `opentelemetry-otlp` crate:

```rust
use opentelemetry_otlp::SpanExporter;

let exporter = SpanExporter::builder()
    .with_http()
    .with_endpoint(endpoint)           // e.g. "https://otel.harness.io"
    .with_headers(headers)             // x-harness-service-token
    .with_protocol(Protocol::HttpBinary)  // protobuf encoding
    .with_compression(compression)     // gzip if configured
    .build()?;
```

### Endpoint Normalization

If the endpoint does not already contain `v1/traces`, the SDK appends it before
handing the endpoint to the exporter. This is done explicitly by this crate
(`normalize_http_traces_endpoint` in `src/exporter.rs`), not by
`opentelemetry-otlp` itself — that crate only auto-appends the path when an
endpoint is picked up from the `OTEL_EXPORTER_OTLP_ENDPOINT` env-var fallback;
an endpoint supplied programmatically via `.with_endpoint()` (which is what
this SDK always does) is used verbatim. Mirrors the Python SDK's
`normalize_otlp_http_traces_endpoint`, so a bare host configured the same way
in both SDKs behaves the same way.

### gRPC Alternative

Support `otlp_grpc` protocol option for environments where gRPC is preferred:

```rust
let exporter = SpanExporter::builder()
    .with_tonic()
    .with_endpoint(endpoint)
    .with_metadata(metadata)           // token as gRPC metadata
    .with_compression(Compression::Gzip)
    .build()?;
```

---

## Resource Attributes

Set on the `TracerProvider` resource. Matches Python SDK conventions:

```rust
Resource::builder()
    .with_attributes([
        KeyValue::new("service.name", service_name),
        KeyValue::new("telemetry.sdk.name", "harness_sdk"),
        KeyValue::new("telemetry.sdk.language", "rust"),
        KeyValue::new("telemetry.sdk.version", env!("CARGO_PKG_VERSION")),
    ])
    .with_attributes(user_resource_attributes)
    .build()
```

**No `org_id`, `project_id`, `environment` as resource attributes** — the Python SDK doesn't set these. The backend derives org/project from the token. The current git-ai code that sets these is non-standard and should be removed.

---

## Span Processing

### Default Pipeline

```
spans → SpanAttributesProcessor → [consumer-supplied processors] → BatchSpanProcessor|SimpleSpanProcessor → OtlpExporter
```

`SpanAttributesProcessor` only runs if `span_attribute()` was called at
least once on the builder. Any processors registered via
`with_span_processor()` run after it and before the batching stage.

### SpanAttributesProcessor

Injects configured `span_attributes` into every span (mirrors Python SDK's `SpanAttributesProcessor`):

```rust
impl SpanProcessor for SpanAttributesProcessor {
    fn on_end(&self, span: &mut SpanData) {
        for (k, v) in &self.attributes {
            span.attributes.push(KeyValue::new(k.clone(), v.clone()));
        }
    }
}
```

### Batching: BatchSpanProcessor vs. SimpleSpanProcessor

`with_batching()` selects between the standard
`opentelemetry_sdk::trace::BatchSpanProcessor` (default) and
`SimpleSpanProcessor`, both used with SDK defaults — no custom retry logic,
the OTel SDK handles retries internally. See the `Batching` enum's doc
comment in `src/lib.rs` for the tradeoff, in particular that
`SimpleSpanProcessor::force_flush` is a no-op in this SDK version and
cannot surface export failures — callers that need to detect failures
(e.g. queue-for-retry logic) must use `Batching::Batch`.

---

## Public API

### Builder Pattern

```rust
use harness_otel_sdk::HarnessOtel;

// Minimal
let provider = HarnessOtel::builder()
    .service_name("git-ai")
    .endpoint("https://otel.harness.io")
    .token("sat.abc123...")
    .build()?;

// Full
let provider = HarnessOtel::builder()
    .service_name("git-ai")
    .endpoint("https://otel.harness.io")
    .token("sat.abc123...")
    .protocol(Protocol::OtlpHttp)
    .compression(Compression::Gzip)
    .resource_attribute("deployment.environment", "production")
    .span_attribute("git.tool", "git-ai")
    .build()?;

// From config file / env vars
let provider = HarnessOtel::from_config()?;
```

### Shutdown

```rust
provider.shutdown()?;  // Flushes pending spans, blocks until complete
```

### Tracer Access

```rust
let tracer = provider.tracer("component-name");
```

---

## Crate Structure

```
harness-otel-sdk/
├── Cargo.toml
├── examples/
│   └── basic.rs
├── src/
│   ├── lib.rs              # Re-exports, HarnessOtel/HarnessOtelBuilder, Batching — unit
│   │                       # tests inline (#[cfg(test)] mod tests)
│   ├── config.rs           # Config loading (env + YAML + builder) and validation —
│   │                       # unit tests inline
│   ├── resource.rs         # Resource attribute construction
│   ├── exporter.rs         # Exporter factory (HTTP/gRPC selection, endpoint
│   │                       # normalization) — unit tests inline
│   ├── processors.rs       # SpanAttributesProcessor
│   ├── error.rs            # Error, Result
│   └── control.rs          # ControlClient, Decision — behind `features = ["control"]`.
│                           # Warn-only; see "Control Plane" section.
```

Endpoint/token validation lives in `Config::validate()` in `config.rs`
rather than a separate `validation.rs` module. There is no top-level
`tests/` directory — all tests are inline `#[cfg(test)]` modules next to
the code they exercise; the mock-collector integration test lives in the
`fork-gitai` consumer repo (`tests/common/mod.rs`), not in this crate.

### Dependencies

```toml
[dependencies]
opentelemetry = "0.32"
opentelemetry_sdk = { version = "0.32", default-features = false, features = ["trace"] }
opentelemetry-otlp = { version = "0.32", default-features = false, features = ["trace", "http-proto", "grpc-tonic", "reqwest-blocking-client"] }
tonic = { version = "0.14", default-features = false }
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"

[features]
control = ["dep:ureq", "dep:serde_json"] # implemented — see src/control.rs
```

`tonic` is a direct dependency (not just pulled in transitively via
`opentelemetry-otlp`'s `grpc-tonic` feature) because `Protocol::OtlpGrpc`
needs its channel types directly in `exporter.rs`.

The exporter deliberately uses `reqwest-blocking-client`, not the async
`reqwest-client`: the blocking client needs no runtime at all, so both
`Batching::Batch` and `Batching::Simple` (see Public API) work from a plain
synchronous `fn main()` — no `tokio`/`smol` setup required by the SDK or the
consumer. This resolves Open Question 2 below: the SDK is runtime-agnostic
by construction, not by picking one of `rt::Tokio`/`rt::Smol`.

---

## Migration Plan for git-ai

### What to Remove from git-ai

1. `src/telemetry/otel_exporter.rs` — entire file (HarnessSpanExporter, OtlpJsonEncoder)
2. `src/telemetry/otel.rs` — replace `OtelRecorder::from_config` with `HarnessOtel::from_config`
3. `src/telemetry/otel_event.rs` — keep (defines event shapes), but remove custom serialization
4. `OtelIngestMode::DualWrite` — remove, switch to `OtelPrimary` only
5. Account-ID-from-token derivation in exporter — remove (backend handles this)
6. `minreq` dependency for OTel export — remove (opentelemetry-otlp uses its own HTTP client)

### What to Keep

- `sei_push.rs` orchestration logic (offset tracking, batch management, retry-on-failure)
- Session tracker, parsers, watcher — these produce the data, unchanged
- The local durability model (usage.jsonl as write-ahead store)

### Migration Steps

1. Publish `harness-otel-sdk` as a workspace crate (or separate repo if shared across Rust projects)
2. Add as dependency to git-ai's `Cargo.toml`
3. Replace `OtelRecorder::from_config` → `HarnessOtel::from_config()`
4. Replace `emit_*_otel_events` to use the standard tracer API instead of custom span builders
5. Remove `DualWrite` mode, legacy SEI POST path
6. Verify with mock collector that spans match Python SDK's wire format
7. Deploy behind feature flag, validate in staging, then make default

---

## Compatibility Matrix

| Aspect | Python SDK | Rust SDK (this spec) | Current git-ai |
|--------|-----------|---------------------|----------------|
| Auth header | `x-harness-service-token` | `x-harness-service-token` | `x-harness-service-token` |
| Protocol | OTLP HTTP/protobuf | OTLP HTTP/protobuf | Custom OTLP-JSON |
| Exporter | Standard `OTLPHttpSpanExporter` | Standard `opentelemetry-otlp` | Hand-rolled `HarnessSpanExporter` |
| Resource attrs | service.name, sdk.name/version/language | Same | service.name + org_id/project_id (non-standard) |
| Endpoint normalization | Auto-append `v1/traces` (SDK-side, `normalize_otlp_http_traces_endpoint`) | Auto-append `v1/traces` (SDK-side, `normalize_http_traces_endpoint`) | Manual |
| Compression | gzip (optional) | gzip (optional) | None |
| TLS enforcement | Configurable (secure flag) | Default ON, must opt out | None |
| Account/org derivation | Backend-side from token | Backend-side from token | Client-side (non-standard) |

---

## Open Questions

1. **Separate repo or workspace crate?** If other Rust services at Harness will use this, separate repo (like the Python SDK). If only git-ai, workspace crate is simpler.
2. ~~**Async runtime**~~ — Resolved: the exporter uses `reqwest-blocking-client`, which needs no async runtime at all. Both `Batching::Batch` and `Batching::Simple` work from git-ai's plain synchronous `fn main()` (no `smol` setup needed for the SDK's own export path). `Batching::Simple` additionally guarantees synchronous, before-exit export for short-lived call sites like `PostCommit`.
3. **Config file format**: Python uses YAML. Should Rust match (add `serde_yaml` dep) or use the existing `config.json` that git-ai already reads?
4. **Legacy endpoint sunset date**: When can `DualWrite` and the legacy SEI POST path be removed?

---

## Control Plane (Future Work — Design Decided, Not Yet Implemented)

### Why this exists

Beyond telemetry export, the SDK will eventually need to answer a synchronous
"should this operation proceed?" question — e.g. a git hook asking whether a
commit/push should be blocked, warned about, or allowed, based on remote
policy/config. This is a **control-plane** concern, categorically different
from telemetry:

| | Telemetry (data path) | Control (control path) |
|---|---|---|
| Timing | Async, batched, fire-and-forget | Synchronous, blocking, in the critical path |
| Failure impact | Lost/delayed span — minor gap | Bug/outage can block every `git commit`/`push` org-wide |
| Existing precedent | This crate, as built | None yet in Rust; exists in Python SDK |

This asymmetry — not API convenience — is what drives every decision below.

### Three-axis separation (decided)

1. **Instrumentation (call site)** — stays in the consumer (e.g. git-ai's
   hook modules). The SDK never creates spans on the consumer's behalf; the
   consumer calls `tracer.in_span(...)` directly. Unaffected by anything
   below.
2. **Telemetry pipeline (passive)** — standard OTel `SpanProcessor` /
   `Sampler` traits, exposed via `HarnessOtelBuilder::with_span_processor()`
   and `with_sampler()` (not yet added to the builder — see Action Items).
   These may only modify, drop, or sample spans that already exist. They
   must **never** raise, panic, or otherwise affect the caller's control
   flow — this must remain a hard invariant, enforced by code review, not
   just convention.
3. **Control plane (active/blocking)** — a wholly separate API surface,
   `ControlClient::evaluate(operation, attrs) -> Decision`. The SDK itself
   never blocks, panics, or forces control flow — it only returns data. The
   **caller** is responsible for inspecting the `Decision` and acting on it
   (returning `Err`, printing a warning, exiting).

Folding control into a `SpanProcessor` was explicitly considered and
rejected: processors run in batch, typically after the operation has already
completed, so they are structurally incapable of blocking pre-execution
anyway. A processor that tried to block would be "spooky action at a
distance" — the caller would be halted by code it never explicitly invoked.

### Precedent: this matches the Python SDK exactly

Verified by reading `harness/otel-python-sdk` source directly (not assumed):

- `ControlRegistry.evaluate()` (`plugins/control.py:94-131`) returns a
  `ControlResult` dataclass (`.block`, `.response_status_code`, ...). The
  registry **never raises or aborts** — every instrumentation wrapper
  (Flask, FastAPI, gRPC, Django, GenAI) inspects `.block` itself and decides
  independently whether to abort/return-early/raise. This is the same
  decision-object-returned-to-caller shape as our proposed `Decision` enum.
- Control plugins register through a **genuinely different interface**
  (`ControlPlugin` protocol: `evaluate` / `evaluate_agent_span` / `on_init` /
  `shutdown`) from observability plugins (`SpanProcessor`), via a separate
  entry-point group (`harness_sdk_control_plugin` vs
  `harness_sdk_observability_plugin`). This validates keeping `control` as
  its own module/Cargo feature rather than folding it into the
  `SpanProcessor` extension point.
- **One anti-pattern to avoid, not replicate**: `DbControlSpanProcessor`
  (`db_control_span_processor.py:9-42`) is a `SpanProcessor` that internally
  calls `ControlRegistry.evaluate()` and silently drops the span on
  `.block`, swallowing evaluation exceptions. It blurs the line every other
  Python call site keeps clean. Do not mirror this in Rust — keep
  `SpanProcessor` impls telemetry-only, always.
- This is also the standard Policy Decision Point / Policy Enforcement
  Point (PDP/PEP) split (e.g. OPA) — the SDK is the PDP, the caller is the
  PEP. We arrived at it independently; it is also established practice.

### Design decisions for `ControlClient` (Rust)

- **Fail-open is the default.** If the backend/policy is unreachable, the
  operation proceeds, but a span/event records that the check was
  degraded/skipped (e.g. `control.status = "degraded"`), so outages are
  observable via telemetry, not silent. Fail-closed (block when policy is
  unreachable) must be an explicit per-policy opt-in, reserved for cases
  where an org has deliberately decided disruption is preferable to risk
  (e.g. active secret-leak prevention).
- **`Warn` ships before `Block`.** `Decision::Warn` has none of the
  fail-open/fail-closed risk calculus of `Decision::Block` and lets the
  fetch/cache/decision plumbing be validated in production before anything
  can actually stop a workflow. During this phase, a bare `Decision` enum is
  sufficient — `GuardedOperation`/`Permit` (below) exists specifically to
  make bypassing a `Block` harder, and adds pure friction with no benefit
  while only `Allow`/`Warn` exist. Introduce `GuardedOperation` only when
  `Decision::Block` is actually added, not before.
- **Caching + bounded-latency fetch is mandatory, not optional.** Bootstrap
  from a local cache, background-refresh with a TTL, never block the
  calling thread on a slow/hung network call for more than a tight timeout
  (feature-flag-SDK pattern, e.g. LaunchDarkly). A `ControlClient` that
  blocks on network on every call with no cache is a reliability hazard by
  construction. **Unknown/newer schema version on a fetch**: fail open on
  the fetch itself (per "Version the policy schema..." below), but continue
  serving the last-known-good cached decision (if any and not yet expired)
  rather than clearing it — a schema mismatch means "can't parse the new
  policy," not "the old policy is now invalid." Only fall through to `Allow`
  when there is no usable cached value at all.
- **Separate wire contract from telemetry.** Policy/config fetch is not a
  tracing signal and must not be smuggled through OTLP spans. It needs its
  own endpoint config (`HA_CONTROL_ENDPOINT`, distinct from
  `HA_REPORTING_ENDPOINT`) and its own simple versioned fetch protocol
  (REST or gRPC, decided later — not yet needed).
- **Timing contract: `evaluate()` must be called before mutation.** The
  consumer must call it before performing the guarded action, not after —
  e.g. in a git-hook consumer, only a `pre-*` hook is a safe enforcement
  point; checking in a `post-*` hook is already too late. This must be
  documented as a hard contract for consumers, once built.
- **Type-level nudge against "forgot to call evaluate()" (from `Block`
  onward only).** Rather than a bare `Decision` enum a caller can silently
  ignore, once `Decision::Block` exists prefer a `GuardedOperation`-style
  wrapper that requires destructuring before the guarded action can run,
  e.g.:

  ```rust
  let permit = control.evaluate("some_operation", &attrs)?;
  permit.proceed(|| do_the_operation())?;  // only callable on Allow
  ```

  This doesn't fully enforce the check (Rust has no way to force a call
  site to invoke something), but it raises the bar from "just don't call a
  function" to "deliberately ignore a returned value."

### Release & versioning strategy (decided)

- **Cargo-feature-gate the control module**: `features = ["control"]`, off
  by default. Telemetry-only consumers must not pull in a policy-fetch
  network client or be exposed to control-path release churn.
- **Version the policy schema independently of the crate's SemVer.**
  Negotiate a schema version at fetch time so the backend can evolve
  policies without a client redeploy. Unknown schema version on a fetch →
  fail open on that fetch (ignore/skip the new response), but keep serving
  the last-known-good cached decision per the caching rule above — never
  fail closed, and don't discard a still-valid cache just because the next
  refresh returned an unparseable version.
- **Control-path changes require staged/canary rollout; telemetry-path
  changes do not.** Telemetry bugs cost data; control-plane bugs can block
  every consumer's guarded operation org-wide. Treat changes to
  `ControlClient` or evaluation logic like changes to an authz system —
  staged by org/team percentage, with an independent kill switch to fully
  disable control checks (fall back to fail-open) without requiring a new
  SDK release.
- **Self-observability is a release gate for `control`, not a nice-to-have.**
  Before shipping to more than one pilot consumer, must have a
  metric/span for "control check attempted / degraded / blocked" so an
  incident is detectable within minutes.
- **Do not build `ControlClient` until there is a real consumer and a real
  policy use case.** Reserve the module path (`pub mod control;` behind the
  feature flag) and keep this section as the intended shape, but do not
  ship a stub implementation — a documented intent costs nothing; dead code
  costs ongoing maintenance.

### Action items when control-plane work actually starts

1. Add `with_span_processor()` / `with_sampler()` to `HarnessOtelBuilder`
   (telemetry-pipeline extensibility) — this can land independently and
   earlier than any control-plane code, since it's the direct Rust
   equivalent of Python's observability-plugin extension point and has none
   of the control path's risk profile.
2. Add `control` Cargo feature, `src/control.rs` module, `ControlClient`,
   `Decision` (or `GuardedOperation`/`Permit`), `HA_CONTROL_ENDPOINT` config.
3. Implement cache + TTL + bounded-timeout fetch before any real policy
   logic.
4. Ship `Decision::Warn` only, end-to-end, before ever adding
   `Decision::Block`.
5. Wire "control check degraded/blocked" into the telemetry pipeline
   (span/event) so control-plane health is observable via the same backend.
6. Define the kill-switch mechanism (config flag, checked before any
   `evaluate()` call) before enabling `Decision::Block` for any consumer.
