//! What an SSH connection to a host needs: the host-key handler, the transport relay,
//! handshake failure mapping and public key authentication.

use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};

use super::pump::{connection_error, describe, lost};
use crate::keys::ClientKey;
use crate::session::{HostKeyPrompt, SessionFailure};
use crate::trust::{HostKey, HostKeyVerdict, verify};

/// Keepalive every 15 s; the third unanswered one ends the connection.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;
/// In-memory pipe between the transport and russh; see [`relay`].
const PIPE_BYTES: usize = 65536;

pub(crate) fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    })
}

/// An untrusted host key waits for the user: the driver answers `reply`.
pub(crate) struct HostKeyRequest {
    pub(crate) prompt: HostKeyPrompt,
    pub(crate) reply: oneshot::Sender<bool>,
}

/// The transport's read side ended (EOF or error) while russh may be stuck awaiting the user.
pub(crate) struct TransportEnd(pub(crate) SessionFailure);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClientError {
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    #[error("host certificates are unsupported")]
    Certificate,
}

/// The russh handler. `E` is the driver's event type, so one handler serves both M1's session
/// driver and the host driver.
pub(crate) struct Client<E> {
    pub(crate) trusted: Vec<HostKey>,
    pub(crate) events: mpsc::Sender<E>,
    pub(crate) checked: bool,
    /// Told why the SSH session ended, once, if it ends after the handshake. Hosts use it to
    /// notice loss that no channel is awaiting.
    pub(crate) ended: Option<oneshot::Sender<SessionFailure>>,
}

impl<E> Client<E> {
    pub(crate) fn new(trusted: Vec<HostKey>, events: mpsc::Sender<E>) -> Self {
        Self {
            trusted,
            events,
            checked: false,
            ended: None,
        }
    }
}

impl<E: From<HostKeyRequest> + Send + 'static> client::Handler for Client<E> {
    type Error = ClientError;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(ClientError::Certificate);
        };
        let presented = HostKey::from_public_key(key.clone());
        if verify(&presented, &self.trusted) == HostKeyVerdict::Trusted {
            self.checked = true;
            return Ok(true);
        }
        // Rekey must not silently change the peer identity or prompt from Connected.
        if self.checked {
            return Ok(false);
        }
        self.checked = true;
        let (reply, decision) = oneshot::channel();
        self.events
            .send(
                HostKeyRequest {
                    prompt: HostKeyPrompt {
                        presented: presented.clone(),
                        previously_trusted: self.trusted.clone(),
                    },
                    reply,
                }
                .into(),
            )
            .await
            .map_err(|_| russh::Error::Disconnect)?;
        let approved = decision.await.unwrap_or(false);
        if approved {
            self.trusted.push(presented);
        }
        Ok(approved)
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        if let Some(ended) = self.ended.take() {
            let _ = ended.send(match &reason {
                client::DisconnectReason::ReceivedDisconnect(info) => {
                    SessionFailure::ConnectionLost(format!(
                        "the server ended the connection ({:?})",
                        info.reason_code
                    ))
                }
                client::DisconnectReason::Error(ClientError::Ssh(error)) => {
                    SessionFailure::ConnectionLost(describe(error))
                }
                client::DisconnectReason::Error(ClientError::Certificate) => {
                    SessionFailure::UnsupportedHostKey("host certificates are unsupported".into())
                }
            });
        }
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(error) => Err(error),
        }
    }
}

/// Maps a failed handshake to the user-visible failure.
pub(crate) fn handshake_failure(error: ClientError) -> SessionFailure {
    match error {
        ClientError::Certificate => {
            SessionFailure::UnsupportedHostKey("host certificates are unsupported".into())
        }
        ClientError::Ssh(
            error @ russh::Error::NoCommonAlgo {
                kind: russh::AlgorithmKind::Key,
                ..
            },
        ) => SessionFailure::UnsupportedHostKey(format!("SSH host key: {error}")),
        ClientError::Ssh(
            error @ (russh::Error::IO(_) | russh::Error::Disconnect | russh::Error::HUP),
        ) => connection_error(error),
        ClientError::Ssh(error) => SessionFailure::Protocol(format!("SSH handshake: {error}")),
    }
}

/// Public key authentication for `username`; a refusal is `AuthenticationRejected`.
pub(crate) async fn authenticate<H: client::Handler>(
    handle: &mut client::Handle<H>,
    username: String,
    key: &ClientKey,
) -> Result<(), SessionFailure> {
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(connection_error)?
        .flatten();
    let auth = handle
        .authenticate_publickey(
            username,
            PrivateKeyWithHashAlg::new(Arc::new(key.private_key().clone()), hash),
        )
        .await
        .map_err(connection_error)?;
    if auth.success() {
        Ok(())
    } else {
        Err(SessionFailure::AuthenticationRejected)
    }
}

/// russh awaits `check_server_key` inside its reader, so while the user decides it cannot
/// notice the peer hanging up. Relay the transport through a bounded in-memory pipe instead:
/// the relay keeps reading the transport, reports its end as [`TransportEnd`], and closes the
/// pipe so russh sees EOF too. Returns russh's end of the pipe and the relay future, which
/// finishes when either direction ends and must be polled alongside the SSH connection.
pub(crate) fn relay<S, E>(
    stream: S,
    events: mpsc::Sender<E>,
) -> (
    DuplexStream,
    impl std::future::Future<Output = Result<(u64, u64), std::io::Error>>,
)
where
    S: AsyncRead + AsyncWrite + Unpin,
    E: From<TransportEnd>,
{
    let (ssh_stream, pipe) = tokio::io::duplex(PIPE_BYTES);
    let relay = async move {
        let (mut read, mut write) = tokio::io::split(stream);
        let (mut pipe_read, mut pipe_write) = tokio::io::split(pipe);
        tokio::try_join!(
            async {
                let result = tokio::io::copy(&mut read, &mut pipe_write).await;
                let _ = pipe_write.shutdown().await;
                let failure = match &result {
                    Ok(_) => lost(),
                    Err(error) => SessionFailure::ConnectionLost(format!(
                        "SSH transport ({:?}): {error}",
                        error.kind()
                    )),
                };
                let _ = events.send(TransportEnd(failure).into()).await;
                result
            },
            tokio::io::copy(&mut pipe_read, &mut write),
        )
    };
    (ssh_stream, relay)
}
