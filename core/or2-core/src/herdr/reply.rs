//! Replying to an agent from its notification (contracts.md, "Reply from a notification"): text
//! sent to a herdr pane and submitted, with no terminal open.
//!
//! herdr's `agent.prompt` submits like the agent's own input (the text, then Enter, honouring the
//! pane's bracketed-paste mode), so it is tried first: [`ReplyRoute::Prompted`]. herdr refuses it
//! in two cases where the pane still has its agent:
//!
//! - `agent_blocked`: the agent waits at an approval or question dialog, the usual case for a
//!   notification;
//! - `agent_not_ready`: herdr knows the agent but does not drive it (herdr 0.9.3 answers this for
//!   every agent it did not start itself with `agent start`, "not an active named agent").
//!
//! Then the text is typed into the pane (`pane.send_text`) and, after the composer's
//! [`SUBMIT_ENTER_DELAY`], Enter is pressed as its own request (`pane.send_keys`):
//! [`ReplyRoute::Typed`], the composer's submit through herdr instead of a terminal. herdr answers
//! `agent_not_found` when no agent is in the pane any more (it exited, or the pane closed): that is
//! [`HerdrError::PaneNotFound`], and nothing is typed, so a reply never runs as a shell command.
//!
//! The text is never logged and is dropped once sent.

use super::HerdrError;
use super::discovery::Directory;
use super::focus::wire_error;
use super::generated::request::{
    AgentPromptParams, PaneSendKeysParams, PaneSendTextParams, RequestBody,
};
use super::scroll::{call, call_raw};
use super::wire::WireError;
use crate::remote::RemoteHost;
use crate::submit::SUBMIT_ENTER_DELAY;

/// The longest reply, in UTF-8 bytes.
pub const MAX_REPLY_BYTES: usize = 4096;

/// herdr's refusal of `agent.prompt` for an agent at an approval or question dialog.
pub const AGENT_BLOCKED: &str = "agent_blocked";
/// herdr's refusal of `agent.prompt` for an agent it does not drive (one it did not start).
pub const AGENT_NOT_READY: &str = "agent_not_ready";
/// herdr's answer to `agent.prompt` when the target has no agent, or no longer exists.
pub const AGENT_NOT_FOUND: &str = "agent_not_found";

/// The key a typed reply is submitted with.
const ENTER: &str = "Enter";

/// Which herdr path carried a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyRoute {
    /// herdr's `agent.prompt` took it.
    Prompted,
    /// herdr refused the prompt (the agent is blocked, or not driven by herdr): typed into the
    /// pane, then Enter after the composer's pause.
    Typed,
}

/// Sends `text` to the agent in `pane_id` of herdr `session` and submits it, with the socket from
/// `directory`. The caller has validated the names and the text (nonempty, at most
/// [`MAX_REPLY_BYTES`]). A pane that is gone, or has no agent any more, is
/// [`HerdrError::PaneNotFound`].
pub async fn reply_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    pane_id: &str,
    text: &str,
) -> Result<ReplyRoute, HerdrError> {
    let prompt = RequestBody::AgentPrompt(AgentPromptParams {
        target: pane_id.to_owned(),
        text: text.to_owned(),
        wait: None,
    });
    match call_raw(host, herdr, directory, session, "or2_prompt", &prompt).await? {
        Ok(_) => return Ok(ReplyRoute::Prompted),
        Err(WireError::Herdr { code, .. }) if code == AGENT_BLOCKED || code == AGENT_NOT_READY => {}
        Err(WireError::Herdr { code, .. }) if code == AGENT_NOT_FOUND => {
            return Err(HerdrError::PaneNotFound);
        }
        Err(error) => return Err(wire_error(error)),
    }
    let typed = RequestBody::PaneSendText(PaneSendTextParams {
        pane_id: pane_id.to_owned(),
        text: text.to_owned(),
    });
    call(host, herdr, directory, session, "or2_reply_text", &typed).await?;
    // Agent TUIs take a burst with Enter in it for a paste: the Enter must come on its own.
    tokio::time::sleep(SUBMIT_ENTER_DELAY).await;
    let enter = RequestBody::PaneSendKeys(PaneSendKeysParams {
        keys: vec![ENTER.to_owned()],
        pane_id: pane_id.to_owned(),
    });
    call(host, herdr, directory, session, "or2_reply_enter", &enter).await?;
    Ok(ReplyRoute::Typed)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{FakeHost, Served, fixture};
    use super::*;

    fn host() -> FakeHost {
        let host = FakeHost::new();
        host.set_listing(&fixture("session_list.json"));
        host
    }

    async fn reply(
        host: &FakeHost,
        directory: &Directory,
        text: &str,
    ) -> Result<ReplyRoute, HerdrError> {
        reply_in(host, "/h", directory, Some("work"), "w1:p1", text).await
    }

    /// What the fake herdr received for replies, in order.
    fn replies(host: &FakeHost) -> Vec<Served> {
        host.served()
            .into_iter()
            .filter(|served| {
                matches!(
                    served,
                    Served::Prompt { .. } | Served::SendText { .. } | Served::SendKeys { .. }
                )
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_working_or_idle_agent_is_prompted_and_nothing_is_typed() {
        let host = host();
        let directory = Directory::new();
        assert_eq!(
            reply(&host, &directory, "run the tests").await,
            Ok(ReplyRoute::Prompted)
        );
        assert_eq!(
            reply(&host, &directory, "line one\nline two").await,
            Ok(ReplyRoute::Prompted)
        );
        assert_eq!(
            replies(&host),
            [
                Served::Prompt {
                    target: "w1:p1".into(),
                    text: "run the tests".into(),
                },
                Served::Prompt {
                    target: "w1:p1".into(),
                    text: "line one\nline two".into(),
                },
            ]
        );
        // The socket came from the listing once (the named session), then from the directory.
        assert_eq!(host.exec_log().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_agent_gets_the_text_typed_then_enter_after_the_pause() {
        for refusal in [AGENT_BLOCKED, AGENT_NOT_READY] {
            let host = host();
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
                Served::Prompt { target, .. },
                Served::SendText {
                    pane_id: text_pane,
                    text,
                    at: typed_at,
                },
                Served::SendKeys {
                    pane_id: keys_pane,
                    keys,
                    at: enter_at,
                },
            ] = served.as_slice()
            else {
                panic!("prompt, text, Enter: {served:?}");
            };
            assert_eq!(
                (target.as_str(), text_pane.as_str(), keys_pane.as_str()),
                ("w1:p1", "w1:p1", "w1:p1")
            );
            assert_eq!(text, "yes, and add a test");
            assert_eq!(keys, &["Enter".to_owned()]);
            assert!(
                *enter_at - *typed_at >= SUBMIT_ENTER_DELAY,
                "Enter waits for the pause: {:?}",
                *enter_at - *typed_at
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_vanished_pane_or_agent_is_pane_not_found_and_nothing_is_typed() {
        let host = host();
        host.fail_prompt(AGENT_NOT_FOUND, "agent target w1:p1 not found");
        let directory = Directory::new();
        assert_eq!(
            reply(&host, &directory, "hello").await,
            Err(HerdrError::PaneNotFound)
        );
        assert_eq!(
            replies(&host).len(),
            1,
            "only the prompt: {:?}",
            host.served()
        );

        // The pane closed between the refusal and the typing.
        let host = self::host();
        host.fail_prompt(AGENT_BLOCKED, "blocked");
        host.fail_send_text("pane_not_found", "pane w1:p1 not found");
        assert_eq!(
            reply(&host, &directory, "hello").await,
            Err(HerdrError::PaneNotFound)
        );
        assert!(
            !replies(&host)
                .iter()
                .any(|served| matches!(served, Served::SendKeys { .. })),
            "no Enter without the text"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn other_refusals_are_failures() {
        let host = host();
        host.fail_prompt("internal", "boom");
        assert!(matches!(
            reply(&host, &Directory::new(), "hello").await,
            Err(HerdrError::Failed(_))
        ));
        assert_eq!(replies(&host).len(), 1);
    }
}
