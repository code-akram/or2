//! The terminal pump shared by M1's one-session connection (`ssh::connect`) and by terminal
//! channels on a host connection (`ssh::connection`).
//!
//! A terminal session is one thread that owns the libghostty `TerminalEngine` (it is `!Send`)
//! and its [`SessionDriver`], plus one network task that owns the SSH channel. The task sends
//! [`Event`]s (channel output, channel end) to the thread; the thread sends [`Write`]s (user
//! input, generated replies, resizes) back. [`TerminalPump`] is the thread's half: it feeds
//! output to the engine, queues the replies the engine generates under a byte budget, turns
//! driver commands into writes and publishes frames. [`pump_channel`] is the task's half.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

use crate::session::{
    CloseReason, Command, HostKeyPrompt, SessionDriver, SessionFailure, SessionState,
};
use crate::term::TerminalSize;
use crate::terminal::TerminalEngine;

/// Terminal replies (cursor reports, colour queries) queued or in flight may not exceed this.
pub(crate) const REPLY_BYTE_BUDGET: usize = 64 * 1024;

/// How long a closing session waits for its channel to say goodbye.
pub(crate) const CHANNEL_CLOSE_GRACE: Duration = Duration::from_millis(250);

/// From the network task to the session thread.
pub(crate) enum Event {
    /// M1 only: an untrusted host key awaits the user.
    HostKey(HostKeyPrompt, oneshot::Sender<bool>),
    /// M1 only.
    Authenticating,
    /// The channel is open and running its program.
    Connected,
    Output(Vec<u8>),
    /// M1 only: the transport ended (read EOF or error).
    TransportEnded(SessionFailure),
    Closed(CloseReason),
}

/// From the session thread to the network task.
pub(crate) enum Write {
    Bytes(Vec<u8>),
    Reply(ReplyBatch),
    Resize(TerminalSize),
}

pub(crate) struct ReplyBatch {
    pub(crate) bytes: Vec<u8>,
    // Credit stays reserved through channel.data(), including a stalled in-flight write.
    permit: OwnedSemaphorePermit,
}

pub(crate) struct GeneratedReplies {
    pub(crate) budget: Arc<Semaphore>,
    batch: Option<ReplyBatch>,
    overflow: bool,
}

impl GeneratedReplies {
    pub(crate) fn new() -> Self {
        Self {
            budget: Arc::new(Semaphore::new(REPLY_BYTE_BUDGET)),
            batch: None,
            overflow: false,
        }
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) {
        if self.overflow || bytes.is_empty() {
            return;
        }
        let permit = u32::try_from(bytes.len())
            .ok()
            .and_then(|len| self.budget.clone().try_acquire_many_owned(len).ok());
        let Some(permit) = permit else {
            self.overflow = true;
            return;
        };
        match &mut self.batch {
            Some(batch) => {
                batch.permit.merge(permit);
                batch.bytes.extend_from_slice(bytes);
            }
            None => {
                self.batch = Some(ReplyBatch {
                    bytes: bytes.to_vec(),
                    permit,
                })
            }
        }
    }

    pub(crate) fn flush(
        &mut self,
        writes: &mpsc::UnboundedSender<Write>,
    ) -> Result<(), SessionFailure> {
        if self.overflow {
            return Err(SessionFailure::Protocol(format!(
                "generated terminal replies exceed {REPLY_BYTE_BUDGET}-byte budget"
            )));
        }
        if let Some(batch) = self.batch.take() {
            let _ = writes.send(Write::Reply(batch));
        }
        Ok(())
    }
}

/// The session thread's half: the engine, its replies and the write queue. Not `Send`.
pub(crate) struct TerminalPump {
    terminal: TerminalEngine,
    replies: Rc<RefCell<GeneratedReplies>>,
    writes: mpsc::UnboundedSender<Write>,
    size: watch::Sender<TerminalSize>,
    connected: bool,
}

impl TerminalPump {
    /// The pump plus what the network task needs: the write queue's receiving end and the
    /// latest requested size (the channel opens at it; a later resize still wins).
    pub(crate) fn new(
        size: TerminalSize,
    ) -> Result<
        (
            Self,
            mpsc::UnboundedReceiver<Write>,
            watch::Receiver<TerminalSize>,
        ),
        SessionFailure,
    > {
        let (writes, outgoing) = mpsc::unbounded_channel();
        let (size_sender, latest_size) = watch::channel(size);
        let replies = Rc::new(RefCell::new(GeneratedReplies::new()));
        let collected = replies.clone();
        let terminal = TerminalEngine::new(size, move |bytes| {
            collected.borrow_mut().append(bytes);
        })
        .map_err(internal)?;
        Ok((
            Self {
                terminal,
                replies,
                writes,
                size: size_sender,
                connected: false,
            },
            outgoing,
            latest_size,
        ))
    }

    /// The channel is running: reconcile even a resize between the network's size sample and
    /// this event (queued before the callback can enqueue input), move the driver to
    /// `Connected` and publish the first frame.
    pub(crate) fn connect(&mut self, driver: &mut SessionDriver) -> Result<(), SessionFailure> {
        self.connected = true;
        let _ = self.writes.send(Write::Resize(*self.size.borrow()));
        driver
            .transition(SessionState::Connected)
            .map_err(internal)?;
        self.publish(driver)
    }

    /// Feeds `first` and every output event already queued behind it to the engine, then
    /// publishes one frame. The batch is a snapshot of what is available, so a continuous
    /// printer cannot starve commands. A non-output event met on the way is left in `pending`
    /// for the caller's loop.
    pub(crate) fn output(
        &mut self,
        driver: &mut SessionDriver,
        first: Vec<u8>,
        incoming: &mut mpsc::Receiver<Event>,
        pending: &mut Option<Event>,
    ) -> Result<(), SessionFailure> {
        self.write_output(&first)?;
        for _ in 0..incoming.len() {
            let Ok(event) = incoming.try_recv() else {
                break;
            };
            if let Event::Output(bytes) = event {
                self.write_output(&bytes)?;
            } else {
                *pending = Some(event);
                break;
            }
        }
        if self.connected {
            self.publish(driver)?;
        }
        Ok(())
    }

    fn write_output(&mut self, bytes: &[u8]) -> Result<(), SessionFailure> {
        self.terminal.write(bytes);
        self.replies.borrow_mut().flush(&self.writes)
    }

    /// Resize, input, scroll and full-frame commands. Anything else is the caller's.
    pub(crate) fn command(
        &mut self,
        driver: &mut SessionDriver,
        command: Command,
    ) -> Result<(), SessionFailure> {
        match command {
            Command::Resize(new_size) => {
                self.size.send_replace(new_size);
                self.terminal.resize(new_size).map_err(internal)?;
                if self.connected {
                    let _ = self.writes.send(Write::Resize(new_size));
                    self.publish(driver)?;
                }
            }
            Command::Text(text) => {
                let _ = self
                    .writes
                    .send(Write::Bytes(crate::input::text_bytes(&text)));
            }
            Command::Key(key) => {
                let bytes = self.terminal.encode_key(&key).map_err(internal)?;
                let _ = self.writes.send(Write::Bytes(bytes));
            }
            Command::Scroll(scroll) => {
                let bytes = self.terminal.scroll(scroll).map_err(internal)?;
                if !bytes.is_empty() {
                    let _ = self.writes.send(Write::Bytes(bytes));
                }
                self.publish(driver)?;
            }
            Command::FullFrame => {
                self.terminal.request_full_frame();
                self.publish(driver)?;
            }
            Command::ApproveHostKey { .. } | Command::RejectHostKey | Command::Disconnect => {}
        }
        Ok(())
    }

    fn publish(&mut self, driver: &mut SessionDriver) -> Result<(), SessionFailure> {
        let frame = self.terminal.frame().map_err(internal)?;
        driver.publish(frame).map_err(internal)
    }
}

/// The network task's half once the channel runs its program: forwards channel output as
/// [`Event::Output`] and the write queue to the channel, concurrently, until the channel ends.
/// `shutdown` resolving closes the channel and ends with `Disconnected`; M1 passes a future
/// that never resolves because dropping the whole connection closes it.
///
/// The channel ending with an exit status or signal is `RemoteExited`; ending without one is
/// loss.
pub(crate) async fn pump_channel(
    channel: russh::Channel<client::Msg>,
    events: &mpsc::Sender<Event>,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    shutdown: impl Future<Output = ()>,
) -> Result<CloseReason, SessionFailure> {
    let (mut reader, writer) = channel.split();
    let mut exit_status = None;
    let mut exit_signal = false;
    let reading = async {
        loop {
            match reader.wait().await {
                Some(
                    russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. },
                ) => {
                    let _ = events.send(Event::Output(data.to_vec())).await;
                }
                Some(russh::ChannelMsg::ExitStatus {
                    exit_status: status,
                }) => exit_status = Some(status),
                Some(russh::ChannelMsg::ExitSignal { .. }) => exit_signal = true,
                Some(russh::ChannelMsg::Close) | None => {
                    return if exit_status.is_some() || exit_signal {
                        Ok(CloseReason::RemoteExited { exit_status })
                    } else {
                        Err(lost())
                    };
                }
                // EOF can precede exit-status. Keep reading until close.
                _ => {}
            }
        }
    };
    let writing = async {
        while let Some(write) = writes.recv().await {
            match write {
                Write::Bytes(bytes) => writer
                    .data(bytes.as_slice())
                    .await
                    .map_err(connection_error)?,
                Write::Reply(batch) => writer
                    .data(batch.bytes.as_slice())
                    .await
                    .map_err(connection_error)?,
                Write::Resize(size) => writer
                    .window_change(u32::from(size.columns()), u32::from(size.rows()), 0, 0)
                    .await
                    .map_err(connection_error)?,
            }
        }
        Err(lost())
    };
    // Keep one writer alive across reads: never restart a partially completed data() call.
    // The read end, shutdown or the enclosing cancellation ends it once, permanently.
    let ended = tokio::select! {
        result = reading => Some(result),
        result = writing => Some(result),
        () = shutdown => None,
    };
    match ended {
        Some(result) => result,
        None => {
            // Best effort: the host may already be gone.
            let _ = tokio::time::timeout(CHANNEL_CLOSE_GRACE, writer.close()).await;
            Ok(CloseReason::Disconnected)
        }
    }
}

/// Waits for the server's reply to a channel request (`pty-req`, `shell`, `exec`). Output that
/// arrives first is passed on so no byte is lost.
pub(crate) async fn request_accepted(
    channel: &mut russh::Channel<client::Msg>,
    events: &mpsc::Sender<Event>,
) -> Result<(), SessionFailure> {
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Success) => return Ok(()),
            Some(russh::ChannelMsg::Failure) => return Err(SessionFailure::ShellRejected),
            Some(
                russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. },
            ) => {
                let _ = events.send(Event::Output(data.to_vec())).await;
            }
            Some(russh::ChannelMsg::Close) | None => return Err(lost()),
            _ => {}
        }
    }
}

pub(crate) fn lost() -> SessionFailure {
    SessionFailure::ConnectionLost(
        "SSH transport EOF or channel closed without exit status/signal".into(),
    )
}

pub(crate) fn connection_error(error: russh::Error) -> SessionFailure {
    SessionFailure::ConnectionLost(describe(&error))
}

pub(crate) fn describe(error: &russh::Error) -> String {
    match error {
        russh::Error::IO(error) => format!("SSH transport ({:?}): {error}", error.kind()),
        error => format!("SSH: {error}"),
    }
}

pub(crate) fn internal(error: impl std::fmt::Display) -> SessionFailure {
    SessionFailure::Internal(error.to_string())
}
