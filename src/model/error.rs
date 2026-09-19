#![allow(dead_code)]

use std::{error::Error, fmt};

use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;

const MAX_PROVIDER_CODE_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorKind {
    AuthenticationFailed,
    RateLimited,
    Timeout,
    InvalidResponse,
    ProviderUnavailable,
    ContextTooLarge,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ModelError {
    kind: ModelErrorKind,
    provider_code: Option<String>,
}

impl ModelError {
    pub fn new(kind: ModelErrorKind) -> Self {
        Self {
            kind,
            provider_code: None,
        }
    }

    pub fn from_status(status: StatusCode, provider_body: &str) -> Self {
        let provider_code = provider_code(provider_body);
        let kind = if status == StatusCode::BAD_REQUEST
            && provider_code.as_deref() == Some("context_length_exceeded")
        {
            ModelErrorKind::ContextTooLarge
        } else {
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    ModelErrorKind::AuthenticationFailed
                }
                StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
                    ModelErrorKind::Timeout
                }
                StatusCode::PAYLOAD_TOO_LARGE => ModelErrorKind::ContextTooLarge,
                StatusCode::TOO_MANY_REQUESTS => ModelErrorKind::RateLimited,
                status if status.is_server_error() => ModelErrorKind::ProviderUnavailable,
                _ => ModelErrorKind::InvalidResponse,
            }
        };
        Self {
            kind,
            provider_code,
        }
    }

    pub fn invalid_response(provider_body: &str) -> Self {
        Self {
            kind: ModelErrorKind::InvalidResponse,
            provider_code: provider_code(provider_body),
        }
    }

    pub fn kind(&self) -> ModelErrorKind {
        self.kind
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ModelErrorKind::RateLimited
                | ModelErrorKind::Timeout
                | ModelErrorKind::ProviderUnavailable
        )
    }

    pub fn provider_code(&self) -> Option<&str> {
        self.provider_code.as_deref()
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.kind)?;
        if let Some(code) = &self.provider_code {
            write!(formatter, " ({code})")?;
        }
        Ok(())
    }
}

impl Error for ModelError {}

impl fmt::Display for ModelErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AuthenticationFailed => "authentication_failed",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid_response",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ContextTooLarge => "context_too_large",
        })
    }
}

fn provider_code(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(body).ok()?;
    let code = parsed.get("error")?.get("code")?.as_str()?;
    (code.len() <= MAX_PROVIDER_CODE_CHARS
        && !code.is_empty()
        && code
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || b"_.-".contains(&character)))
    .then(|| code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_and_retryability() {
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                ModelErrorKind::AuthenticationFailed,
                false,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                ModelErrorKind::RateLimited,
                true,
            ),
            (StatusCode::GATEWAY_TIMEOUT, ModelErrorKind::Timeout, true),
            (
                StatusCode::BAD_GATEWAY,
                ModelErrorKind::ProviderUnavailable,
                true,
            ),
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                ModelErrorKind::ContextTooLarge,
                false,
            ),
            (
                StatusCode::BAD_REQUEST,
                ModelErrorKind::InvalidResponse,
                false,
            ),
        ];

        for (status, kind, retryable) in cases {
            let error = ModelError::from_status(status, "provider error");
            assert_eq!(error.kind(), kind);
            assert_eq!(error.retryable(), retryable);
        }
    }

    #[test]
    fn maps_context_length_code_on_bad_request() {
        let error = ModelError::from_status(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        );

        assert_eq!(error.kind(), ModelErrorKind::ContextTooLarge);
        assert!(!error.retryable());
        assert_eq!(error.provider_code(), Some("context_length_exceeded"));
    }

    #[test]
    fn keeps_only_safe_provider_code() {
        let safe = ModelError::from_status(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"request.invalid-1"}}"#,
        );
        let unsafe_code = ModelError::from_status(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"key=quoted-secret"}}"#,
        );
        let too_long = format!(r#"{{"error":{{"code":"{}"}}}}"#, "x".repeat(65));

        assert_eq!(safe.provider_code(), Some("request.invalid-1"));
        assert_eq!(unsafe_code.provider_code(), None);
        assert_eq!(
            ModelError::from_status(StatusCode::BAD_REQUEST, &too_long).provider_code(),
            None
        );
    }

    #[test]
    fn discards_body_and_secrets_from_display() {
        let bodies = [
            r#"{"error":"secret-in-error-field"}"#,
            r#"{"error":{"code":"bad?key=secret-value"},"url":"https://example.test?key=secret-value"}"#,
            r#"{"message":"api_key=\"quoted-api-key\""}"#,
            r#"{"message":"Bearerwithout-whitespace"}"#,
            "non-JSON secret-value",
        ];

        for body in bodies {
            let error = ModelError::from_status(StatusCode::BAD_REQUEST, body);
            let display = error.to_string();
            assert_eq!(error.provider_code(), None);
            assert_eq!(display, "invalid_response");
            assert!(!display.contains("secret-value"));
            assert!(!display.contains("quoted-api-key"));
            assert!(!display.contains("secret-in-error-field"));
            assert!(!display.contains("Bearer"));
            assert!(!display.contains("https://"));
        }
    }
}
