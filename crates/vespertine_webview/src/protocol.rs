//! The framed message protocol between the application and the web view helper (PLT-015 R1 to
//! R6, PLT-014 R5/R6).
//!
//! Every message has a 9 byte header (kind, page identifier, payload length, little endian)
//! followed by the payload. This module is display independent: it encodes and decodes
//! messages, enforces the size limits, skips unknown kinds, and builds the typed commands the
//! application sends. File descriptors (shared memory pools) arrive as `SCM_RIGHTS` ancillary
//! data on the same message that announces them.

use serde_json::{Map, Value, json};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Header length in bytes: kind (1), page identifier (4), payload length (4).
pub const HEADER_LEN: usize = 9;
/// Largest payload the application may send to the helper (1 MiB).
pub const MAX_COMMAND_PAYLOAD: usize = 1 << 20;
/// Largest payload of any helper message except pixel frames (1 MiB).
pub const MAX_MESSAGE_PAYLOAD: usize = 1 << 20;
/// Largest dimension of a page or frame in device pixels.
pub const MAX_DIMENSION: u32 = 8192;
/// Largest inline frame payload: 8192 x 8192 x 4 pixel bytes plus the 12 byte frame header.
pub const MAX_FRAME_PAYLOAD: usize = (MAX_DIMENSION as usize) * (MAX_DIMENSION as usize) * 4 + 12;
/// Largest payload of the helper's first message.
pub const MAX_STARTUP_PAYLOAD: usize = 16384;
/// Protocol major version this application implements.
pub const PROTOCOL_VERSION: u32 = 1;
/// Lowest and highest page scale.
pub const MIN_SCALE: f32 = 0.5;
pub const MAX_SCALE: f32 = 4.0;

/// Message kinds. Helper to application kinds are upper case letters, the command kind is `K`.
pub mod kind {
    pub const READY: u8 = b'R';
    pub const ERROR: u8 = b'E';
    pub const PONG: u8 = b'Z';
    pub const STATE: u8 = b'S';
    pub const INPUT_METHOD: u8 = b'I';
    pub const MENU: u8 = b'M';
    pub const CLIPBOARD: u8 = b'C';
    pub const NEW_TAB: u8 = b'N';
    pub const EVAL_RESULT: u8 = b'J';
    pub const FRAME: u8 = b'F';
    pub const POOL: u8 = b'P';
    pub const BUFFER: u8 = b'B';
    pub const COMMAND: u8 = b'K';
}

/// Which way a message travels; selects the known kinds and the size limits.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Application to helper: only `K` is known, payloads up to [`MAX_COMMAND_PAYLOAD`].
    ToHelper,
    /// Helper to application.
    ToApplication,
}

impl Direction {
    fn is_known(self, kind: u8) -> bool {
        match self {
            Direction::ToHelper => kind == kind::COMMAND,
            Direction::ToApplication => matches!(
                kind,
                kind::READY
                    | kind::ERROR
                    | kind::PONG
                    | kind::STATE
                    | kind::INPUT_METHOD
                    | kind::MENU
                    | kind::CLIPBOARD
                    | kind::NEW_TAB
                    | kind::EVAL_RESULT
                    | kind::FRAME
                    | kind::POOL
                    | kind::BUFFER
            ),
        }
    }

    /// The payload limit for a message of `kind` travelling in this direction.
    pub fn payload_limit(self, kind: u8) -> usize {
        match self {
            Direction::ToHelper => MAX_COMMAND_PAYLOAD,
            Direction::ToApplication if kind == kind::FRAME => MAX_FRAME_PAYLOAD,
            Direction::ToApplication => MAX_MESSAGE_PAYLOAD,
        }
    }
}

/// A decoded message header.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub kind: u8,
    pub page: u32,
    pub len: u32,
}

impl Header {
    pub fn encode(self) -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[0] = self.kind;
        bytes[1..5].copy_from_slice(&self.page.to_le_bytes());
        bytes[5..9].copy_from_slice(&self.len.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: [u8; HEADER_LEN]) -> Self {
        Self {
            kind: bytes[0],
            page: u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            len: u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
        }
    }
}

/// A message with its payload and any file descriptors that travelled with it.
#[derive(Debug)]
pub struct Message {
    pub kind: u8,
    pub page: u32,
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFd>,
}

impl Message {
    pub fn new(kind: u8, page: u32, payload: Vec<u8>) -> Self {
        Self {
            kind,
            page,
            payload,
            fds: Vec::new(),
        }
    }

    /// The payload parsed as a JSON object, or `None` when it is not one (such messages have no
    /// effect and are not fatal, PLT-015 R1).
    pub fn json_object(&self) -> Option<Map<String, Value>> {
        match serde_json::from_slice::<Value>(&self.payload) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        }
    }
}

/// Encodes one message: header followed by the payload.
///
/// Returns an error when the payload is larger than `direction` allows.
pub fn encode_message(
    direction: Direction,
    kind: u8,
    page: u32,
    payload: &[u8],
) -> Result<Vec<u8>, ProtocolError> {
    let limit = direction.payload_limit(kind);
    if payload.len() > limit {
        return Err(ProtocolError::Oversized {
            kind,
            len: payload.len() as u64,
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(
        &Header {
            kind,
            page,
            len: payload.len() as u32,
        }
        .encode(),
    );
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// Why a message could not be read or written.
#[derive(Debug)]
pub enum ProtocolError {
    /// The peer closed the channel exactly between two messages.
    Eof,
    /// The channel closed in the middle of a header or payload.
    Truncated,
    /// A payload larger than the limit for its kind and direction.
    Oversized { kind: u8, len: u64, limit: usize },
    /// An operating system error while reading or writing.
    Io(io::Error),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Eof => write!(f, "the channel was closed"),
            ProtocolError::Truncated => {
                write!(f, "the channel was closed in the middle of a message")
            }
            ProtocolError::Oversized { kind, len, limit } => write!(
                f,
                "message of kind {:?} announces {len} payload bytes, more than the limit of {limit}",
                *kind as char
            ),
            ProtocolError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        ProtocolError::Io(error)
    }
}

/// A byte source that can also deliver file descriptors received with the bytes.
pub trait RecvBytes {
    /// Reads up to `buf.len()` bytes; descriptors received with them are appended to `fds`.
    fn recv(&mut self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize>;
}

/// Plain byte slices (and anything else `Read`) carry no descriptors.
pub struct NoFds<R>(pub R);

impl<R: Read> RecvBytes for NoFds<R> {
    fn recv(&mut self, buf: &mut [u8], _fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// Upper bound on descriptors accepted with one read.
const MAX_FDS_PER_READ: usize = 16;

/// Receives bytes and `SCM_RIGHTS` descriptors from a Unix socket.
pub fn recv_with_fds(socket: RawFd, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // Space for MAX_FDS_PER_READ descriptors, aligned for cmsghdr.
    let mut control = [0u64; 24];
    let control_len = unsafe { libc::CMSG_SPACE((MAX_FDS_PER_READ * 4) as u32) } as usize;
    debug_assert!(control_len <= std::mem::size_of_val(&control));
    loop {
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control_len as _;
        let received = unsafe { libc::recvmsg(socket, &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&message);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(cmsg);
                    let payload_len = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    for index in 0..payload_len / 4 {
                        let mut raw: libc::c_int = 0;
                        std::ptr::copy_nonoverlapping(
                            data.add(index * 4),
                            (&mut raw as *mut libc::c_int).cast::<u8>(),
                            4,
                        );
                        fds.push(OwnedFd::from_raw_fd(raw));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&message, cmsg);
            }
        }
        return Ok(received as usize);
    }
}

impl RecvBytes for &UnixStream {
    fn recv(&mut self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
        recv_with_fds(self.as_raw_fd(), buf, fds)
    }
}

impl RecvBytes for UnixStream {
    fn recv(&mut self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
        recv_with_fds(self.as_raw_fd(), buf, fds)
    }
}

fn recv_exact<S: RecvBytes>(
    source: &mut S,
    buf: &mut [u8],
    fds: &mut Vec<OwnedFd>,
    at_boundary: bool,
) -> Result<(), ProtocolError> {
    let mut filled = 0;
    while filled < buf.len() {
        match source.recv(&mut buf[filled..], fds) {
            Ok(0) => {
                return Err(if filled == 0 && at_boundary {
                    ProtocolError::Eof
                } else {
                    ProtocolError::Truncated
                });
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ProtocolError::Io(error)),
        }
    }
    Ok(())
}

/// Reads one message in `direction` from `source`.
///
/// Messages of an unknown kind are skipped using their length (without buffering the payload)
/// and the next message is returned. A payload longer than the limit for its kind is an error.
/// Exactly the bytes of the returned message are consumed, so descriptors received with it belong
/// to it.
pub fn read_message<S: RecvBytes>(
    source: &mut S,
    direction: Direction,
) -> Result<Message, ProtocolError> {
    read_message_limited(source, direction, None)
}

/// Like [`read_message`] with an additional cap on the payload length (the 16384 byte limit of the
/// first message, PLT-014 R6).
pub fn read_message_limited<S: RecvBytes>(
    source: &mut S,
    direction: Direction,
    cap: Option<usize>,
) -> Result<Message, ProtocolError> {
    let mut fds = Vec::new();
    loop {
        let mut header_bytes = [0u8; HEADER_LEN];
        recv_exact(source, &mut header_bytes, &mut fds, true)?;
        let header = Header::decode(header_bytes);
        let len = header.len as usize;
        let mut limit = direction.payload_limit(header.kind);
        if let Some(cap) = cap {
            limit = limit.min(cap);
        }
        if len > limit {
            return Err(ProtocolError::Oversized {
                kind: header.kind,
                len: len as u64,
                limit,
            });
        }
        if !direction.is_known(header.kind) {
            let mut remaining = len;
            let mut scratch = [0u8; 4096];
            while remaining > 0 {
                let chunk = remaining.min(scratch.len());
                recv_exact(source, &mut scratch[..chunk], &mut fds, false)?;
                remaining -= chunk;
            }
            // Descriptors attached to a skipped message are closed with `fds`.
            fds.clear();
            continue;
        }
        let mut payload = vec![0u8; len];
        recv_exact(source, &mut payload, &mut fds, false)?;
        return Ok(Message {
            kind: header.kind,
            page: header.page,
            payload,
            fds,
        });
    }
}

/// Writes one complete message with a blocking write, retrying partial writes.
pub fn write_all(socket: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let written = unsafe {
            libc::send(
                socket,
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// Sends one message together with descriptors (used by scripted helpers in tests).
pub fn send_with_fds(socket: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if fds.is_empty() {
        return write_all(socket, bytes);
    }
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut _,
        iov_len: bytes.len(),
    };
    let mut control = [0u64; 24];
    let fd_bytes = fds.len() * 4;
    let space = unsafe { libc::CMSG_SPACE(fd_bytes as u32) } as usize;
    assert!(
        space <= std::mem::size_of_val(&control),
        "too many descriptors"
    );
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space as _;
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&message);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_bytes as u32) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(cmsg), fd_bytes);
    }
    let sent = loop {
        let written = unsafe { libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) };
        if written >= 0 {
            break written as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if sent < bytes.len() {
        write_all(socket, &bytes[sent..])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Ready message (PLT-015 R3)
// ---------------------------------------------------------------------------------------------

/// A pixel transfer mode.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Transfer {
    Inline,
    SharedMemory,
    DmaBuf,
}

impl Transfer {
    pub fn name(self) -> &'static str {
        match self {
            Transfer::Inline => "inline",
            Transfer::SharedMemory => "shm",
            Transfer::DmaBuf => "dmabuf",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "inline" => Some(Transfer::Inline),
            "shm" => Some(Transfer::SharedMemory),
            "dmabuf" => Some(Transfer::DmaBuf),
            _ => None,
        }
    }
}

/// The content of the helper's ready message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ready {
    pub protocol: u32,
    pub engine: String,
    pub transfers: Vec<Transfer>,
    pub capabilities: Vec<String>,
}

impl Ready {
    /// Whether the helper advertised `capability` ("ping", "eval").
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Parses a ready payload. An empty payload is protocol 1 with inline transfer and no
    /// capabilities. A protocol major version above the supported one fails with
    /// `Browser helper protocol version N is not supported`; any other invalid payload fails with
    /// `Invalid browser startup response`.
    pub fn parse(payload: &[u8]) -> Result<Self, String> {
        let invalid = || "Invalid browser startup response".to_owned();
        if payload.is_empty() {
            return Ok(Self {
                protocol: 1,
                engine: String::new(),
                transfers: vec![Transfer::Inline],
                capabilities: Vec::new(),
            });
        }
        let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(payload) else {
            return Err(invalid());
        };
        let protocol = match object.get("protocol") {
            None => 1,
            Some(value) => value.as_u64().ok_or_else(invalid)?,
        };
        if protocol > u64::from(PROTOCOL_VERSION) {
            return Err(format!(
                "Browser helper protocol version {protocol} is not supported"
            ));
        }
        let engine = object
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut transfers = Vec::new();
        if let Some(Value::Array(names)) = object.get("transfers") {
            for name in names {
                if let Some(transfer) = name.as_str().and_then(Transfer::from_name)
                    && !transfers.contains(&transfer)
                {
                    transfers.push(transfer);
                }
            }
        }
        // "inline" is mandatory.
        if !transfers.contains(&Transfer::Inline) {
            transfers.push(Transfer::Inline);
        }
        let capabilities = match object.get("capabilities") {
            Some(Value::Array(names)) => names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            protocol: protocol as u32,
            engine,
            transfers,
            capabilities,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Commands (PLT-015 R4, PLT-016)
// ---------------------------------------------------------------------------------------------

/// The commands the application sends, each addressed to a page (0 for helper-global ones).
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Configure {
        transfer: Transfer,
    },
    Ping {
        nonce: u64,
    },
    Shutdown,
    Release {
        generation: u64,
        slot: u32,
    },
    Create,
    Close,
    Load {
        url: String,
    },
    Reload,
    Back,
    Forward,
    Resize {
        width: u32,
        height: u32,
        scale: f32,
    },
    Visible(bool),
    Move {
        x: f64,
        y: f64,
        mods: u32,
    },
    Down {
        x: f64,
        y: f64,
        button: u32,
        mods: u32,
    },
    Up {
        x: f64,
        y: f64,
        button: u32,
        mods: u32,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        mods: u32,
    },
    Leave {
        x: f64,
        y: f64,
    },
    Focus(bool),
    KeyDown {
        key: String,
        text: String,
        mods: u32,
    },
    KeyUp {
        key: String,
        text: String,
        mods: u32,
    },
    Preedit(String),
    Commit(String),
    Unmark,
    Text(String),
    Copy,
    Cut,
    SelectAll,
    DismissMenu,
    /// A context-menu action returned to the helper: "open-link", "copy-link", "copy",
    /// "select-all", "back", "forward", "reload".
    MenuAction(&'static str),
    /// "option:N" activation of the pending option menu.
    Option(usize),
    Eval {
        script: String,
    },
}

impl Command {
    /// The "cmd" member value.
    pub fn name(&self) -> String {
        match self {
            Command::Configure { .. } => "configure".into(),
            Command::Ping { .. } => "ping".into(),
            Command::Shutdown => "shutdown".into(),
            Command::Release { .. } => "release".into(),
            Command::Create => "create".into(),
            Command::Close => "close".into(),
            Command::Load { .. } => "load".into(),
            Command::Reload => "reload".into(),
            Command::Back => "back".into(),
            Command::Forward => "forward".into(),
            Command::Resize { .. } => "resize".into(),
            Command::Visible(_) => "visible".into(),
            Command::Move { .. } => "move".into(),
            Command::Down { .. } => "down".into(),
            Command::Up { .. } => "up".into(),
            Command::Scroll { .. } => "scroll".into(),
            Command::Leave { .. } => "leave".into(),
            Command::Focus(_) => "focus".into(),
            Command::KeyDown { .. } => "key_down".into(),
            Command::KeyUp { .. } => "key_up".into(),
            Command::Preedit(_) => "preedit".into(),
            Command::Commit(_) => "commit".into(),
            Command::Unmark => "unmark".into(),
            Command::Text(_) => "text".into(),
            Command::Copy => "copy".into(),
            Command::Cut => "cut".into(),
            Command::SelectAll => "select-all".into(),
            Command::DismissMenu => "dismiss-menu".into(),
            Command::MenuAction(action) => (*action).into(),
            Command::Option(index) => format!("option:{index}"),
            Command::Eval { .. } => "eval".into(),
        }
    }

    /// The JSON payload of the command message.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("cmd".into(), Value::String(self.name()));
        let mut set = |key: &str, value: Value| {
            object.insert(key.into(), value);
        };
        match self {
            Command::Configure { transfer } => set("transfer", json!(transfer.name())),
            Command::Ping { nonce } => set("nonce", json!(nonce)),
            Command::Release { generation, slot } => {
                set("generation", json!(generation));
                set("slot", json!(slot));
            }
            Command::Load { url } => set("url", json!(url)),
            Command::Resize {
                width,
                height,
                scale,
            } => {
                set("width", json!(width));
                set("height", json!(height));
                set("scale", json!(f64::from(*scale)));
            }
            Command::Visible(value) => set("value", json!(u8::from(*value))),
            Command::Move { x, y, mods } => {
                set("x", json!(x));
                set("y", json!(y));
                set("mods", json!(mods));
            }
            Command::Down { x, y, button, mods } | Command::Up { x, y, button, mods } => {
                set("x", json!(x));
                set("y", json!(y));
                set("button", json!(button));
                set("mods", json!(mods));
            }
            Command::Scroll { x, y, dx, dy, mods } => {
                set("x", json!(x));
                set("y", json!(y));
                set("dx", json!(dx));
                set("dy", json!(dy));
                set("mods", json!(mods));
            }
            Command::Leave { x, y } => {
                set("x", json!(x));
                set("y", json!(y));
            }
            Command::Focus(value) => set("value", json!(u8::from(*value))),
            Command::KeyDown { key, text, mods } | Command::KeyUp { key, text, mods } => {
                set("key", json!(key));
                set("text", json!(text));
                set("mods", json!(mods));
            }
            Command::Preedit(text) | Command::Commit(text) | Command::Text(text) => {
                set("text", json!(text));
            }
            Command::Eval { script } => set("script", json!(script)),
            Command::Shutdown
            | Command::Create
            | Command::Close
            | Command::Reload
            | Command::Back
            | Command::Forward
            | Command::Unmark
            | Command::Copy
            | Command::Cut
            | Command::SelectAll
            | Command::DismissMenu
            | Command::MenuAction(_)
            | Command::Option(_) => {}
        }
        Value::Object(object)
    }

    /// Encodes the command for `page` as a complete message; fails when the payload exceeds
    /// the 1 MiB command limit.
    pub fn encode(&self, page: u32) -> Result<Vec<u8>, ProtocolError> {
        let payload = serde_json::to_vec(&self.to_json()).expect("command JSON serializes");
        encode_message(Direction::ToHelper, kind::COMMAND, page, &payload)
    }

    /// Decodes a command payload (used by scripted helpers and tests).
    pub fn cmd_name(payload: &Value) -> Option<&str> {
        payload.get("cmd").and_then(Value::as_str)
    }
}

/// Clamps a page size or scale to the helper's accepted ranges (PLT-015 R4).
pub fn clamp_dimension(value: f64) -> u32 {
    (value.round().max(1.0)).min(f64::from(MAX_DIMENSION)) as u32
}

/// Clamps a page scale to 0.5..4.
pub fn clamp_scale(scale: f32) -> f32 {
    if scale.is_nan() {
        1.0
    } else {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    }
}

// ---------------------------------------------------------------------------------------------
// Pixel frame messages (PLT-015 R8 to R11)
// ---------------------------------------------------------------------------------------------

/// Parsed header of an inline frame payload.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct InlineFrameHeader {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// Length of the inline frame header (width, height, scale bits).
pub const INLINE_FRAME_HEADER_LEN: usize = 12;

/// Validates an inline frame payload and returns its header, or `None` when the frame must be
/// discarded without error (PLT-015 R8): shorter than 12 bytes, scale not finite or outside
/// 0.5..4, a dimension outside 1..8192, or a pixel byte count different from width x height x 4.
pub fn parse_inline_frame(payload: &[u8]) -> Option<InlineFrameHeader> {
    if payload.len() < INLINE_FRAME_HEADER_LEN {
        return None;
    }
    let width = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(payload[4..8].try_into().ok()?);
    let scale = f32::from_bits(u32::from_le_bytes(payload[8..12].try_into().ok()?));
    if !scale.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return None;
    }
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    let pixels = u64::from(width) * u64::from(height) * 4;
    if pixels != (payload.len() - INLINE_FRAME_HEADER_LEN) as u64 {
        return None;
    }
    Some(InlineFrameHeader {
        width,
        height,
        scale,
    })
}

/// Encodes an inline frame payload (scripted helpers and tests).
pub fn encode_inline_frame(width: u32, height: u32, scale: f32, pixels: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(INLINE_FRAME_HEADER_LEN + pixels.len());
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    payload.extend_from_slice(&scale.to_bits().to_le_bytes());
    payload.extend_from_slice(pixels);
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(bytes: &[u8], direction: Direction) -> Vec<Message> {
        let mut source = NoFds(bytes);
        let mut messages = Vec::new();
        loop {
            match read_message(&mut source, direction) {
                Ok(message) => messages.push(message),
                Err(ProtocolError::Eof) => return messages,
                Err(error) => panic!("unexpected error: {error}"),
            }
        }
    }

    #[test]
    fn header_round_trips_for_every_kind() {
        let kinds = [
            kind::READY,
            kind::ERROR,
            kind::PONG,
            kind::STATE,
            kind::INPUT_METHOD,
            kind::MENU,
            kind::CLIPBOARD,
            kind::NEW_TAB,
            kind::EVAL_RESULT,
            kind::FRAME,
            kind::POOL,
            kind::BUFFER,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let page = 0x0102_0300 + index as u32;
            let payload = vec![index as u8; index];
            let bytes = encode_message(Direction::ToApplication, kind, page, &payload).unwrap();
            let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap());
            assert_eq!(
                header,
                Header {
                    kind,
                    page,
                    len: index as u32
                }
            );
            let messages = decode_all(&bytes, Direction::ToApplication);
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].kind, kind);
            assert_eq!(messages[0].page, page);
            assert_eq!(messages[0].payload, payload);
        }
        let command = encode_message(Direction::ToHelper, kind::COMMAND, 7, b"{}").unwrap();
        let messages = decode_all(&command, Direction::ToHelper);
        assert_eq!((messages[0].kind, messages[0].page), (kind::COMMAND, 7));
    }

    #[test]
    fn wire_examples_match_the_specification() {
        let ready = encode_message(Direction::ToApplication, kind::READY, 0, b"").unwrap();
        assert_eq!(ready, [0x52, 0, 0, 0, 0, 0, 0, 0, 0]);
        let error = encode_message(Direction::ToApplication, kind::ERROR, 0, b"locked").unwrap();
        let mut expected = vec![0x45, 0, 0, 0, 0, 6, 0, 0, 0];
        expected.extend_from_slice(b"locked");
        assert_eq!(error, expected);
    }

    #[test]
    fn unknown_kinds_are_skipped_with_their_payload() {
        let mut bytes = encode_message(Direction::ToApplication, b'Q', 3, b"skipped payload")
            .unwrap_or_else(|_| unreachable!());
        bytes.extend(
            encode_message(Direction::ToApplication, kind::PONG, 0, b"{\"nonce\":4}").unwrap(),
        );
        let messages = decode_all(&bytes, Direction::ToApplication);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, kind::PONG);
    }

    #[test]
    fn a_skipped_unknown_message_larger_than_one_buffer_is_not_buffered() {
        let mut bytes =
            encode_message(Direction::ToApplication, b'Q', 3, &vec![9u8; 100_000]).unwrap();
        bytes.extend(encode_message(Direction::ToApplication, kind::PONG, 0, b"{}").unwrap());
        let messages = decode_all(&bytes, Direction::ToApplication);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn oversized_payloads_are_rejected_in_both_directions() {
        assert!(matches!(
            encode_message(
                Direction::ToHelper,
                kind::COMMAND,
                1,
                &vec![0u8; MAX_COMMAND_PAYLOAD + 1]
            ),
            Err(ProtocolError::Oversized { .. })
        ));
        assert!(
            encode_message(
                Direction::ToHelper,
                kind::COMMAND,
                1,
                &vec![0u8; MAX_COMMAND_PAYLOAD]
            )
            .is_ok()
        );
        // A helper packet that announces more than the frame bound.
        let header = Header {
            kind: kind::FRAME,
            page: 1,
            len: (MAX_FRAME_PAYLOAD + 1) as u32,
        }
        .encode();
        assert!(matches!(
            read_message(&mut NoFds(&header[..]), Direction::ToApplication),
            Err(ProtocolError::Oversized { .. })
        ));
        // A non-frame message above 1 MiB is also rejected.
        let header = Header {
            kind: kind::STATE,
            page: 1,
            len: (MAX_MESSAGE_PAYLOAD + 1) as u32,
        }
        .encode();
        assert!(matches!(
            read_message(&mut NoFds(&header[..]), Direction::ToApplication),
            Err(ProtocolError::Oversized { .. })
        ));
        // A frame at exactly the bound is acceptable to the header check.
        let header = Header {
            kind: kind::FRAME,
            page: 1,
            len: MAX_FRAME_PAYLOAD as u32,
        }
        .encode();
        assert!(matches!(
            read_message(&mut NoFds(&header[..]), Direction::ToApplication),
            Err(ProtocolError::Truncated)
        ));
        // A command announced above 1 MiB is a violation for the helper's reader.
        let header = Header {
            kind: kind::COMMAND,
            page: 1,
            len: 0xFFFF_FFFF,
        }
        .encode();
        assert!(matches!(
            read_message(&mut NoFds(&header[..]), Direction::ToHelper),
            Err(ProtocolError::Oversized { .. })
        ));
    }

    #[test]
    fn the_startup_cap_limits_the_first_message() {
        let header = Header {
            kind: kind::READY,
            page: 0,
            len: 16385,
        }
        .encode();
        assert!(matches!(
            read_message_limited(
                &mut NoFds(&header[..]),
                Direction::ToApplication,
                Some(MAX_STARTUP_PAYLOAD)
            ),
            Err(ProtocolError::Oversized { .. })
        ));
    }

    #[test]
    fn non_object_payloads_are_ignored_and_unknown_members_too() {
        let message = Message::new(kind::STATE, 1, b"[1,2]".to_vec());
        assert!(message.json_object().is_none());
        let message = Message::new(kind::STATE, 1, b"not json".to_vec());
        assert!(message.json_object().is_none());
        let message = Message::new(
            kind::STATE,
            1,
            br#"{"loading":true,"future":{"a":1}}"#.to_vec(),
        );
        let object = message.json_object().unwrap();
        assert_eq!(object["loading"], Value::Bool(true));
    }

    #[test]
    fn ready_versions_and_defaults() {
        assert_eq!(
            Ready::parse(b"").unwrap(),
            Ready {
                protocol: 1,
                engine: String::new(),
                transfers: vec![Transfer::Inline],
                capabilities: vec![]
            }
        );
        assert_eq!(
            Ready::parse(br#"{"protocol":2}"#).unwrap_err(),
            "Browser helper protocol version 2 is not supported"
        );
        let ready = Ready::parse(
            br#"{"protocol":1,"engine":"WebKitGTK 2.54","transfers":["shm","bogus","inline"],"capabilities":["ping","eval","later"],"extra":1}"#,
        )
        .unwrap();
        assert_eq!(
            ready.transfers,
            vec![Transfer::SharedMemory, Transfer::Inline]
        );
        assert!(ready.has_capability("ping") && ready.has_capability("eval"));
        assert!(!ready.has_capability("dmabuf"));
        assert_eq!(
            Ready::parse(b"[1]").unwrap_err(),
            "Invalid browser startup response"
        );
        // "inline" is mandatory even when the helper forgets it.
        let ready = Ready::parse(br#"{"transfers":["shm"]}"#).unwrap();
        assert!(ready.transfers.contains(&Transfer::Inline));
    }

    #[test]
    fn inline_frame_validation_discards_bad_frames() {
        let pixels = vec![0u8; 2 * 2 * 4];
        let ok = encode_inline_frame(2, 2, 1.0, &pixels);
        assert!(parse_inline_frame(&ok).is_some());
        assert!(parse_inline_frame(&ok[..11]).is_none());
        assert!(parse_inline_frame(&encode_inline_frame(2, 2, f32::NAN, &pixels)).is_none());
        assert!(parse_inline_frame(&encode_inline_frame(2, 2, 0.49, &pixels)).is_none());
        assert!(parse_inline_frame(&encode_inline_frame(2, 2, 4.01, &pixels)).is_none());
        assert!(parse_inline_frame(&encode_inline_frame(2, 2, 0.5, &pixels)).is_some());
        assert!(parse_inline_frame(&encode_inline_frame(2, 2, 4.0, &pixels)).is_some());
        assert!(parse_inline_frame(&encode_inline_frame(2, 3, 1.0, &pixels)).is_none());
        assert!(parse_inline_frame(&encode_inline_frame(0, 0, 1.0, &[])).is_none());
    }

    #[test]
    fn commands_encode_their_members() {
        let value = Command::Resize {
            width: 960,
            height: 540,
            scale: 1.5,
        }
        .to_json();
        assert_eq!(
            value,
            json!({"cmd":"resize","width":960,"height":540,"scale":1.5})
        );
        assert_eq!(Command::Option(3).to_json(), json!({"cmd":"option:3"}));
        assert_eq!(
            Command::Visible(true).to_json(),
            json!({"cmd":"visible","value":1})
        );
        let bytes = Command::Ping { nonce: 9 }.encode(0).unwrap();
        let messages = decode_all(&bytes, Direction::ToHelper);
        let object = messages[0].json_object().unwrap();
        assert_eq!(object["cmd"], "ping");
        assert_eq!(object["nonce"], 9);
    }

    #[test]
    fn descriptors_travel_with_their_message() {
        use std::io::Write;
        let (a, b) = UnixStream::pair().unwrap();
        let (mut keep_read, mut keep_write) = {
            let (r, w) = UnixStream::pair().unwrap();
            (r, w)
        };
        keep_write.write_all(b"hello").unwrap();
        let first = encode_message(Direction::ToApplication, kind::POOL, 5, b"{}").unwrap();
        send_with_fds(a.as_raw_fd(), &first, &[keep_read.as_raw_fd()]).unwrap();
        write_all(
            a.as_raw_fd(),
            &encode_message(Direction::ToApplication, kind::PONG, 0, b"{}").unwrap(),
        )
        .unwrap();
        let mut source = &b;
        let pool = read_message(&mut source, Direction::ToApplication).unwrap();
        assert_eq!(pool.fds.len(), 1);
        let pong = read_message(&mut source, Direction::ToApplication).unwrap();
        assert!(pong.fds.is_empty());
        let mut received = UnixStream::from(pool.fds.into_iter().next().unwrap());
        let mut buf = [0u8; 5];
        received.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        let _ = &mut keep_read;
    }
}
