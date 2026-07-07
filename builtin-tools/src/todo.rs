use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

use agent_tools::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u32,
    pub title: String,
    pub status: TodoStatus,
}

/// A stateful todo list tool. Maintains its item list across calls via shared state.
#[derive(Clone)]
pub struct TodoList {
    items: Arc<Mutex<Vec<TodoItem>>>,
    next_id: Arc<Mutex<u32>>,
}

impl TodoList {
    pub fn new() -> Self {
        Self {
            items: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }
}

impl Default for TodoList {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoList {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "todo_list",
            description: "Manage a shared todo list to track tasks. \
                Use 'add' to create items, 'update' to change title/status, \
                'remove' to delete, and 'list' to view all items. \
                Status values: pending, in_progress, done.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "update", "remove", "list"],
                    "description": "The action to perform"
                },
                "id": {
                    "type": "integer",
                    "description": "Item ID (required for update and remove)"
                },
                "title": {
                    "type": "string",
                    "description": "Item title (required for add, optional for update)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "done"],
                    "description": "Item status (optional for add, defaults to pending)"
                }
            },
            "required": ["action"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: action".into()))?;

        let mut items = self.items.lock().unwrap();

        match action {
            "add" => {
                let title = args["title"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArgs("add requires 'title'".into()))?
                    .to_string();
                let status = parse_status(args["status"].as_str().unwrap_or("pending"))?;
                let id = {
                    let mut next = self.next_id.lock().unwrap();
                    let id = *next;
                    *next += 1;
                    id
                };
                items.push(TodoItem { id, title, status });
            }
            "update" => {
                let id = args["id"]
                    .as_u64()
                    .ok_or_else(|| ToolError::InvalidArgs("update requires 'id'".into()))?
                    as u32;
                let item = items
                    .iter_mut()
                    .find(|i| i.id == id)
                    .ok_or_else(|| ToolError::Execution(format!("no item with id {id}")))?;
                if let Some(title) = args["title"].as_str() {
                    item.title = title.to_string();
                }
                if let Some(s) = args["status"].as_str() {
                    item.status = parse_status(s)?;
                }
            }
            "remove" => {
                let id = args["id"]
                    .as_u64()
                    .ok_or_else(|| ToolError::InvalidArgs("remove requires 'id'".into()))?
                    as u32;
                let before = items.len();
                items.retain(|i| i.id != id);
                if items.len() == before {
                    return Err(ToolError::Execution(format!("no item with id {id}")));
                }
            }
            "list" => {}
            other => {
                return Err(ToolError::InvalidArgs(format!("unknown action: {other}")));
            }
        }

        Ok(ToolCallResult::success(
            json!({ "items": *items }).to_string(),
        ))
    }
}

fn parse_status(s: &str) -> Result<TodoStatus, ToolError> {
    match s {
        "pending" => Ok(TodoStatus::Pending),
        "in_progress" => Ok(TodoStatus::InProgress),
        "done" => Ok(TodoStatus::Done),
        other => Err(ToolError::InvalidArgs(format!("unknown status: {other}"))),
    }
}
