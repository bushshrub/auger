//! TUI settings from the `[tui]` table of `~/.config/auger/config.toml`
//! (or `$XDG_CONFIG_HOME/auger/config.toml`). The rest of that file belongs
//! to the server.

use serde::Deserialize;
use std::path::PathBuf;

/// One field of the bottom statusline.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusItem {
    Model,
    /// Reasoning effort. Hidden until auger has the concept.
    Effort,
    /// Input tokens, with the cached share in parentheses.
    Input,
    Output,
    /// idle / working.
    Status,
    /// Share of the context window in use.
    Context,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    pub statusline: Vec<StatusItem>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            statusline: vec![
                StatusItem::Model,
                StatusItem::Effort,
                StatusItem::Input,
                StatusItem::Output,
                StatusItem::Status,
            ],
        }
    }
}

#[derive(Default, Deserialize)]
struct File {
    #[serde(default)]
    tui: Option<TuiConfig>,
}

impl TuiConfig {
    /// Load the `[tui]` table. A missing file or table gives the defaults.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = config_path().filter(|p| p.exists()) else {
            return Ok(Self::default());
        };
        let contents = std::fs::read_to_string(&path)?;
        let file: File = toml::from_str(&contents)
            .map_err(|error| anyhow::anyhow!("failed to parse {}: {error}", path.display()))?;
        Ok(file.tui.unwrap_or_default())
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(config_home).join("auger/config.toml"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/auger/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_tui_table() {
        let file: File = toml::from_str(
            r#"
model = "m"
[provider]
type = "x"
[tui]
statusline = ["model", "context"]
"#,
        )
        .unwrap();
        assert_eq!(
            file.tui.unwrap().statusline,
            [StatusItem::Model, StatusItem::Context]
        );
    }

    #[test]
    fn a_file_without_the_table_gives_defaults() {
        let file: File = toml::from_str("model = \"m\"").unwrap();
        assert_eq!(file.tui.unwrap_or_default().statusline.len(), 5);
    }
}
