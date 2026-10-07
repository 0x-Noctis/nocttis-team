#![allow(dead_code)]

use std::{error::Error, fmt};

use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorKind {
    AuthenticationFailed,
    RateLimited,
    Timeout,
    InvalidResponse,
    ProviderUnavailable,
    ContextTooLarge,
    /// Budget token run/task/attempt habis; tidak pernah diulang.
    BudgetExceeded,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ModelError {
    kind: ModelErrorKind,
}

impl ModelError {
    pub fn new(kind: ModelErrorKind) -> Self {
        Self { kind }
    }

    pub fn from_status(status: StatusCode, provider_body: &str) -> Self {
        let kind = if status == StatusCode::BAD_REQUEST && is_context_length_exceeded(provider_body)
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
        Self { kind }
    }

    pub fn invalid_response(_provider_body: &str) -> Self {
        Self::new(ModelErrorKind::InvalidResponse)
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
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.kind)
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
            Self::BudgetExceeded => "budget_exceeded",
        })
    }
}

fn is_context_length_exceeded(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.get("code")?.as_str().map(str::to_owned))
        .as_deref()
        == Some("context_length_exceeded")
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
        assert_eq!(error.to_string(), "context_too_large");
    }

    #[test]
    fn arbitrary_provider_code_is_not_stored_or_displayed() {
        let error = ModelError::from_status(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"sk_live_secret"}}"#,
        );

        assert_eq!(error, ModelError::new(ModelErrorKind::InvalidResponse));
        assert_eq!(error.to_string(), "invalid_response");
        assert!(!error.to_string().contains("sk_live_secret"));
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
            assert_eq!(error, ModelError::new(ModelErrorKind::InvalidResponse));
            assert_eq!(display, "invalid_response");
            assert!(!display.contains("secret-value"));
            assert!(!display.contains("quoted-api-key"));
            assert!(!display.contains("secret-in-error-field"));
            assert!(!display.contains("Bearer"));
            assert!(!display.contains("https://"));
        }
    }
}
