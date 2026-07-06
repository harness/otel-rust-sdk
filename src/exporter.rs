use std::collections::HashMap;
use std::time::Duration;

use opentelemetry_otlp::tonic_types::metadata::MetadataMap;
use opentelemetry_otlp::{
    Compression as OtlpCompression, Protocol as OtlpWireProtocol, SpanExporter, WithExportConfig,
    WithHttpConfig, WithTonicConfig,
};

use crate::config::{Compression, Config, Protocol};
use crate::error::{Error, Result};

const AUTH_HEADER: &str = "x-harness-service-token";

/// Builds the OTLP span exporter for the given resolved config.
///
/// Uses the standard `opentelemetry-otlp` crate exclusively — no custom
/// serialization or transport. Auth is a single header
/// (`x-harness-service-token`), matching the Python/JS SDKs; there is no
/// account-ID derivation or query-param auth on the Rust side.
pub fn build(config: &Config) -> Result<SpanExporter> {
    let endpoint = config.endpoint();
    let mut headers = HashMap::new();
    if let Some(token) = &config.reporting.token {
        if !token.is_empty() {
            headers.insert(AUTH_HEADER.to_string(), token.clone());
        }
    }

    match config.reporting.protocol {
        Protocol::OtlpHttp => {
            let mut builder = SpanExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .with_protocol(OtlpWireProtocol::HttpBinary)
                .with_headers(headers)
                .with_timeout(Duration::from_secs(10));
            if config.reporting.compression == Compression::Gzip {
                builder = builder.with_compression(OtlpCompression::Gzip);
            }
            builder
                .build()
                .map_err(|e| Error::ExporterBuild(e.to_string()))
        }
        Protocol::OtlpGrpc => {
            let mut builder = SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_metadata(headers_to_metadata(&headers))
                .with_timeout(Duration::from_secs(10));
            if config.reporting.compression == Compression::Gzip {
                builder = builder.with_compression(OtlpCompression::Gzip);
            }
            builder
                .build()
                .map_err(|e| Error::ExporterBuild(e.to_string()))
        }
    }
}

fn headers_to_metadata(headers: &HashMap<String, String>) -> MetadataMap {
    let mut map = MetadataMap::new();
    for (k, v) in headers {
        if let (Ok(key), Ok(value)) = (
            tonic::metadata::MetadataKey::from_bytes(k.as_bytes()),
            v.parse(),
        ) {
            map.insert(key, value);
        }
    }
    map
}
