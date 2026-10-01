//! The terminal pump of a terminal channel on a host connection (`ssh::connection`).
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
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use super::connection::{OpenedChannel, OpenedWriter};
use crate::session::{CloseReason, Command, SessionDriver, SessionFailure, SessionState};
use crate::submit::SubmitSequencer;
use crate::term::TerminalSize;
use crate::terminal::TerminalEngine;

/// Terminal replies (cursor reports, colour queries) queued or in flight may not exceed this.
pub(crate) const REPLY_BYTE_BUDGET: usize = 64 * 1024;

/// How long a closing session waits for its channel to say goodbye.
pub(crate) const CHANNEL_CLOSE_GRACE: Duration = Duration::from_millis(250);

/// From the network task to the session thread.
pub(crate) enum Event {
    /// The channel is open and running its program.
    Connected,
    Output(Vec<u8>),
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
    submits: SubmitSequencer,
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
                submits: SubmitSequencer::new(),
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

    /// Resolves when a submit's Enter is due ([`TerminalPump::enter_due`] then writes it).
    /// Owns its deadline, so it does not borrow the pump.
    pub(crate) fn enter_pending(&self) -> impl Future<Output = ()> + use<> {
        self.submits.due()
    }

    /// Writes the Enter of a submit, then the input that queued up behind it.
    pub(crate) fn enter_due(&mut self, driver: &mut SessionDriver) -> Result<(), SessionFailure> {
        self.submits.disarm();
        let bytes = self.terminal.submit_enter_bytes().map_err(internal)?;
        let _ = self.writes.send(Write::Bytes(bytes));
        while let Some(command) = self.submits.next_deferred() {
            self.run(driver, command)?;
        }
        Ok(())
    }

    /// Resize, input, scroll and full-frame commands. Anything else is the caller's. Input
    /// arriving while a submit waits to send its Enter queues behind it.
    pub(crate) fn command(
        &mut self,
        driver: &mut SessionDriver,
        command: Command,
    ) -> Result<(), SessionFailure> {
        match self.submits.admit(command) {
            Some(command) => self.run(driver, command),
            None => Ok(()),
        }
    }

    fn run(&mut self, driver: &mut SessionDriver, command: Command) -> Result<(), SessionFailure> {
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
            Command::Submit(text) => {
                let bytes = self.terminal.submit_text_bytes(&text).map_err(internal)?;
                if !bytes.is_empty() {
                    let _ = self.writes.send(Write::Bytes(bytes));
                }
                self.submits.arm();
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
            Command::ApproveHostKey { .. }
            | Command::RejectHostKey
            | Command::Roam
            | Command::Disconnect => {}
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
pub(super) async fn pump_channel(
    channel: OpenedChannel,
    events: &mpsc::Sender<Event>,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    shutdown: impl Future<Output = ()>,
) -> Result<CloseReason, SessionFailure> {
    let (mut reader, mut writer): (_, OpenedWriter) = channel.split();
    // An abort or any other cancellation of this future closes the channel through the
    // connection; the paths below that know the server closed it disarm the guard.
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
        // The server closed the channel or the connection broke: nothing left to close.
        Some(result) => {
            writer.disarm();
            result
        }
        None => {
            // Wait for the `Close` to be queued, but only so long: the connection finishes it
            // if the queue is full, and the session's `Closed` must not wait for that.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, KeyInput, Modifiers};
    use crate::session::{SessionObserver, channel};
    use crate::submit::SUBMIT_ENTER_DELAY;
    use tokio::time::Instant;

    struct Quiet;

    impl SessionObserver for Quiet {
        fn state_changed(&self, _: &SessionState) {}
        fn frame_ready(&self) {}
    }

    /// A pump with its writes, ready to take input, on a driver that is never read.
    fn pump() -> (TerminalPump, SessionDriver, mpsc::UnboundedReceiver<Write>) {
        let (_, driver) = channel(Arc::new(Quiet));
        let (pump, writes, _) = TerminalPump::new(TerminalSize::new(80, 24).unwrap()).unwrap();
        (pump, driver, writes)
    }

    fn bytes(write: Option<Write>) -> Vec<u8> {
        match write.expect("a write") {
            Write::Bytes(bytes) => bytes,
            _ => panic!("expected input bytes"),
        }
    }

    fn drain(writes: &mut mpsc::UnboundedReceiver<Write>) -> Vec<Vec<u8>> {
        let mut all = Vec::new();
        while let Ok(write) = writes.try_recv() {
            all.push(bytes(Some(write)));
        }
        all
    }

    #[test]
    fn submit_text_is_one_bracketed_paste_only_while_the_terminal_has_the_mode_on() {
        let (mut pump, mut driver, mut writes) = pump();
        let submit = || Command::Submit("hi\nthere".into());
        pump.command(&mut driver, submit()).unwrap();
        // Off: typed text with M1's newline mapping.
        assert_eq!(bytes(writes.try_recv().ok()), b"hi\rthere");
        assert!(writes.try_recv().is_err(), "the Enter is not part of it");
        pump.submits.disarm();
        pump.terminal.write(b"\x1b[?2004h");
        pump.command(&mut driver, submit()).unwrap();
        assert_eq!(
            bytes(writes.try_recv().ok()),
            b"\x1b[200~hi\nthere\x1b[201~"
        );
        pump.submits.disarm();
        pump.terminal.write(b"\x1b[?2004l");
        pump.command(&mut driver, submit()).unwrap();
        assert_eq!(bytes(writes.try_recv().ok()), b"hi\rthere");
    }

    #[test]
    fn a_paste_end_marker_in_submitted_text_is_removed() {
        let (mut pump, mut driver, mut writes) = pump();
        pump.terminal.write(b"\x1b[?2004h");
        pump.command(&mut driver, Command::Submit("a\x1b[201~rm -rf b".into()))
            .unwrap();
        assert_eq!(
            bytes(writes.try_recv().ok()),
            b"\x1b[200~arm -rf b\x1b[201~"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_enter_is_a_separate_write_after_the_delay() {
        let (mut pump, mut driver, mut writes) = pump();
        let start = Instant::now();
        pump.command(&mut driver, Command::Submit("go".into()))
            .unwrap();
        assert_eq!(drain(&mut writes), [b"go".to_vec()]);
        // Nothing else is written while the delay runs.
        let early = tokio::time::timeout(
            SUBMIT_ENTER_DELAY - Duration::from_millis(1),
            pump.enter_pending(),
        )
        .await;
        assert!(early.is_err());
        assert!(writes.try_recv().is_err());
        pump.enter_pending().await;
        assert_eq!(start.elapsed(), SUBMIT_ENTER_DELAY);
        pump.enter_due(&mut driver).unwrap();
        assert_eq!(drain(&mut writes), [b"\r".to_vec()]);
        // Nothing is pending afterwards.
        let after = tokio::time::timeout(Duration::from_secs(60), pump.enter_pending()).await;
        assert!(after.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn the_enter_follows_the_terminals_key_modes() {
        let (mut pump, mut driver, mut writes) = pump();
        pump.terminal.write(b"\x1b[>8u");
        pump.command(&mut driver, Command::Submit("go".into()))
            .unwrap();
        pump.enter_pending().await;
        pump.enter_due(&mut driver).unwrap();
        assert_eq!(drain(&mut writes), [b"go".to_vec(), b"\x1b[13u".to_vec()]);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_text_submits_just_the_enter() {
        let (mut pump, mut driver, mut writes) = pump();
        pump.terminal.write(b"\x1b[?2004h");
        pump.command(&mut driver, Command::Submit(String::new()))
            .unwrap();
        assert!(writes.try_recv().is_err(), "no empty paste");
        pump.enter_pending().await;
        pump.enter_due(&mut driver).unwrap();
        assert_eq!(drain(&mut writes), [b"\r".to_vec()]);
    }

    #[tokio::test(start_paused = true)]
    async fn input_after_a_submit_stays_behind_its_enter() {
        let (mut pump, mut driver, mut writes) = pump();
        let tab = KeyInput::new(Key::Tab, Modifiers::default()).unwrap();
        pump.command(&mut driver, Command::Submit("one".into()))
            .unwrap();
        pump.command(&mut driver, Command::Text("x".into()))
            .unwrap();
        pump.command(&mut driver, Command::Key(tab)).unwrap();
        pump.command(&mut driver, Command::Submit("two".into()))
            .unwrap();
        pump.command(&mut driver, Command::Text("y".into()))
            .unwrap();
        assert_eq!(drain(&mut writes), [b"one".to_vec()]);
        pump.enter_pending().await;
        pump.enter_due(&mut driver).unwrap();
        // The held input runs up to the next submit, which waits for its own Enter.
        assert_eq!(
            drain(&mut writes),
            [
                b"\r".to_vec(),
                b"x".to_vec(),
                b"\t".to_vec(),
                b"two".to_vec()
            ]
        );
        pump.enter_pending().await;
        pump.enter_due(&mut driver).unwrap();
        assert_eq!(drain(&mut writes), [b"\r".to_vec(), b"y".to_vec()]);
    }

    #[test]
    fn query_batches_coalesce_and_credit_includes_queued_and_in_flight_bytes() {
        let replies = Rc::new(RefCell::new(GeneratedReplies::new()));
        let collected = replies.clone();
        let mut terminal = TerminalEngine::new(TerminalSize::new(79, 23).unwrap(), move |bytes| {
            collected.borrow_mut().append(bytes)
        })
        .unwrap();
        let (writes, mut queued) = mpsc::unbounded_channel();
        let query = b"\x1b[6n".repeat(1000);
        for _ in 0..10 {
            terminal.write(&query);
            replies.borrow_mut().flush(&writes).unwrap();
        }
        assert_eq!(queued.len(), 10); // One write per vt_write, not per query.
        let Write::Reply(in_flight) = queued.try_recv().unwrap() else {
            panic!("expected reply")
        };
        assert_eq!(in_flight.bytes, b"\x1b[1;1R".repeat(1000));
        let budget = replies.borrow().budget.clone();
        assert_eq!(budget.available_permits(), 65536 - 60000);
        terminal.write(&query);
        assert!(matches!(
            replies.borrow_mut().flush(&writes),
            Err(SessionFailure::Protocol(_))
        ));
        assert_eq!(queued.len(), 9);
        assert_eq!(budget.available_permits(), 4); // Last incomplete batch stays below the cap.
        drop(in_flight);
        drop(queued);
        drop(terminal);
        drop(replies);
        assert_eq!(budget.available_permits(), 65536);
    }
}
