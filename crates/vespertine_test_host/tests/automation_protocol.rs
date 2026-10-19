//! DEV-007: protocol, framing, sessions, strict locators and gestures. These tests drive a
//! recording stub host, so they need no GPU; the end-to-end tests on the real offscreen host are
//! in `automation_host.rs`.

use serde_json::{Value, json};
use std::{
    cell::RefCell,
    io::Cursor,
    rc::Rc,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use vespertine_test_host::{
    Rect, TreeNode,
    automation::{
        Automation, AutomationError, AutomationHost, Capability, DragOptions, ErrorCode, Message,
        PROTOCOL_VERSION, Session, SessionOptions,
        host::call,
        protocol::validate_request,
        serve,
        server::respond,
        sse::{SseDecoder, encode_frame},
    },
};

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Move {
        x: f64,
        y: f64,
        pressed: Option<u8>,
        modifiers: String,
    },
    Down {
        x: f64,
        y: f64,
        button: u8,
        modifiers: String,
    },
    Up {
        x: f64,
        y: f64,
        button: u8,
        modifiers: String,
    },
    Click {
        x: f64,
        y: f64,
        button: u8,
        modifiers: String,
    },
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Keys {
        keys: String,
        element: Option<u64>,
    },
    ClockSet(f64),
}

/// A host with a canned tree that records the input it receives. Time passes only when a
/// locator polls (`idle`), and a node can appear at a given elapsed time.
struct Stub {
    capabilities: Vec<Capability>,
    tree: TreeNode,
    /// Bounds answered by id when the tree node carries none.
    bounds_by_id: Vec<(u64, Rect)>,
    calls: Vec<Call>,
    elapsed: f64,
    appear: Option<(f64, TreeNode)>,
    /// The destination node moves by 50 px after every press (a live preview).
    moving_destination: Option<u64>,
    downs: u32,
}

fn node(
    id: u64,
    element_type: &str,
    text: Option<&str>,
    test_id: Option<&str>,
    bounds: Option<Rect>,
    children: Vec<TreeNode>,
) -> TreeNode {
    TreeNode {
        id,
        element_type: element_type.to_owned(),
        text: text.map(str::to_owned),
        test_id: test_id.map(str::to_owned),
        bounds,
        children,
        ..TreeNode::default()
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

impl Stub {
    fn new(tree: TreeNode) -> Self {
        Self {
            capabilities: vec![
                Capability::Input,
                Capability::Screenshot,
                Capability::Clock,
                Capability::Tree,
            ],
            tree,
            bounds_by_id: Vec::new(),
            calls: Vec::new(),
            elapsed: 0.0,
            appear: None,
            moving_destination: None,
            downs: 0,
        }
    }

    fn current_tree(&self) -> TreeNode {
        let mut tree = self.tree.clone();
        if let Some((at, extra)) = &self.appear
            && self.elapsed >= *at
        {
            tree.children.push(extra.clone());
        }
        if let Some(id) = self.moving_destination {
            let shift = 50.0 * f64::from(self.downs);
            tree.walk_mut(&mut |node| {
                if node.id == id
                    && let Some(bounds) = node.bounds.as_mut()
                {
                    bounds.x += shift;
                }
            });
        }
        tree
    }
}

impl AutomationHost for Stub {
    fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }

    fn viewport(&mut self) -> (f64, f64) {
        (640.0, 480.0)
    }

    fn click(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.calls.push(Call::Click {
            x,
            y,
            button,
            modifiers: modifiers.to_owned(),
        });
        Ok(())
    }

    fn mouse_down(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.downs += 1;
        self.calls.push(Call::Down {
            x,
            y,
            button,
            modifiers: modifiers.to_owned(),
        });
        Ok(())
    }

    fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.calls.push(Call::Up {
            x,
            y,
            button,
            modifiers: modifiers.to_owned(),
        });
        Ok(())
    }

    fn mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: Option<u8>,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.calls.push(Call::Move {
            x,
            y,
            pressed,
            modifiers: modifiers.to_owned(),
        });
        Ok(())
    }

    fn scroll_wheel(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        _modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.calls.push(Call::Wheel { x, y, dx, dy });
        Ok(())
    }

    fn keystrokes(&mut self, keys: &str, element: Option<u64>) -> Result<(), AutomationError> {
        self.calls.push(Call::Keys {
            keys: keys.to_owned(),
            element,
        });
        Ok(())
    }

    fn tree(&mut self) -> Result<Option<TreeNode>, AutomationError> {
        Ok(Some(self.current_tree()))
    }

    fn bounds(&mut self, element: u64) -> Result<Option<Rect>, AutomationError> {
        Ok(self
            .bounds_by_id
            .iter()
            .find(|(id, _)| *id == element)
            .map(|(_, rect)| *rect))
    }

    fn idle(&mut self, ms: f64) {
        self.elapsed += ms;
    }

    fn clock_set(&mut self, now_ms: f64) -> Result<f64, AutomationError> {
        self.calls.push(Call::ClockSet(now_ms));
        Ok(now_ms)
    }
}

fn shared(stub: Stub) -> (Rc<RefCell<Stub>>, Automation) {
    let host = Rc::new(RefCell::new(stub));
    let app = Automation::connect_in_process(host.clone()).expect("initialize");
    (host, app)
}

fn sample_tree() -> TreeNode {
    node(
        1,
        "div",
        None,
        Some("root"),
        Some(rect(0.0, 0.0, 640.0, 480.0)),
        vec![
            node(
                2,
                "text",
                Some("Hello"),
                Some("greeting"),
                Some(rect(10.0, 10.0, 100.0, 20.0)),
                vec![],
            ),
            node(
                3,
                "div",
                None,
                Some("wrapper"),
                Some(rect(10.0, 40.0, 200.0, 40.0)),
                vec![
                    node(
                        4,
                        "text",
                        Some("Wor"),
                        None,
                        Some(rect(10.0, 40.0, 30.0, 20.0)),
                        vec![],
                    ),
                    node(
                        5,
                        "text",
                        Some("ld"),
                        None,
                        Some(rect(40.0, 40.0, 20.0, 20.0)),
                        vec![],
                    ),
                ],
            ),
            node(
                6,
                "button",
                Some("Save"),
                Some("item"),
                Some(rect(10.0, 100.0, 80.0, 30.0)),
                vec![],
            ),
            node(
                7,
                "button",
                Some("Save as"),
                Some("item"),
                Some(rect(100.0, 100.0, 80.0, 30.0)),
                vec![],
            ),
            node(8, "text", Some("hidden text"), Some("hidden"), None, vec![]),
        ],
    )
}

// ---------------------------------------------------------------------------------------------
// Framing

fn frames(messages: &[Message]) -> Vec<u8> {
    messages.iter().flat_map(encode_frame).collect()
}

fn pings() -> Vec<Message> {
    (1..=4)
        .map(|id| Message::Request {
            id: json!(id),
            method: "blur".into(),
            params: json!({}),
        })
        .collect()
}

#[test]
fn dev_007_a2_frames_use_data_only_and_noisy_streams_decode() {
    let frame = encode_frame(&pings()[0]);
    assert_eq!(
        String::from_utf8(frame).unwrap(),
        "data: {\"id\":1,\"method\":\"blur\",\"params\":{}}\n\n",
        "one JSON object per data field; no id or event fields"
    );

    let mut stream = Vec::new();
    stream.extend_from_slice(b"booting the app\nid: 12\nevent: message\nretry: 5\n: a comment\n");
    stream.extend_from_slice(&encode_frame(&pings()[0]));
    stream.extend_from_slice(b"id: 99\nevent: log\ndata-not: a field\nplain log line\n");
    stream.extend_from_slice(b"data: this is not json\n\n");
    stream.extend_from_slice(b"data: {\"unrelated\": true}\n\n");
    stream.extend_from_slice(&encode_frame(&pings()[1]));
    stream.extend_from_slice(b"\r\ndata: {\"id\":3,\"method\":\"blur\",\"params\":{}}\r\n\r\n");
    let mut invalid = Vec::new();
    let mut decoder = SseDecoder::new();
    let messages = decoder.feed(&stream, |text| invalid.push(text.to_owned()));
    assert_eq!(messages, pings()[..3].to_vec());
    assert_eq!(invalid, ["this is not json", "{\"unrelated\": true}"]);
}

#[test]
fn dev_007_a2_frames_split_at_arbitrary_chunks_are_reassembled() {
    let mut stream = b"log before\n".to_vec();
    stream.extend_from_slice(&frames(&pings()));
    stream.extend_from_slice(b"id: 1\nevent: x\n");
    let expected = pings();
    for chunk_size in 1..=stream.len().min(97) {
        let mut decoder = SseDecoder::new();
        let mut messages = Vec::new();
        for chunk in stream.chunks(chunk_size) {
            messages.extend(decoder.feed(chunk, |text| panic!("unexpected invalid frame {text}")));
        }
        assert_eq!(messages, expected, "chunk size {chunk_size}");
    }
    // A multi-byte character split across chunks survives.
    let message = Message::Request {
        id: json!(7),
        method: "keystrokes".into(),
        params: json!({"keys": "é ü"}),
    };
    let bytes = encode_frame(&message);
    let mut decoder = SseDecoder::new();
    let mut got = Vec::new();
    for byte in &bytes {
        got.extend(decoder.feed(std::slice::from_ref(byte), |_| panic!("invalid")));
    }
    assert_eq!(got, [message]);
}

// ---------------------------------------------------------------------------------------------
// Validation

#[test]
fn dev_007_a2_validation_failures_are_protocol_errors_with_issues() {
    let (host, _) = shared(Stub::new(sample_tree()));
    let mut host = host.borrow_mut();

    let error = call(&mut *host, "teleport", json!({})).unwrap_err();
    assert_eq!(error.code, ErrorCode::Protocol);
    assert!(error.message.contains("teleport"));

    for (method, params) in [
        ("click", json!({})),
        ("click", json!({"x": "1", "y": 2})),
        ("click", json!({"x": 1, "y": 2, "button": 3})),
        ("click", json!({"x": 1, "y": 2, "bogus": true})),
        ("click", json!([1, 2])),
        ("keystrokes", json!({"keys": 5})),
        ("keyDown", json!({"key": "a", "isHeld": "yes"})),
        ("scrollWheel", json!({"x": 0, "y": 0, "deltaX": 1})),
        ("focus", json!({"elementId": 1.5})),
    ] {
        let error = call(&mut *host, method, params.clone()).unwrap_err();
        assert_eq!(error.code, ErrorCode::Protocol, "{method} {params}");
        let issues = &error.data.as_ref().expect("issues")["issues"];
        assert!(
            issues.as_array().is_some_and(|issues| !issues.is_empty()),
            "{method} {params}"
        );
    }

    // Negative clock values are protocol errors.
    for (method, params) in [
        ("clockSet", json!({"nowMs": -1})),
        ("clockFastForward", json!({"deltaMs": -0.5})),
        ("clockSet", json!({})),
    ] {
        let error = call(&mut *host, method, params).unwrap_err();
        assert_eq!(error.code, ErrorCode::Protocol, "{method}");
    }

    // Protocol version 1 only.
    for version in [0, 2, -1] {
        let error = call(
            &mut *host,
            "initialize",
            json!({"protocolVersion": version}),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Protocol, "version {version}");
    }
    let init = call(
        &mut *host,
        "initialize",
        json!({"protocolVersion": PROTOCOL_VERSION, "client": "t"}),
    )
    .unwrap();
    assert_eq!(init["protocolVersion"], 1);
    assert_eq!(init["pid"], std::process::id());
    assert_eq!(
        init["capabilities"],
        json!(["input", "screenshot", "clock", "tree"])
    );
    assert_eq!(
        (init["width"].as_f64(), init["height"].as_f64()),
        (Some(640.0), Some(480.0))
    );

    // Request envelopes: non-integer ids.
    for id in [json!("7"), json!(1.5), json!(null), json!(true)] {
        let error = validate_request(&id, "blur", &json!({})).unwrap_err();
        assert_eq!(error.code, ErrorCode::Protocol, "{id}");
    }
    let response = respond(&mut *host, &json!("x"), "blur", &json!({}));
    let Message::Response { id, outcome } = response else {
        panic!("not a response")
    };
    assert_eq!(id, Value::Null);
    assert_eq!(outcome.unwrap_err().code, ErrorCode::Protocol);
}

#[test]
fn dev_007_r1_missing_capability_is_unsupported_not_silent() {
    let mut stub = Stub::new(sample_tree());
    stub.capabilities = vec![Capability::Input];
    let (_host, app) = shared(stub);
    assert_eq!(app.info().capabilities, [Capability::Input]);
    assert_eq!(app.tree().unwrap_err().code, ErrorCode::Unsupported);
    assert_eq!(
        app.clock().pause().unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        app.screenshot("/tmp/never.png").unwrap_err().code,
        ErrorCode::Unsupported
    );

    // A host mode that cannot perform an operation it advertises also fails explicitly: the
    // stub has no `blur`, so the default implementation reports Unsupported.
    let (_host, app) = shared(Stub::new(sample_tree()));
    assert_eq!(app.blur().unwrap_err().code, ErrorCode::Unsupported);
}

#[test]
fn dev_007_a2_server_loop_answers_in_order_and_ignores_noise() {
    let mut stub = Stub::new(sample_tree());
    let mut input = b"stray output\nid: 4\n".to_vec();
    input.extend(frames(&[
        Message::Request {
            id: json!(1),
            method: "initialize".into(),
            params: json!({"protocolVersion": 1}),
        },
        Message::Request {
            id: json!(2),
            method: "click".into(),
            params: json!({"x": 3, "y": 4, "modifiers": "ctrl"}),
        },
        Message::Request {
            id: json!(3),
            method: "nonsense".into(),
            params: json!({}),
        },
        Message::Request {
            id: json!("four"),
            method: "blur".into(),
            params: json!({}),
        },
        Message::Request {
            id: json!(5),
            method: "cancel".into(),
            params: json!({"id": 3}),
        },
    ]));
    input.extend_from_slice(b"data: broken\n\n");
    let mut output = Vec::new();
    serve(&mut stub, Cursor::new(input), &mut output).unwrap();
    let mut decoder = SseDecoder::new();
    let responses = decoder.feed(&output, |text| panic!("server emitted a bad frame: {text}"));
    let outcomes: Vec<_> = responses
        .into_iter()
        .map(|message| match message {
            Message::Response { id, outcome } => (id, outcome.map_err(|error| error.code)),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(outcomes.len(), 5);
    assert_eq!(outcomes[0].0, json!(1));
    assert!(
        outcomes[0]
            .1
            .as_ref()
            .is_ok_and(|init| init["capabilities"].is_array())
    );
    assert_eq!(outcomes[1], (json!(2), Ok(json!({"ok": true}))));
    assert_eq!(outcomes[2], (json!(3), Err(ErrorCode::Protocol)));
    assert_eq!(outcomes[3], (Value::Null, Err(ErrorCode::Protocol)));
    assert_eq!(outcomes[4], (json!(5), Ok(json!({"ok": true}))));
    assert_eq!(
        stub.calls,
        [Call::Click {
            x: 3.0,
            y: 4.0,
            button: 0,
            modifiers: "ctrl".into()
        }]
    );
}

// ---------------------------------------------------------------------------------------------
// Sessions

type Wire = Arc<Mutex<Vec<u8>>>;

fn channel_session(options: SessionOptions) -> (Arc<Session>, Wire) {
    let wire: Wire = Arc::default();
    let sink = wire.clone();
    let session = Session::over_channel(
        move |bytes| {
            sink.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        },
        options,
    );
    (Arc::new(session), wire)
}

/// Waits until `count` requests were written and returns their `(id, method)`.
fn written_requests(wire: &Wire, count: usize) -> Vec<(i64, String)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let bytes = wire.lock().unwrap().clone();
        let mut decoder = SseDecoder::new();
        let requests: Vec<_> = decoder
            .feed(&bytes, |_| {})
            .into_iter()
            .filter_map(|message| match message {
                Message::Request { id, method, .. } => Some((id.as_i64().unwrap(), method)),
                _ => None,
            })
            .collect();
        if requests.len() >= count {
            return requests;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} requests, got {requests:?}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn reply(session: &Session, id: i64, result: Value) {
    session.feed(&encode_frame(&Message::Response {
        id: json!(id),
        outcome: Ok(result),
    }));
}

#[test]
fn dev_007_a2_replies_correlate_by_id_when_they_arrive_out_of_order() {
    let (session, wire) = channel_session(SessionOptions::default());
    let first = {
        let session = session.clone();
        thread::spawn(move || session.call("clockSet", json!({"nowMs": 10})))
    };
    let requests = written_requests(&wire, 1);
    let second = {
        let session = session.clone();
        thread::spawn(move || session.call("clockFastForward", json!({"deltaMs": 20})))
    };
    let requests2 = written_requests(&wire, 2);
    let (first_id, second_id) = (requests[0].0, requests2[1].0);
    assert_ne!(first_id, second_id);
    // The second request is answered first.
    reply(&session, second_id, json!({"nowMs": 222}));
    assert_eq!(second.join().unwrap().unwrap(), json!({"nowMs": 222}));
    reply(&session, first_id, json!({"nowMs": 111}));
    assert_eq!(first.join().unwrap().unwrap(), json!({"nowMs": 111}));
    // Replies for unknown ids are ignored.
    reply(&session, 9999, json!({"ok": true}));
}

#[test]
fn dev_007_a2_cancel_completes_the_caller_with_cancelled() {
    let (session, wire) = channel_session(SessionOptions::default());
    let pending = session.begin("clockPause", json!({})).unwrap();
    let id = pending.id();
    assert!(session.cancel(id));
    assert_eq!(pending.wait().unwrap_err().code, ErrorCode::Cancelled);
    assert!(!session.cancel(id), "nothing pending any more");
    // The host was told to cancel the request.
    let requests = written_requests(&wire, 2);
    assert_eq!(requests[1].1, "cancel");
    // A late reply for the cancelled request is ignored and the session keeps working.
    reply(&session, id, json!({"nowMs": 1}));
    let next = {
        let session = session.clone();
        thread::spawn(move || session.call("clockResume", json!({})))
    };
    let requests = written_requests(&wire, 3);
    reply(&session, requests[2].0, json!({"nowMs": 5}));
    assert_eq!(next.join().unwrap().unwrap()["nowMs"], 5);
}

#[test]
fn dev_007_a2_closing_fails_outstanding_and_later_calls_with_closed() {
    let (session, wire) = channel_session(SessionOptions::default());
    let outstanding = {
        let session = session.clone();
        thread::spawn(move || session.call("clockPause", json!({})))
    };
    written_requests(&wire, 1);
    session.close();
    assert_eq!(
        outstanding.join().unwrap().unwrap_err().code,
        ErrorCode::Closed
    );
    assert!(session.is_closed());
    assert_eq!(
        session.call("blur", json!({})).unwrap_err().code,
        ErrorCode::Closed
    );
    assert_eq!(
        session.begin("blur", json!({})).err().unwrap().code,
        ErrorCode::Closed
    );
    session.close();
}

#[test]
fn dev_007_a2_session_reports_invalid_frames_and_events() {
    let invalid = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (session, _wire) = channel_session(SessionOptions {
        on_invalid_frame: Some(Arc::new({
            let invalid = invalid.clone();
            move |text| invalid.lock().unwrap().push(text)
        })),
        on_event: Some(Arc::new({
            let events = events.clone();
            move |event| events.lock().unwrap().push(event)
        })),
        on_stderr: None,
    });
    session.feed(b"noise\ndata: {broken\n\n");
    session.feed(&encode_frame(&Message::Event {
        name: "console".into(),
        params: json!({"text": "hi"}),
    }));
    session.feed(&encode_frame(&Message::Event {
        name: "frame".into(),
        params: json!({"sequence": 3}),
    }));
    assert_eq!(*invalid.lock().unwrap(), ["{broken"]);
    assert_eq!(
        *events.lock().unwrap(),
        [
            ("console".to_owned(), json!({"text": "hi"})),
            ("frame".to_owned(), json!({"sequence": 3}))
        ]
    );
    // Events nobody consumes are ignored by a session without a callback.
    let (quiet, _) = channel_session(SessionOptions::default());
    quiet.feed(&encode_frame(&Message::Event {
        name: "closed".into(),
        params: json!({"reason": "bye"}),
    }));
    assert!(!quiet.is_closed());
}

#[test]
fn dev_007_a2_session_validates_requests_and_results_against_the_catalog() {
    let (session, wire) = channel_session(SessionOptions::default());
    assert_eq!(
        session.call("nope", json!({})).unwrap_err().code,
        ErrorCode::Protocol
    );
    assert_eq!(
        session
            .call("clockSet", json!({"nowMs": -1}))
            .unwrap_err()
            .code,
        ErrorCode::Protocol
    );
    assert!(
        wire.lock().unwrap().is_empty(),
        "invalid requests are never sent"
    );
    // A result that does not match the catalog shape is a protocol error, not a value.
    let call = {
        let session = session.clone();
        thread::spawn(move || session.call("clockPause", json!({})))
    };
    let requests = written_requests(&wire, 1);
    reply(&session, requests[0].0, json!({"unexpected": true}));
    assert_eq!(call.join().unwrap().unwrap_err().code, ErrorCode::Protocol);
    // An error response carries its code, message and data through.
    let call = {
        let session = session.clone();
        thread::spawn(move || session.call("getBounds", json!({"elementId": 4})))
    };
    let requests = written_requests(&wire, 2);
    session.feed(&encode_frame(&Message::Response {
        id: json!(requests[1].0),
        outcome: Err(AutomationError::with_data(
            ErrorCode::NotFound,
            "nope",
            json!({"id": 4}),
        )),
    }));
    let error = call.join().unwrap().unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str(), error.data),
        (ErrorCode::NotFound, "nope", Some(json!({"id": 4})))
    );
}

// ---------------------------------------------------------------------------------------------
// Locators

#[test]
fn dev_007_a3_strict_locators_report_not_found_and_ambiguity() {
    let (_host, app) = shared(Stub::new(sample_tree()));
    let items = app.get_by_test_id("item");
    assert_eq!(items.count().unwrap(), 2);
    assert_eq!(
        items
            .all()
            .unwrap()
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [6, 7]
    );
    let error = items.element().unwrap_err();
    assert_eq!(error.code, ErrorCode::Ambiguous);
    assert_eq!(error.data, Some(json!({"count": 2})));
    assert!(error.message.contains('2'));
    for operation in [
        items.bounds().map(drop),
        items.click(),
        items.hover(),
        items.wheel(0.0, 1.0),
        items.fill("x"),
        items.press("enter"),
        items.text_content().map(drop),
    ] {
        assert_eq!(operation.unwrap_err().code, ErrorCode::Ambiguous);
    }

    let none = app.get_by_test_id("missing");
    assert_eq!(none.count().unwrap(), 0);
    assert!(none.all().unwrap().is_empty());
    assert_eq!(none.element().unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(none.click().unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn dev_007_a3_matching_rules_are_exact_for_ids_and_types_and_substring_for_own_text() {
    let (_host, app) = shared(Stub::new(sample_tree()));
    assert_eq!(
        app.get_by_test_id("greet").count().unwrap(),
        0,
        "test ids match exactly"
    );
    assert_eq!(app.get_by_type("button").count().unwrap(), 2);
    assert_eq!(
        app.get_by_type("butt").count().unwrap(),
        0,
        "types match exactly"
    );
    // Own text only, case-sensitive substring.
    let save = app.get_by_text("Save");
    assert_eq!(
        save.all()
            .unwrap()
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [6, 7]
    );
    assert_eq!(app.get_by_text("save").count().unwrap(), 0);
    assert_eq!(
        app.get_by_text("World").count().unwrap(),
        0,
        "children's text is not the wrapper's own text"
    );
    // The empty substring matches every node, including nodes without text.
    assert_eq!(app.get_by_text("").count().unwrap(), 8);
    // Locators always search the whole tree including the root.
    assert_eq!(app.get_by_test_id("root").element().unwrap().id, 1);
}

#[test]
fn dev_007_a3_nested_locators_search_only_descendants_in_document_order_without_repeats() {
    let tree = node(
        1,
        "div",
        None,
        Some("x"),
        Some(rect(0.0, 0.0, 100.0, 100.0)),
        vec![
            node(
                2,
                "div",
                None,
                Some("x"),
                Some(rect(0.0, 0.0, 50.0, 50.0)),
                vec![node(
                    3,
                    "div",
                    None,
                    Some("x"),
                    Some(rect(0.0, 0.0, 10.0, 10.0)),
                    vec![],
                )],
            ),
            node(
                4,
                "div",
                None,
                Some("y"),
                Some(rect(50.0, 0.0, 50.0, 50.0)),
                vec![node(
                    5,
                    "div",
                    None,
                    Some("x"),
                    Some(rect(50.0, 0.0, 10.0, 10.0)),
                    vec![],
                )],
            ),
        ],
    );
    let (_host, app) = shared(Stub::new(tree));
    let ids = |locator: &vespertine_test_host::automation::Locator| {
        locator
            .all()
            .unwrap()
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&app.get_by_test_id("x")), [1, 2, 3, 5]);
    // Nested: never the parent itself, and a node reachable from several matched parents once.
    assert_eq!(ids(&app.get_by_test_id("x").get_by_test_id("x")), [2, 3, 5]);
    assert_eq!(
        ids(&app
            .get_by_test_id("x")
            .get_by_test_id("x")
            .get_by_test_id("x")),
        [3]
    );
    assert_eq!(
        ids(&app.get_by_type("div").get_by_type("div")),
        [2, 3, 4, 5]
    );
    assert_eq!(ids(&app.get_by_test_id("y").get_by_test_id("x")), [5]);
    assert_eq!(
        ids(&app.get_by_test_id("y").get_by_test_id("y")),
        Vec::<u64>::new()
    );
}

#[test]
fn dev_007_a3_text_content_concatenates_descendant_text_without_separators() {
    let (_host, app) = shared(Stub::new(sample_tree()));
    assert_eq!(
        app.get_by_test_id("wrapper").text_content().unwrap(),
        "World"
    );
    assert_eq!(
        app.get_by_test_id("greeting").text_content().unwrap(),
        "Hello"
    );
    assert_eq!(
        app.get_by_test_id("root").text_content().unwrap(),
        "HelloWorldSaveSave ashidden text"
    );
}

#[test]
fn dev_007_a3_bounds_come_from_the_node_then_a_query_and_missing_bounds_are_not_found() {
    let mut stub = Stub::new(sample_tree());
    // Node 7 has tree bounds; make node 6's tree bounds vanish but answer by id.
    stub.tree.children[2].bounds = None;
    stub.bounds_by_id.push((6, rect(20.0, 200.0, 40.0, 10.0)));
    let (_host, app) = shared(stub);
    assert_eq!(
        app.get_by_test_id("greeting").bounds().unwrap(),
        rect(10.0, 10.0, 100.0, 20.0)
    );
    assert_eq!(
        app.get_by_test_id("greeting").center().unwrap(),
        (60.0, 20.0)
    );
    let by_query = app.get_by_text("Save").get_by_text("zzz");
    assert_eq!(by_query.count().unwrap(), 0);
    let node6 = app.get_by_text("Save").all().unwrap().remove(0);
    assert_eq!(node6.bounds, None);
    // Node 6 is not unique by text ("Save as" matches too): locate it by position via a query.
    assert_eq!(
        app.bounds_of(6).unwrap(),
        Some(rect(20.0, 200.0, 40.0, 10.0))
    );
    // Hidden or speculative nodes have neither: NotFound.
    assert_eq!(
        app.get_by_test_id("hidden").bounds().unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.get_by_test_id("hidden").click().unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.get_by_test_id("hidden").hover().unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[test]
fn dev_007_r5_pointer_actions_target_the_center_of_the_painted_bounds() {
    let (host, app) = shared(Stub::new(sample_tree()));
    app.get_by_test_id("greeting").click().unwrap();
    app.get_by_test_id("greeting").hover().unwrap();
    app.get_by_test_id("greeting").wheel(0.0, -30.0).unwrap();
    app.mouse().click((5.0, 6.0), 2, "alt").unwrap();
    app.mouse()
        .click(&app.get_by_test_id("wrapper"), 1, "")
        .unwrap();
    assert_eq!(
        host.borrow().calls,
        [
            Call::Click {
                x: 60.0,
                y: 20.0,
                button: 0,
                modifiers: "".into()
            },
            Call::Move {
                x: 60.0,
                y: 20.0,
                pressed: None,
                modifiers: "".into()
            },
            Call::Wheel {
                x: 60.0,
                y: 20.0,
                dx: 0.0,
                dy: -30.0
            },
            Call::Click {
                x: 5.0,
                y: 6.0,
                button: 2,
                modifiers: "alt".into()
            },
            Call::Click {
                x: 110.0,
                y: 60.0,
                button: 1,
                modifiers: "".into()
            },
        ]
    );
}

#[test]
fn dev_007_a3_drag_attaches_modifiers_to_every_event_and_interpolates_the_step_count() {
    let (host, app) = shared(Stub::new(sample_tree()));
    let options = DragOptions {
        steps: 4.7,
        modifiers: "shift".into(),
        offset: (1.0, 2.0),
        ..DragOptions::default()
    };
    app.get_by_test_id("greeting")
        .drag_to((200.0, 100.0), &options)
        .unwrap();
    let start = (61.0, 22.0);
    let dest = (200.0, 100.0);
    let mut expected = vec![
        Call::Move {
            x: start.0,
            y: start.1,
            pressed: None,
            modifiers: "shift".into(),
        },
        Call::Down {
            x: start.0,
            y: start.1,
            button: 0,
            modifiers: "shift".into(),
        },
    ];
    for k in 1..=4 {
        let f = f64::from(k) / 4.0;
        expected.push(Call::Move {
            x: start.0 + (dest.0 - start.0) * f,
            y: start.1 + (dest.1 - start.1) * f,
            pressed: Some(0),
            modifiers: "shift".into(),
        });
    }
    expected.push(Call::Up {
        x: dest.0,
        y: dest.1,
        button: 0,
        modifiers: "shift".into(),
    });
    assert_eq!(
        host.borrow().calls,
        expected,
        "floor(4.7) = 4 moves, modifiers on every event"
    );

    // Steps below one are clamped to one; the default is eight; the button is configurable.
    host.borrow_mut().calls.clear();
    app.mouse()
        .drag(
            (0.0, 0.0),
            (10.0, 0.0),
            &DragOptions {
                steps: 0.2,
                button: 2,
                ..DragOptions::default()
            },
        )
        .unwrap();
    let moves = |host: &Rc<RefCell<Stub>>| {
        host.borrow()
            .calls
            .iter()
            .filter(|call| {
                matches!(
                    call,
                    Call::Move {
                        pressed: Some(_),
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(moves(&host), 1);
    host.borrow_mut().calls.clear();
    app.mouse()
        .drag((0.0, 0.0), (80.0, 0.0), &DragOptions::default())
        .unwrap();
    assert_eq!(moves(&host), 8);
    assert!(matches!(host.borrow().calls.last(), Some(Call::Up { x, .. }) if *x == 80.0));
}

#[test]
fn dev_007_r7_drag_resolves_both_points_before_pressing_and_drag_by_offsets_the_source() {
    let mut stub = Stub::new(sample_tree());
    // The destination (node 7) jumps right by 50 px after every press, like a live preview.
    stub.moving_destination = Some(7);
    let (host, app) = shared(stub);
    app.get_by_test_id("greeting")
        .drag_to(
            &app.get_by_text("Save as"),
            &DragOptions {
                steps: 2.0,
                ..DragOptions::default()
            },
        )
        .unwrap();
    let calls = host.borrow().calls.clone();
    // Node 7's center before the press: x = 100 + 40, y = 115.
    assert!(
        matches!(calls.last(), Some(Call::Up { x, y, .. }) if *x == 140.0 && *y == 115.0),
        "{calls:?}"
    );

    host.borrow_mut().calls.clear();
    app.get_by_test_id("greeting")
        .drag_by(
            30.0,
            -5.0,
            &DragOptions {
                steps: 1.0,
                offset: (2.0, 3.0),
                ..DragOptions::default()
            },
        )
        .unwrap();
    let calls = host.borrow().calls.clone();
    assert!(
        matches!(&calls[1], Call::Down { x, y, .. } if *x == 62.0 && *y == 23.0),
        "{calls:?}"
    );
    assert!(
        matches!(calls.last(), Some(Call::Up { x, y, .. }) if *x == 92.0 && *y == 18.0),
        "{calls:?}"
    );
}

#[test]
fn dev_007_r6_fill_selects_all_then_types_tokens_and_empty_fill_sends_backspace() {
    let (host, app) = shared(Stub::new(sample_tree()));
    app.get_by_test_id("greeting")
        .fill("Hi you\nOK\t!")
        .unwrap();
    app.get_by_test_id("greeting").fill("").unwrap();
    app.get_by_test_id("greeting").press("cmd-s enter").unwrap();
    let select_all = if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    };
    assert_eq!(
        host.borrow().calls,
        [
            Call::Keys {
                keys: select_all.into(),
                element: Some(2)
            },
            Call::Keys {
                keys: "H i space y o u enter O K tab !".into(),
                element: Some(2)
            },
            Call::Keys {
                keys: select_all.into(),
                element: Some(2)
            },
            Call::Keys {
                keys: "backspace".into(),
                element: Some(2)
            },
            Call::Keys {
                keys: "cmd-s enter".into(),
                element: Some(2)
            },
        ]
    );
    // Strict: nothing is sent when the locator is ambiguous.
    host.borrow_mut().calls.clear();
    assert!(app.get_by_test_id("item").fill("x").is_err());
    assert!(host.borrow().calls.is_empty());
}

#[test]
fn dev_007_a4_wait_for_times_out_reports_ambiguity_and_sees_late_nodes() {
    // Nothing appears: Timeout.
    let (_host, app) = shared(Stub::new(sample_tree()));
    let error = app
        .get_by_test_id("late")
        .wait_for(Some(100.0))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Timeout);

    // Two nodes match at expiry: Ambiguous with the count.
    let error = app.get_by_test_id("item").wait_for(Some(64.0)).unwrap_err();
    assert_eq!(error.code, ErrorCode::Ambiguous);
    assert_eq!(error.data, Some(json!({"count": 2})));

    // A node that appears after a delay shorter than the timeout is returned.
    let mut stub = Stub::new(sample_tree());
    stub.appear = Some((
        60.0,
        node(
            20,
            "text",
            Some("late"),
            Some("late"),
            Some(rect(0.0, 300.0, 40.0, 20.0)),
            vec![],
        ),
    ));
    let (_host, app) = shared(stub);
    let found = app.get_by_test_id("late").wait_for(Some(200.0)).unwrap();
    assert_eq!(found.id, 20);
    // Already present: returns on the first poll.
    assert_eq!(app.get_by_test_id("greeting").wait_for(None).unwrap().id, 2);
}

#[test]
fn dev_007_a2_clock_values_are_validated_by_every_entry_point() {
    let (host, app) = shared(Stub::new(sample_tree()));
    assert_eq!(app.clock().set(-1.0).unwrap_err().code, ErrorCode::Protocol);
    assert_eq!(
        app.clock().fast_forward(-1.0).unwrap_err().code,
        ErrorCode::Protocol
    );
    assert_eq!(
        app.clock().set(f64::NAN).unwrap_err().code,
        ErrorCode::Protocol
    );
    assert!(
        host.borrow().calls.is_empty(),
        "invalid values never reach the host"
    );
    assert_eq!(app.clock().set(2000.0).unwrap(), 2000.0);
}

#[test]
fn dev_007_r10_closed_in_process_sessions_reject_calls() {
    let (_host, app) = shared(Stub::new(sample_tree()));
    app.close();
    assert_eq!(app.tree().unwrap_err().code, ErrorCode::Closed);
    assert_eq!(
        app.get_by_test_id("greeting").count().unwrap_err().code,
        ErrorCode::Closed
    );
}
