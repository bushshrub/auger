//! Slash-command autocomplete for the chat input.
//!
//! The match list is derived from the input on every keystroke rather than
//! stored, so it can't drift out of sync with what has been typed. Only the
//! highlighted row and the dismissed flag are state.

use crate::command::COMMANDS;

/// A command offered in the popup: the name as typed and its help text.
pub struct Match {
    pub name: &'static str,
    pub description: &'static str,
    /// Whether the command takes an argument, so accepting it leaves a space
    /// for the user to keep typing.
    pub takes_argument: bool,
}

#[derive(Default)]
pub struct Completion {
    /// Row highlighted in the popup.
    selected: usize,
    /// Set by Esc, cleared by the next edit, so the popup stays out of the way
    /// once dismissed but comes back for the next command.
    dismissed: bool,
}

impl Completion {
    /// Commands matching what has been typed. Empty once the input has an
    /// argument: the popup completes names, not values.
    pub fn matches(&self, input: &str) -> Vec<Match> {
        if self.dismissed || !input.starts_with('/') || input.contains(char::is_whitespace) {
            return vec![];
        }
        COMMANDS
            .iter()
            .filter_map(|(name, description)| {
                let bare = name.split_whitespace().next()?;
                bare.starts_with(input).then_some(Match {
                    name: bare,
                    description,
                    takes_argument: name.contains('['),
                })
            })
            .collect()
    }

    pub fn is_open(&self, input: &str) -> bool {
        !self.matches(input).is_empty()
    }

    /// Highlighted row, clamped in case the match list shrank as the user typed.
    pub fn selected(&self, input: &str) -> usize {
        let count = self.matches(input).len();
        if count == 0 {
            0
        } else {
            self.selected.min(count - 1)
        }
    }

    pub fn next(&mut self, input: &str) {
        let count = self.matches(input).len();
        if count > 0 {
            self.selected = (self.selected(input) + 1) % count;
        }
    }

    pub fn prev(&mut self, input: &str) {
        let count = self.matches(input).len();
        if count > 0 {
            self.selected = (self.selected(input) + count - 1) % count;
        }
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// Called on every edit: a fresh keystroke re-opens the popup and starts
    /// the highlight at the top.
    pub fn reset(&mut self) {
        self.dismissed = false;
        self.selected = 0;
    }

    /// The input text after accepting the highlighted match.
    pub fn accept(&self, input: &str) -> Option<String> {
        let matches = self.matches(input);
        let chosen = matches.get(self.selected(input))?;
        Some(match chosen.takes_argument {
            true => format!("{} ", chosen.name),
            false => chosen.name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(completion: &Completion, input: &str) -> Vec<&'static str> {
        completion.matches(input).iter().map(|m| m.name).collect()
    }

    #[test]
    fn a_bare_slash_offers_every_command() {
        let completion = Completion::default();
        assert_eq!(names(&completion, "/").len(), COMMANDS.len());
    }

    #[test]
    fn typing_narrows_the_list() {
        let completion = Completion::default();
        assert_eq!(names(&completion, "/mo"), ["/model"]);
        assert!(names(&completion, "/zzz").is_empty());
    }

    #[test]
    fn ordinary_messages_never_open_the_popup() {
        let completion = Completion::default();
        assert!(!completion.is_open("hello"));
        assert!(!completion.is_open(""));
    }

    #[test]
    fn the_popup_closes_once_an_argument_is_being_typed() {
        let completion = Completion::default();
        assert!(completion.is_open("/model"));
        assert!(!completion.is_open("/model "), "now completing a value");
    }

    #[test]
    fn accepting_leaves_a_space_only_for_commands_that_take_arguments() {
        let completion = Completion::default();
        assert_eq!(completion.accept("/new").as_deref(), Some("/new "));
        assert_eq!(completion.accept("/he").as_deref(), Some("/help"));
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut completion = Completion::default();
        let count = names(&completion, "/").len();
        completion.prev("/");
        assert_eq!(completion.selected("/"), count - 1);
        completion.next("/");
        assert_eq!(completion.selected("/"), 0);
    }

    #[test]
    fn a_shrinking_list_keeps_the_highlight_in_range() {
        let mut completion = Completion::default();
        for _ in 0..4 {
            completion.next("/");
        }
        // "/mo" matches one command; the old row index is out of bounds.
        assert_eq!(completion.selected("/mo"), 0);
        assert_eq!(completion.accept("/mo").as_deref(), Some("/model "));
    }

    #[test]
    fn dismissing_hides_the_popup_until_the_next_edit() {
        let mut completion = Completion::default();
        completion.dismiss();
        assert!(!completion.is_open("/"));
        completion.reset();
        assert!(completion.is_open("/"));
    }
}
