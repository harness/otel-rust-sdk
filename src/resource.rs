use std::collections::HashMap;

use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;

/// Builds the OTel `Resource` attached to every span emitted by this SDK.
///
/// Matches the attribute set the Python SDK sets on its `TracerProvider`:
/// `service.name`, `telemetry.sdk.name`, `telemetry.sdk.language`,
/// `telemetry.sdk.version`, plus any user-supplied resource attributes.
///
/// Deliberately does NOT set `org_id` / `project_id` / `environment` —
/// the backend derives identity from the auth token, matching the
/// Python/JS SDKs. Do not add per-tenant attributes here.
pub fn build(service_name: &str, user_attributes: &HashMap<String, String>) -> Resource {
    let mut attrs = vec![
        KeyValue::new("service.name", service_name.to_string()),
        KeyValue::new("telemetry.sdk.name", "harness_sdk"),
        KeyValue::new("telemetry.sdk.language", "rust"),
        KeyValue::new("telemetry.sdk.version", env!("CARGO_PKG_VERSION")),
    ];

    for (k, v) in user_attributes {
        attrs.push(KeyValue::new(k.clone(), v.clone()));
    }

    Resource::builder().with_attributes(attrs).build()
}
