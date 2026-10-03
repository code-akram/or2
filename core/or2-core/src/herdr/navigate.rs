//! Moving between herdr's tabs, panes and workspaces ([`TargetNav`]) through its API, with
//! requests to the session's socket (taken from the connection's [`Directory`], like a pane
//! focus; herdr answers one request per connection, so each is its own short-lived stream):
//!
//! - a window move reads `workspace.list` for the focused workspace, then `tab.list` of that
//!   workspace, and focuses the tab after (or before) the workspace's active one with
//!   `tab.focus`, by herdr's tab number, wrapping around;
//! - a pane move is one `pane.focus_direction` (from `pane_id`, else the focused pane); herdr
//!   answers `no_neighbor` as a success when there is no pane that way;
//! - a session move reads `workspace.list` and focuses the workspace after (or before) the
//!   focused one with `workspace.focus`, by number, wrapping around.
//!
//! With one tab or workspace, or none focused, there is nothing to do: `Ok(())`, nothing sent.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::HerdrError;
use super::discovery::Directory;
use super::focus::{discovery_error, wire_error};
use super::generated::request::{
    EmptyParams, PaneDirection, PaneFocusDirectionParams, RequestBody, TabListParams, TabTarget,
    WorkspaceTarget,
};
use super::generated::success_response::{ResponseResult, TabInfo, WorkspaceInfo};
use super::watch::Timing;
use super::wire;
use crate::host::{NavDirection, TargetNav};
use crate::remote::RemoteHost;

const REQUEST_ID: &str = "or2_nav";

/// Moves herdr `session` (`None` is the default session) as `nav` asks; see the module
/// documentation. `pane_id` is the pane a pane move starts from (`None`: the focused one).
pub async fn navigate_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    pane_id: Option<&str>,
    nav: TargetNav,
) -> Result<(), HerdrError> {
    let mut socket = Socket {
        host,
        herdr,
        directory,
        session,
        known: None,
    };
    match nav {
        TargetNav::Pane { direction } => {
            let body = RequestBody::PaneFocusDirection(PaneFocusDirectionParams {
                direction: pane_direction(direction),
                pane_id: pane_id.map(str::to_owned),
            });
            socket.call(&body).await.map(drop)
        }
        TargetNav::NextWindow | TargetNav::PreviousWindow => {
            let workspaces = socket.workspaces().await?;
            let Some(workspace) = workspaces.iter().find(|workspace| workspace.focused) else {
                return Ok(());
            };
            let body = RequestBody::TabList(TabListParams {
                workspace_id: Some(workspace.workspace_id.clone()),
            });
            let ResponseResult::TabList { tabs } = typed(socket.call(&body).await?)? else {
                return Err(unexpected("tab.list"));
            };
            let tabs: Vec<TabInfo> = tabs
                .into_iter()
                .filter(|tab| tab.workspace_id == workspace.workspace_id)
                .collect();
            let next = nav == TargetNav::NextWindow;
            let Some(tab) = neighbour(
                &tabs,
                next,
                |tab| tab.number,
                |tab| tab.tab_id == workspace.active_tab_id,
            ) else {
                return Ok(());
            };
            let body = RequestBody::TabFocus(TabTarget {
                tab_id: tab.tab_id.clone(),
            });
            socket.call(&body).await.map(drop)
        }
        TargetNav::NextSession | TargetNav::PreviousSession => {
            let workspaces = socket.workspaces().await?;
            let next = nav == TargetNav::NextSession;
            let Some(workspace) = neighbour(
                &workspaces,
                next,
                |workspace| workspace.number,
                |workspace| workspace.focused,
            ) else {
                return Ok(());
            };
            let body = RequestBody::WorkspaceFocus(WorkspaceTarget {
                workspace_id: workspace.workspace_id.clone(),
            });
            socket.call(&body).await.map(drop)
        }
    }
}

fn pane_direction(direction: NavDirection) -> PaneDirection {
    match direction {
        NavDirection::Left => PaneDirection::Left,
        NavDirection::Right => PaneDirection::Right,
        NavDirection::Up => PaneDirection::Up,
        NavDirection::Down => PaneDirection::Down,
    }
}

/// The item after (`next`) or before the current one in `number` order, wrapping around.
/// `None` when there are fewer than two items or none is current: nothing to move to.
fn neighbour<T>(
    items: &[T],
    next: bool,
    number: impl Fn(&T) -> u32,
    current: impl Fn(&T) -> bool,
) -> Option<&T> {
    let mut ordered: Vec<&T> = items.iter().collect();
    ordered.sort_by_key(|item| number(item));
    let count = ordered.len();
    if count < 2 {
        return None;
    }
    let at = ordered.iter().position(|item| current(item))?;
    let to = if next {
        (at + 1) % count
    } else {
        (at + count - 1) % count
    };
    Some(ordered[to])
}

fn typed<T: DeserializeOwned>(result: Value) -> Result<T, HerdrError> {
    serde_json::from_value(result)
        .map_err(|error| HerdrError::Failed(format!("unreadable herdr answer: {error}")))
}

fn unexpected(method: &str) -> HerdrError {
    HerdrError::Failed(format!("herdr answered {method} with another result"))
}

/// One session's socket, for a move's requests. herdr answers one request per connection, so
/// each request is its own short-lived stream ([`wire::call`]).
struct Socket<'a, H> {
    host: &'a H,
    herdr: &'a str,
    directory: &'a Directory,
    session: Option<&'a str>,
    /// The socket that answered this move's first request; `None` before it.
    known: Option<String>,
}

impl<H: RemoteHost> Socket<'_, H> {
    /// One request. The first one takes the socket from the directory; a cached socket that
    /// does not open makes the directory read the listing again, once, as a focus does.
    async fn call(&mut self, body: &RequestBody) -> Result<Value, HerdrError> {
        let timeout = Timing::default().request;
        if let Some(socket) = &self.known {
            return wire::call(self.host, socket, REQUEST_ID, body, timeout)
                .await
                .map_err(wire_error);
        }
        let host = self.host;
        let (socket, result) = self
            .directory
            .with_socket(host, self.herdr, self.session, None, |socket| async move {
                wire::call(host, &socket, REQUEST_ID, body, timeout).await
            })
            .await
            .map_err(discovery_error)?;
        self.known = Some(socket);
        result.map_err(wire_error)
    }

    async fn workspaces(&mut self) -> Result<Vec<WorkspaceInfo>, HerdrError> {
        let body = RequestBody::WorkspaceList(EmptyParams(serde_json::Map::new()));
        match typed(self.call(&body).await?)? {
            ResponseResult::WorkspaceList { workspaces } => Ok(workspaces),
            _ => Err(unexpected("workspace.list")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use serde_json::json;
    use tokio::io::{DuplexStream, duplex};

    use super::*;
    use crate::herdr::testing::fixture;
    use crate::herdr::wire::LineReader;
    use crate::remote::{ExecOutput, RemoteError};

    /// A herdr that answers each method with a scripted `result` (or `error`), one request per
    /// connection as herdr does, and records what it was asked.
    #[derive(Clone, Default)]
    struct Herdr {
        answers: Arc<Mutex<HashMap<String, Value>>>,
        asked: Arc<Mutex<Vec<(String, Value)>>>,
        opened: Arc<Mutex<Vec<String>>>,
        dead: Arc<Mutex<Vec<String>>>,
        execs: Arc<Mutex<usize>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    impl Herdr {
        /// `answer` is the whole response line minus the id: `{"result": ..}` or
        /// `{"error": ..}`.
        fn answer(&self, method: &str, answer: Value) {
            lock(&self.answers).insert(method.into(), answer);
        }

        fn asked(&self) -> Vec<(String, Value)> {
            std::mem::take(&mut *lock(&self.asked))
        }

        fn methods(&self) -> Vec<String> {
            self.asked().into_iter().map(|(method, _)| method).collect()
        }
    }

    impl RemoteHost for Herdr {
        type Stream = DuplexStream;

        async fn exec_rendered(&self, _: &str) -> Result<ExecOutput, RemoteError> {
            *lock(&self.execs) += 1;
            Ok(ExecOutput {
                status: Some(0),
                stdout: fixture("session_list.json").as_bytes().into(),
                stderr: Vec::new().into(),
            })
        }

        async fn open_unix(&self, path: &str) -> Result<DuplexStream, RemoteError> {
            lock(&self.opened).push(path.to_owned());
            if lock(&self.dead).iter().any(|dead| dead == path) {
                return Err(RemoteError::Io("connection refused".into()));
            }
            let (client, server) = duplex(1 << 16);
            let herdr = self.clone();
            tokio::spawn(async move {
                let mut conn = LineReader::new(server);
                if let Ok(Some(line)) = conn.next_line().await {
                    let request: Value = serde_json::from_slice(&line).unwrap();
                    let method = request["method"].as_str().unwrap().to_owned();
                    lock(&herdr.asked).push((method.clone(), request["params"].clone()));
                    let mut reply = lock(&herdr.answers)
                        .get(&method)
                        .cloned()
                        .unwrap_or_else(|| json!({"error": {"code": "invalid_request", "message": "unknown method"}}));
                    reply["id"] = request["id"].clone();
                    let _ = conn.send(format!("{reply}\n").as_bytes()).await;
                }
            });
            Ok(client)
        }
    }

    fn workspace(id: &str, number: u32, focused: bool, active_tab: &str) -> Value {
        json!({
            "workspace_id": id, "number": number, "label": format!("ws {number}"),
            "focused": focused, "active_tab_id": active_tab, "agent_status": "idle",
            "pane_count": 1, "tab_count": 1, "future_field": [1, 2]
        })
    }

    fn tab(id: &str, workspace: &str, number: u32) -> Value {
        json!({
            "tab_id": id, "workspace_id": workspace, "number": number, "label": "t",
            "focused": false, "pane_count": 1, "agent_status": "a_future_status"
        })
    }

    fn ok(result: Value) -> Value {
        json!({ "result": result })
    }

    /// Workspaces in herdr's answer out of number order; `w2` (number 2) is focused, its active
    /// tab `t5` is the middle one of three.
    fn scripted() -> Herdr {
        let herdr = Herdr::default();
        herdr.answer(
            "workspace.list",
            ok(json!({"type": "workspace_list", "workspaces": [
                workspace("w3", 3, false, "t9"),
                workspace("w1", 1, false, "t1"),
                workspace("w2", 2, true, "t5"),
            ]})),
        );
        herdr.answer(
            "tab.list",
            ok(json!({"type": "tab_list", "tabs": [
                tab("t6", "w2", 3),
                tab("t4", "w2", 1),
                tab("t5", "w2", 2),
                tab("t1", "w1", 1),
            ]})),
        );
        for method in ["tab.focus", "workspace.focus"] {
            herdr.answer(method, ok(json!({"type": "ok"})));
        }
        herdr
    }

    async fn nav(herdr: &Herdr, directory: &Directory, nav: TargetNav) -> Result<(), HerdrError> {
        navigate_in(herdr, "/opt/herdr", directory, Some("work"), None, nav).await
    }

    #[tokio::test]
    async fn window_moves_focus_the_neighbouring_tab_of_the_focused_workspace_by_number() {
        let herdr = scripted();
        let directory = Directory::new();
        nav(&herdr, &directory, TargetNav::NextWindow)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked(),
            [
                ("workspace.list".to_owned(), json!({})),
                ("tab.list".to_owned(), json!({"workspace_id": "w2"})),
                ("tab.focus".to_owned(), json!({"tab_id": "t6"})),
            ]
        );
        nav(&herdr, &directory, TargetNav::PreviousWindow)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked()[2],
            ("tab.focus".to_owned(), json!({"tab_id": "t4"}))
        );
        // One listing for the socket, then one stream per request.
        assert_eq!(*lock(&herdr.execs), 1);
        assert_eq!(
            *lock(&herdr.opened),
            ["/home/user/.config/herdr/sessions/work/herdr.sock"; 6]
        );
    }

    #[tokio::test]
    async fn moves_wrap_around_at_either_end() {
        let herdr = scripted();
        herdr.answer(
            "workspace.list",
            ok(json!({"type": "workspace_list", "workspaces": [
                workspace("w2", 2, false, "t5"),
                workspace("w3", 3, true, "t6"),
            ]})),
        );
        herdr.answer(
            "tab.list",
            ok(json!({"type": "tab_list", "tabs": [tab("t6", "w3", 2), tab("t7", "w3", 1)]})),
        );
        let directory = Directory::new();
        nav(&herdr, &directory, TargetNav::NextWindow)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked()[2],
            ("tab.focus".to_owned(), json!({"tab_id": "t7"}))
        );
        nav(&herdr, &directory, TargetNav::NextSession)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked(),
            [
                ("workspace.list".to_owned(), json!({})),
                ("workspace.focus".to_owned(), json!({"workspace_id": "w2"})),
            ]
        );
    }

    #[tokio::test]
    async fn session_moves_focus_the_neighbouring_workspace_by_number() {
        let herdr = scripted();
        let directory = Directory::new();
        nav(&herdr, &directory, TargetNav::NextSession)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked()[1],
            ("workspace.focus".to_owned(), json!({"workspace_id": "w3"}))
        );
        nav(&herdr, &directory, TargetNav::PreviousSession)
            .await
            .unwrap();
        assert_eq!(
            herdr.asked()[1],
            ("workspace.focus".to_owned(), json!({"workspace_id": "w1"}))
        );
    }

    #[tokio::test]
    async fn pane_moves_ask_herdr_for_the_direction_from_the_given_or_focused_pane() {
        let herdr = Herdr::default();
        // herdr's answer when there is no pane that way is a success with a reason.
        herdr.answer(
            "pane.focus_direction",
            ok(json!({"type": "pane_focus_direction", "focus": {
                "source_pane_id": "w1:p1", "reason": "no_neighbor", "changed": false,
                "layout": {"unknown": true}
            }})),
        );
        let directory = Directory::new();
        for (direction, word) in [
            (NavDirection::Left, "left"),
            (NavDirection::Right, "right"),
            (NavDirection::Up, "up"),
            (NavDirection::Down, "down"),
        ] {
            nav(&herdr, &directory, TargetNav::Pane { direction })
                .await
                .unwrap();
            assert_eq!(
                herdr.asked(),
                [(
                    "pane.focus_direction".to_owned(),
                    json!({"direction": word})
                )]
            );
        }
        navigate_in(
            &herdr,
            "/opt/herdr",
            &directory,
            None,
            Some("w1:p2"),
            TargetNav::Pane {
                direction: NavDirection::Right,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            herdr.asked(),
            [(
                "pane.focus_direction".to_owned(),
                json!({"direction": "right", "pane_id": "w1:p2"})
            )]
        );
        assert_eq!(
            lock(&herdr.opened).last().unwrap(),
            "/home/user/.config/herdr/herdr.sock",
            "None is the default session"
        );
        herdr.answer(
            "pane.focus_direction",
            serde_json::from_str(&fixture("error_pane_not_found.json")).unwrap(),
        );
        assert_eq!(
            nav(
                &herdr,
                &directory,
                TargetNav::Pane {
                    direction: NavDirection::Up
                }
            )
            .await,
            Err(HerdrError::PaneNotFound)
        );
    }

    #[tokio::test]
    async fn one_tab_or_workspace_or_none_focused_is_nothing_to_do() {
        let herdr = Herdr::default();
        herdr.answer(
            "workspace.list",
            ok(json!({"type": "workspace_list", "workspaces": [workspace("w1", 1, true, "t1")]})),
        );
        herdr.answer(
            "tab.list",
            ok(json!({"type": "tab_list", "tabs": [tab("t1", "w1", 1)]})),
        );
        let directory = Directory::new();
        for nav_to in [
            TargetNav::NextWindow,
            TargetNav::PreviousWindow,
            TargetNav::NextSession,
            TargetNav::PreviousSession,
        ] {
            assert_eq!(nav(&herdr, &directory, nav_to).await, Ok(()));
            assert!(
                !herdr
                    .methods()
                    .iter()
                    .any(|method| method.ends_with(".focus")),
                "{nav_to:?}"
            );
        }
        // An empty session: no workspace is focused.
        herdr.answer(
            "workspace.list",
            ok(json!({"type": "workspace_list", "workspaces": []})),
        );
        assert_eq!(nav(&herdr, &directory, TargetNav::NextWindow).await, Ok(()));
        assert_eq!(herdr.methods(), ["workspace.list"]);
    }

    #[tokio::test]
    async fn a_dead_cached_socket_is_rediscovered_once_and_errors_are_typed() {
        let herdr = scripted();
        let directory = Directory::new();
        nav(&herdr, &directory, TargetNav::NextSession)
            .await
            .unwrap();
        assert_eq!(*lock(&herdr.execs), 1);
        // The cached socket stops answering: the listing is read again, and the same (still
        // dead) socket is then a failure, not a loop.
        lock(&herdr.dead).push("/home/user/.config/herdr/sessions/work/herdr.sock".into());
        assert!(matches!(
            nav(&herdr, &directory, TargetNav::NextSession).await,
            Err(HerdrError::Failed(_))
        ));
        assert_eq!(*lock(&herdr.execs), 2);
        lock(&herdr.dead).clear();
        // An answer of another type, or an unreadable one, is a failure.
        herdr.answer(
            "workspace.list",
            ok(json!({"type": "tab_list", "tabs": []})),
        );
        assert!(matches!(
            nav(&herdr, &directory, TargetNav::NextSession).await,
            Err(HerdrError::Failed(_))
        ));
        herdr.answer("workspace.list", ok(json!({"type": "workspace_list"})));
        assert!(matches!(
            nav(&herdr, &directory, TargetNav::NextSession).await,
            Err(HerdrError::Failed(_))
        ));
    }
}
