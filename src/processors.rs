use std::collections::HashMap;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanProcessor};
use opentelemetry_sdk::Resource;

/// Injects a fixed set of attributes into every span on export.
///
/// Mirrors the Python SDK's `SpanAttributesProcessor` — used for
/// `span_attributes` configured on the SDK (as opposed to `resource_attributes`,
/// which apply to the whole process).
#[derive(Debug)]
pub struct SpanAttributesProcessor {
    attributes: Vec<KeyValue>,
}

impl SpanAttributesProcessor {
    pub fn new(attributes: &HashMap<String, String>) -> Self {
        let attributes = attributes
            .iter()
            .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
            .collect();
        Self { attributes }
    }
}

impl SpanProcessor for SpanAttributesProcessor {
    fn on_start(&self, span: &mut opentelemetry_sdk::trace::Span, _cx: &opentelemetry::Context) {
        let _ = span;
    }

    fn on_end(&self, mut span: SpanData) {
        span.attributes.extend(self.attributes.iter().cloned());
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
