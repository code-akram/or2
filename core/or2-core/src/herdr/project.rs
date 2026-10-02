//! From herdr's `session.snapshot` to or2's [`HerdrView`].

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use super::generated;
use super::generated::success_response::{AgentStatus as WireStatus, SessionSnapshot};
use super::view::{Agent, AgentSession, AgentStatus, HerdrView, Pane, Tab, Workspace};

/// The oldest herdr API protocol whose snapshot this build reads. Newer protocols are accepted
/// (herdr only adds fields; unknown ones are ignored); an older one is
/// `IncompatibleProtocol`.
pub const MIN_PROTOCOL: u32 = generated::PROTOCOL;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// herdr speaks a protocol older than [`MIN_PROTOCOL`].
    Incompatible { protocol: u32 },
    /// The response is not a snapshot this build can read.
    Unreadable(String),
}

/// Types the `result` of a `session.snapshot` response. The protocol number is checked on the
/// raw JSON first, so a too-old herdr is reported as such rather than as a parse failure.
pub fn parse_snapshot(result: &Value) -> Result<SessionSnapshot, SnapshotError> {
    if result.get("type").and_then(Value::as_str) != Some("session_snapshot") {
        return Err(SnapshotError::Unreadable(
            "the response is not a session snapshot".into(),
        ));
    }
    let snapshot = result
        .get("snapshot")
        .ok_or_else(|| SnapshotError::Unreadable("the response has no snapshot".into()))?;
    let protocol = snapshot
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| SnapshotError::Unreadable("the snapshot has no protocol number".into()))?;
    let protocol = u32::try_from(protocol).unwrap_or(u32::MAX);
    if protocol < MIN_PROTOCOL {
        return Err(SnapshotError::Incompatible { protocol });
    }
    SessionSnapshot::deserialize(snapshot)
        .map_err(|error| SnapshotError::Unreadable(format!("unreadable snapshot: {error}")))
}

fn status(wire: &WireStatus) -> AgentStatus {
    use generated::success_response::AgentStatusVariant0 as Known;
    match wire {
        WireStatus::Variant0(Known::Idle) => AgentStatus::Idle,
        WireStatus::Variant0(Known::Working) => AgentStatus::Working,
        WireStatus::Variant0(Known::Blocked) => AgentStatus::Blocked,
        WireStatus::Variant0(Known::Done) => AgentStatus::Done,
        WireStatus::Variant0(Known::Unknown) | WireStatus::Variant1(_) => AgentStatus::Unknown,
    }
}

/// The view of `snapshot`, in herdr's order. `version` is 0: the watch numbers deliveries.
pub fn project(snapshot: &SessionSnapshot) -> HerdrView {
    HerdrView {
        version: 0,
        protocol: snapshot.protocol,
        focused_pane_id: snapshot.focused_pane_id.clone(),
        workspaces: snapshot
            .workspaces
            .iter()
            .map(|workspace| Workspace {
                workspace_id: workspace.workspace_id.clone(),
                number: workspace.number,
                label: workspace.label.clone(),
                focused: workspace.focused,
                agent_status: status(&workspace.agent_status),
            })
            .collect(),
        tabs: snapshot
            .tabs
            .iter()
            .map(|tab| Tab {
                tab_id: tab.tab_id.clone(),
                workspace_id: tab.workspace_id.clone(),
                number: tab.number,
                label: tab.label.clone(),
                focused: tab.focused,
                agent_status: status(&tab.agent_status),
            })
            .collect(),
        panes: snapshot
            .panes
            .iter()
            .map(|pane| Pane {
                pane_id: pane.pane_id.clone(),
                tab_id: pane.tab_id.clone(),
                workspace_id: pane.workspace_id.clone(),
                label: pane.label.clone(),
                agent: pane.agent.clone(),
                agent_status: status(&pane.agent_status),
                cwd: pane.cwd.clone(),
                title: pane.title.clone(),
                focused: pane.focused,
            })
            .collect(),
        agents: snapshot
            .agents
            .iter()
            .map(|agent| Agent {
                pane_id: agent.pane_id.clone(),
                tab_id: agent.tab_id.clone(),
                workspace_id: agent.workspace_id.clone(),
                name: agent.name.clone(),
                agent: agent.agent.clone(),
                display_agent: agent.display_agent.clone(),
                status: status(&agent.agent_status),
                cwd: agent.cwd.clone(),
                title: agent.title.clone(),
                focused: agent.focused,
                state_change_seq: agent.state_change_seq,
                terminal_id: agent.terminal_id.clone(),
                agent_session: agent.agent_session.as_ref().map(|session| AgentSession {
                    kind: session.kind.to_string(),
                    value: session.value.clone(),
                }),
                interactive_ready: agent.interactive_ready == Some(true),
            })
            .collect(),
    }
}

/// The panes that need a `pane.agent_status_changed` subscription, as `(pane_id,
/// terminal_id)`: a reused pane id with a new terminal is a new pane.
pub fn pane_keys(snapshot: &SessionSnapshot) -> BTreeSet<(String, String)> {
    snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.clone(), pane.terminal_id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::herdr::testing::fixture;

    fn result_of(line: &str) -> Value {
        let value: Value = serde_json::from_str(line).unwrap();
        value["result"].clone()
    }

    #[test]
    fn a_real_snapshot_projects_every_field() {
        let snapshot = parse_snapshot(&result_of(&fixture("snapshot_two_panes.json"))).unwrap();
        let view = project(&snapshot);
        assert_eq!(view.version, 0);
        assert_eq!(view.protocol, 22);
        assert_eq!(view.focused_pane_id.as_deref(), Some("w2:p2"));
        assert_eq!(
            view.workspaces,
            [
                Workspace {
                    workspace_id: "w1".into(),
                    number: 1,
                    label: "~".into(),
                    focused: false,
                    agent_status: AgentStatus::Unknown
                },
                Workspace {
                    workspace_id: "w2".into(),
                    number: 2,
                    label: "ws-one".into(),
                    focused: true,
                    agent_status: AgentStatus::Unknown
                }
            ]
        );
        assert_eq!(view.tabs.len(), 2);
        assert_eq!(
            view.tabs[1],
            Tab {
                tab_id: "w2:t1".into(),
                workspace_id: "w2".into(),
                number: 1,
                label: "1".into(),
                focused: true,
                agent_status: AgentStatus::Unknown
            }
        );
        assert_eq!(view.panes.len(), 3);
        assert_eq!(
            view.panes[2],
            Pane {
                pane_id: "w2:p2".into(),
                tab_id: "w2:t1".into(),
                workspace_id: "w2".into(),
                label: None,
                agent: None,
                agent_status: AgentStatus::Unknown,
                cwd: Some("/tmp".into()),
                title: None,
                focused: true
            }
        );
        assert!(view.agents.is_empty());
        assert_eq!(
            pane_keys(&snapshot)
                .into_iter()
                .map(|(pane, _)| pane)
                .collect::<Vec<_>>(),
            ["w1:p1", "w2:p1", "w2:p2"]
        );
    }

    #[test]
    fn agents_and_unknown_fields_and_values_are_tolerated() {
        let snapshot = parse_snapshot(&result_of(&fixture("snapshot_agents_future.json"))).unwrap();
        let view = project(&snapshot);
        assert_eq!(view.protocol, 23, "newer protocols are accepted");
        // An enum value from the future is Unknown, not a parse failure.
        assert_eq!(view.workspaces[0].agent_status, AgentStatus::Unknown);
        assert_eq!(view.tabs[0].agent_status, AgentStatus::Working);
        assert_eq!(
            view.agents,
            [
                Agent {
                    pane_id: "w1:p1".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    name: Some("reviewer".into()),
                    agent: Some("claude".into()),
                    display_agent: Some("Claude Code".into()),
                    status: AgentStatus::Blocked,
                    cwd: Some("/work/project".into()),
                    title: Some("fix the build".into()),
                    focused: true,
                    state_change_seq: 7,
                    terminal_id: "term_a".into(),
                    // A kind this build does not know is kept as herdr named it.
                    agent_session: Some(AgentSession {
                        kind: "teleport".into(),
                        value: "v".into(),
                    }),
                    interactive_ready: true,
                },
                Agent {
                    pane_id: "w1:p2".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    name: None,
                    agent: None,
                    display_agent: None,
                    status: AgentStatus::Unknown,
                    cwd: None,
                    title: None,
                    focused: false,
                    state_change_seq: 0,
                    terminal_id: "term_b".into(),
                    agent_session: None,
                    interactive_ready: false,
                }
            ]
        );
        assert_eq!(view.panes[0].label.as_deref(), Some("main"));
        assert_eq!(view.panes[0].agent.as_deref(), Some("claude"));
        assert_eq!(view.panes[0].agent_status, AgentStatus::Blocked);
        assert_eq!(view.panes[0].title.as_deref(), Some("fix the build"));
    }

    #[test]
    fn old_protocols_and_foreign_responses_are_rejected_precisely() {
        let old = json!({"type":"session_snapshot","snapshot":{"protocol":3}});
        assert_eq!(
            parse_snapshot(&old).unwrap_err(),
            SnapshotError::Incompatible { protocol: 3 }
        );
        for bad in [
            json!({"type":"ok"}),
            json!({"type":"session_snapshot"}),
            json!({"type":"session_snapshot","snapshot":{}}),
            json!({"type":"session_snapshot","snapshot":{"protocol":22}}),
        ] {
            assert!(
                matches!(parse_snapshot(&bad), Err(SnapshotError::Unreadable(_))),
                "{bad}"
            );
        }
    }
}
