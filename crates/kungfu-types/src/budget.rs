use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Budget {
    #[default]
    Small,
    Medium,
    Full,
}

impl Budget {
    pub fn top_k(&self) -> usize {
        match self {
            Budget::Small => 5,
            Budget::Medium => 8,
            Budget::Full => 12,
        }
    }

    pub fn max_lines(&self) -> usize {
        match self {
            Budget::Small => 20,
            Budget::Medium => 40,
            Budget::Full => 100,
        }
    }
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Budget::Small => write!(f, "small"),
            Budget::Medium => write!(f, "medium"),
            Budget::Full => write!(f, "full"),
        }
    }
}

impl FromStr for Budget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "small" => Ok(Budget::Small),
            "medium" => Ok(Budget::Medium),
            "full" => Ok(Budget::Full),
            _ => Err(format!("invalid budget: '{}' (expected small, medium, full)", s)),
        }
    }
}
