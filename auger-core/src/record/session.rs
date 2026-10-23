use chrono::DateTime;
use chrono::Utc;
use getset::CopyGetters;
use getset::Getters;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

/// ID of an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ModelInfo {
    provider: String,
    id: String,
}

impl ModelInfo {
    pub(crate) fn new(provider: String, id: String) -> Self {
        ModelInfo { provider, id }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Getters, CopyGetters)]
pub struct SessionData {
    #[getset(get_copy = "pub")]
    session_id: SessionId,
    #[getset(get_copy = "pub")]
    created_at: DateTime<Utc>,
    #[getset(get = "pub")]
    cwd: PathBuf,
    #[getset(get = "pub")]
    model_info: ModelInfo,
}

impl SessionData {
    pub fn new(
        session_id: SessionId,
        created_at: DateTime<Utc>,
        cwd: PathBuf,
        model_info: ModelInfo,
    ) -> Self {
        Self {
            session_id,
            created_at,
            cwd,
            model_info,
        }
    }
}
