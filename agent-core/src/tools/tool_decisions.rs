use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

#[derive(Debug)]
pub(crate) struct Resolving;
#[derive(Debug)]
pub(crate) struct Resolved;

#[derive(Debug)]
pub enum ToolAuthorization {
    AllAutoApproved,
    PerTool(UserToolDecisions<Resolved>),
}

impl ToolAuthorization {
    pub(crate) fn denial_reason(&self, id: &str) -> Option<String> {
        match self {
            Self::AllAutoApproved => None,
            Self::PerTool(decisions) => match decisions.decided_tools.get(id) {
                Some((true, _)) => None,
                Some((false, reason)) => Some(
                    reason
                        .clone()
                        .unwrap_or_else(|| "Denied by user".to_string()),
                ),
                None => unreachable!("resolved tool authorization is missing tool call {id}"),
            },
        }
    }
}

#[derive(Debug)]
pub struct UserToolDecisions<S> {
    undecided_tool_ids: HashSet<String>,
    decided_tools: HashMap<String, (bool, Option<String>)>,
    _state: PhantomData<S>,
}

impl UserToolDecisions<Resolving> {
    pub fn new_undecided(undecided: Vec<String>) -> Self {
        Self {
            undecided_tool_ids: undecided.into_iter().collect(),
            decided_tools: HashMap::new(),
            _state: PhantomData,
        }
    }

    pub fn decision_needed(&self, id: &str) -> bool {
        self.undecided_tool_ids.contains(id)
    }

    pub fn record_decision(
        mut self,
        id: String,
        decision: bool,
        reason: Option<String>,
    ) -> either::Either<Self, UserToolDecisions<Resolved>> {
        if !self.undecided_tool_ids.remove(&id) {
            return either::Either::Left(self);
        }

        self.decided_tools.insert(id, (decision, reason));

        if self.undecided_tool_ids.is_empty() {
            either::Either::Right(UserToolDecisions {
                undecided_tool_ids: self.undecided_tool_ids,
                decided_tools: self.decided_tools,
                _state: PhantomData,
            })
        } else {
            either::Either::Left(self)
        }
    }
}
