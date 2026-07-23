use serde::Deserialize;
use std::path::PathBuf;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_MODEL: &str = "qwen3.6-35b-q8";
const DEFAULT_PROVIDER_TYPE: &str = "openai-responses";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub(crate) listen_addr: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) user_agent: Option<String>,
    pub(crate) provider: ProviderConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ProviderConfig {
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: Option<String>,
}

impl Config {
    pub(crate) fn load() -> Result<Self, String> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        toml::from_str(&contents)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))
    }

    pub(crate) fn listen_addr(&self) -> String {
        std::env::var("LISTEN_ADDR")
            .ok()
            .or_else(|| self.listen_addr.clone())
            .unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_string())
    }

    pub(crate) fn model(&self) -> String {
        std::env::var("MODEL")
            .ok()
            .or_else(|| self.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }

    pub(crate) fn user_agent(&self) -> String {
        std::env::var("USER_AGENT")
            .ok()
            .or_else(|| self.user_agent.clone())
            .unwrap_or_default()
    }

    pub(crate) fn provider_type(&self) -> String {
        std::env::var("PROVIDER_TYPE")
            .ok()
            .or_else(|| self.provider.kind.clone())
            .unwrap_or_else(|| DEFAULT_PROVIDER_TYPE.to_string())
    }

    pub(crate) fn provider_api_key(&self) -> String {
        std::env::var("PROVIDER_API_KEY")
            .ok()
            .or_else(|| self.provider.api_key.clone())
            .unwrap_or_default()
    }

    pub(crate) fn provider_base_url(&self) -> Option<String> {
        std::env::var("PROVIDER_BASE_URL")
            .ok()
            .or_else(|| self.provider.base_url.clone())
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(config_home).join("auger/config.toml"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".auger/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_server_and_provider_settings() {
        let config: Config = toml::from_str(
            r#"
listen_addr = "0.0.0.0:4000"
model = "test-model"
user_agent = "my-client/1.0"

[provider]
type = "anthropic"
api_key = "secret"
base_url = "https://example.test"
"#,
        )
        .unwrap();

        assert_eq!(config.listen_addr.as_deref(), Some("0.0.0.0:4000"));
        assert_eq!(config.model.as_deref(), Some("test-model"));
        assert_eq!(config.user_agent.as_deref(), Some("my-client/1.0"));
        assert_eq!(config.provider.kind.as_deref(), Some("anthropic"));
        assert_eq!(config.provider.api_key.as_deref(), Some("secret"));
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("https://example.test")
        );
    }
}
