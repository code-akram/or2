//! The thin layer over the generated types: newline-delimited JSON on a herdr socket.
//!
//! Requests are `{"id", "method", "params"}` lines built from a generated
//! [`RequestBody`]. Every incoming line is classified by its top-level key (`error`, `result`
//! or `event`) before any typed parsing, so a message this build cannot fully read (a new
//! event, a new result type) is never mistaken for a broken connection.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::generated::error_response::ErrorBody;
use super::generated::request::RequestBody;
use crate::remote::{RemoteError, RemoteHost};

/// The longest line accepted from herdr. A snapshot of a very busy session is the largest
/// message and stays far below this.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// Spare capacity a reader keeps after a line, in bytes; a burst's larger buffer is given back.
const KEEP_CAPACITY: usize = 64 * 1024;

/// The error code herdr sends, then closes the connection, when an event subscriber fell too
/// far behind.
pub const EVENTS_LOST: &str = "events_lost";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The host connection failed or is closed.
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// The socket could not be opened: nothing listens, or (over OpenSSH, which cannot say
    /// which) the host forbids streamlocal forwarding.
    #[error("cannot reach the herdr socket: {0}")]
    Unreachable(String),
    #[error("herdr socket i/o failed: {0}")]
    Io(String),
    /// The peer closed the connection.
    #[error("herdr closed the connection")]
    Eof,
    #[error("a herdr message exceeded the line limit")]
    TooLong,
    #[error("herdr did not answer in time")]
    TimedOut,
    /// herdr answered the request with an error.
    #[error("herdr rejected the request ({code}): {message}")]
    Herdr { code: String, message: String },
    #[error("herdr sent a message this build cannot read: {0}")]
    Malformed(String),
}

/// One line from herdr.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `{"id", "result"}`: the `result` object, not yet typed.
    Success { id: String, result: Value },
    /// `{"id", "error"}`.
    Error {
        id: String,
        code: String,
        message: String,
    },
    /// A pushed event (`{"event", "data"}`), either family. Events are invalidations, so the
    /// payload is not interpreted; `kind` is only for diagnostics.
    Event { kind: Option<String> },
}

/// Classifies one line. A line that is not a JSON object is [`WireError::Malformed`]; an
/// object with none of the three keys counts as an event, because over-invalidating is safe.
pub fn classify(line: &[u8]) -> Result<Message, WireError> {
    let value: Value =
        serde_json::from_slice(line).map_err(|error| WireError::Malformed(error.to_string()))?;
    let Value::Object(mut object) = value else {
        return Err(WireError::Malformed("not a JSON object".into()));
    };
    let id = match object.remove("id") {
        Some(Value::String(id)) => id,
        _ => String::new(),
    };
    if let Some(error) = object.remove("error") {
        let body = serde_json::from_value(error).unwrap_or_else(|_| ErrorBody {
            code: "unknown".into(),
            message: "unreadable error".into(),
        });
        return Ok(Message::Error {
            id,
            code: body.code,
            message: body.message,
        });
    }
    if let Some(result) = object.remove("result") {
        return Ok(Message::Success { id, result });
    }
    let kind = match object.remove("event") {
        Some(Value::String(kind)) => Some(kind),
        _ => None,
    };
    Ok(Message::Event { kind })
}

/// `{"id", "method", "params"}` and a newline.
pub fn encode_request(id: &str, body: &RequestBody) -> Result<Vec<u8>, WireError> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        id: &'a str,
        #[serde(flatten)]
        body: &'a RequestBody,
    }
    let mut line = serde_json::to_vec(&Envelope { id, body })
        .map_err(|error| WireError::Malformed(error.to_string()))?;
    line.push(b'\n');
    Ok(line)
}

/// Reads newline-delimited messages from a stream with a length bound. [`next_line`] is
/// cancel-safe: whatever was read stays buffered, so it can sit in a `select!`.
///
/// [`next_line`]: LineReader::next_line
pub struct LineReader<S> {
    stream: S,
    buf: Vec<u8>,
    scanned: usize,
    /// The longest line accepted; [`MAX_LINE`] unless a test lowers it.
    limit: usize,
}

impl<S: AsyncRead + AsyncWrite + Unpin> LineReader<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buf: Vec::new(),
            scanned: 0,
            limit: MAX_LINE,
        }
    }

    pub async fn send(&mut self, line: &[u8]) -> Result<(), WireError> {
        self.stream
            .write_all(line)
            .await
            .map_err(|error| WireError::Io(error.to_string()))?;
        self.stream
            .flush()
            .await
            .map_err(|error| WireError::Io(error.to_string()))
    }

    /// The next line without its newline; `None` at end of stream. Bytes after the last
    /// newline when the stream ends are an incomplete message and dropped.
    pub async fn next_line(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        loop {
            if let Some(offset) = self.buf[self.scanned..].iter().position(|b| *b == b'\n') {
                let end = self.scanned + offset;
                // A read can deliver a newline and far more than the limit at once.
                if end > self.limit {
                    return Err(WireError::TooLong);
                }
                let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                line.pop();
                self.scanned = 0;
                // A long-lived stream must not keep the buffer of its largest burst.
                self.buf.shrink_to(KEEP_CAPACITY);
                return Ok(Some(line));
            }
            self.scanned = self.buf.len();
            if self.buf.len() > self.limit {
                return Err(WireError::TooLong);
            }
            self.buf.reserve(4096);
            let read = self
                .stream
                .read_buf(&mut self.buf)
                .await
                .map_err(|error| WireError::Io(error.to_string()))?;
            if read == 0 {
                return Ok(None);
            }
        }
    }

    /// The next classified message; `None` at end of stream.
    pub async fn next_message(&mut self) -> Result<Option<Message>, WireError> {
        match self.next_line().await? {
            Some(line) => classify(&line).map(Some),
            None => Ok(None),
        }
    }

    /// Reads until the response to a request arrives: skips pushed events, turns an error
    /// response into [`WireError::Herdr`].
    pub async fn response(&mut self) -> Result<Value, WireError> {
        loop {
            match self.next_message().await? {
                Some(Message::Success { result, .. }) => return Ok(result),
                Some(Message::Error { code, message, .. }) => {
                    return Err(WireError::Herdr { code, message });
                }
                Some(Message::Event { .. }) => {}
                None => return Err(WireError::Eof),
            }
        }
    }
}

/// Opens a stream to `socket`; a refused connection (nothing listens) is
/// [`WireError::Unreachable`], a closed host stays [`RemoteError::Closed`].
pub async fn open<H: RemoteHost>(
    host: &H,
    socket: &str,
) -> Result<LineReader<H::Stream>, WireError> {
    match host.open_unix(socket).await {
        Ok(stream) => Ok(LineReader::new(stream)),
        Err(RemoteError::Io(message)) => Err(WireError::Unreachable(message)),
        Err(error) => Err(error.into()),
    }
}

/// One request on a short-lived stream: open, send, read the response, drop the stream.
/// The whole exchange is bounded by `timeout`. Returns the `result` object.
pub async fn call<H: RemoteHost>(
    host: &H,
    socket: &str,
    id: &str,
    body: &RequestBody,
    timeout: Duration,
) -> Result<Value, WireError> {
    let line = encode_request(id, body)?;
    tokio::time::timeout(timeout, async {
        let mut reader = open(host, socket).await?;
        reader.send(&line).await?;
        reader.response().await
    })
    .await
    .map_err(|_| WireError::TimedOut)?
}

/// [`call`] on a stream that is already open (the caller opened it, possibly alongside another
/// stream, to save a round trip): sends the request and reads its response, bounded by
/// `timeout`. The stream is left open.
pub async fn call_on<S: AsyncRead + AsyncWrite + Unpin>(
    reader: &mut LineReader<S>,
    id: &str,
    body: &RequestBody,
    timeout: Duration,
) -> Result<Value, WireError> {
    let line = encode_request(id, body)?;
    tokio::time::timeout(timeout, async {
        reader.send(&line).await?;
        reader.response().await
    })
    .await
    .map_err(|_| WireError::TimedOut)?
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::duplex;

    use super::*;
    use crate::herdr::generated::request::{EmptyParams, PaneTarget};

    #[test]
    fn requests_carry_id_method_and_params() {
        let line = encode_request(
            "r1",
            &RequestBody::PaneFocus(PaneTarget {
                pane_id: "w1:p2".into(),
            }),
        )
        .unwrap();
        assert!(line.ends_with(b"\n"));
        let value: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            value,
            json!({"id": "r1", "method": "pane.focus", "params": {"pane_id": "w1:p2"}})
        );
        let line = encode_request(
            "r2",
            &RequestBody::SessionSnapshot(EmptyParams(serde_json::Map::new())),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            value,
            json!({"id": "r2", "method": "session.snapshot", "params": {}})
        );
    }

    #[test]
    fn lines_are_classified_by_their_top_level_key() {
        assert_eq!(
            classify(br#"{"id":"a","result":{"type":"subscription_started"}}"#).unwrap(),
            Message::Success {
                id: "a".into(),
                result: json!({"type": "subscription_started"})
            }
        );
        assert_eq!(
            classify(br#"{"id":"a","error":{"code":"events_lost","message":"behind","extra":1}}"#)
                .unwrap(),
            Message::Error {
                id: "a".into(),
                code: EVENTS_LOST.into(),
                message: "behind".into()
            }
        );
        assert_eq!(
            classify(br#"{"event":"some_future_event","data":{"type":"x","new":[1]}}"#).unwrap(),
            Message::Event {
                kind: Some("some_future_event".into())
            }
        );
        assert_eq!(
            classify(br#"{"surprise":true}"#).unwrap(),
            Message::Event { kind: None }
        );
        assert!(matches!(classify(b"[1]"), Err(WireError::Malformed(_))));
        assert!(matches!(classify(b"nope"), Err(WireError::Malformed(_))));
    }

    #[tokio::test]
    async fn the_reader_splits_lines_across_reads_and_survives_cancellation() {
        let (client, mut server) = duplex(64);
        let mut reader = LineReader::new(client);
        server.write_all(b"one\ntw").await.unwrap();
        assert_eq!(reader.next_line().await.unwrap().unwrap(), b"one");
        // Cancelled mid-line: the partial bytes stay buffered.
        let waited = tokio::time::timeout(Duration::from_millis(30), reader.next_line()).await;
        assert!(waited.is_err());
        server.write_all(b"o\nthree\n").await.unwrap();
        assert_eq!(reader.next_line().await.unwrap().unwrap(), b"two");
        assert_eq!(reader.next_line().await.unwrap().unwrap(), b"three");
        server.write_all(b"partial").await.unwrap();
        drop(server);
        assert_eq!(reader.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_line_over_the_limit_is_rejected_even_when_its_newline_arrived_with_it() {
        let (client, mut server) = duplex(1024);
        let mut reader = LineReader::new(client);
        reader.limit = 64;
        let mut line = vec![b'x'; 100];
        line.push(b'\n');
        server.write_all(&line).await.unwrap();
        assert_eq!(reader.next_line().await, Err(WireError::TooLong));

        let (client, mut server) = duplex(1024);
        let mut reader = LineReader::new(client);
        reader.limit = 64;
        let mut line = vec![b'y'; 64];
        line.push(b'\n');
        server.write_all(&line).await.unwrap();
        assert_eq!(reader.next_line().await.unwrap().unwrap().len(), 64);
    }

    #[tokio::test]
    async fn the_buffer_of_a_burst_is_given_back() {
        let (client, mut server) = duplex(1 << 21);
        let mut reader = LineReader::new(client);
        let mut line = vec![b'z'; 1 << 20];
        line.push(b'\n');
        server.write_all(&line).await.unwrap();
        assert_eq!(reader.next_line().await.unwrap().unwrap().len(), 1 << 20);
        assert!(
            reader.buf.capacity() <= 2 * KEEP_CAPACITY,
            "{}",
            reader.buf.capacity()
        );
    }

    #[tokio::test]
    async fn the_reader_rejects_an_endless_line() {
        let (client, mut server) = duplex(1 << 16);
        let mut reader = LineReader::new(client);
        let writer = tokio::spawn(async move {
            let chunk = vec![b'x'; 1 << 16];
            while server.write_all(&chunk).await.is_ok() {}
        });
        assert_eq!(reader.next_line().await, Err(WireError::TooLong));
        drop(reader);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn responses_skip_events_and_surface_errors() {
        let (client, mut server) = duplex(1024);
        let mut reader = LineReader::new(client);
        server
            .write_all(
                b"{\"event\":\"x\",\"data\":{}}\n{\"id\":\"1\",\"result\":{\"type\":\"ok\"}}\n",
            )
            .await
            .unwrap();
        assert_eq!(reader.response().await.unwrap(), json!({"type": "ok"}));
        server
            .write_all(
                b"{\"id\":\"2\",\"error\":{\"code\":\"pane_not_found\",\"message\":\"no\"}}\n",
            )
            .await
            .unwrap();
        assert_eq!(
            reader.response().await,
            Err(WireError::Herdr {
                code: "pane_not_found".into(),
                message: "no".into()
            })
        );
        drop(server);
        assert_eq!(reader.response().await, Err(WireError::Eof));
    }
}
