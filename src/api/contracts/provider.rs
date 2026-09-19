use serde::{Deserialize, Serialize};

use crate::domain::provider::{
    ClaimedCapabilities, EnvironmentVariable, HttpUrl, Model, ModelCapabilities, NonEmptyString,
    PositiveI64, ProbeResult, Provider, ValidationError,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInput {
    pub id: String,
    pub base_url: String,
    pub api_key_env: String,
    pub request_timeout_seconds: i64,
}

impl TryFrom<ProviderInput> for Provider {
    type Error = ValidationError;

    fn try_from(input: ProviderInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            base_url: HttpUrl::parse(input.base_url)?,
            api_key_env: EnvironmentVariable::parse(input.api_key_env)?,
            request_timeout_seconds: PositiveI64::new(
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
    pub provider_id: String,
    pub remote_name: String,
    pub class: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    #[serde(default)]
    pub claimed_capabilities: ClaimedCapabilities,
}

impl TryFrom<ModelInput> for Model {
    type Error = ValidationError;

    fn try_from(input: ModelInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            provider_id: NonEmptyString::parse("provider_id", input.provider_id)?,
            remote_name: NonEmptyString::parse("remote_name", input.remote_name)?,
            class: NonEmptyString::parse("class", input.class)?,
            context_window: PositiveI64::new("context_window", input.context_window)?,
            max_output_tokens: PositiveI64::new("max_output_tokens", input.max_output_tokens)?,
            capabilities: ModelCapabilities {
                claimed: input.claimed_capabilities,
                ..ModelCapabilities::default()
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
    pub request_timeout_seconds: i64,
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
    pub provider_id: String,
    pub remote_name: String,
    pub class: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    pub capabilities: ModelCapabilities,
}

impl From<&Model> for ModelResponse {
    fn from(model: &Model) -> Self {
        Self {
            id: model.id.as_str().to_owned(),
            provider_id: model.provider_id.as_str().to_owned(),
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
    use crate::domain::provider::VerifiedCapability;

    fn provider_input() -> ProviderInput {
        ProviderInput {
            id: "primary".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key_env: "PRIMARY_API_KEY".into(),
            request_timeout_seconds: 180,
        }
    }

    fn model_input() -> ModelInput {
        ModelInput {
            id: "coding-large".into(),
            provider_id: "primary".into(),
            remote_name: "vendor/model".into(),
            class: "coding".into(),
            context_window: 131_072,
            max_output_tokens: 16_384,
            claimed_capabilities: ClaimedCapabilities::default(),
        }
    }

    #[test]
    fn provider_input_validates_every_field() {
        assert!(Provider::try_from(provider_input()).is_ok());
        for (field, mutation) in [
            ("id", 0_u8),
            ("base_url", 1),
            ("api_key_env", 2),
            ("request_timeout_seconds", 3),
        ] {
            let mut input = provider_input();
            match mutation {
                0 => input.id.clear(),
                1 => input.base_url = "file:///secret".into(),
                2 => input.api_key_env = "not an env name".into(),
                _ => input.request_timeout_seconds = -1,
            }
            assert_eq!(Provider::try_from(input).unwrap_err().field, field);
        }
    }

    #[test]
    fn model_input_requires_provider_and_validates_limits() {
        assert!(Model::try_from(model_input()).is_ok());
        let missing_provider = serde_json::json!({
            "id": "coding-large",
            "remote_name": "vendor/model",
            "class": "coding",
            "context_window": 1,
            "max_output_tokens": 1
        });
        assert!(serde_json::from_value::<ModelInput>(missing_provider).is_err());

        for (field, mutation) in [
            ("provider_id", 0_u8),
            ("remote_name", 1),
            ("context_window", 2),
            ("max_output_tokens", 3),
        ] {
            let mut input = model_input();
            match mutation {
                0 => input.provider_id.clear(),
                1 => input.remote_name.clear(),
                2 => input.context_window = 0,
                _ => input.max_output_tokens = -1,
            }
            assert_eq!(Model::try_from(input).unwrap_err().field, field);
        }
    }

    #[test]
    fn new_model_verified_capabilities_are_unknown() {
        let model = Model::try_from(model_input()).unwrap();
        assert_eq!(
            model.capabilities.verified.tools,
            VerifiedCapability::Unknown
        );
    }

    #[test]
    fn responses_never_contain_secret_value() {
        let provider = Provider::try_from(provider_input()).unwrap();
        let response =
            serde_json::to_value(ProviderResponse::from_domain(&provider, true)).unwrap();
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

        for field in ["api_key", "unexpected"] {
            let mut input = serde_json::json!({
                "id": "coding-large",
                "provider_id": "primary",
                "remote_name": "vendor/model",
                "class": "coding",
                "context_window": 1,
                "max_output_tokens": 1
            });
            input
                .as_object_mut()
                .unwrap()
                .insert(field.into(), serde_json::Value::Null);
            assert!(serde_json::from_value::<ModelInput>(input).is_err());
        }
    }
}
