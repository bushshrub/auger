use std::collections::{HashMap, HashSet};


#[derive(Debug)]
pub struct UserToolDecisions {
    undecided_tool_ids: HashSet<String>,
    decided_tools: HashMap<String, (bool, Option<String>)>
}

impl UserToolDecisions {
    pub fn new_undecided(undecided: Vec<String>) -> Self {
        Self {
            undecided_tool_ids: undecided.into_iter().collect(),
            decided_tools: HashMap::new()
        }
    }

    pub fn decision_needed(&self, id: &str) -> bool {
        self.undecided_tool_ids.contains(&id)
    }

    pub fn record_decision(mut self, id: String, decision: bool, reason: Option<String>) -> Self {
        if self.undecided_tool_ids.remove(&id) {
            self.decided_tools.insert(id, (decision, reason));
            self
        } else {
            panic!("the tool id {id} was not requested")
        }
    }
}