//! Works out how the driver's turns changed between two transitions.

use crate::record::TurnsChange;
use auger_driver::Turn;

/// What the trace last saw of the turns: their count and the last turn.
/// Earlier turns never change, so that is enough to diff against.
#[derive(Default)]
pub(crate) struct TurnsCursor {
    len: usize,
    last: Option<Turn>,
}

impl TurnsCursor {
    pub(crate) fn new(turns: &[Turn]) -> Self {
        Self {
            len: turns.len(),
            last: turns.last().cloned(),
        }
    }

    /// The changes that turn what the cursor saw into `turns`, and move the
    /// cursor there.
    pub(crate) fn advance(&mut self, turns: &[Turn]) -> Vec<TurnsChange> {
        let mut changes = Vec::new();
        // How many leading turns are unchanged.
        let kept = match &self.last {
            None => 0,
            Some(last) => match turns.get(self.len - 1) {
                Some(turn) if turn == last => self.len,
                Some(Turn::Input { entries }) => match last {
                    Turn::Input { entries: seen } if entries.starts_with(seen) => {
                        changes.extend(
                            entries[seen.len()..]
                                .iter()
                                .cloned()
                                .map(TurnsChange::PushEntry),
                        );
                        self.len
                    }
                    _ => self.len - 1,
                },
                _ => (self.len - 1).min(turns.len()),
            },
        };
        if kept < self.len {
            changes.push(TurnsChange::Truncate { len: kept });
        }
        changes.extend(turns[kept..].iter().cloned().map(TurnsChange::PushTurn));
        *self = Self::new(turns);
        changes
    }
}

/// Apply `change` to `turns`. Fails if the change does not fit.
pub(crate) fn apply(turns: &mut Vec<Turn>, change: TurnsChange) -> Result<(), ()> {
    match change {
        TurnsChange::PushTurn(turn) => turns.push(turn),
        TurnsChange::PushEntry(entry) => match turns.last_mut() {
            Some(Turn::Input { entries }) => entries.push(entry),
            _ => return Err(()),
        },
        TurnsChange::Truncate { len } if len <= turns.len() => turns.truncate(len),
        TurnsChange::Truncate { .. } => return Err(()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auger_driver::HarnessEntry;
    use auger_driver::InputEntry;
    use provider::AssistantResponse;
    use provider::Block;
    use provider::ToolResult;
    use provider::UserPrompt;

    fn user(text: &str) -> InputEntry {
        InputEntry::User(UserPrompt::new(text.to_string()))
    }

    fn input(entries: Vec<InputEntry>) -> Turn {
        Turn::Input { entries }
    }

    fn output(text: &str) -> Turn {
        Turn::Output(AssistantResponse::new(vec![Block::Text(text.to_string())]).unwrap())
    }

    fn result(id: &str) -> InputEntry {
        InputEntry::Harness(HarnessEntry::ToolResult(ToolResult::new(
            id.to_string(),
            "out".to_string(),
        )))
    }

    /// Replaying the diffs of every step rebuilds every step, for the
    /// transitions the driver makes: push, grow the last input, amend
    /// (replace the last input), seal a partial, and shrink.
    #[test]
    fn replaying_diffs_rebuilds_each_step() {
        let steps = vec![
            vec![],
            vec![input(vec![user("a")])],
            vec![input(vec![user("a"), user("b")])],
            // amend: same length, different last turn
            vec![input(vec![user("c")])],
            vec![input(vec![user("c")]), output("x")],
            vec![input(vec![user("c")]), output("x"), input(vec![result("1")])],
            vec![
                input(vec![user("c")]),
                output("x"),
                input(vec![result("1"), user("d")]),
                output("y"),
            ],
            // seal a partial and continue in one transition
            vec![
                input(vec![user("c")]),
                output("x"),
                input(vec![result("1"), user("d")]),
                output("y"),
                input(vec![user("e")]),
                output("partial"),
                input(vec![user("f")]),
            ],
            // amend: drop the trailing input, start a new one
            vec![
                input(vec![user("c")]),
                output("x"),
                input(vec![result("1"), user("d")]),
                output("y"),
                input(vec![user("e")]),
                output("partial"),
                input(vec![user("g")]),
            ],
            vec![input(vec![user("c")])],
            vec![],
        ];
        let mut cursor = TurnsCursor::default();
        let mut replayed = Vec::new();
        for step in steps {
            for change in cursor.advance(&step) {
                apply(&mut replayed, change).unwrap();
            }
            assert_eq!(replayed, step);
        }
    }

    #[test]
    fn unchanged_turns_produce_no_changes() {
        let turns = vec![input(vec![user("a")]), output("x")];
        let mut cursor = TurnsCursor::new(&turns);
        assert!(cursor.advance(&turns).is_empty());
    }

    #[test]
    fn growing_the_last_input_pushes_only_the_new_entries() {
        let mut cursor = TurnsCursor::new(&[input(vec![user("a")])]);
        let changes = cursor.advance(&[input(vec![user("a"), result("1")])]);
        assert_eq!(changes, vec![TurnsChange::PushEntry(result("1"))]);
    }
}
