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
        focused_pane_id: snapshot.focused_pane_id.clone(),
        focused_tab_id: snapshot.focused_tab_id.clone(),
        workspaces: snapshot
            .workspaces
            .iter()
            .map(|workspace| Workspace {
                workspace_id: workspace.workspace_id.clone(),
                number: workspace.number,
                label: display_text(&workspace.label),
            })
            .collect(),
        tabs: snapshot
            .tabs
            .iter()
            .map(|tab| Tab {
                tab_id: tab.tab_id.clone(),
                workspace_id: tab.workspace_id.clone(),
                number: tab.number,
                label: display_text(&tab.label),
            })
            .collect(),
        panes: snapshot
            .panes
            .iter()
            .map(|pane| Pane {
                pane_id: pane.pane_id.clone(),
                agent: pane.agent.clone(),
                cwd: pane.cwd.clone(),
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
                display_agent: agent.display_agent.as_deref().map(display_text),
                status: status(&agent.agent_status),
                cwd: agent.cwd.clone(),
                state_change_seq: agent.state_change_seq,
                terminal_id: agent.terminal_id.clone(),
                agent_session: agent.agent_session.as_ref().map(|session| AgentSession {
                    kind: session.kind.to_string(),
                    value: session.value.clone(),
                }),
                interactive_ready: agent.interactive_ready == Some(true),
                title: agent
                    .terminal_title_stripped
                    .as_deref()
                    .and_then(agent_title),
            })
            .collect(),
    }
}

/// Text from the host made safe to show: control characters and the invisible Unicode formatting
/// characters removed, so a title or a label can never reorder or hide what is drawn around it
/// (a right-to-left override turning `review<U+202E>txt.exe` into another name; Codex review v0.1.4, P2).
/// Removed: the bidi embeddings, overrides and isolates (U+202A–U+202E, U+2066–U+2069), the marks
/// LRM, RLM and ALM (U+200E, U+200F, U+061C), zero-width space and non-joiner (U+200B, U+200C),
/// the word joiner and invisible operators (U+2060–U+2064), the deprecated format controls
/// (U+206A–U+206F), the byte-order mark (U+FEFF), the interlinear annotation marks
/// (U+FFF9–U+FFFB), the soft hyphen (U+00AD) and the Mongolian vowel separator (U+180E). The zero-width
/// joiner (U+200D) stays: emoji sequences need it and it moves nothing. Identifiers (pane, tab and
/// workspace ids, an agent's name, which a reply's identity compares) are never passed through this.
pub fn display_text(raw: &str) -> String {
    raw.chars()
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{00AD}'
                        | '\u{061C}'
                        | '\u{180E}'
                        | '\u{200B}'
                        | '\u{200C}'
                        | '\u{200E}'
                        | '\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2060}'..='\u{2064}'
                        | '\u{2066}'..='\u{206F}'
                        | '\u{FEFF}'
                        | '\u{FFF9}'..='\u{FFFB}'
                )
        })
        .collect()
}

/// The most characters of an agent title kept; the rest is cut.
pub const TITLE_MAX_CHARS: usize = 120;

/// An agent's title from herdr's `terminal_title_stripped`: trimmed, control characters removed,
/// a leading spinner or status glyph and the space after it removed (Claude Code animates one in
/// its title: `⠋ Fixing the build`, `✳ Fixing the build`), capped at [`TITLE_MAX_CHARS`]
/// characters. `None` when nothing is left.
///
/// A glyph is removed only when whitespace (or nothing) follows it, so a title that starts with
/// a word (`π - service`, `*args`) is kept whole. What counts as a glyph is [`is_status_glyph`].
pub fn agent_title(raw: &str) -> Option<String> {
    let clean = display_text(raw);
    let mut title = clean.trim();
    let mut chars = title.chars();
    if let Some(first) = chars.next()
        && is_status_glyph(first)
    {
        // An emoji glyph may carry a variation selector.
        let rest = chars.as_str().trim_start_matches(['\u{FE0E}', '\u{FE0F}']);
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            title = rest.trim_start();
        }
    }
    let capped: String = title.chars().take(TITLE_MAX_CHARS).collect();
    let capped = capped.trim_end();
    (!capped.is_empty()).then(|| capped.to_owned())
}

/// A spinner or status symbol a program puts before its title: Braille spinners (U+2800–U+28FF),
/// `*`, `·`, `•`, `∙`, `⋅`, `⋆`, and the arrows, technical symbols (`⏺`, `⏳`), shapes (`●`,
/// `◐`, `■`), miscellaneous symbols and dingbats (`★`, `✓`, `✳`, `✶`, `✻`, `✽`, `✢`) and emoji
/// blocks. Letters (`π`) and ordinary punctuation are never glyphs.
pub fn is_status_glyph(c: char) -> bool {
    matches!(
        c,
        '*' | '\u{00B7}'
            | '\u{2022}'
            | '\u{2023}'
            | '\u{2043}'
            | '\u{2219}'
            | '\u{22C5}'
            | '\u{22C6}'
            | '\u{2190}'..='\u{21FF}'
            | '\u{2300}'..='\u{23FF}'
            | '\u{25A0}'..='\u{25FF}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{2800}'..='\u{28FF}'
            | '\u{2B00}'..='\u{2BFF}'
            | '\u{1F300}'..='\u{1FAFF}'
    )
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
        assert_eq!(view.focused_pane_id.as_deref(), Some("w2:p2"));
        assert_eq!(view.focused_tab_id.as_deref(), Some("w2:t1"));
        assert_eq!(
            view.workspaces,
            [
                Workspace {
                    workspace_id: "w1".into(),
                    number: 1,
                    label: "~".into(),
                },
                Workspace {
                    workspace_id: "w2".into(),
                    number: 2,
                    label: "ws-one".into(),
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
            }
        );
        assert_eq!(view.panes.len(), 3);
        assert_eq!(
            view.panes[2],
            Pane {
                pane_id: "w2:p2".into(),
                agent: None,
                cwd: Some("/tmp".into()),
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
        assert_eq!(snapshot.protocol, 23, "newer protocols are accepted");
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
                    state_change_seq: 7,
                    terminal_id: "term_a".into(),
                    // A kind this build does not know is kept as herdr named it.
                    agent_session: Some(AgentSession {
                        kind: "teleport".into(),
                        value: "v".into(),
                    }),
                    interactive_ready: true,
                    title: Some("Review v013 brief | or2".into()),
                },
                Agent {
                    pane_id: "w1:p2".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    name: None,
                    agent: None,
                    display_agent: None,
                    // An enum value from the future is Unknown, not a parse failure.
                    status: AgentStatus::Unknown,
                    cwd: None,
                    state_change_seq: 0,
                    terminal_id: "term_b".into(),
                    agent_session: None,
                    interactive_ready: false,
                    title: None,
                }
            ]
        );
        assert_eq!(view.panes[0].agent.as_deref(), Some("claude"));
    }

    #[test]
    fn an_agent_title_loses_its_leading_spinner_or_status_glyph() {
        let title = |raw: &str| agent_title(raw);
        // Claude Code's spinners and status marks, each with the space after it.
        for glyph in [
            "⠋", "⠙", "⣾", "✳", "✶", "✻", "✽", "✢", "·", "*", "●", "•", "◐", "⏺", "★", "✓", "⚡️",
            "🤖",
        ] {
            assert_eq!(
                title(&format!("{glyph} Repository context gathering")).as_deref(),
                Some("Repository context gathering"),
                "{glyph:?}"
            );
        }
        // Surrounding whitespace and the space after the glyph go; inner spacing stays.
        assert_eq!(
            title("  ✳   Review v013 brief | or2  ").as_deref(),
            Some("Review v013 brief | or2")
        );
        // Only one glyph, and only a leading one.
        assert_eq!(title("✳ ✳ twice").as_deref(), Some("✳ twice"));
        assert_eq!(title("Done ✓").as_deref(), Some("Done ✓"));
        // A letter or a glyph glued to a word is part of the title.
        assert_eq!(
            title("π - cliproxyapi.service - user").as_deref(),
            Some("π - cliproxyapi.service - user")
        );
        assert_eq!(title("*args parsing").as_deref(), Some("*args parsing"));
        assert_eq!(title("~/code").as_deref(), Some("~/code"));
        // Control characters never reach a line of text.
        assert_eq!(title("one\ntwo\u{7}").as_deref(), Some("onetwo"));
    }

    #[test]
    fn an_agent_title_is_capped_and_an_empty_one_is_absent() {
        let long = "x".repeat(TITLE_MAX_CHARS + 30);
        assert_eq!(agent_title(&long).unwrap().chars().count(), TITLE_MAX_CHARS);
        // Characters, not bytes: a multi-byte title is cut on a character boundary.
        let wide = "é".repeat(TITLE_MAX_CHARS + 1);
        assert_eq!(agent_title(&wide), Some("é".repeat(TITLE_MAX_CHARS)));
        // A cut that ends on a space does not keep it.
        let spaced = format!("{} tail", "y".repeat(TITLE_MAX_CHARS - 1));
        assert_eq!(agent_title(&spaced), Some("y".repeat(TITLE_MAX_CHARS - 1)));
        for empty in ["", "   ", "⠋", "✳  ", "\n"] {
            assert_eq!(agent_title(empty), None, "{empty:?}");
        }
    }

    #[test]
    fn bidi_and_invisible_formatting_never_survive_into_what_is_shown() {
        // A right-to-left override would draw `review<U+202E>txt.exe` reversed after it (Codex review v0.1.4, P2).
        assert_eq!(
            agent_title("review\u{202E}txt.exe"),
            Some("reviewtxt.exe".into())
        );
        for hidden in [
            '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}',
            '\u{2068}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}', '\u{200B}', '\u{200C}',
            '\u{2060}', '\u{2064}', '\u{206A}', '\u{206F}', '\u{FEFF}', '\u{FFF9}', '\u{FFFB}',
            '\u{00AD}', '\u{180E}', '\u{0007}', '\u{009B}',
        ] {
            let text = format!("a{hidden}b");
            assert_eq!(display_text(&text), "ab", "{hidden:?}");
            assert_eq!(agent_title(&text), Some("ab".into()), "{hidden:?}");
        }
        // Only invisible characters: no title at all.
        assert_eq!(agent_title("\u{202E}\u{2066}\u{200B}"), None);
        // The zero-width joiner stays: emoji sequences need it. Other text is untouched.
        assert_eq!(display_text("👩\u{200D}💻 ship it"), "👩\u{200D}💻 ship it");
        assert_eq!(display_text("π - service · ~/code"), "π - service · ~/code");
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
