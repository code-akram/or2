//! Contract probe: a test fixture, not a connection.
//!
//! `contract_probe_session` validates a real `ConnectRequest` and returns a real `Session`
//! whose driver is a deterministic script on a Rust thread instead of an SSH connection. It
//! presents a host key generated once per process, uses the production trust check against
//! the request's trusted keys, and renders fixed cells plus echoes of the input it receives.
//! JVM and device tests use it to exercise listener threading, lifecycle, host-key decisions,
//! frames and input across the real FFI. App code must never call it.

use std::sync::{Arc, OnceLock};

use or2_core::frame::{
    Cell, CellStyle, CellWidth, Cursor, CursorShape, Frame, Rgb, Row, Scrollback, Underline,
};
use or2_core::input::{ViewportScroll, text_bytes};
use or2_core::keys::ClientKey;
use or2_core::session::{
    self as core, CloseReason, Command, HostKeyPrompt, SessionDriver, SessionFailure, SessionState,
};
use or2_core::term::TerminalSize;
use or2_core::trust::{self, HostKey, HostKeyVerdict};

use crate::session::{ConnectError, ConnectRequest, ListenerObserver, Session, SessionListener};

const FOREGROUND: Rgb = Rgb::new(0xd0, 0xd0, 0xd0);
const BACKGROUND: Rgb = Rgb::new(0x10, 0x10, 0x18);
const HISTORY_ROWS: u64 = 100;

/// Test fixture only; see the module documentation. Never connects to anything.
#[uniffi::export]
pub fn contract_probe_session(
    request: ConnectRequest,
    listener: Box<dyn SessionListener>,
) -> Result<Arc<Session>, ConnectError> {
    let request = request.validate()?;
    let (handle, driver) = core::channel(Arc::new(ListenerObserver(listener)));
    std::thread::Builder::new()
        .name("or2-contract-probe".into())
        .spawn(move || run(&request.trusted_host_keys, request.size, driver))
        .expect("spawning the probe thread");
    Ok(Session::new(handle))
}

fn probe_host_key() -> &'static HostKey {
    static KEY: OnceLock<HostKey> = OnceLock::new();
    KEY.get_or_init(|| {
        HostKey::from_openssh(&ClientKey::generate_ed25519("").public_key().openssh)
            .expect("a generated public key parses")
    })
}

fn run(trusted: &[HostKey], mut size: TerminalSize, mut driver: SessionDriver) {
    let presented = probe_host_key();
    if trust::verify(presented, trusted) != HostKeyVerdict::Trusted {
        let prompt = HostKeyPrompt {
            presented: presented.clone(),
            previously_trusted: trusted.to_vec(),
        };
        driver
            .transition(SessionState::AwaitingHostKey(prompt))
            .expect("Connecting -> AwaitingHostKey");
        loop {
            match driver.blocking_next_command() {
                Command::ApproveHostKey { fingerprint }
                    if fingerprint == presented.fingerprint() =>
                {
                    break;
                }
                Command::RejectHostKey => {
                    return driver.close(CloseReason::Failed(SessionFailure::HostKeyRejected));
                }
                Command::Disconnect => return driver.close(CloseReason::Disconnected),
                Command::Resize(new_size) => size = new_size,
                _ => {}
            }
        }
    }
    driver
        .transition(SessionState::Authenticating)
        .expect("-> Authenticating");
    driver
        .transition(SessionState::Connected)
        .expect("-> Connected");

    let mut screen = Screen::new(size);
    publish(&mut driver, screen.full());
    loop {
        match driver.blocking_next_command() {
            Command::Resize(new_size) => {
                screen = Screen {
                    size: new_size,
                    ..screen
                };
                publish(&mut driver, screen.full());
            }
            Command::Text(text) => {
                let hex: Vec<String> = text_bytes(&text)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                screen.text_echo = format!("text {}", hex.join(" "));
                publish(&mut driver, screen.delta(2));
            }
            Command::Key(key) => {
                let m = key.modifiers();
                let modifiers: String = [
                    (m.shift, "+shift"),
                    (m.ctrl, "+ctrl"),
                    (m.alt, "+alt"),
                    (m.meta, "+meta"),
                ]
                .into_iter()
                .filter_map(|(on, name)| on.then_some(name))
                .collect();
                screen.key_echo = format!("key {:?}{modifiers}", key.key());
                publish(&mut driver, screen.delta(3));
            }
            Command::Scroll(scroll) => {
                screen.history_offset = match scroll {
                    ViewportScroll::Top => 0,
                    ViewportScroll::Bottom => HISTORY_ROWS,
                    ViewportScroll::Delta(rows) => screen
                        .history_offset
                        .saturating_add_signed(i64::from(rows))
                        .min(HISTORY_ROWS),
                };
                publish(&mut driver, screen.delta_without_rows());
            }
            Command::FullFrame => publish(&mut driver, screen.full()),
            Command::Disconnect => return driver.close(CloseReason::Disconnected),
            Command::ApproveHostKey { .. } | Command::RejectHostKey => {}
        }
        if matches!(driver.state(), SessionState::Closed(_)) {
            return;
        }
    }
}

fn publish(driver: &mut SessionDriver, frame: Frame) {
    if let Err(error) = driver.publish(frame) {
        driver.close(CloseReason::Failed(SessionFailure::Internal(
            error.to_string(),
        )));
    }
}

struct Screen {
    size: TerminalSize,
    text_echo: String,
    key_echo: String,
    history_offset: u64,
}

impl Screen {
    fn new(size: TerminalSize) -> Self {
        Self {
            size,
            text_echo: String::new(),
            key_echo: String::new(),
            history_offset: HISTORY_ROWS,
        }
    }

    fn plain() -> CellStyle {
        CellStyle::plain(FOREGROUND, BACKGROUND)
    }

    /// Row 1 holds the risky cells: styles, a combining mark, CJK and emoji wide cells.
    fn row(&self, index: u16) -> Row {
        let plain = Self::plain();
        let red_bold = CellStyle {
            bold: true,
            ..CellStyle::plain(Rgb::new(0xff, 0x33, 0x33), BACKGROUND)
        };
        let inverse = CellStyle::plain(BACKGROUND, FOREGROUND);
        let curly = CellStyle {
            underline: Underline::Curly,
            underline_color: Some(Rgb::new(0x33, 0x99, 0xff)),
            italic: true,
            ..plain
        };
        let graphemes: Vec<(String, bool, CellStyle)> = match index {
            0 => ascii("or2 contract probe", plain),
            1 => vec![
                ("R".into(), false, red_bold),
                ("界".into(), true, plain),
                ("😀".into(), true, plain),
                ("e\u{301}".into(), false, curly),
                ("I".into(), false, inverse),
            ],
            2 => ascii(&self.text_echo, plain),
            3 => ascii(&self.key_echo, plain),
            _ => Vec::new(),
        };
        let columns = usize::from(self.size.columns());
        let blank = Cell {
            text: String::new(),
            width: CellWidth::Narrow,
            style: plain,
        };
        let mut cells = Vec::with_capacity(columns);
        for (text, wide, style) in graphemes {
            let needed = if wide { 2 } else { 1 };
            if cells.len() + needed > columns {
                break;
            }
            if wide {
                cells.push(Cell {
                    text,
                    width: CellWidth::Wide,
                    style,
                });
                cells.push(Cell {
                    text: String::new(),
                    width: CellWidth::SpacerTail,
                    style,
                });
            } else {
                cells.push(Cell {
                    text,
                    width: CellWidth::Narrow,
                    style,
                });
            }
        }
        cells.resize(columns, blank);
        Row::new(index, false, cells)
    }

    fn cursor(&self) -> Option<Cursor> {
        // On the wide CJK head at row 1, column 1, when it fits.
        (self.size.rows() > 1 && self.size.columns() >= 3).then_some(Cursor {
            column: 1,
            row: 1,
            wide: true,
            shape: CursorShape::Bar,
            blinking: true,
            color: Rgb::new(0xff, 0xcc, 0x00),
        })
    }

    fn scrollback(&self) -> Scrollback {
        Scrollback {
            total_rows: HISTORY_ROWS + u64::from(self.size.rows()),
            offset: self.history_offset,
        }
    }

    fn full(&self) -> Frame {
        let rows = (0..self.size.rows()).map(|index| self.row(index)).collect();
        Frame::full(
            self.size,
            rows,
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe rows fit the viewport")
    }

    fn delta(&self, index: u16) -> Frame {
        let rows = if index < self.size.rows() {
            vec![self.row(index)]
        } else {
            Vec::new()
        };
        Frame::delta(
            self.size,
            rows,
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe rows fit the viewport")
    }

    fn delta_without_rows(&self) -> Frame {
        Frame::delta(
            self.size,
            Vec::new(),
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe cursor fits the viewport")
    }
}

fn ascii(text: &str, style: CellStyle) -> Vec<(String, bool, CellStyle)> {
    text.chars()
        .map(|c| (c.to_string(), false, style))
        .collect()
}
