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
mod error;
mod exporter;
mod processors;
mod resource;

pub use config::{Compression, Config, Protocol, ReportingConfig};
pub use error::{Error, Result};

use std::collections::HashMap;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;

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
        Self::from_resolved_config(config)
    }

    fn from_resolved_config(config: Config) -> Result<Self> {
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

    pub fn build(self) -> Result<HarnessOtel> {
        let config = Config {
            service_name: self.service_name,
            reporting: config::ReportingConfig {
                endpoint: self.endpoint,
                token: self.token,
                protocol: self.protocol.unwrap_or_default(),
                compression: self.compression.unwrap_or_default(),
                secure: self.secure,
            },
            resource_attributes: self.resource_attributes,
            span_attributes: self.span_attributes,
        };
        HarnessOtel::from_resolved_config(config)
    }
}
