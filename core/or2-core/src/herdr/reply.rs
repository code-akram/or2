//! Replying to an agent from its notification (contracts.md, "Reply from a notification"): text
//! sent to a herdr pane and submitted, with no terminal open.
//!
//! The reply names the agent it is for ([`AgentIdentity`]: the pane's terminal and the agent's
//! kind, from the view that raised the notification). herdr must still report that agent in the
//! pane (`agent.get`), else it is [`HerdrError::PaneNotFound`] and nothing is sent: a pane id that
//! now holds another agent, or another terminal (herdr restarted and numbered its panes again),
//! never gets a stale notification's reply.
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
//! gets the reply typed into its shell.
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
const NO_TIME: &str = "no time was left to send the reply";

/// Which herdr path carried a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyRoute {
    /// herdr's `agent.prompt` took it.
    Prompted,
    /// herdr refused the prompt (the agent is blocked, or not driven by herdr): typed into the
    /// pane with its Enter, in one request.
    Typed,
}

/// The agent a reply is for, as the view that raised its notification saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    /// herdr's id of the pane's terminal (`terminal_id`): a pane id reused for a new terminal is
    /// a new pane.
    pub terminal_id: String,
    /// The agent's kind (`agent`, e.g. `claude`); `None` when herdr named none. A known kind must
    /// match; an unknown one matches any.
    pub agent: Option<String>,
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
    let mut window = Window {
        cancelled: Box::pin(cancelled),
        deadline,
    };
    // The agent the notification was about must still be in the pane.
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
    match call_raw(host, herdr, directory, reply.session, "or2_prompt", &prompt).await? {
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

/// `agent.get`'s answer names the agent the reply is for: the pane, its terminal, and (when the
/// reply names one) its kind.
fn same_agent(answer: &Value, reply: &Reply<'_>) -> Result<(), HerdrError> {
    let info: AgentInfo = serde_json::from_value(answer.get("agent").cloned().unwrap_or_default())
        .map_err(|error| HerdrError::Failed(format!("herdr sent an unreadable agent: {error}")))?;
    let expected = reply.agent;
    let same = info.pane_id == reply.pane_id
        && info.terminal_id == expected.terminal_id
        && expected
            .agent
            .as_ref()
            .is_none_or(|kind| info.agent.as_ref() == Some(kind));
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
        if Instant::now() + send_bound() > self.deadline {
            return Err(HerdrError::Failed(NO_TIME.into()));
        }
        Ok(())
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

    /// The identity [`FakeAgent::default_for`] reports for `pane`.
    fn claude(pane: &str) -> AgentIdentity {
        AgentIdentity {
            terminal_id: format!("term_{pane}"),
            agent: Some("claude".into()),
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
        // A reply that names no kind needs only the terminal.
        let host = self::host();
        let unnamed = AgentIdentity {
            terminal_id: "term_w1:p1".into(),
            agent: None,
        };
        assert_eq!(
            reply_to(&host, &Directory::new(), &unnamed, "yes").await,
            Ok(ReplyRoute::Prompted)
        );
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
}
