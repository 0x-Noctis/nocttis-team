use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl ValidationError {
    const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HttpUrl(String);

impl HttpUrl {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let has_http_authority = value
            .strip_prefix("http://")
            .or_else(|| value.strip_prefix("https://"))
            .is_some_and(|authority| !authority.is_empty() && !authority.starts_with('/'));
        let parsed = reqwest::Url::parse(&value)
            .map_err(|_| ValidationError::new("base_url", "must be a valid HTTP(S) URL"))?;
        if !has_http_authority
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(ValidationError::new(
                "base_url",
                "must be a valid HTTP(S) URL",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for HttpUrl {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EnvironmentVariable(String);

impl EnvironmentVariable {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let mut characters = value.chars();
        let valid_start = characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
        if !valid_start
            || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
        {
            return Err(ValidationError::new(
                "api_key_env",
                "must be an environment variable name",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for EnvironmentVariable {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn parse(field: &'static str, value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ValidationError::new(field, "must not be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PositiveU64(u64);

impl PositiveU64 {
    pub fn new(field: &'static str, value: u64) -> Result<Self, ValidationError> {
        if value == 0 {
            return Err(ValidationError::new(field, "must be positive"));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Provider {
    pub id: NonEmptyString,
    pub base_url: HttpUrl,
    pub api_key_env: EnvironmentVariable,
    pub request_timeout_seconds: PositiveU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Model {
    pub id: NonEmptyString,
    pub remote_name: NonEmptyString,
    pub class: NonEmptyString,
    pub context_window: PositiveU64,
    pub max_output_tokens: PositiveU64,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub tools: bool,
    pub parallel_tools: bool,
    pub streaming: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ModelCapabilities {
    pub claimed: Capabilities,
    pub verified: Capabilities,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProbeResult {
    pub status: ProbeStatus,
    pub verified: Capabilities,
    pub latency_ms: PositiveU64,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_boundary_values() {
        assert!(HttpUrl::parse("https://api.example.com/v1").is_ok());
        assert!(EnvironmentVariable::parse("PRIMARY_API_KEY").is_ok());
        assert!(NonEmptyString::parse("remote_name", "vendor/model").is_ok());
        assert_eq!(PositiveU64::new("context_window", 1).unwrap().get(), 1);
    }

    #[test]
    fn rejects_non_http_urls() {
        for value in [
            "",
            "api.example.com",
            "ftp://api.example.com",
            "https:///v1",
            "https://user:pass@api.example.com/v1",
        ] {
            assert!(HttpUrl::parse(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn rejects_secret_values_as_environment_names() {
        for value in ["", "sk-secret", "secret value", "1_API_KEY", "API.KEY"] {
            assert!(EnvironmentVariable::parse(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn rejects_empty_names_and_zero_limits() {
        assert!(NonEmptyString::parse("remote_name", " \n").is_err());
        assert!(PositiveU64::new("max_output_tokens", 0).is_err());
    }

    #[test]
    fn claimed_and_verified_capabilities_are_independent() {
        let capabilities = ModelCapabilities {
            claimed: Capabilities { tools: true, parallel_tools: true, streaming: true },
            verified: Capabilities::default(),
        };
        assert!(capabilities.claimed.tools);
        assert!(!capabilities.verified.tools);
    }
}
