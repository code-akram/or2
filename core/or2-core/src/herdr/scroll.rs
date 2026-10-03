//! Scrolling a herdr pane's history without the mouse (contracts.md, "Wheel-aware
//! scrolling"): herdr is asked where the pane is (`pane.get`, or `pane.current` for the focused
//! pane), then one `pane.scroll` sets the absolute `offset_from_bottom` a relative swipe gives.
//! New output while a pane is scrolled up moves its offset (herdr keeps the view anchored), so
//! the offset is read fresh each time; [`ScrollOffsets`] keeps the last answer, used only when
//! herdr's reply carries no scroll.
//!
//! herdr answers a `pane.scroll` with the pane's info, whose `scroll.offset_from_bottom` is
//! where the pane really is (herdr clamps an offset past the top of its history): that answer,
//! not the requested offset, becomes the kept one, so a swipe down after an overshoot starts
//! from the top of the history rather than from beyond it.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;
use tokio::time::{Instant, timeout_at};

use super::HerdrError;
use super::discovery::Directory;
use super::focus::{discovery_error, wire_error};
use super::generated::request::{PaneCurrentParams, PaneScrollParams, PaneTarget, RequestBody};
use super::generated::success_response::PaneScrollInfo;
use super::watch::Timing;
use super::wire::{self, WireError};
use crate::host::TargetScroll;
use crate::remote::RemoteHost;

/// The offset from the bottom each herdr pane was last scrolled to through this connection,
/// by session and pane. A pane at the bottom has no entry.
#[derive(Debug, Default)]
pub struct ScrollOffsets {
    offsets: Mutex<HashMap<(Option<String>, String), u64>>,
}

impl ScrollOffsets {
    pub fn new() -> Self {
        Self::default()
    }

    /// The kept offset of `pane_id` in `session`; 0 (the bottom) for a pane never scrolled.
    pub fn get(&self, session: Option<&str>, pane_id: &str) -> u64 {
        self.lock()
            .get(&(session.map(str::to_owned), pane_id.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    fn set(&self, session: Option<&str>, pane_id: &str, offset: u64) {
        let key = (session.map(str::to_owned), pane_id.to_owned());
        let mut offsets = self.lock();
        if offset == 0 {
            offsets.remove(&key);
        } else {
            offsets.insert(key, offset);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(Option<String>, String), u64>> {
        self.offsets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where `scroll` takes a pane that is `offset` rows above its bottom.
pub fn next_offset(offset: u64, scroll: TargetScroll) -> u64 {
    match scroll {
        TargetScroll::Up { lines } => offset.saturating_add(u64::from(lines)),
        TargetScroll::Down { lines } => offset.saturating_sub(u64::from(lines)),
        TargetScroll::Bottom => 0,
    }
}

/// Scrolls `pane_id` (`None`: the session's focused pane) of herdr `session`, with the socket
/// from `directory` and the offset from `offsets`. A pane that no longer exists is
/// [`HerdrError::PaneNotFound`].
pub async fn scroll_pane_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    offsets: &ScrollOffsets,
    session: Option<&str>,
    pane_id: Option<&str>,
    scroll: TargetScroll,
) -> Result<(), HerdrError> {
    // Where the pane is now, asked of herdr: new output while a pane is scrolled up moves its
    // offset (herdr keeps the view anchored), so the kept one may be stale.
    let current = match pane_id {
        Some(pane_id) => {
            let body = RequestBody::PaneGet(PaneTarget {
                pane_id: pane_id.to_owned(),
            });
            call(host, herdr, directory, session, "or2_pane", &body).await
        }
        None => {
            let body = RequestBody::PaneCurrent(PaneCurrentParams::default());
            call(host, herdr, directory, session, "or2_pane", &body).await
        }
    };
    let current = match current {
        Ok(current) => current,
        Err(error) => {
            if let (Some(pane_id), HerdrError::PaneNotFound) = (pane_id, &error) {
                offsets.set(session, pane_id, 0);
            }
            return Err(error);
        }
    };
    let pane_id = match pane_id {
        Some(pane_id) => pane_id.to_owned(),
        None => current
            .pointer("/pane/pane_id")
            .and_then(Value::as_str)
            .ok_or_else(|| HerdrError::Failed("herdr named no focused pane".into()))?
            .to_owned(),
    };
    let offset = scroll_info(&current)
        .map(|info| info.offset_from_bottom)
        .unwrap_or_else(|| offsets.get(session, &pane_id));
    let wanted = next_offset(offset, scroll);
    let body = RequestBody::PaneScroll(PaneScrollParams {
        offset_from_bottom: wanted,
        pane_id: pane_id.clone(),
    });
    let answer = call(host, herdr, directory, session, "or2_scroll", &body).await;
    match answer {
        Ok(answer) => {
            let actual = scroll_info(&answer).map_or(wanted, |info| info.offset_from_bottom);
            offsets.set(session, &pane_id, actual);
            Ok(())
        }
        Err(error) => {
            if error == HerdrError::PaneNotFound {
                offsets.set(session, &pane_id, 0);
            }
            Err(error)
        }
    }
}

/// The `scroll` of the pane in a `pane_info` or `pane_current` answer, if it has one.
fn scroll_info(answer: &Value) -> Option<PaneScrollInfo> {
    serde_json::from_value(answer.pointer("/pane/scroll")?.clone()).ok()
}

/// One request on a short-lived stream, the socket from `directory`; a cached socket that no
/// longer opens makes it read the listing again and try once more.
pub(super) async fn call<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    id: &str,
    body: &RequestBody,
) -> Result<Value, HerdrError> {
    call_raw(host, herdr, directory, session, id, body, None)
        .await?
        .map_err(wire_error)
}

/// [`call`] with herdr's answer unmapped, for a caller that acts on a particular error code:
/// the outer error is the socket's discovery failing, the inner one the request's.
///
/// `start_by` is the latest moment the request may be sent (a reply that must not land after
/// its caller's deadline): a re-locate after a dead cached socket is bounded by it, and the
/// request is never sent past it ([`super::reply::NO_TIME`]).
pub(super) async fn call_raw<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    id: &str,
    body: &RequestBody,
    start_by: Option<Instant>,
) -> Result<Result<Value, WireError>, HerdrError> {
    let no_time = || HerdrError::Failed(super::reply::NO_TIME.into());
    let mut fresh = false;
    loop {
        let cached = if fresh {
            None
        } else {
            directory.cached_socket(session)
        };
        let from_cache = cached.is_some();
        let socket = match cached {
            Some(socket) => socket,
            None => {
                let locate = directory.locate_fresh(host, herdr, session);
                match start_by {
                    Some(by) => timeout_at(by, locate).await.map_err(|_| no_time())?,
                    None => locate.await,
                }
                .map_err(discovery_error)?
            }
        };
        if start_by.is_some_and(|by| Instant::now() > by) {
            return Err(no_time());
        }
        match wire::call(host, &socket, id, body, Timing::default().request).await {
            Ok(answer) => return Ok(Ok(answer)),
            Err(WireError::Unreachable(_)) if from_cache => {
                directory.invalidate();
                fresh = true;
            }
            Err(error) => return Ok(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{FakeHost, Served, fixture};
    use super::*;

    const SOCKET: &str = "/home/user/.config/herdr/herdr.sock";

    fn host() -> FakeHost {
        let host = FakeHost::new();
        host.set_listing(&fixture("session_list.json"));
        host
    }

    fn scrolls(host: &FakeHost) -> Vec<String> {
        host.served()
            .into_iter()
            .filter_map(|served| match served {
                Served::Scroll { pane_id, offset } => Some(format!("{pane_id}@{offset}")),
                Served::Other(method) => Some(method),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn offsets_move_by_lines_and_bottom_is_zero() {
        assert_eq!(next_offset(0, TargetScroll::Up { lines: 3 }), 3);
        assert_eq!(next_offset(5, TargetScroll::Down { lines: 2 }), 3);
        assert_eq!(next_offset(2, TargetScroll::Down { lines: 9 }), 0);
        assert_eq!(
            next_offset(u64::MAX, TargetScroll::Up { lines: 1 }),
            u64::MAX
        );
        assert_eq!(next_offset(40, TargetScroll::Bottom), 0);
    }

    #[tokio::test]
    async fn a_named_pane_is_scrolled_from_where_herdr_says_it_is() {
        let host = host();
        host.set_scroll_max(10);
        let (directory, offsets) = (Directory::new(), ScrollOffsets::new());
        let run = |scroll| {
            scroll_pane_in(
                &host,
                "/h",
                &directory,
                &offsets,
                None,
                Some("w1:p1"),
                scroll,
            )
        };
        run(TargetScroll::Up { lines: 4 }).await.unwrap();
        run(TargetScroll::Up { lines: 4 }).await.unwrap();
        assert_eq!(offsets.get(None, "w1:p1"), 8);
        // Past the top herdr stops at its history; the next swipe starts where it stopped.
        run(TargetScroll::Up { lines: 50 }).await.unwrap();
        assert_eq!(offsets.get(None, "w1:p1"), 10);
        run(TargetScroll::Down { lines: 3 }).await.unwrap();
        assert_eq!(offsets.get(None, "w1:p1"), 7);
        run(TargetScroll::Bottom).await.unwrap();
        assert_eq!(offsets.get(None, "w1:p1"), 0);
        assert_eq!(
            scrolls(&host),
            [
                "pane.get", "w1:p1@4", "pane.get", "w1:p1@8", "pane.get", "w1:p1@58", "pane.get",
                "w1:p1@7", "pane.get", "w1:p1@0"
            ]
        );
        // Offsets are per pane and per session.
        assert_eq!(offsets.get(Some("other"), "w1:p1"), 0);
        assert_eq!(offsets.get(None, "w1:p2"), 0);
        // The socket came from the listing once, then from the directory.
        assert_eq!(host.opened(), vec![SOCKET; 10]);
        assert_eq!(host.exec_log().len(), 1);
    }

    #[tokio::test]
    async fn output_that_moved_a_scrolled_pane_is_taken_into_account() {
        let host = host();
        let (directory, offsets) = (Directory::new(), ScrollOffsets::new());
        let run = |scroll| {
            scroll_pane_in(
                &host,
                "/h",
                &directory,
                &offsets,
                None,
                Some("w1:p1"),
                scroll,
            )
        };
        run(TargetScroll::Up { lines: 7 }).await.unwrap();
        // Three new lines arrive: herdr keeps the view anchored, three rows further up.
        host.set_pane_offset("w1:p1", 10);
        run(TargetScroll::Down { lines: 2 }).await.unwrap();
        assert_eq!(offsets.get(None, "w1:p1"), 8);
        assert_eq!(
            scrolls(&host),
            ["pane.get", "w1:p1@7", "pane.get", "w1:p1@8"]
        );
    }

    #[tokio::test]
    async fn without_a_pane_the_focused_one_is_asked_for_with_its_offset() {
        let host = host();
        host.set_current("w2:p3", 6);
        let (directory, offsets) = (Directory::new(), ScrollOffsets::new());
        scroll_pane_in(
            &host,
            "/h",
            &directory,
            &offsets,
            None,
            None,
            TargetScroll::Up { lines: 2 },
        )
        .await
        .unwrap();
        assert_eq!(scrolls(&host), ["pane.current", "w2:p3@8"]);
        assert_eq!(offsets.get(None, "w2:p3"), 8);
    }

    #[tokio::test]
    async fn a_vanished_pane_is_pane_not_found_and_forgets_its_offset() {
        let host = host();
        let (directory, offsets) = (Directory::new(), ScrollOffsets::new());
        let run = |scroll| {
            scroll_pane_in(
                &host,
                "/h",
                &directory,
                &offsets,
                None,
                Some("w1:p1"),
                scroll,
            )
        };
        run(TargetScroll::Up { lines: 4 }).await.unwrap();
        host.fail_scroll("pane_not_found", "no such pane");
        assert_eq!(
            run(TargetScroll::Up { lines: 1 }).await,
            Err(HerdrError::PaneNotFound)
        );
        assert_eq!(offsets.get(None, "w1:p1"), 0);
        host.fail_scroll("internal", "boom");
        assert!(matches!(
            run(TargetScroll::Bottom).await,
            Err(HerdrError::Failed(_))
        ));
    }
}
