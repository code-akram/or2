//! Reading a herdr pane's history as plain text for the app's history sheet
//! ([`crate::history`]): one `pane.read` of the pane's recent output, `format: text` with
//! `strip_ansi`, on the session's socket from the connection's [`Directory`]. Nothing is
//! scrolled or focused, so herdr's own view of the pane is unchanged.

use serde_json::Value;

use super::HerdrError;
use super::discovery::Directory;
use super::generated::request::{
    PaneCurrentParams, PaneReadParams, ReadFormat, ReadSource, RequestBody,
};
use super::generated::success_response::ResponseResult;
use super::scroll::call;
use crate::history::{HistoryText, history_lines, newest_lines};
use crate::remote::RemoteHost;

/// Reads up to `lines` lines (clamped, [`history_lines`]) of `pane_id`'s recent history in herdr
/// `session` (`pane_id` `None`: the session's focused pane, asked of herdr with `pane.current`
/// first). The text is kept within [`crate::history::MAX_HISTORY_BYTES`], dropping the oldest
/// lines ([`newest_lines`]); `truncated` is herdr's, or set by that cut. A pane that no longer
/// exists is [`HerdrError::PaneNotFound`].
pub async fn read_history_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    pane_id: Option<&str>,
    lines: u32,
) -> Result<HistoryText, HerdrError> {
    let pane_id = match pane_id {
        Some(pane_id) => pane_id.to_owned(),
        None => {
            let body = RequestBody::PaneCurrent(PaneCurrentParams::default());
            call(host, herdr, directory, session, "or2_pane", &body)
                .await?
                .pointer("/pane/pane_id")
                .and_then(Value::as_str)
                .ok_or_else(|| HerdrError::Failed("herdr named no focused pane".into()))?
                .to_owned()
        }
    };
    let body = RequestBody::PaneRead(PaneReadParams {
        format: ReadFormat::Text,
        lines: Some(history_lines(lines)),
        pane_id,
        source: ReadSource::Recent,
        strip_ansi: true,
    });
    let answer = call(host, herdr, directory, session, "or2_history", &body).await?;
    let ResponseResult::PaneRead { read } = serde_json::from_value(answer)
        .map_err(|error| HerdrError::Failed(format!("unreadable herdr answer: {error}")))?
    else {
        return Err(HerdrError::Failed(
            "herdr answered pane.read with another result".into(),
        ));
    };
    let mut history = newest_lines(&read.text, false);
    history.truncated |= read.truncated;
    Ok(history)
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

    fn reads(host: &FakeHost) -> Vec<Served> {
        host.served()
            .into_iter()
            .filter(|served| matches!(served, Served::Read { .. } | Served::Other(_)))
            .collect()
    }

    #[tokio::test]
    async fn a_named_pane_is_read_as_plain_recent_text() {
        let host = host();
        host.set_history("w1:p2", "one\ntwo\nthree\n", false);
        let directory = Directory::new();
        let read = read_history_in(&host, "/h", &directory, None, Some("w1:p2"), 2000)
            .await
            .unwrap();
        assert_eq!(
            read,
            HistoryText {
                text: "one\ntwo\nthree\n".into(),
                truncated: false,
            }
        );
        // One request: recent text without escapes, the count as asked; nothing scrolled.
        assert_eq!(
            reads(&host),
            [Served::Read {
                pane_id: "w1:p2".into(),
                params: serde_json::json!({
                    "pane_id": "w1:p2", "source": "recent", "lines": 2000,
                    "format": "text", "strip_ansi": true,
                }),
            }]
        );
    }

    #[tokio::test]
    async fn without_a_pane_the_focused_one_is_read_and_herdrs_truncation_is_kept() {
        let host = host();
        host.set_current("w1:p3", 0);
        host.set_history("w1:p3", "late\n", true);
        let directory = Directory::new();
        let read = read_history_in(&host, "/h", &directory, Some("work"), None, 99_999)
            .await
            .unwrap();
        assert!(read.truncated);
        assert_eq!(read.text, "late\n");
        let served = reads(&host);
        assert_eq!(served[0], Served::Other("pane.current".into()));
        let Served::Read { pane_id, params } = &served[1] else {
            panic!("{served:?}")
        };
        assert_eq!(pane_id, "w1:p3");
        // The count is clamped to 5000.
        assert_eq!(params["lines"], 5000);
    }

    #[tokio::test]
    async fn an_answer_past_the_cap_keeps_the_newest_lines() {
        let host = host();
        let text: String = (0..200_000).map(|n| format!("line {n}\n")).collect();
        host.set_history("w1:p1", &text, false);
        let read = read_history_in(&host, "/h", &Directory::new(), None, Some("w1:p1"), 5000)
            .await
            .unwrap();
        assert!(read.truncated);
        assert!(read.text.len() <= crate::history::MAX_HISTORY_BYTES);
        assert!(read.text.starts_with("line "));
        assert!(read.text.ends_with("line 199999\n"));
    }

    #[tokio::test]
    async fn a_pane_that_has_gone_is_pane_not_found() {
        let host = host();
        let read = read_history_in(&host, "/h", &Directory::new(), None, Some("w9:p9"), 10).await;
        assert_eq!(read, Err(HerdrError::PaneNotFound));
    }
}
