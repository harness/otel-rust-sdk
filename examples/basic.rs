//! Minimal usage example.
//!
//! Run with a local collector listening on http://localhost:5442, or set
//! `HA_REPORTING_ENDPOINT` / `HA_REPORTING_TOKEN` to point at a real one:
//!
//!   cargo run --example basic

use harness_otel_sdk::HarnessOtel;
use opentelemetry::trace::Tracer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let otel = HarnessOtel::builder()
        .service_name("git-ai-example")
        .endpoint("http://localhost:5442")
        .secure(false) // local dev collector, not https
        .build()?;

    let tracer = otel.tracer("basic-example");
    tracer.in_span("example-span", |_cx| {
        println!("doing work inside a traced span");
    });

    otel.shutdown()?;
    Ok(())
}
