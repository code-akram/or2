//! Replying to an agent from its notification (contracts.md, "Reply from a notification"): text
//! sent to a herdr pane and submitted, with no terminal open.
//!
//! The reply names the agent instance it is for ([`AgentIdentity`], from the view that raised the
//! notification: the pane's terminal, the agent's kind, and herdr's `agent_session` of it or, for
//! an agent herdr started, its name). herdr must still report exactly that instance in the pane
//! (`agent.get`, before every request that sends), else it is [`HerdrError::PaneNotFound`] and
//! nothing is sent: a pane id that now holds another agent, another terminal (herdr restarted and
//! numbered its panes again), or another agent of the same kind in the same terminal never gets a
//! stale notification's reply. An identity that names no instance is refused before anything is
//! sent ([`OPEN_THE_PANE`]).
//!
//! herdr's `agent.prompt` submits like the agent's own input (the text, then Enter, honouring the
//! pane's bracketed-paste mode, in one request) and checks on herdr's side that the agent herdr
//! started still has the pane's foreground. It is tried first: [`ReplyRoute::Prompted`]. herdr
//! refuses it, sending nothing, with:
//!
//! - `agent_blocked`: the agent waits at an approval or question dialog, the usual case for a
//!   notification;
//! - `agent_not_ready`: herdr does not drive the agent (herdr 0.9.3 answers this for every agent
//!   it did not start itself, "not an active named agent", which includes every reported agent
//!   such as Claude Code through its hooks), **or the agent left the pane's foreground** ("no
//!   longer the pane foreground process").
//!
//! Then the reply is typed: one `pane.send_input` carrying both the text and the Enter key
//! ([`ReplyRoute::Typed`]), which herdr writes as one submission (the text as a bracketed paste
//! when the program enabled that mode, then Enter), so the text and its Enter can never be split.
//! Immediately before it, the pane must still pass both checks: herdr reports the same agent
//! (`agent.get`), and the pane's foreground process group is not its shell (`pane.process_info`:
//! neither the shell's own group nor one led by a shell). herdr keeps reporting an agent for about
//! half a second after its process exits, while the shell already has the foreground, so the
//! foreground check is what keeps a reply from running as a shell command. The streams for the
//! checks and for the send are opened together beforehand, so the send follows the checks by one
//! round trip to the host.
//!
//! **What remains.** herdr has no request that types into a pane only while a given agent holds
//! it (`agent.send_keys` takes key names, writes them as one unbracketed burst, has no way to say
//! a newline, and, like `agent.prompt`, refuses every agent herdr did not start). An agent that
//! exits within that one round trip, after the checks and before herdr receives the send, still
//! gets the reply typed into its shell. The owner kept this typed reply for agents herdr did not
//! start (Claude Code panes) with that risk stated (contracts.md, owner decision of 2026-10-02);
//! closing it needs an atomic agent-bound request from herdr.
//!
//! **Cancellation.** A caller that stops waiting (its future dropped, or its timeout) stops the
//! reply before any request that sends: the checks give way at once, but a request that sends
//! (`agent.prompt`, `pane.send_input`) is never abandoned mid-way. One starts only while its own
//! bound ends before the caller's deadline, so no reply is sent after its caller gave up.
//!
//! The text is never logged and is dropped once sent.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use super::HerdrError;
use super::discovery::Directory;
use super::focus::{discovery_error, wire_error};
use super::generated::request::{
    AgentPromptParams, AgentTarget, PaneProcessInfoParams, PaneSendInputParams, RequestBody,
};
use super::generated::success_response::{AgentInfo, PaneProcessInfo};
use super::scroll::call_raw;
use super::view::{Agent, AgentSession};
use super::watch::{PANE_NOT_FOUND, Timing};
use super::wire::{self, WireError};
use crate::remote::RemoteHost;

/// The longest reply, in UTF-8 bytes.
pub const MAX_REPLY_BYTES: usize = 4096;

/// herdr's refusal of `agent.prompt` for an agent at an approval or question dialog.
pub const AGENT_BLOCKED: &str = "agent_blocked";
/// herdr's refusal of `agent.prompt` for an agent it does not drive, or one that left the pane's
/// foreground.
pub const AGENT_NOT_READY: &str = "agent_not_ready";
/// herdr's answer to an agent request when the target has no agent, or no longer exists.
pub const AGENT_NOT_FOUND: &str = "agent_not_found";

/// The key a typed reply is submitted with.
const ENTER: &str = "Enter";

/// Interactive shells by process name: a foreground group led by one of these is a shell prompt
/// (or a shell running a script), never an agent.
const SHELLS: &[&str] = &[
    "sh", "ash", "bash", "dash", "zsh", "ksh", "mksh", "oksh", "pdksh", "yash", "fish", "tcsh",
    "csh", "nu", "elvish", "xonsh", "ion", "pwsh", "rc", "es", "osh", "murex", "busybox",
];

/// Why a reply stopped because its caller gave up (never shown: nobody waits for it).
const CANCELLED: &str = "the reply was cancelled";
/// Why a reply stopped because a request that sends could no longer end before the deadline.
pub(super) const NO_TIME: &str = "no time was left to send the reply";
/// Why a reply to an agent herdr reports no instance of ([`AgentIdentity::is_instance`]) is
/// refused: only the pane itself can be answered.
pub const OPEN_THE_PANE: &str = "open the pane to reply";

/// Which herdr path carried a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyRoute {
    /// herdr's `agent.prompt` took it.
    Prompted,
    /// herdr refused the prompt (the agent is blocked, or not driven by herdr): typed into the
    /// pane with its Enter, in one request.
    Typed,
}

/// The agent instance a reply is for, as the view that raised its notification saw it
/// ([`AgentIdentity::of`]).
///
/// A pane id and a terminal outlive the agents that run in them (a Codex exits and another
/// starts in the same terminal), so a reply names the instance too:
///
/// - with herdr's `agent_session` ([`Self::session`]), herdr must still report exactly that
///   session in the pane;
/// - without one, for an agent herdr started (it has a [`Self::name`], which herdr clears when
///   that agent exits, is released or is replaced), herdr must still report that name.
///
/// The terminal and the kind must match as well. Nothing absent ever matches anything: an
/// identity with no kind, or with neither a session nor a name, is refused before anything is
/// sent ([`OPEN_THE_PANE`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    /// herdr's id of the pane's terminal (`terminal_id`): a pane id reused for a new terminal is
    /// a new pane.
    pub terminal_id: String,
    /// The agent's kind (`agent`, e.g. `claude`); required.
    pub agent: Option<String>,
    /// herdr's name for an agent it started (`name`).
    pub name: Option<String>,
    /// herdr's `agent_session` of the agent instance.
    pub session: Option<AgentSession>,
}

impl AgentIdentity {
    /// The identity a reply to `agent` names, or `None` when herdr reports nothing that tells
    /// this instance from the next one in the same terminal: no kind, or neither an
    /// `agent_session` nor the name of an agent herdr started (`interactive_ready`). Such an
    /// agent gets no reply from a notification.
    pub fn of(agent: &Agent) -> Option<Self> {
        agent.agent.as_ref()?;
        let started = agent.name.is_some() && agent.interactive_ready;
        (agent.agent_session.is_some() || started).then(|| Self {
            terminal_id: agent.terminal_id.clone(),
            agent: agent.agent.clone(),
            name: agent.name.clone(),
            session: agent.agent_session.clone(),
        })
    }

    /// Whether this names one agent instance: a kind, and a session or a name.
    pub fn is_instance(&self) -> bool {
        self.agent.is_some() && (self.session.is_some() || self.name.is_some())
    }
}

/// One reply: where it goes, which agent it is for, and what it says.
#[derive(Debug, Clone, Copy)]
pub struct Reply<'a> {
    pub session: Option<&'a str>,
    pub pane_id: &'a str,
    pub agent: &'a AgentIdentity,
    pub text: &'a str,
}

/// Sends `reply.text` to the agent in `reply.pane_id` and submits it, with the socket from
/// `directory`. The caller has validated the names and the text (nonempty, at most
/// [`MAX_REPLY_BYTES`]). A pane that is gone, holds another agent or no agent, or whose shell
/// has its foreground is [`HerdrError::PaneNotFound`], with nothing sent.
///
/// `cancelled` resolves when the caller stops waiting; `deadline` is when it will. Neither ever
/// interrupts a request that sends; see the module documentation.
pub async fn reply_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    reply: Reply<'_>,
    deadline: Instant,
    cancelled: impl Future<Output = ()>,
) -> Result<ReplyRoute, HerdrError> {
    // Only one agent instance can be answered: never "any agent in the pane".
    if !reply.agent.is_instance() {
        return Err(HerdrError::Failed(OPEN_THE_PANE.into()));
    }
    let mut window = Window {
        cancelled: Box::pin(cancelled),
        deadline,
    };
    // The agent instance the notification was about must still be in the pane.
    let get = agent_get(reply.pane_id);
    let answer = window
        .check(async {
            call_raw(
                host,
                herdr,
                directory,
                reply.session,
                "or2_reply_agent",
                &get,
                None,
            )
            .await?
            .map_err(gone)
        })
        .await?;
    same_agent(&answer, &reply)?;

    window.may_send().await?;
    let prompt = RequestBody::AgentPrompt(AgentPromptParams {
        target: reply.pane_id.to_owned(),
        text: reply.text.to_owned(),
        wait: None,
    });
    // A dead cached socket is located again before the prompt goes: that may only spend the
    // time the prompt's own bound leaves before the deadline.
    let start_by = window.start_by();
    match call_raw(
        host,
        herdr,
        directory,
        reply.session,
        "or2_prompt",
        &prompt,
        Some(start_by),
    )
    .await?
    {
        Ok(_) => return Ok(ReplyRoute::Prompted),
        Err(WireError::Herdr { code, .. }) if code == AGENT_BLOCKED || code == AGENT_NOT_READY => {}
        Err(error) => return Err(gone(error)),
    }
    typed(host, herdr, directory, &reply, &mut window).await
}

/// The typed path: the checks and the one `pane.send_input`, on streams opened together first.
async fn typed<H: RemoteHost, C: Future<Output = ()>>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    reply: &Reply<'_>,
    window: &mut Window<C>,
) -> Result<ReplyRoute, HerdrError> {
    let bound = Timing::default().request;
    let (mut agent, mut process, mut send) = window
        .check(async {
            let socket = match directory.cached_socket(reply.session) {
                Some(socket) => socket,
                None => directory
                    .locate_fresh(host, herdr, reply.session)
                    .await
                    .map_err(discovery_error)?,
            };
            let open = || async { wire::open(host, &socket).await.map_err(wire_error) };
            tokio::try_join!(open(), open(), open())
        })
        .await?;
    let get = agent_get(reply.pane_id);
    let info = RequestBody::PaneProcessInfo(PaneProcessInfoParams {
        pane_id: Some(reply.pane_id.to_owned()),
    });
    let (answer, foreground) = window
        .check(async {
            let (answer, foreground) = tokio::join!(
                wire::call_on(&mut agent, "or2_reply_agent", &get, bound),
                wire::call_on(&mut process, "or2_reply_process", &info, bound),
            );
            Ok((answer.map_err(gone)?, foreground.map_err(gone)?))
        })
        .await?;
    same_agent(&answer, reply)?;
    agent_has_the_foreground(&foreground)?;
    window.may_send().await?;
    // Nothing between the checks and this request: its stream is already open.
    let input = RequestBody::PaneSendInput(PaneSendInputParams {
        keys: vec![ENTER.to_owned()],
        pane_id: reply.pane_id.to_owned(),
        text: Some(reply.text.to_owned()),
    });
    wire::call_on(&mut send, "or2_reply", &input, bound)
        .await
        .map_err(gone)?;
    Ok(ReplyRoute::Typed)
}

fn agent_get(pane_id: &str) -> RequestBody {
    RequestBody::AgentGet(AgentTarget {
        target: pane_id.to_owned(),
    })
}

/// herdr's answer as a reply's error: no agent there (any more), or no such pane, is
/// [`HerdrError::PaneNotFound`].
fn gone(error: WireError) -> HerdrError {
    match error {
        WireError::Herdr { code, .. } if code == AGENT_NOT_FOUND || code == PANE_NOT_FOUND => {
            HerdrError::PaneNotFound
        }
        other => wire_error(other),
    }
}

/// `agent.get`'s answer names the agent instance the reply is for: the pane, its terminal, its
/// kind, and its session (the reply names one) or else its name. Nothing absent matches.
fn same_agent(answer: &Value, reply: &Reply<'_>) -> Result<(), HerdrError> {
    let info: AgentInfo = serde_json::from_value(answer.get("agent").cloned().unwrap_or_default())
        .map_err(|error| HerdrError::Failed(format!("herdr sent an unreadable agent: {error}")))?;
    let expected = reply.agent;
    let instance = match (&expected.session, &expected.name) {
        (Some(session), _) => info
            .agent_session
            .as_ref()
            .is_some_and(|now| now.kind.to_string() == session.kind && now.value == session.value),
        (None, Some(name)) => info.name.as_ref() == Some(name),
        (None, None) => false,
    };
    let same = instance
        && info.pane_id == reply.pane_id
        && info.terminal_id == expected.terminal_id
        && expected.agent.is_some()
        && info.agent == expected.agent;
    if same {
        Ok(())
    } else {
        Err(HerdrError::PaneNotFound)
    }
}

/// `pane.process_info`'s answer shows a foreground process group that is not the pane's shell:
/// neither the shell's own group nor one led by a shell. A pane whose foreground herdr cannot
/// tell is refused too.
fn agent_has_the_foreground(answer: &Value) -> Result<(), HerdrError> {
    let unknown = || HerdrError::Failed("herdr cannot tell what runs in the pane".into());
    let info: PaneProcessInfo =
        serde_json::from_value(answer.get("process_info").cloned().unwrap_or_default())
            .map_err(|_| unknown())?;
    let (Some(shell), Some(group)) = (info.shell_pid, info.foreground_process_group_id) else {
        return Err(unknown());
    };
    if group == shell {
        return Err(HerdrError::PaneNotFound);
    }
    let leader = info
        .foreground_processes
        .iter()
        .find(|process| process.pid == group)
        .ok_or_else(unknown)?;
    // A login shell's name may start with `-`.
    let name = leader.name.trim_start_matches('-');
    if SHELLS.contains(&name) {
        return Err(HerdrError::PaneNotFound);
    }
    Ok(())
}

/// What may still stop a reply: its caller giving up, and the caller's deadline.
struct Window<C> {
    cancelled: Pin<Box<C>>,
    deadline: Instant,
}

impl<C: Future<Output = ()>> Window<C> {
    /// `step`, unless the caller gives up first. Only steps that send nothing run under this.
    async fn check<T>(
        &mut self,
        step: impl Future<Output = Result<T, HerdrError>>,
    ) -> Result<T, HerdrError> {
        tokio::select! {
            biased;
            () = self.cancelled.as_mut() => Err(HerdrError::Failed(CANCELLED.into())),
            result = step => result,
        }
    }

    /// Whether a request that sends may start now: the caller still waits, and the request's
    /// own bound ends before the caller's deadline.
    async fn may_send(&mut self) -> Result<(), HerdrError> {
        tokio::select! {
            biased;
            () = self.cancelled.as_mut() => return Err(HerdrError::Failed(CANCELLED.into())),
            () = std::future::ready(()) => {}
        }
        if Instant::now() > self.start_by() {
            return Err(HerdrError::Failed(NO_TIME.into()));
        }
        Ok(())
    }

    /// The latest moment a request that sends may start: its own bound before the deadline.
    fn start_by(&self) -> Instant {
        self.deadline
            .checked_sub(send_bound())
            .unwrap_or_else(Instant::now)
    }
}

/// How long a request that sends may take, at most: its own bound and a margin.
fn send_bound() -> Duration {
    Timing::default().request + Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Notify;

    use super::super::testing::{FakeAgent, FakeHost, Served, fixture};
    use super::*;

    const PANE: &str = "w1:p1";

    fn host() -> FakeHost {
        let host = FakeHost::new();
        host.set_listing(&fixture("session_list.json"));
        host
    }

    /// The identity [`FakeAgent::default_for`] reports for `pane`: its terminal, its kind and
    /// its session.
    fn claude(pane: &str) -> AgentIdentity {
        AgentIdentity {
            terminal_id: format!("term_{pane}"),
            agent: Some("claude".into()),
            name: None,
            session: Some(AgentSession {
                kind: "id".into(),
                value: format!("sess_{pane}"),
            }),
        }
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    async fn reply_to(
        host: &FakeHost,
        directory: &Directory,
        agent: &AgentIdentity,
        text: &str,
    ) -> Result<ReplyRoute, HerdrError> {
        let reply = Reply {
            session: Some("work"),
            pane_id: PANE,
            agent,
            text,
        };
        reply_in(host, "/h", directory, reply, far(), std::future::pending()).await
    }

    async fn reply(
        host: &FakeHost,
        directory: &Directory,
        text: &str,
    ) -> Result<ReplyRoute, HerdrError> {
        reply_to(host, directory, &claude(PANE), text).await
    }

    /// What the fake herdr received for replies, in order.
    fn replies(host: &FakeHost) -> Vec<Served> {
        host.served()
            .into_iter()
            .filter(|served| {
                matches!(
                    served,
                    Served::Prompt { .. }
                        | Served::AgentGet { .. }
                        | Served::ProcessInfo { .. }
                        | Served::SendInput { .. }
                        | Served::Other(_)
                )
            })
            .collect()
    }

    /// Whether anything was written to a pane: a prompt or typed input.
    fn sent(host: &FakeHost) -> bool {
        host.served().iter().any(|served| {
            matches!(served, Served::SendInput { .. } | Served::Prompt { .. })
                || matches!(served, Served::Other(method) if method.starts_with("pane.send"))
        })
    }

    fn typed(host: &FakeHost) -> Vec<Served> {
        host.served()
            .into_iter()
            .filter(|served| {
                matches!(served, Served::SendInput { .. })
                    || matches!(served, Served::Other(method) if method.starts_with("pane.send"))
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_working_or_idle_agent_is_prompted_and_nothing_is_typed() {
        let host = self::host();
        let directory = Directory::new();
        assert_eq!(
            reply(&host, &directory, "run the tests").await,
            Ok(ReplyRoute::Prompted)
        );
        assert_eq!(
            reply(&host, &directory, "line one\nline two").await,
            Ok(ReplyRoute::Prompted)
        );
        let get = || Served::AgentGet {
            target: PANE.into(),
        };
        assert_eq!(
            replies(&host),
            [
                get(),
                Served::Prompt {
                    target: PANE.into(),
                    text: "run the tests".into(),
                },
                get(),
                Served::Prompt {
                    target: PANE.into(),
                    text: "line one\nline two".into(),
                },
            ]
        );
        // The socket came from the listing once (the named session), then from the directory.
        assert_eq!(host.exec_log().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_agent_gets_the_text_and_enter_in_one_request_right_after_the_checks() {
        for refusal in [AGENT_BLOCKED, AGENT_NOT_READY] {
            let host = self::host();
            host.fail_prompt(
                refusal,
                "agent w1:p1 is blocked and requires interactive input",
            );
            let directory = Directory::new();
            assert_eq!(
                reply(&host, &directory, "yes, and add a test").await,
                Ok(ReplyRoute::Typed),
                "{refusal}"
            );
            let served = replies(&host);
            let [
                Served::AgentGet { .. },
                Served::Prompt { .. },
                checks @ ..,
                Served::SendInput {
                    pane_id,
                    text,
                    keys,
                    ..
                },
            ] = served.as_slice()
            else {
                panic!("agent, prompt, checks, one input: {served:?}");
            };
            // The checks, in either order: they run together.
            assert_eq!(checks.len(), 2, "{served:?}");
            assert!(checks.contains(&Served::AgentGet {
                target: PANE.into()
            }));
            assert!(checks.contains(&Served::ProcessInfo {
                pane_id: PANE.into()
            }));
            assert_eq!(pane_id, PANE);
            assert_eq!(text.as_deref(), Some("yes, and add a test"));
            assert_eq!(keys, &["Enter".to_owned()], "the Enter goes with the text");
            // The send's stream was opened with the checks' streams, before either check: the
            // send follows the checks with nothing in between.
            let opened = host.opened();
            assert_eq!(
                opened.len(),
                5,
                "agent, prompt, then three at once: {opened:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_that_left_the_foreground_gets_nothing_typed() {
        // herdr refuses the prompt because the agent left the pane's foreground, but still
        // reports the agent (for about half a second, as herdr 0.9.3 does): the shell has it.
        let host = self::host();
        host.fail_prompt(
            AGENT_NOT_READY,
            "agent w1:p1 is no longer the pane foreground process",
        );
        host.set_agent(PANE, FakeAgent::default_for(PANE).at_shell());
        assert_eq!(
            reply(&host, &Directory::new(), "echo ran-in-shell").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(typed(&host).is_empty(), "{:?}", host.served());
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_that_exits_after_the_prompt_is_refused_gets_nothing_typed() {
        // Blocked when asked, then it exits before the typing: the checks see the shell.
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        host.exit_agents_after("agent.prompt");
        assert_eq!(
            reply(&host, &Directory::new(), "rm -rf build").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(typed(&host).is_empty(), "{:?}", host.served());
    }

    #[tokio::test(start_paused = true)]
    async fn a_shell_leading_the_foreground_gets_nothing_typed() {
        // The agent ran from a nested shell (`bash` inside the pane's `zsh`) and exited: the
        // foreground is that bash's own group, not the pane's shell, but a shell leads it.
        for leader in ["bash", "-zsh", "fish"] {
            let host = self::host();
            host.fail_prompt(AGENT_NOT_READY, "not an active named agent");
            let mut fake = FakeAgent::default_for(PANE);
            fake.foreground = vec![(200, leader.to_owned())];
            host.set_agent(PANE, fake);
            assert_eq!(
                reply(&host, &Directory::new(), "echo hi").await,
                Err(HerdrError::PaneNotFound),
                "{leader}"
            );
            assert!(typed(&host).is_empty());
        }
        // A shell the agent runs (in its own group, which the agent leads) does not count.
        let host = self::host();
        host.fail_prompt(AGENT_NOT_READY, "not an active named agent");
        let mut fake = FakeAgent::default_for(PANE);
        fake.foreground = vec![(200, "claude".into()), (201, "bash".into())];
        host.set_agent(PANE, fake);
        assert_eq!(
            reply(&host, &Directory::new(), "go on").await,
            Ok(ReplyRoute::Typed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_foreground_herdr_cannot_tell_is_refused() {
        let changes: [fn(&mut FakeAgent); 3] = [
            |fake| fake.shell_pid = None,
            |fake| fake.foreground_group = None,
            |fake| fake.foreground.clear(),
        ];
        for change in changes {
            let host = self::host();
            host.fail_prompt(AGENT_BLOCKED, "blocked");
            let mut fake = FakeAgent::default_for(PANE);
            change(&mut fake);
            host.set_agent(PANE, fake);
            assert!(matches!(
                reply(&host, &Directory::new(), "hello").await,
                Err(HerdrError::Failed(_))
            ));
            assert!(typed(&host).is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn another_agent_in_the_pane_gets_nothing() {
        // A stale notification: the pane id now holds another terminal (herdr restarted), or
        // another kind of agent. Nothing is prompted or typed.
        for (terminal, kind) in [("term_new", Some("claude")), ("term_w1:p1", Some("codex"))] {
            let host = self::host();
            host.set_agent(PANE, FakeAgent::running(terminal, kind, "agent"));
            assert_eq!(
                reply(&host, &Directory::new(), "yes").await,
                Err(HerdrError::PaneNotFound),
                "{terminal} {kind:?}"
            );
            assert!(!sent(&host), "{:?}", host.served());
        }
        // The agent changes between the prompt's refusal and the typing: nothing is typed.
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        let directory = Directory::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        host.hold_next("agent.prompt", Arc::clone(&entered), Arc::clone(&release));
        let swap = async {
            entered.notified().await;
            host.set_agent(
                PANE,
                FakeAgent::running("term_w1:p1", Some("codex"), "codex"),
            );
            release.notify_one();
        };
        let (result, ()) = tokio::join!(reply(&host, &directory, "yes"), swap);
        assert_eq!(result, Err(HerdrError::PaneNotFound));
        assert!(typed(&host).is_empty(), "{:?}", host.served());
    }

    /// Another instance of the same kind in the same terminal (a Codex exited and another
    /// started there): its session differs, or it has none yet. Nothing is prompted or typed,
    /// also when the replacement comes between the prompt's refusal and the typing.
    #[tokio::test(start_paused = true)]
    async fn a_replacement_of_the_same_kind_in_the_same_terminal_gets_nothing() {
        let replacement = |session: Option<&str>| FakeAgent {
            session: session.map(|value| ("id".to_owned(), value.to_owned())),
            ..FakeAgent::default_for(PANE)
        };
        for (session, refusal) in [
            (Some("sess_next"), None),
            (None, None),
            (Some("sess_next"), Some(AGENT_BLOCKED)),
            (None, Some(AGENT_NOT_READY)),
        ] {
            let host = self::host();
            if let Some(refusal) = refusal {
                host.fail_prompt(refusal, "refused");
            }
            host.set_agent(PANE, replacement(session));
            assert_eq!(
                reply(&host, &Directory::new(), "yes").await,
                Err(HerdrError::PaneNotFound),
                "{session:?} {refusal:?}"
            );
            assert!(!sent(&host), "{:?}", host.served());
        }
        // A session of another kind (`path`) with the same value is another session.
        let host = self::host();
        host.set_agent(
            PANE,
            FakeAgent {
                session: Some(("path".to_owned(), format!("sess_{PANE}"))),
                ..FakeAgent::default_for(PANE)
            },
        );
        assert_eq!(
            reply(&host, &Directory::new(), "yes").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(!sent(&host), "{:?}", host.served());

        for session in [Some("sess_next"), None] {
            let host = self::host();
            host.fail_prompt(AGENT_BLOCKED, "blocked");
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            host.hold_next("agent.prompt", Arc::clone(&entered), Arc::clone(&release));
            let swap = async {
                entered.notified().await;
                host.set_agent(PANE, replacement(session));
                release.notify_one();
            };
            let directory = Directory::new();
            let (result, ()) = tokio::join!(reply(&host, &directory, "yes"), swap);
            assert_eq!(result, Err(HerdrError::PaneNotFound), "{session:?}");
            assert!(typed(&host).is_empty(), "{:?}", host.served());
        }
    }

    /// An unchanged session is the same agent: prompted, or typed when herdr refuses the prompt.
    #[tokio::test(start_paused = true)]
    async fn an_unchanged_session_gets_the_reply() {
        let host = self::host();
        assert_eq!(
            reply(&host, &Directory::new(), "go on").await,
            Ok(ReplyRoute::Prompted)
        );
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        assert_eq!(
            reply(&host, &Directory::new(), "yes").await,
            Ok(ReplyRoute::Typed)
        );
        assert_eq!(typed(&host).len(), 1);
    }

    /// An agent herdr started, with no session: its name (which herdr clears when it exits or is
    /// replaced) names the instance, with its terminal and kind.
    #[tokio::test(start_paused = true)]
    async fn an_agent_herdr_started_is_named_by_its_name() {
        let named = AgentIdentity {
            name: Some("reviewer".into()),
            session: None,
            ..claude(PANE)
        };
        let started = |name: Option<&str>, terminal: &str| FakeAgent {
            name: name.map(str::to_owned),
            ..FakeAgent::running(terminal, Some("claude"), "claude")
        };
        let host = self::host();
        host.set_agent(PANE, started(Some("reviewer"), "term_w1:p1"));
        assert_eq!(
            reply_to(&host, &Directory::new(), &named, "go on").await,
            Ok(ReplyRoute::Prompted)
        );
        for (name, terminal) in [
            (None, "term_w1:p1"),
            (Some("other"), "term_w1:p1"),
            (Some("reviewer"), "term_new"),
        ] {
            let host = self::host();
            host.set_agent(PANE, started(name, terminal));
            assert_eq!(
                reply_to(&host, &Directory::new(), &named, "go on").await,
                Err(HerdrError::PaneNotFound),
                "{name:?} {terminal}"
            );
            assert!(!sent(&host), "{:?}", host.served());
        }
    }

    /// Nothing absent is a wildcard: an identity with no kind, or with neither a session nor a
    /// name, is refused before anything reaches herdr, whatever the pane holds.
    #[tokio::test(start_paused = true)]
    async fn an_identity_without_kind_or_instance_is_refused_before_anything_is_sent() {
        let no_kind = AgentIdentity {
            agent: None,
            ..claude(PANE)
        };
        let no_instance = AgentIdentity {
            session: None,
            name: None,
            ..claude(PANE)
        };
        for identity in [no_kind, no_instance] {
            for fake in [
                FakeAgent::default_for(PANE),
                FakeAgent::running("term_w1:p1", None, "agent"),
                FakeAgent::running("term_w1:p1", Some("claude"), "claude"),
            ] {
                let host = self::host();
                host.set_agent(PANE, fake);
                assert_eq!(
                    reply_to(&host, &Directory::new(), &identity, "yes").await,
                    Err(HerdrError::Failed(OPEN_THE_PANE.into())),
                    "{identity:?}"
                );
                assert!(replies(&host).is_empty(), "{:?}", host.served());
            }
        }
        // A pane whose agent herdr reports with no kind gets nothing, even for a reply naming its
        // terminal and session.
        let host = self::host();
        host.set_agent(
            PANE,
            FakeAgent {
                agent: Some(("term_w1:p1".into(), None)),
                ..FakeAgent::default_for(PANE)
            },
        );
        assert_eq!(
            reply(&host, &Directory::new(), "yes").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(!sent(&host), "{:?}", host.served());
    }

    /// The identity a view's agent gives a reply: a kind and a session, or a kind and the name of
    /// an agent herdr started; none otherwise.
    #[test]
    fn only_an_agent_herdr_identifies_gets_an_identity() {
        let agent = Agent {
            pane_id: PANE.into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            name: None,
            agent: Some("claude".into()),
            display_agent: None,
            status: super::super::AgentStatus::Blocked,
            cwd: None,
            title: None,
            focused: false,
            state_change_seq: 1,
            terminal_id: "term_w1:p1".into(),
            agent_session: Some(AgentSession {
                kind: "id".into(),
                value: "sess_w1:p1".into(),
            }),
            interactive_ready: false,
        };
        assert_eq!(AgentIdentity::of(&agent), Some(claude(PANE)));
        let started = Agent {
            name: Some("reviewer".into()),
            agent_session: None,
            interactive_ready: true,
            ..agent.clone()
        };
        assert_eq!(
            AgentIdentity::of(&started),
            Some(AgentIdentity {
                name: Some("reviewer".into()),
                session: None,
                ..claude(PANE)
            })
        );
        for none in [
            // Reported, with no session (yet).
            Agent {
                agent_session: None,
                ..agent.clone()
            },
            // Renamed, but herdr did not start it.
            Agent {
                name: Some("renamed".into()),
                agent_session: None,
                ..agent.clone()
            },
            // No kind.
            Agent {
                agent: None,
                ..agent.clone()
            },
            Agent {
                agent: None,
                ..started.clone()
            },
        ] {
            assert_eq!(AgentIdentity::of(&none), None, "{none:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_vanished_pane_or_agent_is_pane_not_found_and_nothing_is_sent() {
        let host = self::host();
        host.set_agent(
            PANE,
            FakeAgent {
                agent: None,
                ..FakeAgent::default_for(PANE).at_shell()
            },
        );
        assert_eq!(
            reply(&host, &Directory::new(), "hello").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(!sent(&host), "{:?}", host.served());

        // The pane closed between the checks and the typing.
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        host.fail_send_input("pane_not_found", "pane w1:p1 not found");
        assert_eq!(
            reply(&host, &Directory::new(), "hello").await,
            Err(HerdrError::PaneNotFound)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn other_refusals_are_failures() {
        let host = self::host();
        host.fail_prompt("internal", "boom");
        assert!(matches!(
            reply(&host, &Directory::new(), "hello").await,
            Err(HerdrError::Failed(_))
        ));
        assert!(typed(&host).is_empty());
    }

    /// A caller that stops waiting while a check is in flight: nothing is sent, ever after.
    #[tokio::test(start_paused = true)]
    async fn a_caller_that_gives_up_during_a_check_stops_the_reply_before_anything_is_sent() {
        for held in ["agent.get", "pane.process_info"] {
            let host = self::host();
            host.fail_prompt(AGENT_BLOCKED, "blocked");
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let directory = Directory::new();
            // The first `agent.get` is the identity check before the prompt.
            host.hold_next(held, Arc::clone(&entered), Arc::clone(&release));
            let (gave_up, cancelled) = tokio::sync::oneshot::channel::<()>();
            let agent = claude(PANE);
            let reply = Reply {
                session: Some("work"),
                pane_id: PANE,
                agent: &agent,
                text: "yes",
            };
            let run = reply_in(&host, "/h", &directory, reply, far(), async {
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
            assert!(typed(&host).is_empty(), "{held}: {:?}", host.served());
            if held == "agent.get" {
                assert!(!sent(&host), "not even the prompt: {:?}", host.served());
            }
        }
    }

    /// Slow checks use up the caller's time: a request that sends starts only while its own
    /// bound ends before the caller's deadline, so nothing lands after the caller timed out.
    #[tokio::test(start_paused = true)]
    async fn a_reply_out_of_time_sends_nothing_after_its_callers_deadline() {
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        host.hold_next(
            "pane.process_info",
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let directory = Directory::new();
        let agent = claude(PANE);
        let reply = Reply {
            session: Some("work"),
            pane_id: PANE,
            agent: &agent,
            text: "yes",
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let slow = async {
            entered.notified().await;
            // The caller has 15 s left (the probe took the rest); herdr answers the check 6 s
            // on, within its own bound: 9 s are left, less than a send may take.
            tokio::time::sleep(Duration::from_secs(6)).await;
            release.notify_one();
        };
        let (result, ()) = tokio::join!(
            reply_in(
                &host,
                "/h",
                &directory,
                reply,
                deadline,
                std::future::pending()
            ),
            slow
        );
        assert_eq!(result, Err(HerdrError::Failed(NO_TIME.into())));
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(typed(&host).is_empty(), "{:?}", host.served());
    }

    /// The cached socket dies between the identity check and the prompt (herdr restarted on
    /// another socket) and the host is slow to list the sessions again: the re-locate spends
    /// the prompt's time, so the prompt is not sent once it could land after the deadline (it
    /// used to be sent whenever the listing came back).
    #[tokio::test(start_paused = true)]
    async fn a_dead_cached_socket_is_located_again_only_within_the_replys_time() {
        const WORK: &str = "/home/user/.config/herdr/sessions/work/herdr.sock";
        let host = self::host();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        host.hold_next("agent.get", Arc::clone(&entered), Arc::clone(&release));
        let directory = Directory::new();
        let agent = claude(PANE);
        let reply = Reply {
            session: Some("work"),
            pane_id: PANE,
            agent: &agent,
            text: "yes",
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let restart = async {
            entered.notified().await;
            host.kill_socket(WORK);
            host.set_listing(
                &fixture("session_list.json")
                    .replace("sessions/work/herdr.sock", "sessions/work/herdr-2.sock"),
            );
            host.set_exec_delay(Duration::from_secs(20));
            release.notify_one();
        };
        let (result, ()) = tokio::join!(
            reply_in(
                &host,
                "/h",
                &directory,
                reply,
                deadline,
                std::future::pending()
            ),
            restart
        );
        assert_eq!(result, Err(HerdrError::Failed(NO_TIME.into())));
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!sent(&host), "{:?}", host.served());
    }
}
