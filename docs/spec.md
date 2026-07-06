# Harness OTel Rust SDK — Specification

**Date:** 2026-07-06  
**Author:** Sunil Gattupalle  
**Status:** Draft  
**Reference:** [harness/otel-python-sdk](https://github.com/harness/otel-python-sdk)

---

## Goal

Build a thin Rust OTel SDK crate (`harness-otel-sdk`) that mirrors the Python SDK's contract, so git-ai (and any future Rust services) ship telemetry to the same backend using the same protocol, auth, and conventions as the Python/JS SDKs.

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

If the endpoint does not end with `/v1/traces`, the OTLP HTTP exporter appends it automatically (standard behavior, matches Python SDK).

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
spans → SpanAttributesProcessor → BatchSpanProcessor → OtlpHttpExporter
```

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

### BatchSpanProcessor

Use standard `opentelemetry_sdk::trace::BatchSpanProcessor` with defaults. No custom retry logic — the OTel SDK handles retries internally.

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
├── src/
│   ├── lib.rs              # Re-exports, HarnessOtel builder
│   ├── config.rs           # Config loading (env + YAML + builder)
│   ├── resource.rs         # Resource attribute construction
│   ├── exporter.rs         # Exporter factory (HTTP/gRPC selection)
│   ├── processors.rs       # SpanAttributesProcessor
│   └── validation.rs       # Endpoint/token validation rules
└── tests/
    ├── config_test.rs
    ├── validation_test.rs
    └── integration_test.rs # Mock collector, verify span shape
```

### Dependencies

```toml
[dependencies]
opentelemetry = "0.32"
opentelemetry_sdk = { version = "0.32", features = ["trace", "rt-smol"] }
opentelemetry-otlp = { version = "0.32", features = ["http-proto", "grpc-tonic"] }
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
```

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
| Endpoint normalization | Auto-append `/v1/traces` | Auto (standard behavior) | Manual |
| Compression | gzip (optional) | gzip (optional) | None |
| TLS enforcement | Configurable (secure flag) | Default ON, must opt out | None |
| Account/org derivation | Backend-side from token | Backend-side from token | Client-side (non-standard) |

---

## Open Questions

1. **Separate repo or workspace crate?** If other Rust services at Harness will use this, separate repo (like the Python SDK). If only git-ai, workspace crate is simpler.
2. **Async runtime**: Python SDK doesn't care (GIL). Rust needs a choice. git-ai uses `smol` — should the SDK be runtime-agnostic (use `opentelemetry_sdk::rt::Tokio` vs `rt::Smol`) or pick one?
3. **Config file format**: Python uses YAML. Should Rust match (add `serde_yaml` dep) or use the existing `config.json` that git-ai already reads?
4. **Legacy endpoint sunset date**: When can `DualWrite` and the legacy SEI POST path be removed?
