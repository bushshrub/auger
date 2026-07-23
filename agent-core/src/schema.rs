use std::path::PathBuf;
use chrono::{DateTime, Utc};
use getset::Getters;
use serde::{Deserialize, Serialize};
use auger_driver::ToolCallId;
use crate::session::history::ModelInfo;
use crate::SessionId;
use crate::tools::tool_execution::ToolData;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TraceLine {
    SessionHeader(SessionHeader)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionMetadata {
    cwd: PathBuf
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionHeader {
    version: u32,
    session_id: SessionId,
    created_at: DateTime<Utc>,
    metadata: SessionMetadata,
    model: ModelInfo
}