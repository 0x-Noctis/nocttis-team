use serde::{Deserialize, Serialize};

use crate::domain::provider::{
    Capabilities, EnvironmentVariable, HttpUrl, Model, ModelCapabilities, NonEmptyString,
    PositiveU64, ProbeResult, Provider, ValidationError,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInput {
    pub id: String,
    pub base_url: String,
    pub api_key_env: String,
    pub request_timeout_seconds: u64,
}

impl TryFrom<ProviderInput> for Provider {
    type Error = ValidationError;

    fn try_from(input: ProviderInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            base_url: HttpUrl::parse(input.base_url)?,
            api_key_env: EnvironmentVariable::parse(input.api_key_env)?,
            request_timeout_seconds: PositiveU64::new(
                "request_timeout_seconds",
                input.request_timeout_seconds,
            )?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInput {
    pub id: String,
    pub remote_name: String,
    pub class: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    #[serde(default)]
    pub claimed_capabilities: Capabilities,
}

impl TryFrom<ModelInput> for Model {
    type Error = ValidationError;

    fn try_from(input: ModelInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            remote_name: NonEmptyString::parse("remote_name", input.remote_name)?,
            class: NonEmptyString::parse("class", input.class)?,
            context_window: PositiveU64::new("context_window", input.context_window)?,
            max_output_tokens: PositiveU64::new(
                "max_output_tokens",
                input.max_output_tokens,
            )?,
            capabilities: ModelCapabilities {
                claimed: input.claimed_capabilities,
                verified: Capabilities::default(),
            },
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderResponse {
    pub id: String,
    pub base_url: String,
    pub api_key_env: String,
    pub secret_configured: bool,
    pub request_timeout_seconds: u64,
}

impl ProviderResponse {
    pub fn from_domain(provider: &Provider, secret_configured: bool) -> Self {
        Self {
            id: provider.id.as_str().to_owned(),
            base_url: provider.base_url.as_str().to_owned(),
            api_key_env: provider.api_key_env.as_str().to_owned(),
            secret_configured,
            request_timeout_seconds: provider.request_timeout_seconds.get(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelResponse {
    pub id: String,
    pub remote_name: String,
    pub class: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub capabilities: ModelCapabilities,
}

impl From<&Model> for ModelResponse {
    fn from(model: &Model) -> Self {
        Self {
            id: model.id.as_str().to_owned(),
            remote_name: model.remote_name.as_str().to_owned(),
            class: model.class.as_str().to_owned(),
            context_window: model.context_window.get(),
            max_output_tokens: model.max_output_tokens.get(),
            capabilities: model.capabilities.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeResponse {
    pub result: ProbeResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_input_validates_every_field() {
        let valid = || ProviderInput {
            id: "primary".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key_env: "PRIMARY_API_KEY".into(),
            request_timeout_seconds: 180,
        };
        assert!(Provider::try_from(valid()).is_ok());

        let mut input = valid();
        input.id = " ".into();
        assert_eq!(Provider::try_from(input).unwrap_err().field, "id");
        let mut input = valid();
        input.base_url = "file:///secret".into();
        assert_eq!(Provider::try_from(input).unwrap_err().field, "base_url");
        let mut input = valid();
        input.api_key_env = "secret-value".into();
        assert_eq!(Provider::try_from(input).unwrap_err().field, "api_key_env");
        let mut input = valid();
        input.request_timeout_seconds = 0;
        assert_eq!(
            Provider::try_from(input).unwrap_err().field,
            "request_timeout_seconds"
        );
    }

    #[test]
    fn model_input_validates_names_and_limits() {
        let valid = || ModelInput {
            id: "coding-large".into(),
            remote_name: "vendor/model".into(),
            class: "coding".into(),
            context_window: 131_072,
            max_output_tokens: 16_384,
            claimed_capabilities: Capabilities::default(),
        };
        assert!(Model::try_from(valid()).is_ok());

        for (field, mutate) in [
            ("id", 0_u8),
            ("remote_name", 1),
            ("class", 2),
            ("context_window", 3),
            ("max_output_tokens", 4),
        ] {
            let mut input = valid();
            match mutate {
                0 => input.id.clear(),
                1 => input.remote_name.clear(),
                2 => input.class.clear(),
                3 => input.context_window = 0,
                _ => input.max_output_tokens = 0,
            }
            assert_eq!(Model::try_from(input).unwrap_err().field, field);
        }
    }

    #[test]
    fn responses_never_contain_secret_value() {
        let provider = Provider::try_from(ProviderInput {
            id: "primary".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key_env: "PRIMARY_API_KEY".into(),
            request_timeout_seconds: 180,
        })
        .unwrap();
        let response = serde_json::to_value(ProviderResponse::from_domain(&provider, true)).unwrap();
        assert_eq!(response["api_key_env"], "PRIMARY_API_KEY");
        assert_eq!(response["secret_configured"], true);
        assert!(response.get("api_key").is_none());
    }

    #[test]
    fn input_rejects_secret_and_unknown_fields() {
        for input in [
            serde_json::json!({
                "id": "primary",
                "base_url": "https://api.example.com/v1",
                "api_key_env": "PRIMARY_API_KEY",
                "api_key": null,
                "request_timeout_seconds": 180
            }),
            serde_json::json!({
                "id": "primary",
                "base_url": "https://api.example.com/v1",
                "api_key_env": "PRIMARY_API_KEY",
                "request_timeout_seconds": 180,
                "unexpected": true
            }),
        ] {
            assert!(serde_json::from_value::<ProviderInput>(input).is_err());
        }
    }
}
