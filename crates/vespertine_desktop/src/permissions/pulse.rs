//! A minimal client of the PulseAudio native protocol, enough to decide whether
//! a microphone is available (PLT-013 R5).
//!
//! PulseAudio and PipeWire (through its PulseAudio-compatible server) both
//! answer on the native unix socket (`$PULSE_SERVER`, else
//! `$XDG_RUNTIME_DIR/pulse/native`). The probe authenticates, names itself,
//! reads the server info (default source) and the source list, and classifies
//! them. It is read-only: it creates no stream and shows no prompt. Remote
//! (`tcp:`) servers are not probed. The probe is blocking; callers run it on a
//! thread pool ([`probe_microphone_devices`]).

use super::{MicrophoneDevices, PermissionError};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

const PROTOCOL_VERSION: u32 = 32;
const COMMAND_ERROR: u32 = 0;
const COMMAND_REPLY: u32 = 2;
const COMMAND_AUTH: u32 = 8;
const COMMAND_SET_CLIENT_NAME: u32 = 9;
const COMMAND_GET_SERVER_INFO: u32 = 20;
const COMMAND_GET_SOURCE_INFO_LIST: u32 = 24;
const INVALID_INDEX: u32 = u32::MAX;
const CONTROL_CHANNEL: u32 = u32::MAX;
const MAX_PACKET: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const PORT_AVAILABLE_NO: u32 = 1;

const TAG_STRING: u8 = b't';
const TAG_STRING_NULL: u8 = b'N';
const TAG_U32: u8 = b'L';
const TAG_U8: u8 = b'B';
const TAG_ARBITRARY: u8 = b'x';
const TAG_BOOL_TRUE: u8 = b'1';
const TAG_BOOL_FALSE: u8 = b'0';
const TAG_SAMPLE_SPEC: u8 = b'a';
const TAG_CHANNEL_MAP: u8 = b'm';
const TAG_CVOLUME: u8 = b'v';
const TAG_VOLUME: u8 = b'V';
const TAG_USEC: u8 = b'U';
const TAG_PROPLIST: u8 = b'P';
const TAG_FORMAT_INFO: u8 = b'f';

/// A source reported by the server, reduced to what the probe needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Source name.
    pub name: String,
    /// Whether the source monitors a sink (not a capture device).
    pub monitor: bool,
    /// Whether the source has ports and none of them is available.
    pub disabled: bool,
}

/// Classifies sources and the default source name:
/// no non-monitor source is [`NoCaptureSource`](MicrophoneDevices::NoCaptureSource);
/// capture sources that are all disabled, or a default source that is not one of
/// the enabled capture sources, is
/// [`NoDefaultSource`](MicrophoneDevices::NoDefaultSource); otherwise
/// [`Ready`](MicrophoneDevices::Ready).
pub fn classify_sources(sources: &[Source], default_source: Option<&str>) -> MicrophoneDevices {
    let captures: Vec<&Source> = sources.iter().filter(|source| !source.monitor).collect();
    if captures.is_empty() {
        return MicrophoneDevices::NoCaptureSource;
    }
    let default_enabled = default_source.is_some_and(|default| {
        captures
            .iter()
            .any(|source| !source.disabled && source.name == default)
    });
    if default_enabled {
        MicrophoneDevices::Ready
    } else {
        MicrophoneDevices::NoDefaultSource
    }
}

/// The native socket path from the environment: `PULSE_SERVER` when it names a
/// unix socket, else `$XDG_RUNTIME_DIR/pulse/native`. `None` when the server
/// is remote or no runtime directory exists.
pub fn socket_path(pulse_server: Option<&str>, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(server) = pulse_server.filter(|server| !server.is_empty()) {
        // The first entry of a space-separated list; an optional `{machine-id}`
        // prefix and `unix:` scheme are stripped.
        let first = server.split_whitespace().next()?;
        let first = match first.strip_prefix('{') {
            Some(rest) => rest.split_once('}').map_or(rest, |(_, tail)| tail),
            None => first,
        };
        if let Some(path) = first.strip_prefix("unix:") {
            return Some(PathBuf::from(path));
        }
        if first.starts_with('/') {
            return Some(PathBuf::from(first));
        }
        return None;
    }
    Some(runtime_dir?.join("pulse").join("native"))
}

/// Probes the sound server off-thread with the environment's socket.
pub async fn probe_microphone_devices() -> Result<MicrophoneDevices, PermissionError> {
    let pulse_server = std::env::var("PULSE_SERVER").ok();
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let Some(path) = socket_path(pulse_server.as_deref(), runtime_dir.as_deref()) else {
        return Ok(MicrophoneDevices::NoSoundServer);
    };
    smol::unblock(move || probe_socket(&path)).await
}

/// Probes the sound server listening on `path` (blocking).
pub fn probe_socket(path: &Path) -> Result<MicrophoneDevices, PermissionError> {
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(MicrophoneDevices::NoSoundServer);
        }
        Err(error) => return Err(PermissionError::Failed(error.to_string())),
    };
    probe_stream(&mut stream).map_err(|error| PermissionError::Failed(error.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProtocolError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("sound server error code {0}")]
    Server(u32),
    #[error("malformed sound server reply: {0}")]
    Malformed(&'static str),
    #[error("sound server speaks protocol version {0}, which is too old")]
    OldServer(u32),
}

type Result<T, E = ProtocolError> = std::result::Result<T, E>;

fn probe_stream(stream: &mut UnixStream) -> Result<MicrophoneDevices> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut tag = 0u32;
    let mut next_tag = || {
        tag += 1;
        tag
    };

    let mut auth = TagWriter::default();
    auth.u32(PROTOCOL_VERSION);
    auth.arbitrary(&read_cookie());
    let reply = transact(stream, COMMAND_AUTH, next_tag(), auth)?;
    let version = TagReader::new(&reply).u32()? & 0xFFFF;
    if version < 16 {
        return Err(ProtocolError::OldServer(version));
    }
    let version = version.min(PROTOCOL_VERSION);

    let mut name = TagWriter::default();
    name.proplist(&[("application.name", "vespertine permission probe")]);
    transact(stream, COMMAND_SET_CLIENT_NAME, next_tag(), name)?;

    let reply = transact(
        stream,
        COMMAND_GET_SERVER_INFO,
        next_tag(),
        TagWriter::default(),
    )?;
    let mut info = TagReader::new(&reply);
    for _ in 0..4 {
        info.string()?; // server name, version, user, host
    }
    info.sample_spec()?;
    info.string()?; // default sink
    let default_source = info.string()?;

    let reply = transact(
        stream,
        COMMAND_GET_SOURCE_INFO_LIST,
        next_tag(),
        TagWriter::default(),
    )?;
    let sources = parse_source_list(&reply, version)?;
    Ok(classify_sources(&sources, default_source.as_deref()))
}

fn read_cookie() -> Vec<u8> {
    let path = std::env::var_os("PULSE_COOKIE")
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })?;
            Some(home.join("pulse").join("cookie"))
        });
    path.and_then(|path| std::fs::read(path).ok())
        .filter(|cookie| cookie.len() == 256)
        .unwrap_or_else(|| vec![0; 256])
}

fn parse_source_list(payload: &[u8], version: u32) -> Result<Vec<Source>> {
    let mut reader = TagReader::new(payload);
    let mut sources = Vec::new();
    while !reader.at_end() {
        reader.u32()?; // index
        let name = reader.string()?.unwrap_or_default();
        reader.string()?; // description
        reader.sample_spec()?;
        reader.channel_map()?;
        reader.u32()?; // owner module
        reader.cvolume()?;
        reader.bool()?; // mute
        let monitor_of_sink = reader.u32()?;
        reader.string()?; // monitor of sink name
        reader.usec()?; // latency
        reader.string()?; // driver
        reader.u32()?; // flags
        if version >= 13 {
            reader.proplist()?;
        }
        if version >= 15 {
            reader.usec()?; // configured latency
            reader.volume()?; // base volume
            reader.u32()?; // state
            reader.u32()?; // volume steps
            reader.u32()?; // card
        }
        let mut disabled = false;
        if version >= 16 {
            let port_count = reader.u32()?;
            let mut available_ports = 0usize;
            for _ in 0..port_count {
                reader.string()?; // name
                reader.string()?; // description
                reader.u32()?; // priority
                let available = if version >= 24 { reader.u32()? } else { 0 };
                if available != PORT_AVAILABLE_NO {
                    available_ports += 1;
                }
            }
            reader.string()?; // active port
            disabled = port_count > 0 && available_ports == 0;
        }
        if version >= 21 {
            let formats = reader.u8()?;
            for _ in 0..formats {
                reader.format_info()?;
            }
        }
        sources.push(Source {
            name,
            monitor: monitor_of_sink != INVALID_INDEX,
            disabled,
        });
    }
    Ok(sources)
}

/// Sends one command and returns the reply body after the reply header.
fn transact(
    stream: &mut (impl Read + Write),
    command: u32,
    tag: u32,
    body: TagWriter,
) -> Result<Vec<u8>> {
    let mut payload = TagWriter::default();
    payload.u32(command);
    payload.u32(tag);
    payload.0.extend_from_slice(&body.0);
    let mut packet = Vec::with_capacity(20 + payload.0.len());
    packet.extend_from_slice(&(payload.0.len() as u32).to_be_bytes());
    packet.extend_from_slice(&CONTROL_CHANNEL.to_be_bytes());
    packet.extend_from_slice(&[0; 12]);
    packet.extend_from_slice(&payload.0);
    stream.write_all(&packet)?;
    stream.flush()?;

    loop {
        let mut descriptor = [0u8; 20];
        stream.read_exact(&mut descriptor)?;
        let length = u32::from_be_bytes(descriptor[0..4].try_into().unwrap()) as usize;
        let channel = u32::from_be_bytes(descriptor[4..8].try_into().unwrap());
        if length > MAX_PACKET {
            return Err(ProtocolError::Malformed("packet too large"));
        }
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body)?;
        if channel != CONTROL_CHANNEL {
            continue;
        }
        let mut reader = TagReader::new(&body);
        let reply_command = reader.u32()?;
        let reply_tag = reader.u32()?;
        match reply_command {
            COMMAND_REPLY if reply_tag == tag => return Ok(body[reader.pos..].to_vec()),
            COMMAND_ERROR if reply_tag == tag => return Err(ProtocolError::Server(reader.u32()?)),
            _ => continue,
        }
    }
}

/// Encoder for the protocol's tagged structures.
#[derive(Default)]
pub(crate) struct TagWriter(pub(crate) Vec<u8>);

impl TagWriter {
    pub(crate) fn u32(&mut self, value: u32) {
        self.0.push(TAG_U32);
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    #[cfg(test)]
    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(TAG_U8);
        self.0.push(value);
    }

    pub(crate) fn string(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.0.push(TAG_STRING);
                self.0.extend_from_slice(value.as_bytes());
                self.0.push(0);
            }
            None => self.0.push(TAG_STRING_NULL),
        }
    }

    pub(crate) fn arbitrary(&mut self, value: &[u8]) {
        self.0.push(TAG_ARBITRARY);
        self.0
            .extend_from_slice(&(value.len() as u32).to_be_bytes());
        self.0.extend_from_slice(value);
    }

    #[cfg(test)]
    pub(crate) fn bool(&mut self, value: bool) {
        self.0
            .push(if value { TAG_BOOL_TRUE } else { TAG_BOOL_FALSE });
    }

    #[cfg(test)]
    pub(crate) fn sample_spec(&mut self) {
        self.0.push(TAG_SAMPLE_SPEC);
        self.0.extend_from_slice(&[3, 1, 0, 0, 0xAC, 0x44]); // s16le mono 44100
    }

    #[cfg(test)]
    pub(crate) fn channel_map(&mut self) {
        self.0.extend_from_slice(&[TAG_CHANNEL_MAP, 1, 0]);
    }

    #[cfg(test)]
    pub(crate) fn cvolume(&mut self) {
        self.0.extend_from_slice(&[TAG_CVOLUME, 1]);
        self.0.extend_from_slice(&0x10000u32.to_be_bytes());
    }

    #[cfg(test)]
    pub(crate) fn usec(&mut self, value: u64) {
        self.0.push(TAG_USEC);
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    #[cfg(test)]
    pub(crate) fn volume(&mut self, value: u32) {
        self.0.push(TAG_VOLUME);
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn proplist(&mut self, entries: &[(&str, &str)]) {
        self.0.push(TAG_PROPLIST);
        for (key, value) in entries {
            self.string(Some(key));
            self.u32(value.len() as u32 + 1);
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            self.arbitrary(&bytes);
        }
        self.string(None);
    }

    #[cfg(test)]
    pub(crate) fn format_info(&mut self) {
        self.0.push(TAG_FORMAT_INFO);
        self.u8(1);
        self.proplist(&[]);
    }
}

/// Decoder for the protocol's tagged structures.
pub(crate) struct TagReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> TagReader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.buf.len())
            .ok_or(ProtocolError::Malformed("truncated"))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn tag(&mut self, expected: u8) -> Result<()> {
        if self.take(1)?[0] == expected {
            Ok(())
        } else {
            Err(ProtocolError::Malformed("unexpected tag"))
        }
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        self.tag(TAG_U32)?;
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u8(&mut self) -> Result<u8> {
        self.tag(TAG_U8)?;
        Ok(self.take(1)?[0])
    }

    pub(crate) fn string(&mut self) -> Result<Option<String>> {
        match self.take(1)?[0] {
            TAG_STRING_NULL => Ok(None),
            TAG_STRING => {
                let rest = &self.buf[self.pos..];
                let end = rest
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or(ProtocolError::Malformed("unterminated string"))?;
                let value = String::from_utf8_lossy(&rest[..end]).into_owned();
                self.pos += end + 1;
                Ok(Some(value))
            }
            _ => Err(ProtocolError::Malformed("unexpected tag")),
        }
    }

    fn arbitrary(&mut self) -> Result<&'a [u8]> {
        self.tag(TAG_ARBITRARY)?;
        let length = u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize;
        self.take(length)
    }

    fn bool(&mut self) -> Result<bool> {
        match self.take(1)?[0] {
            TAG_BOOL_TRUE => Ok(true),
            TAG_BOOL_FALSE => Ok(false),
            _ => Err(ProtocolError::Malformed("unexpected tag")),
        }
    }

    fn sample_spec(&mut self) -> Result<()> {
        self.tag(TAG_SAMPLE_SPEC)?;
        self.take(6)?;
        Ok(())
    }

    fn channel_map(&mut self) -> Result<()> {
        self.tag(TAG_CHANNEL_MAP)?;
        let channels = self.take(1)?[0] as usize;
        self.take(channels)?;
        Ok(())
    }

    fn cvolume(&mut self) -> Result<()> {
        self.tag(TAG_CVOLUME)?;
        let channels = self.take(1)?[0] as usize;
        self.take(channels * 4)?;
        Ok(())
    }

    fn usec(&mut self) -> Result<()> {
        self.tag(TAG_USEC)?;
        self.take(8)?;
        Ok(())
    }

    fn volume(&mut self) -> Result<()> {
        self.tag(TAG_VOLUME)?;
        self.take(4)?;
        Ok(())
    }

    fn proplist(&mut self) -> Result<()> {
        self.tag(TAG_PROPLIST)?;
        while self.string()?.is_some() {
            self.u32()?; // value length
            self.arbitrary()?;
        }
        Ok(())
    }

    fn format_info(&mut self) -> Result<()> {
        self.tag(TAG_FORMAT_INFO)?;
        self.u8()?; // encoding
        self.proplist()
    }
}

/// A scripted PulseAudio server for tests, over a unix socket.
#[cfg(test)]
pub(crate) mod scripted_server {
    use super::*;
    use std::os::unix::net::UnixListener;

    pub(crate) struct ScriptedSource {
        pub name: &'static str,
        pub monitor: bool,
        /// Availability of each port (1 = no, 2 = yes).
        pub ports: Vec<u32>,
    }

    pub(crate) struct ScriptedServer {
        pub default_source: Option<&'static str>,
        pub sources: Vec<ScriptedSource>,
        pub version: u32,
    }

    fn reply(stream: &mut UnixStream, tag: u32, body: TagWriter) {
        let mut payload = TagWriter::default();
        payload.u32(COMMAND_REPLY);
        payload.u32(tag);
        payload.0.extend_from_slice(&body.0);
        let mut packet = Vec::new();
        packet.extend_from_slice(&(payload.0.len() as u32).to_be_bytes());
        packet.extend_from_slice(&CONTROL_CHANNEL.to_be_bytes());
        packet.extend_from_slice(&[0; 12]);
        packet.extend_from_slice(&payload.0);
        stream.write_all(&packet).unwrap();
    }

    impl ScriptedServer {
        /// Serves one client on a fresh socket under `dir`; returns the path.
        pub(crate) fn spawn(self, dir: &Path) -> PathBuf {
            let path = dir.join("native");
            let listener = UnixListener::bind(&path).unwrap();
            std::thread::spawn(move || {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                loop {
                    let mut descriptor = [0u8; 20];
                    if stream.read_exact(&mut descriptor).is_err() {
                        return;
                    }
                    let length = u32::from_be_bytes(descriptor[0..4].try_into().unwrap()) as usize;
                    let mut body = vec![0u8; length];
                    stream.read_exact(&mut body).unwrap();
                    let mut reader = TagReader::new(&body);
                    let command = reader.u32().unwrap();
                    let tag = reader.u32().unwrap();
                    let mut out = TagWriter::default();
                    match command {
                        COMMAND_AUTH => out.u32(self.version),
                        COMMAND_SET_CLIENT_NAME => out.u32(1),
                        COMMAND_GET_SERVER_INFO => {
                            for text in ["server", "1.0", "user", "host"] {
                                out.string(Some(text));
                            }
                            out.sample_spec();
                            out.string(Some("sink"));
                            out.string(self.default_source);
                            out.u32(0);
                        }
                        COMMAND_GET_SOURCE_INFO_LIST => {
                            for (index, source) in self.sources.iter().enumerate() {
                                out.u32(index as u32);
                                out.string(Some(source.name));
                                out.string(Some("description"));
                                out.sample_spec();
                                out.channel_map();
                                out.u32(INVALID_INDEX);
                                out.cvolume();
                                out.bool(false);
                                out.u32(if source.monitor { 0 } else { INVALID_INDEX });
                                out.string(None);
                                out.usec(0);
                                out.string(Some("driver"));
                                out.u32(0);
                                let v = self.version.min(PROTOCOL_VERSION);
                                if v >= 13 {
                                    out.proplist(&[("device.class", "sound")]);
                                }
                                if v >= 15 {
                                    out.usec(0);
                                    out.volume(0x10000);
                                    out.u32(0);
                                    out.u32(0);
                                    out.u32(INVALID_INDEX);
                                }
                                if v >= 16 {
                                    out.u32(source.ports.len() as u32);
                                    for available in &source.ports {
                                        out.string(Some("port"));
                                        out.string(Some("port description"));
                                        out.u32(100);
                                        if v >= 24 {
                                            out.u32(*available);
                                        }
                                    }
                                    out.string(None);
                                }
                                if v >= 21 {
                                    out.u8(1);
                                    out.format_info();
                                }
                            }
                        }
                        _ => {
                            let mut error = TagWriter::default();
                            error.u32(COMMAND_ERROR);
                            error.u32(tag);
                            error.u32(7);
                            let mut packet = Vec::new();
                            packet.extend_from_slice(&(error.0.len() as u32).to_be_bytes());
                            packet.extend_from_slice(&CONTROL_CHANNEL.to_be_bytes());
                            packet.extend_from_slice(&[0; 12]);
                            packet.extend_from_slice(&error.0);
                            stream.write_all(&packet).unwrap();
                            continue;
                        }
                    }
                    reply(&mut stream, tag, out);
                }
            });
            path
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{scripted_server::*, *};

    fn source(name: &str, monitor: bool, disabled: bool) -> Source {
        Source {
            name: name.into(),
            monitor,
            disabled,
        }
    }

    #[test]
    fn classification_maps_to_microphone_devices() {
        assert_eq!(
            classify_sources(&[], None),
            MicrophoneDevices::NoCaptureSource
        );
        assert_eq!(
            classify_sources(&[source("out.monitor", true, false)], Some("out.monitor")),
            MicrophoneDevices::NoCaptureSource
        );
        assert_eq!(
            classify_sources(&[source("mic", false, false)], None),
            MicrophoneDevices::NoDefaultSource
        );
        assert_eq!(
            classify_sources(
                &[
                    source("mic", false, false),
                    source("out.monitor", true, false)
                ],
                Some("out.monitor")
            ),
            MicrophoneDevices::NoDefaultSource
        );
        assert_eq!(
            classify_sources(&[source("mic", false, true)], Some("mic")),
            MicrophoneDevices::NoDefaultSource
        );
        assert_eq!(
            classify_sources(&[source("mic", false, false)], Some("mic")),
            MicrophoneDevices::Ready
        );
    }

    #[test]
    fn socket_path_follows_the_environment() {
        let runtime = Path::new("/run/user/1000");
        assert_eq!(
            socket_path(None, Some(runtime)),
            Some(PathBuf::from("/run/user/1000/pulse/native"))
        );
        assert_eq!(
            socket_path(Some("unix:/tmp/pa"), Some(runtime)),
            Some(PathBuf::from("/tmp/pa"))
        );
        assert_eq!(
            socket_path(Some("{abc}unix:/tmp/pa other"), None),
            Some(PathBuf::from("/tmp/pa"))
        );
        assert_eq!(
            socket_path(Some("/tmp/pa"), None),
            Some(PathBuf::from("/tmp/pa"))
        );
        assert_eq!(socket_path(Some("tcp:host:4713"), Some(runtime)), None);
        assert_eq!(socket_path(None, None), None);
        assert_eq!(
            socket_path(Some(""), Some(runtime)),
            Some(PathBuf::from("/run/user/1000/pulse/native"))
        );
    }

    #[test]
    fn missing_socket_means_no_sound_server() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            probe_socket(&dir.path().join("absent")),
            Ok(MicrophoneDevices::NoSoundServer)
        );
    }

    fn probe(server: ScriptedServer) -> Result<MicrophoneDevices, PermissionError> {
        let dir = tempfile::tempdir().unwrap();
        let path = server.spawn(dir.path());
        probe_socket(&path)
    }

    fn scripted(
        default: Option<&'static str>,
        sources: Vec<ScriptedSource>,
        version: u32,
    ) -> ScriptedServer {
        ScriptedServer {
            default_source: default,
            sources,
            version,
        }
    }

    fn mic(name: &'static str, ports: Vec<u32>) -> ScriptedSource {
        ScriptedSource {
            name,
            monitor: false,
            ports,
        }
    }

    #[test]
    fn probe_speaks_the_native_protocol() {
        let monitor = ScriptedSource {
            name: "speaker.monitor",
            monitor: true,
            ports: vec![],
        };
        for version in [32, 35, 24, 21, 16] {
            assert_eq!(
                probe(scripted(
                    Some("mic"),
                    vec![monitor_clone(&monitor), mic("mic", vec![2])],
                    version
                )),
                Ok(MicrophoneDevices::Ready),
                "protocol {version}"
            );
        }
        assert_eq!(
            probe(scripted(Some("mic"), vec![monitor_clone(&monitor)], 35)),
            Ok(MicrophoneDevices::NoCaptureSource)
        );
        assert_eq!(
            probe(scripted(None, vec![mic("mic", vec![])], 35)),
            Ok(MicrophoneDevices::NoDefaultSource)
        );
        // The default capture source has only unavailable ports: disabled.
        assert_eq!(
            probe(scripted(Some("mic"), vec![mic("mic", vec![1, 1])], 35)),
            Ok(MicrophoneDevices::NoDefaultSource)
        );
        // A source with at least one available port is enabled.
        assert_eq!(
            probe(scripted(Some("mic"), vec![mic("mic", vec![1, 2])], 35)),
            Ok(MicrophoneDevices::Ready)
        );
    }

    fn monitor_clone(source: &ScriptedSource) -> ScriptedSource {
        ScriptedSource {
            name: source.name,
            monitor: source.monitor,
            ports: source.ports.clone(),
        }
    }

    #[test]
    fn ancient_servers_are_reported_as_failures() {
        let result = probe(scripted(Some("mic"), vec![mic("mic", vec![])], 12));
        assert!(
            matches!(result, Err(PermissionError::Failed(_))),
            "{result:?}"
        );
    }
}
