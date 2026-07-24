//! Slash commands typed into the chat input.
//!
//! Parsing is kept separate from execution so the command set can be tested
//! without a server: [`parse`] is pure, and [`Command`] is acted on in
//! `main.rs` where the API client and event channel live.

/// A recognised slash command. Anything not starting with `/` is an ordinary
/// message and never reaches this type.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start a new session, optionally with a specific model.
    New { model: Option<String> },
    /// Return to the session list.
    Sessions,
    /// Show the current model, or start a new session on a different one.
    Model { name: Option<String> },
    /// List the available commands.
    Help,
    /// Exit the TUI.
    Quit,
    /// Typed something starting with `/` that isn't a command.
    Unknown { name: String },
}

/// Every command, with help text. Kept as data so `/help` can't drift out of
/// sync with what `parse` accepts.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/new [model]", "start a new session"),
    ("/sessions", "back to the session list"),
    (
        "/model [name]",
        "show the model, or start a session on another",
    ),
    ("/help", "show this list"),
    ("/quit", "exit auger"),
];

/// Parse input as a slash command. Returns `None` for ordinary messages.
pub fn parse(input: &str) -> Option<Command> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;

    // A bare "/" is still being typed; treat it as a normal message so it
    // doesn't flash an "unknown command" error mid-keystroke.
    if rest.is_empty() {
        return None;
    }

    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let argument = parts.next().map(str::trim).filter(|s| !s.is_empty());

    let command = match name {
        "new" => Command::New {
            model: argument.map(str::to_string),
        },
        "sessions" | "list" => Command::Sessions,
        "model" => Command::Model {
            name: argument.map(str::to_string),
        },
        "help" | "?" => Command::Help,
        "quit" | "exit" | "q" => Command::Quit,
        other => Command::Unknown {
            name: other.to_string(),
        },
    };
    Some(command)
}

/// Body of the `/help` listing.
pub fn help_text() -> String {
    let mut out = String::from("Commands:");
    for (name, description) in COMMANDS {
        out.push_str(&format!("\n  {name:<16} {description}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_messages_are_not_commands() {
        assert!(parse("hello").is_none());
        assert!(parse("what is /new").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn a_bare_slash_is_not_yet_a_command() {
        // Mid-typing; erroring here would flash on every leading keystroke.
        assert!(parse("/").is_none());
    }

    #[test]
    fn new_parses_with_and_without_a_model() {
        assert_eq!(parse("/new"), Some(Command::New { model: None }));
        assert_eq!(
            parse("/new gpt-5"),
            Some(Command::New {
                model: Some("gpt-5".into())
            })
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(parse("  /new  "), Some(Command::New { model: None }));
        assert_eq!(
            parse("/new   gpt-5  "),
            Some(Command::New {
                model: Some("gpt-5".into())
            })
        );
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(parse("/q"), Some(Command::Quit));
        assert_eq!(parse("/exit"), Some(Command::Quit));
        assert_eq!(parse("/list"), Some(Command::Sessions));
        assert_eq!(parse("/?"), Some(Command::Help));
    }

    #[test]
    fn unknown_commands_are_reported_by_name() {
        assert_eq!(
            parse("/nope"),
            Some(Command::Unknown {
                name: "nope".into()
            })
        );
    }

    #[test]
    fn model_takes_an_optional_name() {
        assert_eq!(parse("/model"), Some(Command::Model { name: None }));
        assert_eq!(
            parse("/model opus"),
            Some(Command::Model {
                name: Some("opus".into())
            })
        );
    }

    #[test]
    fn help_lists_every_command_that_parses() {
        let text = help_text();
        for (name, _) in COMMANDS {
            let bare = name.split_whitespace().next().unwrap();
            assert!(text.contains(bare), "{bare} missing from help");
            assert!(
                parse(bare).is_some_and(|c| !matches!(c, Command::Unknown { .. })),
                "{bare} is listed but does not parse"
            );
        }
    }
}
