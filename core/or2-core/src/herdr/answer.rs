//! Approving or denying an agent's permission prompt from its notification (contracts.md,
//! "Answer: approve or deny a permission prompt"): one key sent to a herdr pane, with no
//! terminal open.
//!
//! Only Claude Code (agent kind `claude`) for now. herdr's screen detection names the rule that
//! matched (`agent.explain`), and only two of Claude Code's rules are a yes/no permission prompt:
//! [`BASH_PERMISSION_PROMPT`] and [`GENERIC_PERMISSION_PROMPT`]. Codex's blocked rules mix
//! permission prompts with question forms, so no rule id says "approve" there yet.
//!
//! An answer names the agent instance and the prompt it is for: the instance as a reply does
//! ([`AgentIdentity`], herdr's `agent.get` must still report it), and the prompt by herdr's
//! `state_change_seq` when it was notified ([`PermissionPrompt`]). Immediately before the one
//! request that sends, every check runs again, all at once on streams opened together with the
//! send's:
//!
//! - `agent.get`: that agent instance, `blocked`, its `state_change_seq` still the notified one
//!   (any transition since means another prompt, or none);
//! - `agent.explain`: a `claude` agent, one of the two rules matched, and herdr sees the blocker
//!   on screen (`visible_blocker`);
//! - `pane.read` of the visible screen: for an approval, the highlighted option is the first
//!   "Yes" ([`HIGHLIGHTED_YES`]), so Enter takes it;
//! - `pane.process_info`: the agent, not the pane's shell, holds the foreground (as for a typed
//!   reply), so Enter or Escape never reaches a shell.
//!
//! Then one `pane.send_keys`: [`ENTER`] approves, `esc` denies (Claude Code's own "No, and tell
//! Claude what to do differently (esc)"). Any failed check is a typed error and nothing is sent:
//! another instance or a shell in the foreground is [`HerdrError::PaneNotFound`]; anything else
//! about the prompt is [`HerdrError::PromptChanged`].
//!
//! **What remains**, as for a typed reply: a prompt answered or replaced within the one round
//! trip between the checks and the send still gets the key. Closing that needs an atomic
//! agent-bound request from herdr.
//!
//! **Cancellation** is a reply's ([`super::reply`]): the checks give way at once when the caller
//! stops waiting, the send is never abandoned mid-way, and it starts only while its own bound
//! ends before the caller's deadline.

use std::future::Future;

use serde::Deserialize;
use serde_json::Value;
use tokio::time::Instant;

use super::HerdrError;
use super::discovery::Directory;
use super::focus::{discovery_error, wire_error};
use super::generated::request::{
    AgentTarget, PaneProcessInfoParams, PaneReadParams, PaneSendKeysParams, ReadFormat, ReadSource,
    RequestBody,
};
use super::generated::success_response::{AgentInfo, AgentStatus, AgentStatusVariant0};
use super::reply::{
    AgentIdentity, ENTER, OPEN_THE_PANE, Window, agent_get, agent_has_the_foreground, gone,
    same_agent_in,
};
use super::scroll::call_raw;
use super::watch::Timing;
use super::wire;
use crate::remote::RemoteHost;

/// The only agent kind whose permission prompts can be answered.
pub const CLAUDE: &str = "claude";
/// herdr's rule for Claude Code's "Bash command ... Do you want to proceed?".
pub const BASH_PERMISSION_PROMPT: &str = "bash_permission_prompt";
/// herdr's rule for Claude Code's other permission dialogs (edits, fetches, tools).
pub const GENERIC_PERMISSION_PROMPT: &str = "generic_permission_prompt";
/// How Claude Code's dialog shows its first option highlighted: the line an approval's Enter
/// takes must start with this.
pub const HIGHLIGHTED_YES: &str = "❯ 1. Yes";
/// The marker of Claude Code's highlighted option.
const HIGHLIGHT: char = '❯';
/// herdr's name for the Escape key, which denies.
const ESC: &str = "esc";

/// A yes/no permission prompt an agent waits at: what an answer to it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionPrompt {
    /// herdr's `state_change_seq` of the agent when it was found at the prompt: any transition
    /// since means the prompt is not the one notified.
    pub state_change_seq: u64,
}

/// What a permission prompt is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAnswer {
    /// The highlighted first option, "Yes" (Enter).
    Approve,
    /// "No, and tell Claude what to do differently" (Escape).
    Deny,
}

/// One answer: where it goes, which agent and prompt it is for, and what it says.
#[derive(Debug, Clone, Copy)]
pub struct Answer<'a> {
    pub session: Option<&'a str>,
    pub pane_id: &'a str,
    pub agent: &'a AgentIdentity,
    /// The [`PermissionPrompt::state_change_seq`] the notification was posted for.
    pub seq: u64,
    pub answer: PermissionAnswer,
}

/// The part of `agent.explain`'s answer an answer reads (herdr leaves it untyped in its schema;
/// shaped like herdr 0.9.3's). Unknown fields are ignored; one missing is no prompt.
#[derive(Debug, Default, Deserialize)]
struct Explain {
    agent: Option<String>,
    matched_rule: Option<MatchedRule>,
    #[serde(default)]
    visible_blocker: bool,
}

#[derive(Debug, Deserialize)]
struct MatchedRule {
    id: String,
}

/// The permission prompt `agent` waits at in `pane_id`, with the socket from `directory`, or
/// `None` when it waits at none this build can answer (see the module documentation). The caller
/// has validated the names. A pane that is gone or holds another agent instance is
/// [`HerdrError::PaneNotFound`]; an identity that names no instance is refused before anything
/// reaches herdr ([`OPEN_THE_PANE`]). Sends nothing to the pane.
pub async fn permission_prompt_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    pane_id: &str,
    agent: &AgentIdentity,
) -> Result<Option<PermissionPrompt>, HerdrError> {
    if !agent.is_instance() {
        return Err(HerdrError::Failed(OPEN_THE_PANE.into()));
    }
    let call = |id: &'static str, body: RequestBody| async move {
        call_raw(host, herdr, directory, session, id, &body, None)
            .await?
            .map_err(gone)
    };
    let (get, explain) = tokio::try_join!(
        call("or2_answer_agent", agent_get(pane_id)),
        call("or2_answer_explain", agent_explain(pane_id)),
    )?;
    prompt_of(&get, &explain, pane_id, agent)
}

/// Answers the permission prompt `answer.agent` waits at in `answer.pane_id` (one key; see the
/// module documentation), with the socket from `directory`. The caller has validated the names.
/// Every check runs again first, and any that fails sends nothing:
/// [`HerdrError::PaneNotFound`] for a pane that is gone, holds another agent instance, or whose
/// shell has its foreground; [`HerdrError::PromptChanged`] when the agent is not at the prompt
/// notified (another `state_change_seq`, no permission prompt, or, to approve, another option
/// highlighted); [`HerdrError::Failed`] when herdr cannot tell.
///
/// `cancelled` resolves when the caller stops waiting; `deadline` is when it will. Neither ever
/// interrupts the request that sends.
pub async fn answer_permission_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    answer: Answer<'_>,
    deadline: Instant,
    cancelled: impl Future<Output = ()>,
) -> Result<(), HerdrError> {
    if !answer.agent.is_instance() {
        return Err(HerdrError::Failed(OPEN_THE_PANE.into()));
    }
    let mut window = Window {
        cancelled: Box::pin(cancelled),
        deadline,
    };
    let bound = Timing::default().request;
    // The streams for the checks and the send, opened together: the send follows the checks by
    // one round trip. A cached socket that no longer opens is located again, once.
    let (mut agent, mut explain, mut screen, mut process, mut send) = window
        .check(async {
            let opened = directory
                .with_socket(host, herdr, answer.session, None, |socket| async move {
                    let open = || wire::open(host, &socket);
                    tokio::try_join!(open(), open(), open(), open(), open())
                })
                .await
                .map_err(discovery_error)?;
            opened.1.map_err(wire_error)
        })
        .await?;
    let get = agent_get(answer.pane_id);
    let explained = agent_explain(answer.pane_id);
    let read = RequestBody::PaneRead(PaneReadParams {
        format: ReadFormat::Text,
        lines: None,
        pane_id: answer.pane_id.to_owned(),
        source: ReadSource::Visible,
        strip_ansi: true,
    });
    let info = RequestBody::PaneProcessInfo(PaneProcessInfoParams {
        pane_id: Some(answer.pane_id.to_owned()),
    });
    let (get, explained, read, foreground) = window
        .check(async {
            let (get, explained, read, foreground) = tokio::join!(
                wire::call_on(&mut agent, "or2_answer_agent", &get, bound),
                wire::call_on(&mut explain, "or2_answer_explain", &explained, bound),
                wire::call_on(&mut screen, "or2_answer_screen", &read, bound),
                wire::call_on(&mut process, "or2_answer_process", &info, bound),
            );
            Ok((
                get.map_err(gone)?,
                explained.map_err(gone)?,
                read.map_err(gone)?,
                foreground.map_err(gone)?,
            ))
        })
        .await?;
    match prompt_of(&get, &explained, answer.pane_id, answer.agent)? {
        Some(prompt) if prompt.state_change_seq == answer.seq => {}
        _ => return Err(HerdrError::PromptChanged),
    }
    if answer.answer == PermissionAnswer::Approve && !yes_is_highlighted(&read) {
        return Err(HerdrError::PromptChanged);
    }
    agent_has_the_foreground(&foreground)?;
    window.may_send().await?;
    let key = match answer.answer {
        PermissionAnswer::Approve => ENTER,
        PermissionAnswer::Deny => ESC,
    };
    // Nothing between the checks and this request: its stream is already open.
    let keys = RequestBody::PaneSendKeys(PaneSendKeysParams {
        keys: vec![key.to_owned()],
        pane_id: answer.pane_id.to_owned(),
    });
    wire::call_on(&mut send, "or2_answer", &keys, bound)
        .await
        .map_err(gone)?;
    Ok(())
}

fn agent_explain(pane_id: &str) -> RequestBody {
    RequestBody::AgentExplain(AgentTarget {
        target: pane_id.to_owned(),
    })
}

/// The prompt `agent.get`'s and `agent.explain`'s answers show: that agent instance (else
/// [`HerdrError::PaneNotFound`]), a blocked `claude` whose matched rule is a permission prompt
/// herdr sees on screen; `None` otherwise.
fn prompt_of(
    get: &Value,
    explain: &Value,
    pane_id: &str,
    agent: &AgentIdentity,
) -> Result<Option<PermissionPrompt>, HerdrError> {
    let info: AgentInfo = same_agent_in(get, pane_id, agent)?;
    let explain: Explain =
        serde_json::from_value(explain.get("explain").cloned().unwrap_or_default())
            .unwrap_or_default();
    let blocked = matches!(
        info.agent_status,
        AgentStatus::Variant0(AgentStatusVariant0::Blocked)
    );
    let rule = explain.matched_rule.as_ref().is_some_and(|rule| {
        rule.id == BASH_PERMISSION_PROMPT || rule.id == GENERIC_PERMISSION_PROMPT
    });
    let prompt = blocked
        && info.agent.as_deref() == Some(CLAUDE)
        && explain.agent.as_deref() == Some(CLAUDE)
        && rule
        && explain.visible_blocker;
    Ok(prompt.then_some(PermissionPrompt {
        state_change_seq: info.state_change_seq,
    }))
}

/// `pane.read`'s answer shows the first "Yes" highlighted: the last highlighted line on the
/// screen (the dialog is at the bottom; an earlier one may still show above it) starts with
/// [`HIGHLIGHTED_YES`].
fn yes_is_highlighted(read: &Value) -> bool {
    read.pointer("/read/text")
        .and_then(Value::as_str)
        .and_then(|text| {
            text.lines()
                .map(str::trim)
                .rfind(|line| line.starts_with(HIGHLIGHT))
        })
        .is_some_and(|line| line.starts_with(HIGHLIGHTED_YES))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::super::reply::NO_TIME;
    use super::super::testing::{FakeAgent, FakeHost, PERMISSION_DIALOG, Served, fixture};
    use super::super::view::AgentSession;
    use super::*;

    const PANE: &str = "w1:p1";

    fn host() -> FakeHost {
        let host = FakeHost::new();
        host.set_listing(&fixture("session_list.json"));
        host
    }

    /// The identity [`FakeAgent::default_for`] reports for [`PANE`].
    fn claude() -> AgentIdentity {
        AgentIdentity {
            terminal_id: format!("term_{PANE}"),
            agent: Some(CLAUDE.into()),
            name: None,
            session: Some(AgentSession {
                kind: "id".into(),
                value: format!("sess_{PANE}"),
            }),
        }
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    async fn prompt_for(
        host: &FakeHost,
        agent: &AgentIdentity,
    ) -> Result<Option<PermissionPrompt>, HerdrError> {
        permission_prompt_in(host, "/h", &Directory::new(), Some("work"), PANE, agent).await
    }

    async fn prompt(host: &FakeHost) -> Result<Option<PermissionPrompt>, HerdrError> {
        prompt_for(host, &claude()).await
    }

    async fn answer_for(
        host: &FakeHost,
        agent: &AgentIdentity,
        seq: u64,
        answer: PermissionAnswer,
    ) -> Result<(), HerdrError> {
        let answer = Answer {
            session: Some("work"),
            pane_id: PANE,
            agent,
            seq,
            answer,
        };
        answer_permission_in(
            host,
            "/h",
            &Directory::new(),
            answer,
            far(),
            std::future::pending(),
        )
        .await
    }

    async fn answer(host: &FakeHost, answer: PermissionAnswer) -> Result<(), HerdrError> {
        answer_for(host, &claude(), 1, answer).await
    }

    /// Whatever was written to a pane: keys, input or a prompt.
    fn sent(host: &FakeHost) -> Vec<Served> {
        host.served()
            .into_iter()
            .filter(|served| {
                matches!(
                    served,
                    Served::SendKeys { .. } | Served::SendInput { .. } | Served::Prompt { .. }
                ) || matches!(served, Served::Other(method) if method.starts_with("pane.send"))
            })
            .collect()
    }

    fn with(change: impl FnOnce(&mut FakeAgent)) -> FakeHost {
        let host = self::host();
        let mut fake = FakeAgent::default_for(PANE);
        change(&mut fake);
        host.set_agent(PANE, fake);
        host
    }

    #[tokio::test(start_paused = true)]
    async fn a_claude_permission_prompt_is_found_with_its_seq_and_nothing_is_sent() {
        for rule in [BASH_PERMISSION_PROMPT, GENERIC_PERMISSION_PROMPT] {
            let host = with(|fake| {
                fake.rule = Some(rule.into());
                fake.seq = 7;
            });
            assert_eq!(
                prompt(&host).await,
                Ok(Some(PermissionPrompt {
                    state_change_seq: 7
                })),
                "{rule}"
            );
            let served = host.served();
            assert!(served.contains(&Served::AgentGet {
                target: PANE.into()
            }));
            assert!(served.contains(&Served::Explain {
                target: PANE.into()
            }));
            assert!(sent(&host).is_empty(), "{served:?}");
        }
    }

    /// Approve is Enter, Deny is Escape: one `pane.send_keys`, after the four checks, on a
    /// stream opened together with theirs.
    #[tokio::test(start_paused = true)]
    async fn an_answer_sends_exactly_its_one_key_right_after_the_checks() {
        for (choice, key) in [
            (PermissionAnswer::Approve, "Enter"),
            (PermissionAnswer::Deny, "esc"),
        ] {
            let host = self::host();
            assert_eq!(answer(&host, choice).await, Ok(()), "{choice:?}");
            let served = host.served();
            let [checks @ .., last] = served.as_slice() else {
                panic!("checks and a send: {served:?}");
            };
            assert_eq!(
                last,
                &Served::SendKeys {
                    pane_id: PANE.into(),
                    keys: vec![key.into()],
                }
            );
            assert_eq!(checks.len(), 4, "{served:?}");
            for check in [
                Served::AgentGet {
                    target: PANE.into(),
                },
                Served::Explain {
                    target: PANE.into(),
                },
                Served::ProcessInfo {
                    pane_id: PANE.into(),
                },
            ] {
                assert!(checks.contains(&check), "{check:?} in {served:?}");
            }
            assert!(
                checks.iter().any(|check| matches!(check, Served::Read { pane_id, params }
                    if pane_id == PANE && params["source"] == "visible" && params["format"] == "text")),
                "the visible screen is read: {served:?}"
            );
            assert_eq!(sent(&host).len(), 1);
            assert_eq!(host.opened().len(), 5, "all at once: {:?}", host.opened());
        }
    }

    /// Anything but a blocked `claude` at one of the two permission rules herdr sees on screen
    /// is no prompt, and an answer to it sends nothing.
    #[tokio::test(start_paused = true)]
    async fn anything_but_a_visible_claude_permission_prompt_is_none_and_gets_nothing() {
        type Change = fn(&mut FakeAgent);
        let changes: [(&str, Change); 6] = [
            ("another rule", |fake| {
                fake.rule = Some("live_blocked_form".into());
            }),
            ("a question form", |fake| {
                fake.rule = Some("mcp_elicitation_prompt".into());
            }),
            ("no rule", |fake| fake.rule = None),
            ("not visible", |fake| fake.visible_blocker = false),
            ("working", |fake| fake.status = "working".into()),
            ("idle", |fake| fake.status = "idle".into()),
        ];
        for (what, change) in changes {
            let host = with(change);
            assert_eq!(prompt(&host).await, Ok(None), "{what}");
            for choice in [PermissionAnswer::Approve, PermissionAnswer::Deny] {
                let host = with(change);
                assert_eq!(
                    answer(&host, choice).await,
                    Err(HerdrError::PromptChanged),
                    "{what} {choice:?}"
                );
                assert!(sent(&host).is_empty(), "{what}: {:?}", host.served());
            }
        }
    }

    /// Another kind of agent, at a rule of the same name: not answerable in this version.
    #[tokio::test(start_paused = true)]
    async fn another_kind_of_agent_is_none_and_gets_nothing() {
        let codex = AgentIdentity {
            agent: Some("codex".into()),
            ..claude()
        };
        let host = with(|fake| {
            fake.agent = Some((format!("term_{PANE}"), Some("codex".into())));
        });
        assert_eq!(prompt_for(&host, &codex).await, Ok(None));
        assert_eq!(
            answer_for(&host, &codex, 1, PermissionAnswer::Approve).await,
            Err(HerdrError::PromptChanged)
        );
        assert!(sent(&host).is_empty(), "{:?}", host.served());
    }

    /// Any transition since the notification (another prompt, or the same agent blocked again):
    /// not the prompt notified.
    #[tokio::test(start_paused = true)]
    async fn a_changed_seq_is_another_prompt_and_gets_nothing() {
        for choice in [PermissionAnswer::Approve, PermissionAnswer::Deny] {
            let host = with(|fake| fake.seq = 2);
            assert_eq!(
                answer(&host, choice).await,
                Err(HerdrError::PromptChanged),
                "{choice:?}"
            );
            assert!(sent(&host).is_empty(), "{:?}", host.served());
        }
    }

    /// Another agent instance in the pane (another session, terminal or kind), or none: the pane
    /// is not the notification's, as for a reply.
    #[tokio::test(start_paused = true)]
    async fn another_agent_instance_or_none_is_pane_not_found_and_gets_nothing() {
        let changes: [fn(&mut FakeAgent); 5] = [
            |fake| fake.session = Some(("id".into(), "sess_next".into())),
            |fake| fake.session = None,
            |fake| fake.agent = Some(("term_new".into(), Some(CLAUDE.into()))),
            |fake| fake.agent = Some((format!("term_{PANE}"), Some("codex".into()))),
            |fake| fake.agent = None,
        ];
        for change in changes {
            let host = with(change);
            assert_eq!(prompt(&host).await, Err(HerdrError::PaneNotFound));
            for choice in [PermissionAnswer::Approve, PermissionAnswer::Deny] {
                let host = with(change);
                assert_eq!(
                    answer(&host, choice).await,
                    Err(HerdrError::PaneNotFound),
                    "{choice:?}"
                );
                assert!(sent(&host).is_empty(), "{:?}", host.served());
            }
        }
    }

    /// Enter takes the highlighted option: an approval needs the first "Yes" highlighted. A
    /// denial (Escape) does not depend on it.
    #[tokio::test(start_paused = true)]
    async fn an_approval_needs_the_first_yes_highlighted() {
        let moved = PERMISSION_DIALOG
            .replace("❯ 1. Yes", "  1. Yes")
            .replace("  2. Yes, and", "❯ 2. Yes, and");
        // An earlier dialog still shows above, its first option highlighted; the current one,
        // at the bottom, has its third.
        let stale = format!(
            "{PERMISSION_DIALOG}{}",
            PERMISSION_DIALOG
                .replace("❯ 1. Yes", "  1. Yes")
                .replace("  3. No", "❯ 3. No")
        );
        for screen in [moved, stale, "no dialog at all".to_owned(), String::new()] {
            let host = with(|fake| fake.screen = screen.clone());
            assert_eq!(
                answer(&host, PermissionAnswer::Approve).await,
                Err(HerdrError::PromptChanged),
                "{screen}"
            );
            assert!(sent(&host).is_empty(), "{:?}", host.served());
            let host = with(|fake| fake.screen = screen.clone());
            assert_eq!(answer(&host, PermissionAnswer::Deny).await, Ok(()));
        }
        // Indented or not, and with more after "Yes", the first option is the first "Yes".
        let host = with(|fake| fake.screen = "❯ 1. Yes, allow once\n  2. No".into());
        assert_eq!(answer(&host, PermissionAnswer::Approve).await, Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_shell_in_the_foreground_gets_nothing() {
        for choice in [PermissionAnswer::Approve, PermissionAnswer::Deny] {
            let host = self::host();
            host.set_agent(PANE, FakeAgent::default_for(PANE).at_shell());
            assert_eq!(
                answer(&host, choice).await,
                Err(HerdrError::PaneNotFound),
                "{choice:?}"
            );
            assert!(sent(&host).is_empty(), "{:?}", host.served());
        }
        // A foreground herdr cannot tell is refused too.
        let host = with(|fake| fake.foreground.clear());
        assert!(matches!(
            answer(&host, PermissionAnswer::Approve).await,
            Err(HerdrError::Failed(_))
        ));
        assert!(sent(&host).is_empty(), "{:?}", host.served());
    }

    /// An identity that names no instance is refused before anything reaches herdr.
    #[tokio::test(start_paused = true)]
    async fn an_identity_without_an_instance_is_refused_before_anything_is_sent() {
        for identity in [
            AgentIdentity {
                agent: None,
                ..claude()
            },
            AgentIdentity {
                session: None,
                name: None,
                ..claude()
            },
        ] {
            let host = self::host();
            assert_eq!(
                prompt_for(&host, &identity).await,
                Err(HerdrError::Failed(OPEN_THE_PANE.into()))
            );
            assert_eq!(
                answer_for(&host, &identity, 1, PermissionAnswer::Approve).await,
                Err(HerdrError::Failed(OPEN_THE_PANE.into()))
            );
            assert!(host.served().is_empty(), "{:?}", host.served());
        }
    }

    /// A caller that stops waiting while a check is in flight: nothing is sent, ever after.
    #[tokio::test(start_paused = true)]
    async fn a_caller_that_gives_up_during_a_check_gets_nothing_sent() {
        for held in [
            "agent.get",
            "agent.explain",
            "pane.read",
            "pane.process_info",
        ] {
            let host = self::host();
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            host.hold_next(held, Arc::clone(&entered), Arc::clone(&release));
            let (gave_up, cancelled) = tokio::sync::oneshot::channel::<()>();
            let agent = claude();
            let answer = Answer {
                session: Some("work"),
                pane_id: PANE,
                agent: &agent,
                seq: 1,
                answer: PermissionAnswer::Approve,
            };
            let directory = Directory::new();
            let run = answer_permission_in(&host, "/h", &directory, answer, far(), async {
                let _ = cancelled.await;
            });
            let give_up = async {
                entered.notified().await;
                drop(gave_up);
            };
            let (result, ()) = tokio::join!(run, give_up);
            release.notify_one();
            tokio::time::sleep(Duration::from_secs(60)).await;
            assert!(matches!(result, Err(HerdrError::Failed(_))), "{result:?}");
            assert!(sent(&host).is_empty(), "{held}: {:?}", host.served());
        }
    }

    /// Slow checks use up the caller's time: the key goes only while its own bound ends before
    /// the caller's deadline.
    #[tokio::test(start_paused = true)]
    async fn an_answer_out_of_time_sends_nothing() {
        let host = self::host();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        host.hold_next("pane.read", Arc::clone(&entered), Arc::clone(&release));
        let agent = claude();
        let answer = Answer {
            session: Some("work"),
            pane_id: PANE,
            agent: &agent,
            seq: 1,
            answer: PermissionAnswer::Deny,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let slow = async {
            entered.notified().await;
            // 9 s are left once herdr answers: less than a send may take.
            tokio::time::sleep(Duration::from_secs(6)).await;
            release.notify_one();
        };
        let directory = Directory::new();
        let (result, ()) = tokio::join!(
            answer_permission_in(
                &host,
                "/h",
                &directory,
                answer,
                deadline,
                std::future::pending()
            ),
            slow
        );
        assert_eq!(result, Err(HerdrError::Failed(NO_TIME.into())));
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(sent(&host).is_empty(), "{:?}", host.served());
    }

    /// herdr's real answers (captured from an isolated herdr 0.9.3 session showing
    /// [`PERMISSION_DIALOG`]), trimmed: a prompt, read whatever else they carry.
    #[test]
    fn herdrs_answers_are_read_ignoring_what_is_not_needed() {
        let get = serde_json::json!({"type": "agent_info", "agent": {
            "agent": "claude", "agent_status": "blocked", "cwd": "/tmp", "focused": true,
            "foreground_cwd": "/tmp", "pane_id": PANE, "revision": 0, "state_change_seq": 1,
            "tab_id": "w1:t1", "terminal_id": format!("term_{PANE}"), "workspace_id": "w1",
            "agent_session": {"agent": "claude", "kind": "id", "source": "herdr:claude",
                "value": format!("sess_{PANE}")},
        }});
        let explain = serde_json::json!({"type": "agent_explain", "explain": {
            "agent": "claude", "cached_remote_version": "2026.09.11.1",
            "evaluated_rules": [{"id": "osc_title_working", "matched": false, "priority": 1100,
                "region": "osc_title", "state": "working"}],
            "fallback_reason": null, "local_override_shadowing_remote": false,
            "manifest_version": "2026.09.11.1",
            "matched_rule": {"id": "bash_permission_prompt", "priority": 850,
                "region": "whole_recent", "state": "blocked"},
            "screen_detection_skipped": false, "skip_state_update": false, "state": "blocked",
            "visible_blocker": true, "visible_idle": false, "visible_working": false,
            "warning": null, "a_future_field": {"nested": [1, 2]},
        }});
        assert_eq!(
            prompt_of(&get, &explain, PANE, &claude()),
            Ok(Some(PermissionPrompt {
                state_change_seq: 1
            }))
        );
        // An explanation of another shape is no prompt.
        for unreadable in [
            serde_json::json!({"type": "agent_explain", "explain": "no"}),
            serde_json::json!({"type": "agent_explain"}),
        ] {
            assert_eq!(prompt_of(&get, &unreadable, PANE, &claude()), Ok(None));
        }
    }
}
