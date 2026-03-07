use serde::{Deserialize, Serialize};

use crate::symbol::Span;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub file_id: String,
    pub span: Span,
    pub content: String,
    pub kind: ChunkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    Code,
    Comment,
    Doc,
    Config,
    Text,
}
