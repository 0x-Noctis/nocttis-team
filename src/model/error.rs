#![allow(dead_code)]

use std::{error::Error, fmt};

use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;

const MAX_PROVIDER_BODY_CHARS: usize = 500;
const REDACTED: &str = "[REDACTED]";

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
    provider_body: Option<String>,
}

impl ModelError {
    pub fn new(kind: ModelErrorKind) -> Self {
        Self {
            kind,
            provider_body: None,
        }
    }

    pub fn from_status(status: StatusCode, provider_body: &str) -> Self {
        let kind = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                ModelErrorKind::AuthenticationFailed
            }
            StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => ModelErrorKind::Timeout,
            StatusCode::PAYLOAD_TOO_LARGE => ModelErrorKind::ContextTooLarge,
            StatusCode::TOO_MANY_REQUESTS => ModelErrorKind::RateLimited,
            status if status.is_server_error() => ModelErrorKind::ProviderUnavailable,
            _ => ModelErrorKind::InvalidResponse,
        };
        Self {
            kind,
            provider_body: Some(sanitize_provider_body(provider_body)),
        }
    }

    pub fn invalid_response(provider_body: &str) -> Self {
        Self {
            kind: ModelErrorKind::InvalidResponse,
            provider_body: Some(sanitize_provider_body(provider_body)),
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

    pub fn provider_body(&self) -> Option<&str> {
        self.provider_body.as_deref()
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.kind)?;
        if let Some(body) = &self.provider_body {
            write!(formatter, ": {body}")?;
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

fn sanitize_provider_body(body: &str) -> String {
    let sanitized = match serde_json::from_str::<Value>(body) {
        Ok(mut value) => {
            redact_json(&mut value);
            value.to_string()
        }
        Err(_) => body
            .lines()
            .map(|line| {
                if contains_sensitive_name(line) {
                    REDACTED.to_owned()
                } else {
                    redact_bearer(line)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    sanitized.chars().take(MAX_PROVIDER_BODY_CHARS).collect()
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if contains_sensitive_name(name) {
                    *value = Value::String(REDACTED.to_owned());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_json),
        Value::String(value) => *value = redact_bearer(value),
        _ => {}
    }
}

fn contains_sensitive_name(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace(['-', ' '], "_");
    ["authorization", "api_key", "apikey", "secret", "token"]
        .iter()
        .any(|name| normalized.contains(name))
}

fn redact_bearer(value: &str) -> String {
    let Some(start) = value.to_ascii_lowercase().find("bearer ") else {
        return value.to_owned();
    };
    let token_start = start + "bearer ".len();
    let token_end = value[token_start..]
        .find(char::is_whitespace)
        .map_or(value.len(), |offset| token_start + offset);
    format!(
        "{}Bearer {REDACTED}{}",
        &value[..start],
        &value[token_end..]
    )
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
    fn sanitizes_and_truncates_provider_body() {
        let secret = "never-print-this";
        let body = format!(
            r#"{{"error":"{}","authorization":"Bearer {}","nested":{{"api-key":"{}"}}}}"#,
            "x".repeat(600),
            secret,
            secret,
        );
        let error = ModelError::from_status(StatusCode::BAD_REQUEST, &body);
        let sanitized = error.provider_body().unwrap();

        assert!(sanitized.chars().count() <= MAX_PROVIDER_BODY_CHARS);
        assert!(!sanitized.contains(secret));
        assert!(!sanitized.to_ascii_lowercase().contains("bearer "));
        assert!(sanitized.contains(REDACTED));
    }

    #[test]
    fn sanitizes_plaintext_authorization_and_bearer_values() {
        let body = "Authorization: Bearer header-secret\nupstream said Bearer body-secret denied";
        let sanitized = sanitize_provider_body(body);

        assert!(!sanitized.contains("header-secret"));
        assert!(!sanitized.contains("body-secret"));
        assert_eq!(sanitized, "[REDACTED]\n[REDACTED]");
    }
}
