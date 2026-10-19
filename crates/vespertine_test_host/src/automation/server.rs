//! The server loop: serves an [`AutomationHost`] over a byte channel (standard input and output
//! for a launched application).

use super::{
    host::{AutomationHost, dispatch},
    protocol::{Message, validate_request},
    sse::{SseDecoder, encode_frame},
};
use serde_json::Value;
use std::io::{self, Read, Write};

/// Answers one request message: validates it against the catalog and dispatches it. The
/// response carries the request's id, or `null` when the request had no valid integer id.
pub fn respond(host: &mut dyn AutomationHost, id: &Value, method: &str, params: &Value) -> Message {
    match validate_request(id, method, params) {
        Ok(request) => Message::Response {
            id: Value::from(request.id),
            outcome: dispatch(host, &request),
        },
        Err(error) => Message::Response {
            id: if id.is_i64() { id.clone() } else { Value::Null },
            outcome: Err(error),
        },
    }
}

/// Serves `host` until `input` reaches end of stream: decodes SSE frames from the chunks read,
/// answers every request in order and writes each response as one frame, flushing after each.
/// Frames that are not valid messages are ignored, as is anything else on the stream.
pub fn serve(
    host: &mut dyn AutomationHost,
    mut input: impl Read,
    mut output: impl Write,
) -> io::Result<()> {
    let mut decoder = SseDecoder::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        for message in decoder.feed(&buffer[..read], |_| {}) {
            if let Message::Request { id, method, params } = message {
                let response = respond(host, &id, &method, &params);
                output.write_all(&encode_frame(&response))?;
                output.flush()?;
            }
        }
    }
}

/// Serves `host` over this process's standard input and output.
pub fn serve_stdio(host: &mut dyn AutomationHost) -> io::Result<()> {
    serve(host, io::stdin().lock(), io::stdout().lock())
}
