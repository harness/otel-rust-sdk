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

## Configuration

| Env var | Purpose | Default |
|---|---|---|
| `HA_SERVICE_NAME` | Service name resource attribute | `otel-sdk` |
| `HA_REPORTING_ENDPOINT` | OTLP collector endpoint | `http://localhost:5442` |
| `HA_REPORTING_TOKEN` | Auth token (`x-harness-service-token` header) | none |
| `HA_REPORTING_PROTOCOL` | `otlp_http` \| `otlp_grpc` | `otlp_http` |
| `HA_REPORTING_COMPRESSION` | `none` \| `gzip` | `none` |
| `HA_REPORTING_SECURE` | Require `https://` (`false` to opt out) | `true` |
| `HA_CONFIG_FILE` | Path to a YAML config file (see below) | none |

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

## Design

See [`docs/spec.md`](docs/spec.md) for the full design rationale, the
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
