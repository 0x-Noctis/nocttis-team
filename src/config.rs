use std::{collections::HashMap, fs, net::SocketAddr, path::PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub database: Database,
    pub scheduler: Scheduler,
    pub budgets: Budgets,
    pub git: Git,
    pub artifacts: Artifacts,
    pub runner: Runner,
    pub provider: ProviderBootstrap,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Database {
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scheduler {
    pub max_parallel_agents: usize,
    pub heartbeat_seconds: u64,
    pub stale_after_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budgets {
    pub default_project_tokens: u64,
    pub default_task_input_tokens: u64,
    pub default_task_output_tokens: u64,
    pub reserve_percent: u8,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git {
    pub worktree_root: PathBuf,
    pub retention_hours: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Artifacts {
    pub root: PathBuf,
    pub max_tool_output_bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runner {
    pub network_enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderBootstrap {
    pub base_url: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct Secrets {
    pub database_url: String,
}

impl Config {
    pub fn load() -> anyhow::Result<(Self, Secrets)> {
        Self::load_from(std::env::vars().collect())
    }

    fn load_from(env: HashMap<String, String>) -> anyhow::Result<(Self, Secrets)> {
        let mut value = match env.get("NOCTIS_CONFIG") {
            Some(path) => toml::from_str(
                &fs::read_to_string(path)
                    .with_context(|| format!("failed to read config file {path}"))?,
            )
            .context("invalid config file")?,
            None => toml::Value::Table(Default::default()),
        };

        for (name, override_value) in env.iter().filter(|(name, _)| name.starts_with("NOCTIS__")) {
            apply_override(&mut value, name, override_value)?;
        }

        let config: Self = value.try_into().context("invalid runtime config")?;
        config.validate()?;
        let secrets = Secrets {
            database_url: required_secret(&env, "DATABASE_URL")?,
        };
        required_secret(&env, "PRIMARY_API_KEY")?;
        Ok((config, secrets))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.database.max_connections == 0 {
            bail!("database.max_connections must be greater than zero");
        }
        if self.database.acquire_timeout_seconds == 0 {
            bail!("database.acquire_timeout_seconds must be greater than zero");
        }
        if self.scheduler.max_parallel_agents == 0 {
            bail!("scheduler.max_parallel_agents must be greater than zero");
        }
        if self.scheduler.heartbeat_seconds == 0 {
            bail!("scheduler.heartbeat_seconds must be greater than zero");
        }
        if self.scheduler.stale_after_seconds <= self.scheduler.heartbeat_seconds {
            bail!("scheduler.stale_after_seconds must exceed scheduler.heartbeat_seconds");
        }
        if self.budgets.default_project_tokens == 0
            || self.budgets.default_task_input_tokens == 0
            || self.budgets.default_task_output_tokens == 0
        {
            bail!("budget token limits must be greater than zero");
        }
        if self.budgets.reserve_percent > 100 {
            bail!("budgets.reserve_percent must be at most 100");
        }
        if self.git.retention_hours == 0 {
            bail!("git.retention_hours must be greater than zero");
        }
        if self.artifacts.max_tool_output_bytes == 0 {
            bail!("artifacts.max_tool_output_bytes must be greater than zero");
        }
        if self.provider.base_url.trim().is_empty() {
            bail!("provider.base_url must not be empty");
        }
        if self.provider.model.trim().is_empty() {
            bail!("provider.model must not be empty");
        }
        Ok(())
    }
}

fn apply_override(root: &mut toml::Value, name: &str, raw: &str) -> anyhow::Result<()> {
    let path: Vec<String> = name["NOCTIS__".len()..]
        .split("__")
        .map(str::to_ascii_lowercase)
        .collect();
    if path.len() != 2 || path.iter().any(String::is_empty) {
        bail!("invalid environment override {name}");
    }
    let parsed = format!("value = {raw}")
        .parse::<toml::Table>()
        .map(|mut table| table.remove("value").unwrap())
        .unwrap_or_else(|_| toml::Value::String(raw.to_owned()));
    root.as_table_mut()
        .context("runtime config root must be a table")?
        .entry(&path[0])
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .with_context(|| format!("config section {} must be a table", path[0]))?
        .insert(path[1].clone(), parsed);
    Ok(())
}

fn required_secret(env: &HashMap<String, String>, name: &str) -> anyhow::Result<String> {
    env.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .with_context(|| format!("{name} is required and must come from environment"))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: Server::default(),
            database: Database::default(),
            scheduler: Scheduler::default(),
            budgets: Budgets::default(),
            git: Git::default(),
            artifacts: Artifacts::default(),
            runner: Runner::default(),
            provider: ProviderBootstrap::default(),
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:7410".parse().unwrap(),
            data_dir: "./data".into(),
        }
    }
}

impl Default for Database {
    fn default() -> Self {
        Self {
            max_connections: 10,
            acquire_timeout_seconds: 5,
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            max_parallel_agents: 2,
            heartbeat_seconds: 10,
            stale_after_seconds: 60,
        }
    }
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            default_project_tokens: 300_000,
            default_task_input_tokens: 30_000,
            default_task_output_tokens: 8_000,
            reserve_percent: 15,
        }
    }
}

impl Default for Git {
    fn default() -> Self {
        Self {
            worktree_root: "./data/worktrees".into(),
            retention_hours: 24,
        }
    }
}

impl Default for Artifacts {
    fn default() -> Self {
        Self {
            root: "./data/artifacts".into(),
            max_tool_output_bytes: 1_048_576,
        }
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            network_enabled: false,
        }
    }
}

impl Default for ProviderBootstrap {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5-mini".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HashMap<String, String> {
        HashMap::from([
            ("DATABASE_URL".into(), "postgres://localhost/noctis".into()),
            ("PRIMARY_API_KEY".into(), "test-secret".into()),
        ])
    }

    #[test]
    fn valid_defaults_are_safe() {
        let (config, _) = Config::load_from(env()).unwrap();
        assert_eq!(config.server.bind, "127.0.0.1:7410".parse().unwrap());
        assert_eq!(config.scheduler.max_parallel_agents, 2);
        assert!(!config.runner.network_enabled);
    }

    #[test]
    fn valid_toml_loads() {
        let mut values = env();
        values.insert("NOCTIS_CONFIG".into(), "config/example.toml".into());
        let (config, _) = Config::load_from(values).unwrap();
        assert_eq!(config.artifacts.max_tool_output_bytes, 1_048_576);
    }

    #[test]
    fn environment_overrides_typed_value() {
        let mut values = env();
        values.insert("NOCTIS__SCHEDULER__MAX_PARALLEL_AGENTS".into(), "4".into());
        let (config, _) = Config::load_from(values).unwrap();
        assert_eq!(config.scheduler.max_parallel_agents, 4);
    }

    #[test]
    fn missing_secret_names_field_without_value() {
        let mut values = env();
        values.remove("PRIMARY_API_KEY");
        let error = Config::load_from(values).unwrap_err().to_string();
        assert!(error.contains("PRIMARY_API_KEY"));
        assert!(!error.contains("test-secret"));
    }

    #[test]
    fn invalid_limit_names_field() {
        let mut values = env();
        values.insert("NOCTIS__BUDGETS__RESERVE_PERCENT".into(), "101".into());
        let error = Config::load_from(values).unwrap_err().to_string();
        assert!(error.contains("budgets.reserve_percent"));
    }
}
