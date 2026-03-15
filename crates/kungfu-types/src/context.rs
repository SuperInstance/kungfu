use serde::{Deserialize, Serialize};

use crate::budget::Budget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Lookup,
    Debug,
    Understand,
    Impact,
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Intent::Lookup => write!(f, "lookup"),
            Intent::Debug => write!(f, "debug"),
            Intent::Understand => write!(f, "understand"),
            Intent::Impact => write!(f, "impact"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPacket {
    pub query: String,
    pub budget: Budget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    pub items: Vec<ContextItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    #[serde(rename = "type")]
    pub item_type: ContextItemType,
    pub path: String,
    pub name: String,
    pub signature: Option<String>,
    pub why: String,
    pub score: f64,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemType {
    Symbol,
    File,
    Chunk,
    Config,
    Test,
}
