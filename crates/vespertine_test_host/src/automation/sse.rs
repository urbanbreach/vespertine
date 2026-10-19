//! Server-Sent Events framing (WHATWG) for the automation protocol.
//!
//! Each message travels as one JSON object in the `data` field of an SSE frame: `data: ` followed
//! by the JSON text and a blank line. Encoders never emit `id:` or `event:` fields, and the decoder
//! only ever interprets `data:` lines, so ordinary log lines sharing the stream (including lines
//! that begin with `id:`, `event:`, `retry:` or `:`) are never taken for messages.

use super::protocol::Message;
use serde_json::Value;

/// Encodes a message as one SSE frame.
pub fn encode_frame(message: &Message) -> Vec<u8> {
    encode_json(&message.to_json())
}

/// Encodes a JSON value as one SSE frame (compact JSON on a single `data:` line).
pub fn encode_json(value: &Value) -> Vec<u8> {
    let mut frame = Vec::with_capacity(32);
    frame.extend_from_slice(b"data: ");
    frame.extend_from_slice(value.to_string().as_bytes());
    frame.extend_from_slice(b"\n\n");
    frame
}

/// Reassembles SSE frames from stream chunks that may split frames at arbitrary byte positions.
#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    previous_was_cr: bool,
    data: Vec<String>,
}

impl SseDecoder {
    /// A decoder with no buffered input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one chunk and returns the messages completed by it, in order. Frames whose data is
    /// not a valid message are passed to `on_invalid` (with the frame's data text) and otherwise
    /// dropped.
    pub fn feed(&mut self, chunk: &[u8], mut on_invalid: impl FnMut(&str)) -> Vec<Message> {
        let mut messages = Vec::new();
        for &byte in chunk {
            match byte {
                b'\n' => {
                    if std::mem::take(&mut self.previous_was_cr) {
                        // The LF of a CRLF pair: the line already ended at the CR.
                        continue;
                    }
                    self.end_line(&mut messages, &mut on_invalid);
                }
                b'\r' => {
                    self.previous_was_cr = true;
                    self.end_line(&mut messages, &mut on_invalid);
                }
                other => {
                    self.previous_was_cr = false;
                    self.line.push(other);
                }
            }
        }
        messages
    }

    fn end_line(&mut self, messages: &mut Vec<Message>, on_invalid: &mut impl FnMut(&str)) {
        let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
        if line.is_empty() {
            // A blank line dispatches the accumulated event, if it has data.
            if !self.data.is_empty() {
                let data = self.data.join("\n");
                self.data.clear();
                match serde_json::from_str::<Value>(&data)
                    .ok()
                    .as_ref()
                    .and_then(Message::from_json)
                {
                    Some(message) => messages.push(message),
                    None => on_invalid(&data),
                }
            }
            return;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            // This protocol sends exactly one data line per frame, so a data line that arrives
            // while an earlier one is still waiting for its blank line means the earlier frame
            // was never terminated (a log line interleaved with it): report it as invalid
            // rather than gluing the two together.
            if !self.data.is_empty() {
                let stale = self.data.join("\n");
                self.data.clear();
                on_invalid(&stale);
            }
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        }
        // Every other line (ids, event names, retry, comments, plain log output) is ignored.
    }
}
