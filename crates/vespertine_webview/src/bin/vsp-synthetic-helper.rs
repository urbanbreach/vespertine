//! A scripted synthetic web view helper that speaks exactly the protocol of PLT-015 without
//! any engine. Tests configure it through environment variables:
//!
//! * `VSP_SYNTH_LOG=<file>`: every command received is appended as one JSON line
//!   `{"page":N,"msg":{...}}`; start-up is logged as `{"started":<pid>}`.
//! * `VSP_SYNTH_GATE=<file>`: the ready message is withheld until the file exists.
//! * `VSP_SYNTH_FAIL=<file>`: while the file exists the helper answers start-up with the error
//!   message "Synthetic storage initialization failure" and exits with status 1.
//! * `VSP_SYNTH_MODE=<mode>`: `ready` (default: ping and eval capable, inline and shm),
//!   `empty-ready`, `raw:<hex>` (send these bytes as the first message), `silent` (send nothing),
//!   `ignore-eof` (never exit on end-of-file), `no-pong` (ignore pings), `exit-after-ready`.
//! * `VSP_SYNTH_FRAME=<rrggbb>`: answer every resize with an inline frame of that colour.
//!
//! The helper answers `load` with the state messages a real helper sends and `eval` with `J`.

use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use vespertine_webview::protocol::{
    Direction, ProtocolError, encode_inline_frame, encode_message, kind, read_message, write_all,
};

fn log_line(value: &Value) {
    if let Some(path) = std::env::var_os("VSP_SYNTH_LOG")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        // One write per line: helpers of several pages append to the same log, and `writeln!`
        // issues the text and the newline separately, so lines of two helpers could merge.
        let _ = file.write_all(format!("{value}\n").as_bytes());
    }
}

fn send(socket: &UnixStream, kind: u8, page: u32, payload: &[u8]) {
    if let Ok(bytes) = encode_message(Direction::ToApplication, kind, page, payload) {
        let _ = write_all(socket.as_raw_fd(), &bytes);
    }
}

fn send_json(socket: &UnixStream, kind: u8, page: u32, value: Value) {
    send(socket, kind, page, value.to_string().as_bytes());
}

fn main() {
    let mut channel = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--channel" {
            channel = arguments.next().and_then(|value| value.parse::<i32>().ok());
        }
    }
    let Some(fd) = channel else {
        eprintln!("vsp-synthetic-helper: --channel <fd> is required");
        std::process::exit(2);
    };
    // SAFETY: the parent passed this descriptor as the helper's end of the channel.
    let socket = unsafe { UnixStream::from_raw_fd(fd) };
    log_line(&json!({"started": std::process::id()}));

    if let Some(gate) = std::env::var_os("VSP_SYNTH_GATE") {
        while !std::path::Path::new(&gate).exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    if let Some(fail) = std::env::var_os("VSP_SYNTH_FAIL")
        && std::path::Path::new(&fail).exists()
    {
        send(
            &socket,
            kind::ERROR,
            0,
            b"Synthetic storage initialization failure",
        );
        std::process::exit(1);
    }
    let mode = std::env::var("VSP_SYNTH_MODE").unwrap_or_else(|_| "ready".into());
    match mode.as_str() {
        "silent" => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        "empty-ready" => send(&socket, kind::READY, 0, b""),
        raw if raw.starts_with("raw:") => {
            let hex = &raw[4..];
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
                .collect();
            let _ = write_all(socket.as_raw_fd(), &bytes);
            // Keep the channel open so the application must judge the bytes it received.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        _ => send_json(
            &socket,
            kind::READY,
            0,
            json!({
                "protocol": 1,
                "engine": "synthetic",
                "transfers": ["inline", "shm"],
                "capabilities": ["ping", "eval"],
            }),
        ),
    }
    if mode == "exit-after-ready" {
        std::process::exit(0);
    }

    let frame_color = std::env::var("VSP_SYNTH_FRAME")
        .ok()
        .and_then(|value| u32::from_str_radix(&value, 16).ok());
    let mut pages: HashSet<u32> = HashSet::new();
    let mut sizes: std::collections::HashMap<u32, (u32, u32, f32)> = Default::default();
    let mut reader = &socket;
    loop {
        let message = match read_message(&mut reader, Direction::ToHelper) {
            Ok(message) => message,
            Err(ProtocolError::Eof) | Err(ProtocolError::Truncated) | Err(ProtocolError::Io(_)) => {
                if mode == "ignore-eof" {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    }
                }
                std::process::exit(0);
            }
            Err(ProtocolError::Oversized { .. }) => std::process::exit(3),
        };
        let Some(command) = message.json_object() else {
            continue;
        };
        let command = Value::Object(command);
        log_line(&json!({"page": message.page, "msg": command}));
        let name = command["cmd"].as_str().unwrap_or("").to_owned();
        let page = message.page;
        if page != 0 && name != "create" && !pages.contains(&page) {
            continue;
        }
        match name.as_str() {
            "ping" if mode != "no-pong" => {
                send_json(&socket, kind::PONG, 0, json!({"nonce": command["nonce"]}));
            }
            "shutdown" => std::process::exit(0),
            "create" => {
                if pages.insert(page) {
                    sizes.insert(page, (800, 600, 1.0));
                }
            }
            "close" => {
                pages.remove(&page);
            }
            "load" => {
                let url = command["url"].as_str().unwrap_or("").to_owned();
                send_json(
                    &socket,
                    kind::STATE,
                    page,
                    json!({"url": url, "title": "", "error": null, "loading": true,
                           "can_back": false, "can_forward": false}),
                );
                send_json(
                    &socket,
                    kind::STATE,
                    page,
                    json!({"url": url, "title": format!("Synthetic {url}"), "error": null,
                           "loading": false, "can_back": true, "can_forward": false}),
                );
            }
            "resize" => {
                let width = command["width"].as_u64().unwrap_or(1).clamp(1, 8192) as u32;
                let height = command["height"].as_u64().unwrap_or(1).clamp(1, 8192) as u32;
                let scale = command["scale"].as_f64().unwrap_or(1.0).clamp(0.5, 4.0) as f32;
                sizes.insert(page, (width, height, scale));
                if let Some(color) = frame_color {
                    let pixel = [color as u8, (color >> 8) as u8, (color >> 16) as u8, 255];
                    let pixels: Vec<u8> = pixel
                        .iter()
                        .copied()
                        .cycle()
                        .take(width as usize * height as usize * 4)
                        .collect();
                    send(
                        &socket,
                        kind::FRAME,
                        page,
                        &encode_inline_frame(width, height, scale, &pixels),
                    );
                }
            }
            "eval" => {
                send(&socket, kind::EVAL_RESULT, page, b"null");
            }
            _ => {}
        }
    }
}
