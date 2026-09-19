use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
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

impl TryFrom<String> for HttpUrl {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<HttpUrl> for String {
    fn from(value: HttpUrl) -> Self {
        value.0
    }
}

impl FromStr for HttpUrl {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
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

impl TryFrom<String> for EnvironmentVariable {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<EnvironmentVariable> for String {
    fn from(value: EnvironmentVariable) -> Self {
        value.0
    }
}

impl FromStr for EnvironmentVariable {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
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

impl TryFrom<String> for NonEmptyString {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse("value", value)
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct PositiveI64(i64);

impl PositiveI64 {
    pub fn new(field: &'static str, value: i64) -> Result<Self, ValidationError> {
        if !(1..=MAX_SAFE_INTEGER).contains(&value) {
            return Err(ValidationError::new(
                field,
                "must be positive and no greater than Number.MAX_SAFE_INTEGER",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for PositiveI64 {
    type Error = ValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new("value", value)
    }
}

impl From<PositiveI64> for i64 {
    fn from(value: PositiveI64) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct NonNegativeI64(i64);

impl NonNegativeI64 {
    pub fn new(field: &'static str, value: i64) -> Result<Self, ValidationError> {
        if !(0..=MAX_SAFE_INTEGER).contains(&value) {
            return Err(ValidationError::new(
                field,
                "must be non-negative and no greater than Number.MAX_SAFE_INTEGER",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for NonNegativeI64 {
    type Error = ValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new("value", value)
    }
}

impl From<NonNegativeI64> for i64 {
    fn from(value: NonNegativeI64) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub id: NonEmptyString,
    pub base_url: HttpUrl,
    pub api_key_env: EnvironmentVariable,
    pub request_timeout_seconds: PositiveI64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: NonEmptyString,
    pub provider_id: NonEmptyString,
    pub remote_name: NonEmptyString,
    pub class: NonEmptyString,
    pub context_window: PositiveI64,
    pub max_output_tokens: PositiveI64,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimedCapabilities {
    pub chat: bool,
    pub streaming: bool,
    pub tools: bool,
    pub parallel_tools: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedCapability {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedCapabilities {
    pub chat: VerifiedCapability,
    pub streaming: VerifiedCapability,
    pub tools: VerifiedCapability,
    pub parallel_tools: VerifiedCapability,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilities {
    pub claimed: ClaimedCapabilities,
    pub verified: VerifiedCapabilities,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Chat,
    Streaming,
    Tools,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorCode {
    AuthenticationFailed,
    RateLimited,
    Timeout,
    InvalidResponse,
    ProviderUnavailable,
    ContextTooLarge,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub kind: ProbeKind,
    pub status: ProbeStatus,
    pub verified: VerifiedCapability,
    pub latency_ms: NonNegativeI64,
    pub error_code: Option<ProbeErrorCode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> Provider {
        Provider {
            id: NonEmptyString::parse("id", "primary").unwrap(),
            base_url: HttpUrl::parse("https://api.example.com/v1").unwrap(),
            api_key_env: EnvironmentVariable::parse("PRIMARY_API_KEY").unwrap(),
            request_timeout_seconds: PositiveI64::new("request_timeout_seconds", 180).unwrap(),
        }
    }

    fn model() -> Model {
        Model {
            id: NonEmptyString::parse("id", "coding-large").unwrap(),
            provider_id: NonEmptyString::parse("provider_id", "primary").unwrap(),
            remote_name: NonEmptyString::parse("remote_name", "vendor/model").unwrap(),
            class: NonEmptyString::parse("class", "coding").unwrap(),
            context_window: PositiveI64::new("context_window", 131_072).unwrap(),
            max_output_tokens: PositiveI64::new("max_output_tokens", 16_384).unwrap(),
            capabilities: ModelCapabilities::default(),
        }
    }

    #[test]
    fn rejects_invalid_urls_and_environment_names() {
        for value in [
            "",
            "api.example.com",
            "ftp://api.example.com",
            "https:///v1",
            "https://user:pass@api.example.com/v1",
        ] {
            assert!(HttpUrl::parse(value).is_err(), "accepted {value}");
        }
        for value in ["", "sk-secret", "secret value", "1_API_KEY", "API.KEY"] {
            assert!(
                EnvironmentVariable::parse(value).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn rejects_zero_and_negative_limits() {
        for value in [0, -1] {
            assert!(PositiveI64::new("request_timeout_seconds", value).is_err());
            assert!(PositiveI64::new("context_window", value).is_err());
            assert!(PositiveI64::new("max_output_tokens", value).is_err());
        }
        assert!(NonNegativeI64::new("latency_ms", -1).is_err());
        assert_eq!(NonNegativeI64::new("latency_ms", 0).unwrap().get(), 0);
        let above_i32 = i64::from(i32::MAX) + 1;
        assert_eq!(
            PositiveI64::new("limit", above_i32).unwrap().get(),
            above_i32
        );
        assert_eq!(
            PositiveI64::new("limit", MAX_SAFE_INTEGER).unwrap().get(),
            MAX_SAFE_INTEGER
        );
        assert_eq!(
            NonNegativeI64::new("latency_ms", MAX_SAFE_INTEGER)
                .unwrap()
                .get(),
            MAX_SAFE_INTEGER
        );
        assert!(PositiveI64::new("limit", MAX_SAFE_INTEGER + 1).is_err());
        assert!(NonNegativeI64::new("latency_ms", MAX_SAFE_INTEGER + 1).is_err());
    }

    #[test]
    fn model_starts_with_unknown_verified_capabilities() {
        let capabilities = model().capabilities;
        assert_eq!(
            capabilities.verified,
            VerifiedCapabilities {
                chat: VerifiedCapability::Unknown,
                streaming: VerifiedCapability::Unknown,
                tools: VerifiedCapability::Unknown,
                parallel_tools: VerifiedCapability::Unknown,
            }
        );
        assert_ne!(
            VerifiedCapability::Supported,
            VerifiedCapability::Unsupported
        );
    }

    #[test]
    fn capability_serialization_has_exact_keys() {
        let expected = ["chat", "parallel_tools", "streaming", "tools"];
        for value in [
            serde_json::to_value(ClaimedCapabilities::default()).unwrap(),
            serde_json::to_value(VerifiedCapabilities::default()).unwrap(),
        ] {
            let mut keys: Vec<_> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, expected);
        }
        let claimed = serde_json::json!({
            "chat": true,
            "streaming": true,
            "tools": true,
            "parallel_tools": true,
            "unknown": true
        });
        let verified = serde_json::json!({
            "chat": "unknown",
            "streaming": "unknown",
            "tools": "unknown",
            "parallel_tools": "unknown",
            "unknown": "unknown"
        });
        assert!(serde_json::from_value::<ClaimedCapabilities>(claimed).is_err());
        assert!(serde_json::from_value::<VerifiedCapabilities>(verified).is_err());
    }

    #[test]
    fn verified_states_roundtrip() {
        for state in [
            VerifiedCapability::Unknown,
            VerifiedCapability::Supported,
            VerifiedCapability::Unsupported,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(
                serde_json::from_str::<VerifiedCapability>(&json).unwrap(),
                state
            );
        }
    }

    #[test]
    fn probe_error_codes_are_typed_and_roundtrip() {
        for code in [
            ProbeErrorCode::AuthenticationFailed,
            ProbeErrorCode::RateLimited,
            ProbeErrorCode::Timeout,
            ProbeErrorCode::InvalidResponse,
            ProbeErrorCode::ProviderUnavailable,
            ProbeErrorCode::ContextTooLarge,
        ] {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(serde_json::from_str::<ProbeErrorCode>(&json).unwrap(), code);
        }
        assert!(serde_json::from_str::<ProbeErrorCode>("\"arbitrary\"").is_err());
    }

    #[test]
    fn provider_model_and_probe_serialize_roundtrip() {
        let provider = provider();
        let model = model();
        let probe = ProbeResult {
            provider_id: NonEmptyString::parse("provider_id", "primary").unwrap(),
            model_id: NonEmptyString::parse("model_id", "coding-large").unwrap(),
            kind: ProbeKind::Tools,
            status: ProbeStatus::Succeeded,
            verified: VerifiedCapability::Supported,
            latency_ms: NonNegativeI64::new("latency_ms", 0).unwrap(),
            error_code: None,
        };

        let provider_json = serde_json::to_string(&provider).unwrap();
        let model_json = serde_json::to_string(&model).unwrap();
        let probe_json = serde_json::to_string(&probe).unwrap();
        assert_eq!(
            serde_json::from_str::<Provider>(&provider_json).unwrap(),
            provider
        );
        assert_eq!(serde_json::from_str::<Model>(&model_json).unwrap(), model);
        assert_eq!(
            serde_json::from_str::<ProbeResult>(&probe_json).unwrap(),
            probe
        );
    }
}
