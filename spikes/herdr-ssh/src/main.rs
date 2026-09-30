// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{Duration, timeout};

struct Client {
    expected_fingerprint: String,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.fingerprint(HashAlg::Sha256).to_string()
            }
            PublicKeyOrCertificate::Certificate(_) => return Ok(false),
        };
        println!("host_fingerprint={fingerprint}");
        Ok(fingerprint == self.expected_fingerprint)
    }
}

async fn exec(handle: &client::Handle<Client>, command: &str) -> Result<String> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        if let russh::ChannelMsg::Data { data } = message {
            output.extend_from_slice(&data);
        }
    }
    Ok(String::from_utf8(output)?)
}

async fn open_socket(
    handle: &client::Handle<Client>,
    socket: &str,
) -> Result<russh::ChannelStream<russh::client::Msg>> {
    Ok(handle
        .channel_open_direct_streamlocal(socket)
        .await?
        .into_stream())
}

async fn request(handle: &client::Handle<Client>, socket: &str, value: Value) -> Result<Value> {
    let mut stream = open_socket(handle, socket).await?;
    stream.write_all(value.to_string().as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await?;
    serde_json::from_str(&line).context("decode socket response")
}

fn count(snapshot: &Value, field: &str) -> usize {
    snapshot
        .pointer(&format!("/result/snapshot/{field}"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn result_field<'a>(response: &'a Value, field: &str) -> Result<&'a Value> {
    response
        .pointer(&format!("/result/{field}"))
        .with_context(|| format!("missing result.{field}: {response}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: herdr-ssh-spike KEY live|SESSION");
    }
    let started = Instant::now();
    let key = load_secret_key(&args[1], None)?;
    let config = Arc::new(client::Config::default());
    let expected_fingerprint = std::env::var("EXPECTED_HOST_FINGERPRINT")
        .context("EXPECTED_HOST_FINGERPRINT must pin the throwaway sshd host key")?;
    let mut handle = client::connect(
        config,
        ("127.0.0.1", 2222),
        Client {
            expected_fingerprint,
        },
    )
    .await?;
    let user = std::env::var("USER")?;
    let auth = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await?;
    if !auth.success() {
        bail!("Ed25519 public-key authentication failed");
    }
    println!("auth=ed25519 success");

    let session_arg = if args[2] == "live" {
        String::new()
    } else {
        format!("--session {} ", args[2])
    };
    let command = format!(
        "PATH=$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin herdr {session_arg}status server"
    );
    let status = exec(&handle, &command).await?;
    let socket = status
        .lines()
        .find_map(|line| line.trim().strip_prefix("socket: "))
        .context("no socket line in herdr status")?
        .to_owned();
    println!("socket={socket}");

    let pane_list = request(
        &handle,
        &socket,
        json!({"id":"panes_1","method":"pane.list","params":{}}),
    )
    .await?;
    let mut subscriptions: Vec<Value> = [
        "workspace.created",
        "workspace.updated",
        "workspace.renamed",
        "workspace.closed",
        "workspace.focused",
        "tab.created",
        "tab.closed",
        "tab.focused",
        "tab.renamed",
        "pane.created",
        "pane.updated",
        "pane.closed",
        "pane.focused",
        "pane.moved",
        "pane.exited",
        "pane.agent_detected",
        "layout.updated",
    ]
    .map(|kind| json!({"type": kind}))
    .into();
    for pane_id in pane_list
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
    {
        subscriptions.push(json!({"type":"pane.agent_status_changed","pane_id":pane_id}));
    }
    let mut events = open_socket(&handle, &socket).await?;
    events
        .write_all(
            format!(
                "{}\n",
                json!({"id":"sub_1","method":"events.subscribe","params":{"subscriptions":subscriptions}})
            )
            .as_bytes(),
        )
        .await?;
    events.flush().await?;
    let mut event_lines = BufReader::new(events).lines();
    let ack = timeout(Duration::from_secs(5), event_lines.next_line())
        .await??
        .context("subscription closed before ack")?;
    println!("subscription_ack={ack}");
    if !ack.contains("subscription_started") {
        bail!("unexpected subscription ack: {ack}");
    }

    let snapshot = request(
        &handle,
        &socket,
        json!({"id":"snapshot_1","method":"session.snapshot","params":{}}),
    )
    .await?;
    if let Ok(path) = std::env::var("HERDR_CAPTURE_SNAPSHOT") {
        std::fs::write(path, snapshot.to_string())?;
    }
    println!(
        "snapshot_summary workspaces={} tabs={} panes={} agents={} installed_ms={}",
        count(&snapshot, "workspaces"),
        count(&snapshot, "tabs"),
        count(&snapshot, "panes"),
        count(&snapshot, "agents"),
        started.elapsed().as_millis()
    );
    let states: Vec<_> = snapshot
        .pointer("/result/snapshot/agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("agent_status").and_then(Value::as_str))
        .collect();
    println!("agent_states={states:?}");

    if args[2] != "live" {
        let created = request(&handle, &socket, json!({"id":"mut_1","method":"workspace.create","params":{"cwd":"/tmp","label":"or2-spike","focus":false}})).await?;
        let workspace_id = result_field(&created, "workspace")?
            .get("workspace_id")
            .and_then(Value::as_str)
            .context("workspace id")?;
        let pane_id = result_field(&created, "root_pane")?
            .get("pane_id")
            .and_then(Value::as_str)
            .context("root pane id")?;
        let split = request(&handle, &socket, json!({"id":"mut_2","method":"pane.split","params":{"pane_id":pane_id,"direction":"right","focus":false}})).await?;
        let split_id = result_field(&split, "pane")?
            .get("pane_id")
            .and_then(Value::as_str)
            .context("split pane id")?;
        request(
            &handle,
            &socket,
            json!({"id":"mut_3","method":"pane.close","params":{"pane_id":split_id}}),
        )
        .await?;
        request(
            &handle,
            &socket,
            json!({"id":"mut_4","method":"workspace.close","params":{"workspace_id":workspace_id}}),
        )
        .await?;
        let mut received = 0;
        let mut saw_pane_closed = false;
        let mut saw_workspace_closed = false;
        while received < 24 && !(saw_pane_closed && saw_workspace_closed) {
            let line = timeout(Duration::from_secs(5), event_lines.next_line())
                .await??
                .context("event stream closed")?;
            println!("live_event={line}");
            saw_pane_closed |= line.contains("pane_closed");
            saw_workspace_closed |= line.contains("workspace_closed");
            received += 1;
        }
        if !(saw_pane_closed && saw_workspace_closed) {
            bail!("did not observe pane_closed and workspace_closed lifecycle events");
        }
    }
    handle
        .disconnect(russh::Disconnect::ByApplication, "done", "en")
        .await?;
    Ok(())
}
