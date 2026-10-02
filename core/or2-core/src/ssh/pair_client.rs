//! The SSH side of Easy pair (contracts.md, "The phone's connection"): one handshake that accepts
//! only the pinned host key, one authentication with the bootstrap key, one session channel running
//! `or2-pair`, and the bytes of the exchange over it. The exchange itself is in [`crate::pair`].
//!
//! It is built on the host connection's machinery rather than a second SSH client: the same
//! [`Client`] handler (every key but the pinned one is refused), [`relay`] and [`authenticate`], and
//! an [`SshHost`] whose connection-owned open and close paths carry the channel. A pairing connection
//! is never registered for `network_changed`: it lives for a few round trips.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::client::{Client, authenticate, handshake_failure, pinned_config, relay};
use super::connection::{HostEvent, OpenedChannel, SshHost};
use super::pump::CHANNEL_CLOSE_GRACE;
use super::runtime;
use crate::keys::ClientKey;
use crate::pair::{PAIR_COMMAND, PairError};
use crate::session::SessionFailure;
use crate::trust::HostKey;

/// How long the relay may flush the final disconnect to the transport before it is cut.
const RELAY_FLUSH: Duration = Duration::from_millis(100);
/// How long the disconnect itself may take to queue.
const DISCONNECT_GRACE: Duration = Duration::from_millis(400);

/// What the exec channel delivered next.
pub(crate) enum Next {
    /// Standard output of `or2-pair` (standard error is ignored: it is where shells complain).
    Data(Vec<u8>),
    /// The server ended the channel (the command exited or was refused to run): the connection is
    /// fine.
    Closed,
    /// The connection ended under the channel.
    Lost,
}

/// An authenticated pairing connection with its exec channel. Closing it (explicitly, or by
/// dropping it: a cancelled pairing) goes through the connection's close path, then disconnects.
pub(crate) struct PairSession {
    inner: Option<Inner>,
}

struct Inner {
    host: Arc<SshHost>,
    /// `None` until the channel is open, and after the server has closed it.
    channel: Option<OpenedChannel>,
    server_closed: bool,
    relay: Relay,
    /// Stdout that arrived with the exec's reply, before anyone asked.
    early: Vec<Vec<u8>>,
}

/// The relay between the transport and the SSH connection, stopped when dropped.
struct Relay(JoinHandle<()>);

impl Drop for Relay {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl PairSession {
    /// Handshake, authentication, channel and `exec "or2-pair"` on `stream`, each step bounded by
    /// `step`. `key` is the derived bootstrap key; it is dropped (zeroized) as soon as
    /// authentication is over.
    pub(crate) async fn open<S>(
        stream: S,
        username: &str,
        host_key: &HostKey,
        key: ClientKey,
        step: Duration,
    ) -> Result<Self, PairError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (events, mut incoming) = mpsc::channel::<HostEvent>(8);
        let (ssh_stream, relay) = relay(stream, events.clone());
        let relay = Relay(tokio::spawn(async move {
            let _ = relay.await;
        }));

        // russh awaits `check_server_key` in its reader: answer the one question the handler can
        // ask (a key that is not the pinned one) while the handshake runs.
        let client = Client::new(vec![host_key.clone()], events);
        #[cfg(test)]
        let reader_gate = Arc::clone(&client.reader_gate);
        let mut mismatch = false;
        let connecting = russh::client::connect_stream(
            pinned_config(host_key.public_key().algorithm()),
            ssh_stream,
            client,
        );
        tokio::pin!(connecting);
        let handshake = async {
            loop {
                tokio::select! {
                    result = &mut connecting => break result,
                    Some(event) = incoming.recv() => {
                        if let HostEvent::HostKey(_, reply) = event {
                            mismatch = true;
                            let _ = reply.send(false);
                        }
                    }
                }
            }
        };
        let handshake = timeout(step, handshake)
            .await
            .map_err(|_| PairError::TimedOut)?;
        let mut handle = match handshake {
            Ok(handle) => handle,
            Err(_) if mismatch => return Err(PairError::HostKeyMismatch),
            Err(error) => return Err(handshake_error(handshake_failure(error))),
        };

        let authenticated = timeout(step, authenticate(&mut handle, username.to_owned(), &key))
            .await
            .map_err(|_| PairError::TimedOut);
        // The bootstrap key is needed for this one authentication only.
        drop(key);
        match authenticated? {
            Ok(()) => {}
            Err(SessionFailure::AuthenticationRejected) => return Err(PairError::BootstrapRefused),
            Err(failure) => return Err(handshake_error(failure)),
        }

        let host = SshHost::new(
            handle,
            step,
            #[cfg(test)]
            reader_gate,
        );
        // From here a failure or a cancellation drops the session, which closes through the
        // connection.
        let mut session = Self {
            inner: Some(Inner {
                host: Arc::clone(&host),
                channel: None,
                server_closed: false,
                relay,
                early: Vec::new(),
            }),
        };
        let mut opening = host.start_open();
        let channel = timeout(step, opening.wait())
            .await
            .map_err(|_| PairError::TimedOut)?
            .map_err(open_error)?;
        let inner = session.inner.as_mut().expect("an open session");
        inner.channel = Some(channel);
        timeout(step, inner.exec())
            .await
            .map_err(|_| PairError::TimedOut)??;
        Ok(session)
    }

    /// The next thing the command printed, or how the channel ended.
    pub(crate) async fn next(&mut self) -> Next {
        self.inner.as_mut().expect("an open session").next().await
    }

    /// Writes `bytes` to the command's standard input.
    pub(crate) async fn send(&mut self, bytes: &[u8]) -> Result<(), PairError> {
        let inner = self.inner.as_mut().expect("an open session");
        match inner.channel.as_ref() {
            Some(channel) => channel
                .data(bytes)
                .await
                .map_err(|_| PairError::ConnectionLost),
            None => Err(PairError::ConnectionLost),
        }
    }

    /// Closes the channel and the connection, waiting (bounded) for both.
    pub(crate) async fn close(mut self) {
        if let Some(inner) = self.inner.take() {
            inner.teardown().await;
        }
    }
}

impl Drop for PairSession {
    fn drop(&mut self) {
        // A cancelled pairing, or an early error: the same teardown, on the network runtime.
        if let Some(inner) = self.inner.take() {
            runtime().spawn(inner.teardown());
        }
    }
}

impl Inner {
    /// `exec "or2-pair"`: waits for the server's answer to the request.
    async fn exec(&mut self) -> Result<(), PairError> {
        let channel = self.channel.as_mut().expect("an open channel");
        channel
            .exec(true, PAIR_COMMAND)
            .await
            .map_err(|_| PairError::ConnectionLost)?;
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Success) => return Ok(()),
                Some(russh::ChannelMsg::Failure) => return Err(PairError::NotOr2Pair),
                Some(russh::ChannelMsg::Data { data }) => self.early.push(data.to_vec()),
                Some(russh::ChannelMsg::Close | russh::ChannelMsg::Eof) => {
                    self.server_closed = true;
                    return Err(PairError::NotOr2Pair);
                }
                None => return Err(PairError::ConnectionLost),
                Some(_) => {}
            }
        }
    }

    async fn next(&mut self) -> Next {
        if !self.early.is_empty() {
            return Next::Data(self.early.remove(0));
        }
        let Some(channel) = self.channel.as_mut() else {
            return Next::Closed;
        };
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => return Next::Data(data.to_vec()),
                Some(russh::ChannelMsg::Close | russh::ChannelMsg::Eof) => {
                    self.server_closed = true;
                    return Next::Closed;
                }
                None => return Next::Lost,
                // Standard error, the exit status, window adjusts.
                Some(_) => {}
            }
        }
    }

    async fn teardown(self) {
        let Inner {
            host,
            channel,
            server_closed,
            relay,
            ..
        } = self;
        if let Some(channel) = channel {
            if server_closed {
                // The server has closed it: nothing to send, and the guard is disarmed.
                drop(channel.into_inner());
            } else {
                let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
            }
        }
        let _ = timeout(DISCONNECT_GRACE, host.disconnect("pairing finished")).await;
        // Let the relay flush the disconnect to the transport; then it is cut with the session.
        let mut relay = relay;
        let _ = timeout(RELAY_FLUSH, &mut relay.0).await;
        host.end_opens();
    }
}

/// A failure of the handshake or of authentication, as the pairing's own error.
fn handshake_error(failure: SessionFailure) -> PairError {
    match failure {
        // The host has no key of the pinned algorithm, or only a certificate.
        SessionFailure::UnsupportedHostKey(_) => PairError::HostKeyMismatch,
        SessionFailure::ConnectionLost(_) | SessionFailure::Unreachable(_) => {
            PairError::ConnectionLost
        }
        SessionFailure::TimedOut => PairError::TimedOut,
        _ => PairError::Protocol,
    }
}

/// A refused session channel (`MaxSessions`, a policy) or a connection that ended.
fn open_error(error: russh::Error) -> PairError {
    match error {
        russh::Error::ChannelOpenFailure(_) => PairError::Protocol,
        _ => PairError::ConnectionLost,
    }
}
