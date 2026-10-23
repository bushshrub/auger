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

impl StatusItem {
    const ALL: [(StatusItem, &'static str); 6] = [
        (StatusItem::Model, "model"),
        (StatusItem::Effort, "effort"),
        (StatusItem::Input, "input"),
        (StatusItem::Output, "output"),
        (StatusItem::Status, "status"),
        (StatusItem::Context, "context"),
    ];
    pub const ALL_NAMES: [&'static str; 6] = ["model", "effort", "input", "output", "status", "context"];

    fn name(self) -> &'static str {
        Self::ALL.iter().find(|(item, _)| *item == self).map_or("?", |(_, n)| n)
    }
}

/// Space-separated field names, e.g. "model input status".
pub fn names(items: &[StatusItem]) -> String {
    items.iter().map(|i| i.name()).collect::<Vec<_>>().join(" ")
}

/// Parse space- or comma-separated field names.
pub fn parse_items(text: &str) -> Result<Vec<StatusItem>, String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(|word| {
            StatusItem::ALL
                .iter()
                .find(|(_, n)| *n == word)
                .map(|(item, _)| *item)
                .ok_or_else(|| {
                    format!("unknown statusline field `{word}`; try: {}", StatusItem::ALL_NAMES.join(" "))
                })
        })
        .collect()
}

/// Write `items` to `[tui] statusline` in the config file, keeping the rest
/// of the file (comments included) as it is.
pub fn save_statusline(items: &[StatusItem]) -> anyhow::Result<PathBuf> {
    let path = config_path().ok_or_else(|| anyhow::anyhow!("no HOME or XDG_CONFIG_HOME"))?;
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = contents.parse()?;
    let mut array = toml_edit::Array::new();
    for item in items {
        array.push(item.name());
    }
    if !doc.contains_table("tui") {
        doc["tui"] = toml_edit::table();
    }
    doc["tui"]["statusline"] = toml_edit::value(array);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, doc.to_string())?;
    Ok(path)
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
