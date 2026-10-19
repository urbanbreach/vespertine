#![allow(clippy::disallowed_methods)]
//! Tests against the real WebKitGTK helper (embedded in this crate, extracted like a shipped
//! application would): protocol behaviour, pixel transfer, input, policy and profiles. They need
//! a display (Wayland or X11) and the WebKitGTK 4.1 runtime, like the rest of the workspace's
//! graphical tests.

mod site;

use serde_json::{Value, json};
use site::TestSite;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use vespertine_webview::extract::EMBEDDED_HELPER;
use vespertine_webview::frames::Frame;
use vespertine_webview::protocol::{Command, Transfer};
use vespertine_webview::testing::wait_until;
use vespertine_webview::{
    BrowserContext, BrowserProfile, HelperConfig, NativePage, PageEvent, Presentation,
};

/// At most this many real engines run at once: parallel WebKit instances make timing flaky.
static ENGINE_SLOTS: LazyLock<Mutex<usize>> = LazyLock::new(|| Mutex::new(0));
const MAX_ENGINES: usize = 3;

struct EngineSlot;

impl EngineSlot {
    fn take() -> EngineSlot {
        loop {
            {
                let mut running = ENGINE_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
                if *running < MAX_ENGINES {
                    *running += 1;
                    return EngineSlot;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for EngineSlot {
    fn drop(&mut self) {
        *ENGINE_SLOTS.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    }
}

/// The helper executable built by `build.rs` (the same bytes the crate embeds), referenced in
/// place so that tests leave no files behind.
fn helper_path() -> PathBuf {
    PathBuf::from(env!("VSP_WEBKIT_HELPER"))
}

fn config() -> HelperConfig {
    HelperConfig {
        helper_path: Some(helper_path()),
        ..HelperConfig::default()
    }
}

fn start(context: &BrowserContext) -> NativePage {
    context
        .start_page()
        .recv_blocking()
        .expect("startup result")
        .expect("the real helper starts")
}

/// A page configured like a live surface of the given logical size at scale 1.
fn live_page(context: &BrowserContext, width: u32, height: u32) -> NativePage {
    let page = start(context);
    page.present(Presentation::Live);
    page.sync(width as f32, height as f32, 1.0);
    page
}

/// Evaluates `script` in the page and returns the JSON result.
fn eval(page: &NativePage, script: &str) -> Value {
    let events = page.events();
    page.command(Command::Eval {
        script: script.to_owned(),
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match events.try_recv() {
            Ok(PageEvent::EvalResult(text)) => {
                return serde_json::from_str(&text).unwrap_or(Value::Null);
            }
            Ok(_) => {}
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "evaluation of {script:?} timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// The RGB value of pixel (x, y) of a frame.
fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
    let offset = (y * frame.stride() + x * 4) as usize;
    let bytes = frame.pixels();
    [bytes[offset + 2], bytes[offset + 1], bytes[offset]]
}

/// Takes frames until one satisfies `accept` and returns it.
fn latest_frame_where(page: &NativePage, accept: impl Fn(&Frame) -> bool) -> Frame {
    let mut found = None;
    assert!(
        wait_until(Duration::from_secs(15), || {
            if let Some(frame) = page.take_frame()
                && accept(&frame)
            {
                found = Some(frame);
            }
            found.is_some()
        }),
        "no frame satisfied the condition"
    );
    found.unwrap()
}

fn load_and_wait(page: &NativePage, url: &str) {
    page.load(url).unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || {
            let state = page.state();
            state.url.as_deref() == Some(url) && !state.loading && state.title != ""
        }),
        "page did not finish loading {url}: {:?}",
        page.state()
    );
}

// ---- helpers --------------------------------------------------------------------------------

/// Evaluates `script` until `accept` holds for its result (polling every 50 ms).
fn wait_eval(page: &NativePage, script: &str, accept: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let value = eval(page, script);
        if accept(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{script:?} never satisfied the condition; last {value}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The centre of the first element matching `selector`, in page pixels.
fn center(page: &NativePage, selector: &str) -> (f64, f64) {
    let value = eval(
        page,
        &format!(
            "(() => {{ const r = document.querySelector({selector:?}).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()"
        ),
    );
    (value[0].as_f64().unwrap(), value[1].as_f64().unwrap())
}

fn click_at(page: &NativePage, x: f64, y: f64, button: u32) {
    page.command(Command::Move { x, y, mods: 0 }).unwrap();
    page.command(Command::Down {
        x,
        y,
        button,
        mods: 0,
    })
    .unwrap();
    page.command(Command::Up {
        x,
        y,
        button,
        mods: 0,
    })
    .unwrap();
}

fn click(page: &NativePage, selector: &str) {
    let (x, y) = center(page, selector);
    click_at(page, x, y, 1);
}

fn key(page: &NativePage, name: &str, text: &str) {
    page.command(Command::KeyDown {
        key: name.into(),
        text: text.into(),
        mods: 0,
    })
    .unwrap();
    page.command(Command::KeyUp {
        key: name.into(),
        text: text.into(),
        mods: 0,
    })
    .unwrap();
}

/// Waits for a page event satisfying `pick` (other events are skipped).
fn wait_event<T>(page: &NativePage, pick: impl Fn(PageEvent) -> Option<T>) -> T {
    let events = page.events();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match events.try_recv() {
            Ok(event) => {
                if let Some(found) = pick(event) {
                    return found;
                }
            }
            Err(_) => {
                assert!(Instant::now() < deadline, "no matching page event");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Collects the events that arrive within `duration`.
fn events_within(page: &NativePage, duration: Duration) -> Vec<PageEvent> {
    let events = page.events();
    let end = Instant::now() + duration;
    let mut collected = Vec::new();
    while Instant::now() < end {
        while let Ok(event) = events.try_recv() {
            collected.push(event);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    collected
}

fn focus_field(page: &NativePage, selector: &str) {
    eval(
        page,
        &format!("document.querySelector({selector:?}).focus(); true"),
    );
    page.command(Command::Focus(true)).unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || page.input_state().focused),
        "the page never reported an editable focus"
    );
}

fn loaded_index(site: &TestSite, context: &BrowserContext, width: u32, height: u32) -> NativePage {
    let page = live_page(context, width, height);
    load_and_wait(&page, &site.url("/"));
    page
}

macro_rules! real_test {
    ($name:ident, $body:expr) => {
        #[test]
        fn $name() {
            let _slot = EngineSlot::take();
            let body: fn() = $body;
            body();
        }
    };
}

// ---- PLT-014/015 basics --------------------------------------------------------------------

real_test!(the_real_helper_loads_a_page_and_delivers_frames, || {
    let site = TestSite::start();
    let context = BrowserContext::with_config(config());
    let page = live_page(&context, 400, 300);
    load_and_wait(&page, &site.url("/"));
    let state = page.state();
    assert_eq!(state.title, "Embedded page");
    assert!(state.error.is_none());
    // The page background is white (the engine paints the single top-left pixel of a scrollable
    // page with the overlay scrollbar colour, so sample inside the page).
    let frame = latest_frame_where(&page, |frame| pixel(frame, 390, 290) == [255, 255, 255]);
    assert_eq!((frame.width(), frame.height()), (400, 300));
    assert!(
        matches!(frame, Frame::Buffer(_)),
        "shm is the preferred transfer"
    );
    assert_eq!(page.transfer(), Transfer::SharedMemory);
    // The colour channels arrive in the right order: a solid blue page.
    load_and_wait(&page, &site.url("/solid"));
    let solid = latest_frame_where(&page, |frame| pixel(frame, 200, 150) == [0x20, 0x60, 0xc0]);
    assert_eq!(solid.top_left_rgb(), [0x20, 0x60, 0xc0]);
    // The page's CSS viewport equals the logical element size.
    let width = eval(&page, "window.innerWidth");
    assert_eq!(width, json!(400));
    assert_eq!(eval(&page, "window.devicePixelRatio").as_f64(), Some(1.0));
});

real_test!(
    inline_transfer_works_when_shared_memory_is_not_allowed,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(HelperConfig {
            transfers: vec![Transfer::Inline],
            ..config()
        });
        let page = live_page(&context, 300, 200);
        load_and_wait(&page, &site.url("/solid"));
        assert_eq!(page.transfer(), Transfer::Inline);
        let frame = latest_frame_where(&page, |frame| frame.top_left_rgb() == [0x20, 0x60, 0xc0]);
        assert!(matches!(frame, Frame::Inline(_)));
        assert_eq!((frame.width(), frame.height()), (300, 200));
    }
);

real_test!(
    page_scale_is_engine_zoom_and_frames_follow_the_device_size,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = start(&context);
        page.present(Presentation::Live);
        // A 640 x 360 logical page at scale 1.5 is 960 x 540 device pixels.
        page.sync(640.0, 360.0, 1.5);
        load_and_wait(&page, &site.url("/solid"));
        let frame = latest_frame_where(&page, |f| {
            f.width() == 960 && f.height() == 540 && f.top_left_rgb() == [0x20, 0x60, 0xc0]
        });
        assert_eq!(frame.scale(), 1.5);
        let viewport = eval(
            &page,
            "[window.innerWidth, window.innerHeight, window.devicePixelRatio]",
        );
        let width = viewport[0].as_f64().unwrap();
        assert!((width - 640.0).abs() <= 2.0, "css viewport width {width}");
        assert_eq!(viewport[2].as_f64(), Some(1.5));
    }
);

real_test!(
    hidden_pages_produce_no_frames_and_showing_marks_them_dirty,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = live_page(&context, 300, 200);
        load_and_wait(&page, &site.url("/solid"));
        latest_frame_where(&page, |f| f.top_left_rgb() == [0x20, 0x60, 0xc0]);
        page.present(Presentation::Hidden);
        std::thread::sleep(Duration::from_millis(300));
        while page.take_frame().is_some() {}
        // An animated page would keep producing frames; hidden produces none.
        eval(&page, "document.body.style.background = '#123456'; true");
        std::thread::sleep(Duration::from_millis(400));
        assert!(!page.has_pending_frame(), "a hidden page produced a frame");
        // Showing produces a full frame within a few ticks.
        page.present(Presentation::Live);
        let frame = latest_frame_where(&page, |f| f.top_left_rgb() == [0x12, 0x34, 0x56]);
        assert_eq!((frame.width(), frame.height()), (300, 200));
    }
);

real_test!(
    backpressure_never_has_more_than_two_slots_outstanding,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 320, 240);
        // Make the page animate so that frames keep coming.
        eval(
            &page,
            "const e = document.createElement('div'); e.id = 'spin'; e.style.cssText = 'position:fixed;left:0;top:0;width:50px;height:50px;background:#f00'; document.body.append(e); setInterval(() => { e.style.left = (Math.random() * 200) + 'px'; }, 10); true",
        );
        // Hold two frames without releasing them.
        let mut held = Vec::new();
        assert!(wait_until(Duration::from_secs(10), || {
            if let Some(frame) = page.take_frame() {
                held.push(frame);
            }
            held.len() >= 2
        }));
        // With both slots owned by the application no further frame may arrive.
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            !page.has_pending_frame(),
            "a third buffer frame arrived while two slots were owned"
        );
        // The first release triggers one frame with accumulated damage.
        held.remove(0);
        assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
        let frame = page.take_frame().unwrap();
        let Frame::Buffer(buffer) = &frame else {
            panic!("buffer frame")
        };
        assert!(buffer.damage.width > 0 && buffer.damage.height > 0);
    }
);

// ---- PLT-014 profile and process ------------------------------------------------------------

real_test!(
    contexts_for_one_profile_share_a_helper_until_the_last_handle_drops,
    || {
        let data = tempfile::tempdir().unwrap();
        let profile = BrowserProfile::new(data.path(), &["share"]).unwrap();
        let first = BrowserContext::for_profile_with_config(&profile, config());
        let second = BrowserContext::for_profile_with_config(&profile, config());
        let page_a = start(&first);
        let pid = first.helper_pid().unwrap();
        let page_b = start(&second);
        assert_eq!(second.helper_pid(), Some(pid));
        drop(page_a);
        drop(first);
        // One handle is gone, the helper stays (a second page can still be opened on it).
        let page_c = start(&second);
        assert_eq!(second.helper_pid(), Some(pid));
        drop((page_b, page_c, second));
        // Immediately reopening the profile acquires the released lock with a new process.
        let reopened = BrowserContext::for_profile_with_config(&profile, config());
        let page = start(&reopened);
        assert_ne!(reopened.helper_pid(), Some(pid));
        assert!(page.state().error.is_none());
    }
);

fn run_helper_once(args: &[&str]) -> (std::process::ExitStatus, String, Vec<u8>) {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    let (ours, theirs) = UnixStream::pair().unwrap();
    let fd = theirs.as_raw_fd();
    let mut command = std::process::Command::new(helper_path());
    command.arg("--channel").arg(fd.to_string()).args(args);
    // SAFETY: clears close-on-exec on the helper's channel end between fork and exec.
    unsafe {
        command.pre_exec(move || {
            libc::fcntl(fd, libc::F_SETFD, 0);
            Ok(())
        });
    }
    command.stdout(std::process::Stdio::null());
    let mut child = command.spawn().unwrap();
    drop(theirs);
    let message = vespertine_webview::protocol::read_message(
        &mut &ours,
        vespertine_webview::protocol::Direction::ToApplication,
    );
    let (kind, payload) = match message {
        Ok(m) => (m.kind as char, m.payload),
        Err(_) => ('?', Vec::new()),
    };
    // Closing the channel asks the helper to shut down.
    drop(ours);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the helper did not exit after the channel closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    (status, kind.to_string(), payload)
}

real_test!(
    a_helper_exits_with_status_zero_when_the_channel_closes,
    || {
        let started = Instant::now();
        let (status, kind, payload) = run_helper_once(&[]);
        assert_eq!(kind, "R", "{}", String::from_utf8_lossy(&payload));
        assert!(status.success(), "{status:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let ready: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(ready["protocol"], 1);
        assert!(
            ready["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!("ping"))
        );
        assert!(
            ready["transfers"]
                .as_array()
                .unwrap()
                .contains(&json!("inline"))
        );
        assert!(
            !ready["transfers"]
                .as_array()
                .unwrap()
                .contains(&json!("dmabuf"))
        );
    }
);

real_test!(
    profile_arguments_are_validated_and_a_locked_profile_is_reported,
    || {
        // A relative path is refused.
        let (status, kind, payload) = run_helper_once(&["--profile", "relative/dir"]);
        assert_eq!(kind, "E");
        assert_eq!(
            String::from_utf8_lossy(&payload),
            "Browser profile requires an absolute storage directory"
        );
        assert_eq!(status.code(), Some(1));
        // A wrong argument count (flag without a value) is refused too.
        let (status, kind, payload) = run_helper_once(&["--profile"]);
        assert_eq!(kind, "E");
        assert_eq!(
            String::from_utf8_lossy(&payload),
            "Browser profile requires an absolute storage directory"
        );
        assert_eq!(status.code(), Some(1));
        // A path that is a regular file reports a storage error.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let (status, kind, payload) = run_helper_once(&["--profile", file.to_str().unwrap()]);
        assert_eq!(kind, "E");
        let text = String::from_utf8_lossy(&payload).into_owned();
        assert!(
            text.starts_with("Could not create or write the browser profile directory")
                || text.starts_with("Could not open browser profile: "),
            "{text}"
        );
        assert_eq!(status.code(), Some(1));
        // A locked profile: the second helper reports the lock error and exits nonzero.
        let root = dir.path().join("locked");
        let data = tempfile::tempdir().unwrap();
        let _ = data;
        let profile = BrowserProfile::new(dir.path(), &["locked"]).unwrap();
        let holder = BrowserContext::for_profile_with_config(&profile, config());
        let _page = start(&holder);
        let (status, kind, payload) =
            run_helper_once(&["--profile", profile.root().to_str().unwrap()]);
        assert_eq!(kind, "E");
        assert_eq!(
            String::from_utf8_lossy(&payload),
            "Browser profile is in use by another process or cannot be locked. Close the other instance and reopen this tab."
        );
        assert_eq!(status.code(), Some(1));
        let _ = root;
        // Two different profiles may run concurrently.
        let other = BrowserProfile::new(dir.path(), &["other"]).unwrap();
        let second = BrowserContext::for_profile_with_config(&other, config());
        let _second_page = start(&second);
        assert_ne!(holder.helper_pid(), second.helper_pid());
    }
);

fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).unwrap().mode() & 0o777
}

real_test!(website_data_persists_per_profile_and_stays_private, || {
    let site = TestSite::start();
    let data = tempfile::tempdir().unwrap();
    let profile_a = BrowserProfile::new(data.path(), &["profile-a"]).unwrap();
    let profile_b = BrowserProfile::new(data.path(), &["profile-b"]).unwrap();
    let root = profile_a.root().to_path_buf();

    let open = |profile: &BrowserProfile, path: &str| -> (BrowserContext, NativePage) {
        let context = BrowserContext::for_profile_with_config(profile, config());
        let page = live_page(&context, 300, 200);
        load_and_wait(&page, &site.url(path));
        (context, page)
    };
    // Profile A writes cookies (persistent, HttpOnly, expired, deleted) and storage.
    {
        let (_context, page) = open(&profile_a, "/set-cookies");
        let (_c2, storage) = {
            let page2 = {
                let p = live_page(&_context, 300, 200);
                load_and_wait(&p, &site.url("/storage"));
                p
            };
            ((), page2)
        };
        wait_eval(&storage, "document.title", |title| title == "stored");
        drop(page);
    }
    // All tabs closed and the helper restarted: the data is still there.
    std::thread::sleep(Duration::from_millis(500));
    {
        let (_context, page) = open(&profile_a, "/echo-cookies");
        let header = eval(&page, "document.getElementById('h').textContent");
        let header = header.as_str().unwrap().to_owned();
        assert!(header.contains("persist=yes"), "{header}");
        assert!(
            header.contains("secret=hidden"),
            "HttpOnly cookies are sent to the site: {header}"
        );
        assert!(!header.contains("expired"), "{header}");
        assert!(!header.contains("doomed"), "{header}");
        // HttpOnly is not visible to scripts.
        let script_visible = eval(&page, "document.cookie");
        let script_visible = script_visible.as_str().unwrap();
        assert!(
            script_visible.contains("persist=yes") && !script_visible.contains("secret"),
            "{script_visible}"
        );
        let reader = live_page(&_context, 300, 200);
        load_and_wait(&reader, &site.url("/read-storage"));
        let title = wait_eval(&reader, "document.title", |t| {
            t.as_str().is_some_and(|t| t.starts_with("read:"))
        });
        assert_eq!(
            title,
            json!("read:{\"ls\":\"remembered\",\"idb\":\"idb-value\"}")
        );
    }
    // A different profile sees none of it.
    {
        let (_context, page) = open(&profile_b, "/echo-cookies");
        let header = eval(&page, "document.getElementById('h').textContent");
        assert_eq!(header.as_str().unwrap().trim(), "");
        let reader = live_page(&_context, 300, 200);
        load_and_wait(&reader, &site.url("/read-storage"));
        let title = wait_eval(&reader, "document.title", |t| {
            t.as_str().is_some_and(|t| t.starts_with("read:"))
        });
        assert_eq!(title, json!("read:{\"ls\":null,\"idb\":null}"));
    }
    // Returning to the first profile recovers the data.
    {
        let (_context, page) = open(&profile_a, "/echo-cookies");
        let header = eval(&page, "document.getElementById('h').textContent");
        assert!(header.as_str().unwrap().contains("persist=yes"));
    }
    std::thread::sleep(Duration::from_millis(500));
    // Everything is private to the user.
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("data")), 0o700);
    assert_eq!(mode(&root.join("cache")), 0o700);
    assert_eq!(mode(&root.join("profile.lock")), 0o600);
    assert_eq!(mode(&root.join("cookies.sqlite")), 0o600);
    for entry in walk(&root) {
        assert_eq!(
            mode(&entry) & 0o077,
            0,
            "{} has group or world permission bits",
            entry.display()
        );
    }
});

fn walk(root: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        found.push(path.clone());
        if path.is_dir()
            && let Ok(entries) = std::fs::read_dir(&path)
        {
            pending.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    found
}

real_test!(a_missing_runtime_is_a_retryable_startup_error, || {
    // Copy the helper and corrupt the name of one of its shared-library dependencies so the
    // dynamic loader cannot resolve it, like a machine without the runtime packages.
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("helper");
    let mut bytes = EMBEDDED_HELPER.to_vec();
    let needle = b"libjson-glib-1.0.so.0";
    let position = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("the helper links json-glib");
    bytes[position + needle.len() - 1] = b'9';
    std::fs::write(&broken, &bytes).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let broken_config = HelperConfig {
        helper_path: Some(broken.clone()),
        ..HelperConfig::default()
    };
    let context = BrowserContext::with_config(broken_config);
    let error = context
        .start_page()
        .recv_blocking()
        .unwrap()
        .err()
        .expect("startup fails");
    assert!(
        error.starts_with("Browser helper could not start: "),
        "{error}"
    );
    assert!(
        error.ends_with("Check the WebKitGTK 4.1 runtime and display."),
        "{error}"
    );
    // Fixing the runtime makes the same context's retry succeed.
    bytes[position + needle.len() - 1] = b'0';
    std::fs::write(&broken, &bytes).unwrap();
    let page = context
        .start_page()
        .recv_blocking()
        .unwrap()
        .expect("retry succeeds");
    drop(page);
});

#[test]
fn the_application_binary_does_not_link_the_engine() {
    // PLT-014 R1 / A13: no dynamic dependency on WebKitGTK, GTK or json-glib.
    let output = std::process::Command::new("ldd")
        .arg(std::env::current_exe().unwrap())
        .output()
        .expect("ldd");
    let listing = String::from_utf8_lossy(&output.stdout).into_owned();
    for library in ["libwebkit2gtk", "libgtk-3", "libjson-glib", "libgdk-3"] {
        assert!(
            !listing.contains(library),
            "{library} is linked:\n{listing}"
        );
    }
}

// ---- PLT-014 R10 / A12: policy ---------------------------------------------------------------

fn has_new_tab(events: &[PageEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            PageEvent::NewTab(url) => Some(url.clone()),
            _ => None,
        })
        .collect()
}

real_test!(
    rejected_navigations_are_ignored_and_never_reach_an_opener,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 700, 500);
        let before = page.state().url;
        for link in ["#l-js", "#l-file", "#l-custom", "#l-cred"] {
            click(&page, link);
            let events = events_within(&page, Duration::from_millis(500));
            assert!(
                has_new_tab(&events).is_empty(),
                "{link} produced a new-tab request"
            );
            let state = page.state();
            assert_eq!(state.url, before, "{link} navigated the page");
            assert!(state.error.is_none(), "{link}: {:?}", state.error);
        }
        assert_eq!(
            eval(&page, "window.jsran"),
            Value::Null,
            "the javascript link did not run"
        );
        // A user-initiated target=_blank link with an allowed address produces one new-tab request.
        click(&page, "#l-blank");
        let url = wait_event(&page, |event| match event {
            PageEvent::NewTab(url) => Some(url),
            _ => None,
        });
        assert_eq!(url, site.url("/second"));
        let extra = events_within(&page, Duration::from_millis(400));
        assert!(
            has_new_tab(&extra).is_empty(),
            "exactly one new-tab request"
        );
        assert_eq!(
            page.state().url,
            before,
            "the page itself never opens the new window"
        );
        // An allowed ordinary link navigates.
        click(&page, "#l-ok");
        assert!(wait_until(Duration::from_secs(10), || page
            .state()
            .url
            .as_deref()
            == Some(&site.url("/second"))));
    }
);

real_test!(script_opened_windows_without_a_gesture_are_ignored, || {
    let site = TestSite::start();
    let context = BrowserContext::with_config(config());
    let page = live_page(&context, 400, 300);
    load_and_wait(&page, &site.url("/?open=1"));
    let events = events_within(&page, Duration::from_millis(1200));
    assert!(
        has_new_tab(&events).is_empty(),
        "window.open without a gesture opened a tab"
    );
});

real_test!(
    frames_may_load_documents_of_the_page_itself_and_load_commands_are_checked,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 400, 300);
        wait_eval(&page, "window.frames_loaded", |count| {
            count.as_i64() == Some(2)
        });
        // A load command with a rejected address does not navigate and sets the page error.
        let before = page.state().url;
        page.load("file:///etc/hostname").unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            page.state().error.as_deref() == Some("Only http and https addresses are supported.")
        }));
        assert_eq!(page.state().url, before);
        assert!(!page.state().loading);
        for rejected in [
            "javascript:alert(1)",
            "mailto:a@example.com",
            "http://user:pw@127.0.0.1/",
            "x-app://thing",
            "data:text/html,hi",
        ] {
            page.load(rejected).unwrap();
            assert!(
                wait_until(Duration::from_secs(5), || {
                    page.state().error.as_deref()
                        == Some("Only http and https addresses are supported.")
                }),
                "{rejected}"
            );
            assert_eq!(page.state().url, before);
        }
        // A new load start clears the error.
        load_and_wait(&page, &site.url("/second"));
        assert!(page.state().error.is_none());
    }
);

real_test!(permissions_are_denied_and_downloads_are_cancelled, || {
    let site = TestSite::start();
    let context = BrowserContext::with_config(config());
    let page = loaded_index(&site, &context, 400, 300);
    eval(
        &page,
        "window.geo = 'pending'; navigator.geolocation.getCurrentPosition(() => { window.geo = 'granted'; }, (e) => { window.geo = 'denied:' + e.code; }); true",
    );
    let result = wait_eval(&page, "window.geo", |v| v != "pending");
    assert_eq!(result, json!("denied:1"));
    eval(
        &page,
        "window.notif = 'pending'; Notification.requestPermission().then((p) => { window.notif = p; }); true",
    );
    let result = wait_eval(&page, "window.notif", |v| v != "pending");
    assert_eq!(result, json!("denied"));
    // A download is cancelled the moment it starts: the page stays where it is.
    let before = page.state().url;
    page.load(&site.url("/download")).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert!(wait_until(Duration::from_secs(10), || !page
        .state()
        .loading));
    assert_eq!(eval(&page, "document.title"), json!("Embedded page"));
    let _ = before;
});

// ---- PLT-015 R4: unknown commands, unknown pages, create twice -------------------------------

real_test!(
    unknown_commands_and_pages_are_ignored_and_create_is_idempotent,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 400, 300);
        eval(
            &page,
            "window.seen = []; for (const t of ['keydown','keyup','mousedown','mouseup','click']) document.addEventListener(t, (e) => window.seen.push(t + ':' + (e.key || ''))); true",
        );
        // The raw channel is the page's helper: use the page handle's command path with names the
        // helper does not know, a page that was never created, and a second create.
        // Unknown commands go through `MenuAction`, which carries arbitrary static names.
        page.command(Command::MenuAction("frobnicate")).unwrap();
        page.command(Command::MenuAction("key_down2")).unwrap();
        // A second create for the same page identifier creates it once (the page keeps working).
        page.command(Command::Create).unwrap();
        // Commands for a page that does not exist are ignored: close it and keep using the helper.
        let other = start(&context);
        let other_id = other.id();
        assert_ne!(other_id, page.id());
        drop(other);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            eval(&page, "window.seen"),
            json!([]),
            "unknown commands produced input events"
        );
        // The helper is still healthy.
        assert_eq!(eval(&page, "1 + 1"), json!(2));
    }
);

// ---- PLT-016: input, composition, clipboard -------------------------------------------------

real_test!(typing_paste_composition_and_copy_edit_a_text_field, || {
    let site = TestSite::start();
    let context = BrowserContext::with_config(config());
    let page = loaded_index(&site, &context, 700, 500);
    focus_field(&page, "#f");
    // Individual key events.
    for ch in "hello linux".chars() {
        let text = ch.to_string();
        let name = if ch == ' ' {
            "space".to_owned()
        } else {
            text.clone()
        };
        key(&page, &name, &text);
    }
    wait_eval(&page, "document.querySelector('#f').value", |v| {
        v == "hello linux"
    });
    // Paste via the plain insertion command: non-ASCII content round trips exactly.
    page.command(Command::Text(" — café 日本語".into()))
        .unwrap();
    wait_eval(&page, "document.querySelector('#f').value", |v| {
        v == "hello linux — café 日本語"
    });
    // Select all and copy: the helper reports the selected text for the system clipboard.
    eval(&page, "document.querySelector('#f').select(); true");
    page.command(Command::Copy).unwrap();
    let copied = wait_event(&page, |event| match event {
        PageEvent::Clipboard(text) => Some(text),
        _ => None,
    });
    assert_eq!(copied, "hello linux — café 日本語");
    // Copy with no selection yields nothing for the clipboard.
    eval(
        &page,
        "const f = document.querySelector('#f'); f.setSelectionRange(f.value.length, f.value.length); true",
    );
    page.command(Command::Copy).unwrap();
    let events = events_within(&page, Duration::from_millis(500));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PageEvent::Clipboard(text) if !text.is_empty())),
        "{events:?}"
    );
    // Composition: preedit, preedit again, then commit.
    page.command(Command::Preedit("にほ".into())).unwrap();
    page.command(Command::Preedit("日本語".into())).unwrap();
    page.command(Command::Commit("日本語".into())).unwrap();
    wait_eval(&page, "document.querySelector('#f').value", |v| {
        v.as_str()
            .is_some_and(|v| v == "hello linux — café 日本語日本語")
    });
    // Cut removes the selection.
    eval(&page, "document.querySelector('#f').select(); true");
    page.command(Command::Cut).unwrap();
    let cut = wait_event(&page, |event| match event {
        PageEvent::Clipboard(text) => Some(text),
        _ => None,
    });
    assert_eq!(cut, "hello linux — café 日本語日本語");
    wait_eval(&page, "document.querySelector('#f').value", |v| v == "");
    // The page reported its input state: focused, a caret rectangle and the surrounding text.
    page.command(Command::Text("abc".into())).unwrap();
    wait_eval(&page, "document.querySelector('#f').value", |v| v == "abc");
    assert!(
        wait_until(Duration::from_secs(5), || {
            let state = page.input_state();
            state.focused && state.caret.is_some() && state.text.as_deref() == Some("abc")
        }),
        "{:?}",
        page.input_state()
    );
    // SelectAll via the editing command.
    page.command(Command::SelectAll).unwrap();
    wait_eval(
        &page,
        "document.getSelection().toString() + '|' + document.querySelector('#f').selectionEnd",
        |v| v.as_str().is_some_and(|v| v.ends_with("|3")),
    );
});

real_test!(password_contents_never_leave_the_engine, || {
    let site = TestSite::start();
    let context = BrowserContext::with_config(config());
    let page = loaded_index(&site, &context, 700, 500);
    focus_field(&page, "#p");
    eval(
        &page,
        "const p = document.querySelector('#p'); p.setSelectionRange(p.value.length, p.value.length); true",
    );
    page.command(Command::Text("more".into())).unwrap();
    wait_eval(&page, "document.querySelector('#p').value", |v| {
        v == "s3cretmore"
    });
    std::thread::sleep(Duration::from_millis(300));
    let state = page.input_state();
    assert!(state.focused);
    assert!(
        state.text.is_none() || !state.text.as_deref().unwrap().contains("s3cret"),
        "{state:?}"
    );
    eval(&page, "document.querySelector('#p').select(); true");
    page.command(Command::Copy).unwrap();
    let events = events_within(&page, Duration::from_millis(600));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PageEvent::Clipboard(text) if !text.is_empty())),
        "copy returned password text: {events:?}"
    );
});

real_test!(
    wheel_scrolls_clicks_count_once_and_frames_keep_rendering,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 700, 500);
        let frames_before = eval(&page, "window.frameCounter").as_f64().unwrap();
        // Each click on the button increments the counter exactly once.
        for expected in 1..=3 {
            click(&page, "#inc");
            wait_eval(&page, "window.counter", |v| v.as_i64() == Some(expected));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(eval(&page, "window.counter"), json!(3));
        // A wheel delta of 6.25 smooth-scroll units scrolls the page by more than 50 CSS pixels.
        page.command(Command::Scroll {
            x: 100.0,
            y: 100.0,
            dx: 0.0,
            dy: 6.25,
            mods: 0,
        })
        .unwrap();
        wait_eval(&page, "window.scrollY", |y| {
            y.as_f64().is_some_and(|y| y > 50.0)
        });
        // Animation frames kept running during the interactions.
        std::thread::sleep(Duration::from_millis(1200));
        let frames_after = eval(&page, "window.frameCounter").as_f64().unwrap();
        assert!(
            frames_after - frames_before > 60.0,
            "{}",
            frames_after - frames_before
        );
    }
);

real_test!(
    context_menus_and_option_menus_are_reported_and_actions_work,
    || {
        let site = TestSite::start();
        let context = BrowserContext::with_config(config());
        let page = loaded_index(&site, &context, 700, 500);
        // Right-click on a link: link rows first, then the page rows.
        let (x, y) = center(&page, "#l-ok");
        click_at(&page, x, y, 3);
        let menu = wait_event(&page, |event| match event {
            PageEvent::Menu(menu) => Some(menu),
            _ => None,
        });
        let labels: Vec<_> = menu.items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels[0], "Open link in new tab");
        assert_eq!(labels[1], "Copy link address");
        assert!(
            labels.contains(&"Copy")
                && labels.contains(&"Select all")
                && labels.contains(&"Back")
                && labels.contains(&"Forward")
                && labels.contains(&"Reload")
        );
        assert!(menu.items[0].enabled && menu.items[1].enabled);
        assert_eq!(menu.items[0].action, "open-link");
        assert_eq!(menu.items[1].action, "copy-link");
        assert!(
            (menu.x - x).abs() < 3.0 && (menu.y - y).abs() < 3.0,
            "anchored at the pointer: {:?}",
            (menu.x, menu.y)
        );
        page.command(Command::MenuAction("copy-link")).unwrap();
        let copied = wait_event(&page, |event| match event {
            PageEvent::Clipboard(text) => Some(text),
            _ => None,
        });
        assert_eq!(copied, site.url("/second"));
        page.command(Command::MenuAction("open-link")).unwrap();
        let opened = wait_event(&page, |event| match event {
            PageEvent::NewTab(url) => Some(url),
            _ => None,
        });
        assert_eq!(opened, site.url("/second"));
        page.command(Command::DismissMenu).unwrap();
        // A <select> opens an option menu; choosing item 1 changes the value.
        click(&page, "#s");
        let menu = wait_event(&page, |event| match event {
            PageEvent::Menu(menu) if menu.items.iter().any(|i| i.action.starts_with("option:")) => {
                Some(menu)
            }
            _ => None,
        });
        let labels: Vec<_> = menu.items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["Alpha", "Beta", "Gamma"]);
        assert!(menu.items[0].selected && !menu.items[1].selected);
        assert_eq!(menu.items[1].action, "option:1");
        page.command(Command::Option(1)).unwrap();
        wait_eval(&page, "document.querySelector('#s').value", |v| v == "b");
    }
);

real_test!(
    pings_are_answered_so_a_responsive_helper_is_never_declared_hung,
    || {
        let site = TestSite::start();
        let mut config = config();
        config.timings.ping_interval = Duration::from_millis(100);
        config.timings.hang_deadline = Duration::from_millis(1500);
        let context = BrowserContext::with_config(config);
        let page = loaded_index(&site, &context, 300, 200);
        std::thread::sleep(Duration::from_secs(4));
        assert!(page.state().error.is_none(), "{:?}", page.state());
        assert_eq!(eval(&page, "1 + 1"), json!(2));
    }
);
