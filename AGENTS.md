# AGENTS.md — harness-otel-sdk

## Build & Test

```bash
cargo build                      # debug build
cargo test                       # unit tests (config validation)
cargo clippy --all-targets       # lint — CI treats warnings as errors
cargo fmt -- --check             # format check
cargo fmt                        # auto-format
cargo run --example basic        # runnable usage example
```

Toolchain: Rust 1.93.0, pinned to match `fork-gitai` (the primary
initial consumer). Install via `rustup toolchain install 1.93.0`.

## Architecture

Single-crate library, no workspace. Five modules in `src/`:

- `lib.rs` — public API: `HarnessOtel` (wraps `SdkTracerProvider`) and
  `HarnessOtelBuilder`. This is the only file consumers should need to
  read to use the crate.
- `config.rs` — `Config`/`ReportingConfig`, env var + YAML file loading,
  and `Config::validate()` (rejects plaintext endpoints, missing tokens).
- `exporter.rs` — builds the OTLP exporter (HTTP or gRPC) via the
  standard `opentelemetry-otlp` crate. This is the only place that talks
  to the network.
- `resource.rs` — builds the OTel `Resource` (service.name,
  telemetry.sdk.* attributes). No tenant/org attributes here — see
  `docs/spec.md` for why.
- `processors.rs` — `SpanAttributesProcessor`, injects fixed key/value
  pairs into every span (only wired in when `span_attributes` is
  non-empty).

## Key Conventions

- **No hand-rolled OTLP serialization.** All export goes through
  `opentelemetry-otlp`. If a wire-format change is needed, it belongs in
  that upstream crate or in a version bump, not in this crate's code.
- **Single auth mechanism**: `x-harness-service-token` header, token
  passed as-is. No query params, no derived account IDs — matches the
  Python SDK exactly. Do not add client-side tenant derivation.
- **Fail closed on config**: `Config::validate()` rejects `http://`
  endpoints (unless `secure: false` is explicit or the host is
  localhost) and rejects empty tokens for non-localhost endpoints. Do
  not weaken this to "warn and continue."
- **Error type**: manual `Display`/`std::error::Error` impls in
  `error.rs` (no `thiserror`), matching `fork-gitai`'s convention.

## Why this crate exists

`fork-gitai` (a Rust git-proxy binary) previously shipped telemetry via a
hand-rolled `HarnessSpanExporter` that serialized a custom OTLP-JSON
payload and POSTed it with `minreq`. That diverged from the Python/JS
SDKs in wire format, auth, and resource attributes. This crate is the
shared contract other Rust services (starting with `fork-gitai`) should
depend on instead of re-implementing OTLP export. Full rationale and the
`fork-gitai` migration plan: `docs/spec.md`.

## Gotchas

- `opentelemetry-otlp` 0.32's builder traits (`WithHttpConfig`,
  `WithTonicConfig`, `WithExportConfig`) must be imported explicitly —
  the methods (`.with_headers()`, `.with_metadata()`, `.with_endpoint()`)
  don't resolve without them in scope.
- `opentelemetry_otlp::Compression` has no `None` variant — compression
  is opt-in only; omit the `.with_compression()` call instead of passing
  a "none" value.
- gRPC metadata uses `tonic::metadata::MetadataMap` — re-exported at
  `opentelemetry_otlp::tonic_types::metadata::MetadataMap`, but building
  `MetadataKey`/`MetadataValue` still requires a direct `tonic`
  dependency (see `Cargo.toml`).
