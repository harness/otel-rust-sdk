//! Harness OTel SDK for Rust.
//!
//! Ships telemetry using the same wire contract as the Harness Python/JS
//! SDKs: standard OTLP (HTTP/protobuf or gRPC), auth via the
//! `x-harness-service-token` header, and no client-side org/account
//! derivation — the backend resolves identity from the token.
//!
//! See `docs/specs/otel-rust-sdk-spec.md` in `fork-gitai` for the design
//! rationale and migration plan.

mod config;
#[cfg(feature = "control")]
mod control;
mod error;
mod exporter;
mod processors;
mod resource;

#[cfg(feature = "control")]
pub use control::{ControlClient, Decision};

pub use config::{Compression, Config, Protocol, ReportingConfig};
pub use error::{Error, Result};

use std::collections::HashMap;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{SdkTracerProvider, ShouldSample, SpanProcessor};

/// A configured Harness OTel pipeline. Wraps an `SdkTracerProvider`.
pub struct HarnessOtel {
    provider: SdkTracerProvider,
}

impl HarnessOtel {
    pub fn builder() -> HarnessOtelBuilder {
        HarnessOtelBuilder::default()
    }

    /// Build directly from environment variables / `HA_CONFIG_FILE`.
    pub fn from_config() -> Result<Self> {
        let config = Config::from_env()?;
        Self::from_resolved_config(config, Vec::new(), None)
    }

    fn from_resolved_config(
        config: Config,
        extra_processors: Vec<Box<dyn SpanProcessor>>,
        sampler: Option<Box<dyn ShouldSample>>,
    ) -> Result<Self> {
        config.validate()?;
        let resource = resource::build(&config.service_name(), &config.resource_attributes);
        let exporter = exporter::build(&config)?;

        let mut builder = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter);

        if !config.span_attributes.is_empty() {
            builder = builder.with_span_processor(processors::SpanAttributesProcessor::new(
                &config.span_attributes,
            ));
        }

        for processor in extra_processors {
            builder = builder.with_span_processor(processors::BoxedSpanProcessor(processor));
        }

        if let Some(sampler) = sampler {
            builder = builder.with_sampler(sampler);
        }

        Ok(Self {
            provider: builder.build(),
        })
    }

    /// Get a tracer for the given component/instrumentation name.
    pub fn tracer(&self, name: &'static str) -> opentelemetry_sdk::trace::Tracer {
        self.provider.tracer(name)
    }

    /// Flush and shut down the pipeline. Blocks until pending spans are
    /// exported or the shutdown timeout elapses.
    pub fn shutdown(self) -> Result<()> {
        self.provider
            .shutdown()
            .map_err(|e| Error::ExporterBuild(e.to_string()))
    }
}

#[derive(Default)]
pub struct HarnessOtelBuilder {
    service_name: Option<String>,
    endpoint: Option<String>,
    token: Option<String>,
    protocol: Option<Protocol>,
    compression: Option<Compression>,
    secure: Option<bool>,
    resource_attributes: HashMap<String, String>,
    span_attributes: HashMap<String, String>,
    span_processors: Vec<Box<dyn SpanProcessor>>,
    sampler: Option<Box<dyn ShouldSample>>,
}

impl HarnessOtelBuilder {
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = Some(protocol);
        self
    }

    pub fn compression(mut self, compression: Compression) -> Self {
        self.compression = Some(compression);
        self
    }

    /// Explicitly allow a plaintext (`http://`) endpoint. Defaults to
    /// `true` (HTTPS required) if not called.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = Some(secure);
        self
    }

    pub fn resource_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource_attributes.insert(key.into(), value.into());
        self
    }

    pub fn span_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.span_attributes.insert(key.into(), value.into());
        self
    }

    /// Register an additional `SpanProcessor` on the pipeline, e.g. for
    /// redaction or custom enrichment. Runs after the built-in
    /// `SpanAttributesProcessor` (if configured) and before batching/export.
    /// Must only observe/modify/drop spans — never affect caller control
    /// flow. See "Control Plane" in the spec for why blocking decisions
    /// belong in a separate API, not here.
    pub fn with_span_processor(mut self, processor: impl SpanProcessor + 'static) -> Self {
        self.span_processors.push(Box::new(processor));
        self
    }

    /// Set a custom `Sampler` for the pipeline. Defaults to the OTel SDK's
    /// built-in default (always-on) if not called.
    pub fn with_sampler(mut self, sampler: impl ShouldSample + 'static) -> Self {
        self.sampler = Some(Box::new(sampler));
        self
    }

    pub fn build(self) -> Result<HarnessOtel> {
        let config = Config {
            service_name: self.service_name,
            reporting: config::ReportingConfig {
                endpoint: self.endpoint,
                token: self.token,
                protocol: self.protocol.unwrap_or_default(),
                compression: self.compression.unwrap_or_default(),
                secure: self.secure,
                #[cfg(feature = "control")]
                control_endpoint: None,
            },
            resource_attributes: self.resource_attributes,
            span_attributes: self.span_attributes,
        };
        HarnessOtel::from_resolved_config(config, self.span_processors, self.sampler)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use opentelemetry::trace::Tracer;
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::SpanData;
    use opentelemetry_sdk::Resource;

    use super::*;

    #[derive(Debug)]
    struct FlagOnEnd(Arc<AtomicBool>);

    impl SpanProcessor for FlagOnEnd {
        fn on_start(
            &self,
            _span: &mut opentelemetry_sdk::trace::Span,
            _cx: &opentelemetry::Context,
        ) {
        }

        fn on_end(&self, _span: SpanData) {
            self.0.store(true, Ordering::SeqCst);
        }

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            Ok(())
        }

        fn set_resource(&mut self, _resource: &Resource) {}
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn consumer_supplied_span_processor_runs_on_span_end() {
        let flag = Arc::new(AtomicBool::new(false));
        let otel = HarnessOtel::builder()
            .service_name("test")
            .endpoint("http://localhost:5442")
            .secure(false)
            .with_span_processor(FlagOnEnd(flag.clone()))
            .build()
            .expect("build should succeed with a valid local config");

        let tracer = otel.tracer("test");
        tracer.in_span("test-span", |_cx| {});

        assert!(
            flag.load(Ordering::SeqCst),
            "custom span processor's on_end should have run"
        );
    }
}
