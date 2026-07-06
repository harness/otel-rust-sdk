use std::fmt;

/// Errors returned while configuring or building the Harness OTel SDK.
#[derive(Debug)]
pub enum Error {
    /// The configured endpoint does not use `https://` and `secure: false`
    /// was not explicitly set.
    InsecureEndpoint(String),
    /// No endpoint was provided and no default could be resolved.
    MissingEndpoint,
    /// A non-empty auth token is required for non-localhost endpoints.
    MissingToken,
    /// Failed to load or parse the config file.
    ConfigFile(String),
    /// The underlying OTLP exporter could not be constructed.
    ExporterBuild(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InsecureEndpoint(endpoint) => write!(
                f,
                "endpoint '{endpoint}' is not https:// — set `secure: false` explicitly to allow plaintext (not recommended)"
            ),
            Error::MissingEndpoint => write!(f, "no reporting endpoint configured"),
            Error::MissingToken => write!(
                f,
                "no auth token configured for a non-localhost endpoint"
            ),
            Error::ConfigFile(msg) => write!(f, "failed to load config file: {msg}"),
            Error::ExporterBuild(msg) => write!(f, "failed to build OTLP exporter: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
