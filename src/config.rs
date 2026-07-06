use std::collections::HashMap;
use std::env;
use std::fs;

use crate::error::{Error, Result};

/// Wire protocol used to export spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    OtlpHttp,
    OtlpGrpc,
}

/// Export payload compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    #[default]
    None,
    Gzip,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ReportingConfig {
    pub endpoint: Option<String>,
    pub token: Option<String>,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub compression: Compression,
    /// Defaults to `true`. Set to `false` to explicitly allow a plaintext
    /// (`http://`) endpoint — e.g. talking to a local collector in dev.
    pub secure: Option<bool>,
    /// Control-plane policy-fetch endpoint. Deliberately separate from
    /// `endpoint` (telemetry) — policy fetch is not a tracing signal and
    /// must not share the OTLP wire contract. Only used when the `control`
    /// feature is enabled.
    #[cfg(feature = "control")]
    pub control_endpoint: Option<String>,
}

/// Fully resolved SDK configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Config {
    pub service_name: Option<String>,
    #[serde(default)]
    pub reporting: ReportingConfig,
    #[serde(default)]
    pub resource_attributes: HashMap<String, String>,
    #[serde(default)]
    pub span_attributes: HashMap<String, String>,
}

const DEFAULT_SERVICE_NAME: &str = "otel-sdk";
const DEFAULT_ENDPOINT: &str = "http://localhost:5442";

impl Config {
    /// Load configuration from (in precedence order, highest first):
    /// 1. Environment variables (`HA_*`)
    /// 2. YAML file at `HA_CONFIG_FILE`, if set
    /// 3. Built-in defaults
    ///
    /// This mirrors the precedence used by the Harness Python SDK.
    pub fn from_env() -> Result<Self> {
        let mut config = if let Ok(path) = env::var("HA_CONFIG_FILE") {
            Self::from_yaml_file(&path)?
        } else {
            Config::default()
        };

        config.apply_env_overrides();
        Ok(config)
    }

    pub fn from_yaml_file(path: &str) -> Result<Self> {
        let contents =
            fs::read_to_string(path).map_err(|e| Error::ConfigFile(format!("{path}: {e}")))?;
        serde_yaml::from_str(&contents).map_err(|e| Error::ConfigFile(format!("{path}: {e}")))
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = env::var("HA_SERVICE_NAME") {
            self.service_name = Some(v);
        }
        if let Ok(v) = env::var("HA_REPORTING_ENDPOINT") {
            self.reporting.endpoint = Some(v);
        }
        if let Ok(v) = env::var("HA_REPORTING_TOKEN") {
            self.reporting.token = Some(v);
        }
        if let Ok(v) = env::var("HA_REPORTING_PROTOCOL") {
            self.reporting.protocol = match v.as_str() {
                "otlp_grpc" => Protocol::OtlpGrpc,
                _ => Protocol::OtlpHttp,
            };
        }
        if let Ok(v) = env::var("HA_REPORTING_COMPRESSION") {
            self.reporting.compression = match v.as_str() {
                "gzip" => Compression::Gzip,
                _ => Compression::None,
            };
        }
        if let Ok(v) = env::var("HA_REPORTING_SECURE") {
            self.reporting.secure = Some(v != "false" && v != "0");
        }
        #[cfg(feature = "control")]
        if let Ok(v) = env::var("HA_CONTROL_ENDPOINT") {
            self.reporting.control_endpoint = Some(v);
        }
    }

    pub fn service_name(&self) -> String {
        self.service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string())
    }

    pub fn endpoint(&self) -> String {
        self.reporting
            .endpoint
            .clone()
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
    }

    /// Control-plane policy-fetch endpoint. Falls back to the telemetry
    /// endpoint's default host if `HA_CONTROL_ENDPOINT` /
    /// `reporting.control_endpoint` is not set — override explicitly in
    /// any real deployment; the fallback exists only so local dev against
    /// `localhost:5442` works without extra config.
    #[cfg(feature = "control")]
    pub fn control_endpoint(&self) -> String {
        self.reporting
            .control_endpoint
            .clone()
            .unwrap_or_else(|| format!("{}/v1/control/policy", DEFAULT_ENDPOINT))
    }

    /// Validate the resolved config. Mirrors the Python SDK's contract:
    /// endpoints must be `https://` unless `secure: false` is explicit,
    /// and a token is required for any non-localhost endpoint.
    pub fn validate(&self) -> Result<()> {
        let endpoint = self.endpoint();
        let secure = self.reporting.secure.unwrap_or(true);
        let is_localhost = endpoint.contains("localhost") || endpoint.contains("127.0.0.1");

        if secure && !endpoint.starts_with("https://") && !is_localhost {
            return Err(Error::InsecureEndpoint(endpoint));
        }

        if !is_localhost && self.reporting.token.as_deref().unwrap_or("").is_empty() {
            return Err(Error::MissingToken);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_localhost_and_insecure_allowed() {
        let config = Config::default();
        assert_eq!(config.service_name(), DEFAULT_SERVICE_NAME);
        assert_eq!(config.endpoint(), DEFAULT_ENDPOINT);
        // localhost default should validate even without a token/https.
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_plaintext_non_localhost_endpoint() {
        let mut config = Config::default();
        config.reporting.endpoint = Some("http://otel.harness.io".to_string());
        config.reporting.token = Some("tok".to_string());
        let err = config.validate().unwrap_err();
        assert!(matches!(err, Error::InsecureEndpoint(_)));
    }

    #[test]
    fn allows_plaintext_when_secure_explicitly_false() {
        let mut config = Config::default();
        config.reporting.endpoint = Some("http://otel.harness.io".to_string());
        config.reporting.token = Some("tok".to_string());
        config.reporting.secure = Some(false);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn requires_token_for_non_localhost_endpoint() {
        let mut config = Config::default();
        config.reporting.endpoint = Some("https://otel.harness.io".to_string());
        let err = config.validate().unwrap_err();
        assert!(matches!(err, Error::MissingToken));
    }
}
