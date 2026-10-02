//! herdr watch contract for Kotlin: the projected view, watch state, the `HerdrListener`
//! callback and the `HerdrWatch` object. See docs/contracts.md.

use std::sync::Arc;

use or2_core::herdr as core;

use crate::session::ListenerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrWorkspace {
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrTab {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrPane {
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

/// The agent a notification's reply is for (API 16; `HostConnection.reply_to_pane`), from the
/// [`HerdrAgent`] that raised the notification: its `terminal_id` and its kind (`agent`). herdr
/// must still report that agent in the pane, else the reply is `PaneNotFound` and nothing is
/// sent. A `None` kind matches any.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentIdentity {
    pub terminal_id: String,
    pub agent: Option<String>,
}

impl From<AgentIdentity> for core::AgentIdentity {
    fn from(identity: AgentIdentity) -> Self {
        Self {
            terminal_id: identity.terminal_id,
            agent: identity.agent,
        }
    }
}

/// A pane running an agent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrAgent {
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
    pub state_change_seq: u64,
    /// herdr's id of the pane's terminal (API 16): a pane id reused for a new terminal (herdr
    /// restarted) is a new pane. A reply names it ([`AgentIdentity`]).
    pub terminal_id: String,
}

/// or2's projection of one herdr session, delivered whole.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrView {
    /// Increases with every delivered view of one watch.
    pub version: u64,
    pub protocol: u32,
    pub focused_pane_id: Option<String>,
    pub workspaces: Vec<HerdrWorkspace>,
    pub tabs: Vec<HerdrTab>,
    pub panes: Vec<HerdrPane>,
    pub agents: Vec<HerdrAgent>,
}

/// `NotInstalled` and `IncompatibleProtocol` are final; `NotRunning` and `Failed` retry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum HerdrUnavailable {
    NotInstalled,
    IncompatibleProtocol { protocol: u32 },
    NotRunning,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum HerdrState {
    /// Initial; not delivered as a change.
    Starting,
    Live {
        view: HerdrView,
    },
    /// `message` is a diagnostic without secrets; do not match on it.
    Unavailable {
        reason: HerdrUnavailable,
        message: String,
    },
    Closed,
}

impl From<core::AgentStatus> for AgentStatus {
    fn from(status: core::AgentStatus) -> Self {
        match status {
            core::AgentStatus::Idle => Self::Idle,
            core::AgentStatus::Working => Self::Working,
            core::AgentStatus::Blocked => Self::Blocked,
            core::AgentStatus::Done => Self::Done,
            core::AgentStatus::Unknown => Self::Unknown,
        }
    }
}

impl From<core::Workspace> for HerdrWorkspace {
    fn from(w: core::Workspace) -> Self {
        Self {
            workspace_id: w.workspace_id,
            number: w.number,
            label: w.label,
            focused: w.focused,
            agent_status: w.agent_status.into(),
        }
    }
}

impl From<core::Tab> for HerdrTab {
    fn from(t: core::Tab) -> Self {
        Self {
            tab_id: t.tab_id,
            workspace_id: t.workspace_id,
            number: t.number,
            label: t.label,
            focused: t.focused,
            agent_status: t.agent_status.into(),
        }
    }
}

impl From<core::Pane> for HerdrPane {
    fn from(p: core::Pane) -> Self {
        Self {
            pane_id: p.pane_id,
            tab_id: p.tab_id,
            workspace_id: p.workspace_id,
            label: p.label,
            agent: p.agent,
            agent_status: p.agent_status.into(),
            cwd: p.cwd,
            title: p.title,
            focused: p.focused,
        }
    }
}

impl From<core::Agent> for HerdrAgent {
    fn from(a: core::Agent) -> Self {
        Self {
            pane_id: a.pane_id,
            tab_id: a.tab_id,
            workspace_id: a.workspace_id,
            name: a.name,
            agent: a.agent,
            display_agent: a.display_agent,
            status: a.status.into(),
            cwd: a.cwd,
            title: a.title,
            focused: a.focused,
            state_change_seq: a.state_change_seq,
            terminal_id: a.terminal_id,
        }
    }
}

impl From<core::HerdrView> for HerdrView {
    fn from(view: core::HerdrView) -> Self {
        Self {
            version: view.version,
            protocol: view.protocol,
            focused_pane_id: view.focused_pane_id,
            workspaces: view.workspaces.into_iter().map(Into::into).collect(),
            tabs: view.tabs.into_iter().map(Into::into).collect(),
            panes: view.panes.into_iter().map(Into::into).collect(),
            agents: view.agents.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::HerdrUnavailable> for HerdrUnavailable {
    fn from(reason: core::HerdrUnavailable) -> Self {
        match reason {
            core::HerdrUnavailable::NotInstalled => Self::NotInstalled,
            core::HerdrUnavailable::IncompatibleProtocol { protocol } => {
                Self::IncompatibleProtocol { protocol }
            }
            core::HerdrUnavailable::NotRunning => Self::NotRunning,
            core::HerdrUnavailable::Failed => Self::Failed,
        }
    }
}

impl From<core::HerdrState> for HerdrState {
    fn from(state: core::HerdrState) -> Self {
        match state {
            core::HerdrState::Starting => Self::Starting,
            core::HerdrState::Live { view } => Self::Live { view: view.into() },
            core::HerdrState::Unavailable { reason, message } => Self::Unavailable {
                reason: reason.into(),
                message,
            },
            core::HerdrState::Closed => Self::Closed,
        }
    }
}

/// Implemented in Kotlin. Same threading rules as `SessionListener`: a Rust-owned thread, never
/// concurrent for one watch, in order; exceptions are ignored; released right after `Closed`.
#[uniffi::export(callback_interface)]
pub trait HerdrListener: Send + Sync {
    /// Every change after the initial `Starting`. `Closed` is delivered exactly once, last.
    fn on_herdr_state_changed(&self, state: HerdrState) -> Result<(), ListenerError>;
}

pub(crate) struct HerdrListenerObserver(pub(crate) Box<dyn HerdrListener>);

impl core::HerdrObserver for HerdrListenerObserver {
    fn state_changed(&self, state: &core::HerdrState) {
        let _ = self.0.on_herdr_state_changed(state.clone().into());
    }
}

/// A watch of one herdr session. Methods never block. Kotlin owns it; `close()` without
/// `stop()` also stops the watch.
#[derive(uniffi::Object)]
pub struct HerdrWatch {
    handle: core::HerdrWatchHandle,
}

impl HerdrWatch {
    pub(crate) fn new(handle: core::HerdrWatchHandle) -> Arc<Self> {
        Arc::new(Self { handle })
    }
}

#[uniffi::export]
impl HerdrWatch {
    pub fn state(&self) -> HerdrState {
        self.handle.state().into()
    }

    /// Idempotent; `Closed` follows through the listener.
    pub fn stop(&self) {
        self.handle.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_state_and_the_view_losslessly() {
        let view = core::HerdrView {
            version: 7,
            protocol: 22,
            focused_pane_id: Some("p1".into()),
            workspaces: vec![core::Workspace {
                workspace_id: "w1".into(),
                number: 1,
                label: "main".into(),
                focused: true,
                agent_status: core::AgentStatus::Blocked,
            }],
            tabs: vec![core::Tab {
                tab_id: "t1".into(),
                workspace_id: "w1".into(),
                number: 2,
                label: "tab".into(),
                focused: false,
                agent_status: core::AgentStatus::Done,
            }],
            panes: vec![core::Pane {
                pane_id: "p1".into(),
                tab_id: "t1".into(),
                workspace_id: "w1".into(),
                label: None,
                agent: Some("claude".into()),
                agent_status: core::AgentStatus::Working,
                cwd: Some("/x".into()),
                title: None,
                focused: true,
            }],
            agents: vec![core::Agent {
                pane_id: "p1".into(),
                tab_id: "t1".into(),
                workspace_id: "w1".into(),
                name: Some("n".into()),
                agent: Some("claude".into()),
                display_agent: Some("Claude".into()),
                status: core::AgentStatus::Unknown,
                cwd: None,
                title: Some("t".into()),
                focused: true,
                state_change_seq: u64::MAX,
                terminal_id: "term_1".into(),
            }],
        };
        let HerdrState::Live { view: mapped } = core::HerdrState::Live { view }.into() else {
            panic!("expected Live");
        };
        assert_eq!((mapped.version, mapped.protocol), (7, 22));
        assert_eq!(mapped.focused_pane_id.as_deref(), Some("p1"));
        assert_eq!(mapped.workspaces[0].agent_status, AgentStatus::Blocked);
        assert_eq!(mapped.workspaces[0].label, "main");
        assert_eq!(mapped.tabs[0].number, 2);
        assert_eq!(mapped.tabs[0].agent_status, AgentStatus::Done);
        assert_eq!(mapped.panes[0].agent.as_deref(), Some("claude"));
        assert_eq!(mapped.panes[0].agent_status, AgentStatus::Working);
        assert_eq!(mapped.agents[0].display_agent.as_deref(), Some("Claude"));
        assert_eq!(mapped.agents[0].status, AgentStatus::Unknown);
        assert_eq!(mapped.agents[0].state_change_seq, u64::MAX);

        assert_eq!(
            HerdrState::from(core::HerdrState::Starting),
            HerdrState::Starting
        );
        assert_eq!(
            HerdrState::from(core::HerdrState::Closed),
            HerdrState::Closed
        );
        assert_eq!(
            HerdrState::from(core::HerdrState::Unavailable {
                reason: core::HerdrUnavailable::IncompatibleProtocol { protocol: 99 },
                message: "m".into()
            }),
            HerdrState::Unavailable {
                reason: HerdrUnavailable::IncompatibleProtocol { protocol: 99 },
                message: "m".into()
            }
        );
        for (core_reason, reason) in [
            (
                core::HerdrUnavailable::NotInstalled,
                HerdrUnavailable::NotInstalled,
            ),
            (
                core::HerdrUnavailable::NotRunning,
                HerdrUnavailable::NotRunning,
            ),
            (core::HerdrUnavailable::Failed, HerdrUnavailable::Failed),
        ] {
            assert_eq!(HerdrUnavailable::from(core_reason), reason);
        }
    }
}
