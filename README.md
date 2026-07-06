# harness-otel-sdk

Harness OTel SDK for Rust. Ships telemetry using the same wire contract as
the Harness [Python SDK](https://github.com/harness/otel-python-sdk):
standard OTLP (HTTP/protobuf or gRPC), auth via a single
`x-harness-service-token` header, and no client-side org/account
derivation — the backend resolves identity from the token.

This crate is intentionally thin: it configures an OTel `TracerProvider`
and exporter for you; callers create spans with the standard
`opentelemetry` API.

## Usage

```rust
use harness_otel_sdk::HarnessOtel;
use opentelemetry::trace::Tracer;

let otel = HarnessOtel::builder()
    .service_name("git-ai")
    .endpoint("https://otel.harness.io")
    .token(std::env::var("HA_REPORTING_TOKEN")?)
    .build()?;

let tracer = otel.tracer("git-ai-telemetry");
tracer.in_span("session_summary", |_cx| {
    // ... do work ...
});

otel.shutdown()?;
```

Or configure entirely from environment variables / a config file:

```rust
let otel = harness_otel_sdk::HarnessOtel::from_config()?;
```

See `examples/basic.rs` for a runnable example.

### Custom span processors and samplers

Register additional `SpanProcessor`s (e.g. redaction, custom enrichment) or
override the `Sampler` on the builder. Processors run after the built-in
`SpanAttributesProcessor` (if `span_attribute()` is used) and before
batching/export. Processors must only observe/modify/drop spans — they must
never affect caller control flow (see "Control Plane" below for why blocking
decisions live in a separate API):

```rust
let otel = HarnessOtel::builder()
    .service_name("git-ai")
    .with_span_processor(MyRedactionProcessor::new())
    .with_sampler(MyCustomSampler::new())
    .build()?;
```

### Batching

`with_batching()` chooses how spans are handed off to the exporter:

```rust
use harness_otel_sdk::Batching;

let otel = HarnessOtel::builder()
    .service_name("git-ai")
    .with_batching(Batching::Simple) // synchronous, guaranteed-before-exit export
    .build()?;
```

- `Batching::Batch` (default) buffers spans and exports them on a
  background thread. Fits long-running processes.
- `Batching::Simple` exports each span synchronously as it ends, with no
  background thread. Fits short-lived processes (e.g. a one-shot CLI
  invocation) that need every span flushed before exit.

Neither variant requires an async runtime — the exporter's HTTP client is
blocking (`reqwest-blocking-client`), so both work from a plain synchronous
`fn main()`.

## Configuration

| Env var | Purpose | Default |
|---|---|---|
| `HA_SERVICE_NAME` | Service name resource attribute | `otel-sdk` |
| `HA_REPORTING_ENDPOINT` | OTLP collector endpoint (a bare host is fine — `v1/traces` is appended automatically for `otlp_http` if missing) | `http://localhost:5442` |
| `HA_REPORTING_TOKEN` | Auth token (`x-harness-service-token` header) | none |
| `HA_REPORTING_PROTOCOL` | `otlp_http` \| `otlp_grpc` | `otlp_http` |
| `HA_REPORTING_COMPRESSION` | `none` \| `gzip` | `none` |
| `HA_REPORTING_SECURE` | Require `https://` (`false` to opt out) | `true` |
| `HA_CONFIG_FILE` | Path to a YAML config file (see below) | none |
| `HA_CONTROL_ENDPOINT` | Control-plane policy-fetch endpoint (`control` feature only) | telemetry endpoint's host |

Endpoints must be `https://` unless `HA_REPORTING_SECURE=false` or the
endpoint is `localhost`/`127.0.0.1`. A non-empty token is required for any
non-localhost endpoint. Both are enforced at build time via
`Config::validate()` — this SDK will not silently send secrets over
plaintext HTTP.

### Config file

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

## Control plane (`control` feature)

Separate from the telemetry pipeline above, the optional `control` feature
adds a synchronous "should this operation proceed?" check backed by remote
policy, fetched over its own wire contract (never OTLP):

```toml
harness-otel-sdk = { version = "...", features = ["control"] }
```

```rust
use harness_otel_sdk::{ControlClient, Decision};

let client = ControlClient::from_config(&config);

match client.evaluate("push", &attrs) {
    Decision::Allow => { /* proceed */ }
    Decision::Warn { reason } => {
        eprintln!("warning: {reason}");
        // proceed anyway — Warn does not block.
    }
}
```

Key properties:

- **Fail-open.** If policy can't be fetched (network error, unreachable
  collector, no cache yet), `evaluate()` returns `Decision::Allow` — it never
  blocks or panics.
- **`Warn`-only for now.** `Decision::Block` is not implemented yet; see
  [`docs/harness-otel-sdk-specification.md`](docs/harness-otel-sdk-specification.md)
  for the phased rollout plan.
- **Cached + bounded fetch.** Policy is cached with a TTL (default 60s);
  `evaluate()` never blocks on the network beyond a bounded-timeout
  background-style refresh, and a stale cache is served rather than
  discarded if a refresh fails or returns an unrecognized schema version.
- **The SDK never blocks itself.** `ControlClient` only returns a
  `Decision` — the caller decides what to do with it. This mirrors the
  Python SDK's `ControlRegistry`/`ControlResult` pattern.
- Configure via `HA_CONTROL_ENDPOINT` (falls back to the telemetry
  endpoint's host if unset — override explicitly in production).

## Design

See [`docs/harness-otel-sdk-specification.md`](docs/harness-otel-sdk-specification.md) for the full design rationale, the
compatibility matrix against the Python SDK, and the migration plan for
consumers currently using a hand-rolled OTLP exporter.

## Development

Pinned toolchain: Rust 1.93.0 (matches
[fork-gitai](https://github.com/harness/fork-gitai), the primary initial
consumer of this crate).

```bash
cargo build
cargo test
cargo clippy --all-targets
cargo fmt -- --check
cargo run --example basic

# with the control-plane feature
cargo build --features control
cargo test --features control
cargo clippy --all-targets --features control
```

## Non-goals

- **No auto-instrumentation.** Unlike the Python SDK (which patches
  Flask/Django/OpenAI/etc. at import time), this crate does not hook into
  any framework. Rust has no equivalent import-hook mechanism; consumers
  instrument manually.
- **No custom wire format.** All export goes through the standard
  `opentelemetry-otlp` crate. If you need a new transport or encoding,
  that belongs upstream in `opentelemetry-otlp`, not in this crate.
- **No client-side tenant derivation.** `org_id` / `project_id` /
  `account_id` are never computed client-side from the token — the
  backend owns that mapping.
