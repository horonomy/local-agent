//! Tool capability classes used by the policy decision contract (ADR-019).

use serde::{Deserialize, Serialize};

/// What kind of authority a tool call exercises. A tool that does not
/// declare one is [`ToolCapability::Unknown`], which policy treats as
/// unclassified and never grants implicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCapability {
    FsRead,
    FsWrite,
    Shell,
    Network,
    Memory,
    Mcp,
    Subagent,
    Browser,
    Desktop,
    Plugin,
    AgentInternal,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ToolCapability::FsRead).unwrap(),
            "\"fs_read\""
        );
        assert_eq!(
            serde_json::from_str::<ToolCapability>("\"agent_internal\"").unwrap(),
            ToolCapability::AgentInternal
        );
    }
}
