//! or2's projection of a herdr session: what the inbox and host screens render. Built by the
//! herdr client from snapshots and events; delivered whole, never as patches.

/// herdr's agent status; values this build does not know become `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub label: Option<String>,
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub focused: bool,
}

/// A pane running an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub display_agent: Option<String>,
    pub status: AgentStatus,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub focused: bool,
    /// herdr's counter of status transitions for this pane.
    pub state_change_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrView {
    /// Increases with every delivered view of one watch.
    pub version: u64,
    /// herdr's API protocol number.
    pub protocol: u32,
    pub focused_pane_id: Option<String>,
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    pub agents: Vec<Agent>,
}
