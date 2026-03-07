use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub root: String,
    pub created_at: DateTime<Utc>,
    pub kungfu_version: String,
}
