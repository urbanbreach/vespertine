//! PLT-015 frame transfer: latest-wins retention, event channel bounds, shared-memory pools,
//! damage union, release and fallback, against an in-process scripted helper.

use serde_json::json;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use vespertine_webview::frames::{Frame, Pool, memfd_sealed, write_memfd};
use vespertine_webview::protocol::{encode_inline_frame, kind};
use vespertine_webview::testing::{Peer, wait_until};
use vespertine_webview::{Helper, NativePage, PageEvent};

fn page() -> (Arc<Helper>, Peer, NativePage) {
    let (helper, peer) = Peer::start();
    let page = helper.new_page().unwrap();
    (helper, peer, page)
}

fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Vec<u8> {
    bgra.into_iter()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}

fn announce(
    peer: &Peer,
    generation: u64,
    width: u32,
    height: u32,
    stride: u32,
    slots: usize,
) -> Vec<std::os::fd::OwnedFd> {
    let (object, fds) = Pool::announcement_for_tests(generation, width, height, stride, 1.0, slots);
    let raws: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    peer.send_with_fds(
        kind::POOL,
        1,
        serde_json::Value::Object(object).to_string().as_bytes(),
        &raws,
    );
    fds
}

fn buffer(
    peer: &Peer,
    generation: u64,
    slot: u32,
    seq: u64,
    width: u32,
    height: u32,
    damage: [u32; 4],
) {
    peer.send_json(
        kind::BUFFER,
        1,
        json!({"generation": generation, "slot": slot, "seq": seq, "width": width,
               "height": height, "scale": 1.0, "damage": damage}),
    );
}

#[test]
fn a_hundred_inline_frames_leave_exactly_one_retained_frame() {
    let (_helper, peer, page) = page();
    for index in 0..100u32 {
        peer.send(
            kind::FRAME,
            1,
            &encode_inline_frame(8, 8, 2.0, &solid(8, 8, [index as u8, 0, 0, 255])),
        );
    }
    // Bad frames in between never replace the retained one.
    peer.send(kind::FRAME, 1, &[0; 11]);
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(8, 8, f32::NAN, &solid(8, 8, [1, 1, 1, 255])),
    );
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(8, 8, 0.49, &solid(8, 8, [1, 1, 1, 255])),
    );
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(8, 8, 4.01, &solid(8, 8, [1, 1, 1, 255])),
    );
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(8, 8, 1.0, &solid(8, 8, [1, 1, 1, 255])[..8 * 8 * 4 - 4]),
    );
    // A sentinel proves the reader drained everything before it.
    peer.send_json(
        kind::STATE,
        1,
        json!({"url": "http://sentinel/", "title": "", "error": null, "loading": false,
               "can_back": false, "can_forward": false}),
    );
    assert!(wait_until(Duration::from_secs(5), || page
        .state()
        .url
        .as_deref()
        == Some("http://sentinel/")));
    let events = page.events();
    assert!(events.len() <= 64);
    let frame = page.take_frame().expect("one frame");
    assert_eq!(frame.scale(), 2.0);
    assert_eq!(frame.top_left_rgb(), [0, 0, 99]);
    assert!(page.take_frame().is_none());
    // Scale 0.5 and 4 exactly are accepted.
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(2, 2, 0.5, &solid(2, 2, [9, 9, 9, 255])),
    );
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    assert_eq!(page.take_frame().unwrap().scale(), 0.5);
    peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(2, 2, 4.0, &solid(2, 2, [9, 9, 9, 255])),
    );
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    assert_eq!(page.take_frame().unwrap().scale(), 4.0);
}

#[test]
fn lossy_events_are_bounded_and_data_events_are_never_dropped() {
    let (_helper, peer, page) = page();
    let events = page.events();
    let state = json!({"url": "http://x/", "title": "", "error": null, "loading": false,
                       "can_back": false, "can_forward": false});
    // 200 lossy notifications with nobody draining: the queue never exceeds 64.
    for _ in 0..200 {
        peer.send_json(kind::STATE, 1, state.clone());
    }
    assert!(wait_until(Duration::from_secs(5), || events.len() == 64));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(events.len(), 64);
    // Data events behind a full queue wait for room instead of being dropped.
    peer.send_json(kind::MENU, 1, json!({"x": 1.0, "y": 2.0, "items": []}));
    peer.send(kind::CLIPBOARD, 1, "copied".as_bytes());
    peer.send(kind::NEW_TAB, 1, b"http://example.test/");
    let mut menus = 0;
    let mut clipboards = 0;
    let mut tabs = 0;
    assert!(wait_until(Duration::from_secs(5), || {
        while let Ok(event) = events.try_recv() {
            match event {
                PageEvent::Menu(_) => menus += 1,
                PageEvent::Clipboard(text) => {
                    assert_eq!(text, "copied");
                    clipboards += 1
                }
                PageEvent::NewTab(url) => {
                    assert_eq!(url, "http://example.test/");
                    tabs += 1
                }
                _ => {}
            }
        }
        (menus, clipboards, tabs) == (1, 1, 1)
    }));
}

#[test]
fn a_pool_announcement_maps_both_slots_and_buffer_frames_release_their_slot() {
    let (_helper, peer, page) = page();
    let fds = announce(&peer, 1, 40, 30, 192, 2);
    // Slot 0 holds a recognizable pixel at column 39 of row 1 (stride 192).
    write_memfd(&fds[0], 192 + 39 * 4, &[10, 20, 30, 255]);
    assert!(wait_until(Duration::from_secs(5), || page.pool().is_some()));
    buffer(&peer, 1, 0, 1, 40, 30, [0, 0, 40, 30]);
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    let frame = page.take_frame().unwrap();
    let Frame::Buffer(buffer_frame) = &frame else {
        panic!("expected a buffer frame");
    };
    assert_eq!(buffer_frame.stride(), 192);
    assert_eq!(
        &buffer_frame.pixels()[192 + 39 * 4..192 + 39 * 4 + 4],
        &[10, 20, 30, 255]
    );
    // The tightly packed copy has no row padding.
    let packed = frame.packed_pixels();
    assert_eq!(&packed[160 + 39 * 4..160 + 39 * 4 + 4], &[10, 20, 30, 255]);
    peer.assert_no_more("release", 0);
    // Releasing consumes the frame: its pixels cannot be borrowed afterwards.
    let Frame::Buffer(buffer_frame) = frame else {
        unreachable!("matched above");
    };
    buffer_frame.release();
    let releases = peer.wait_named("release", 1);
    assert_eq!(releases[0]["generation"], 1);
    assert_eq!(releases[0]["slot"], 0);
    // The consumed frame leaves nothing to release a second time.
    peer.assert_no_more("release", 1);
}

#[test]
fn an_invalid_pool_falls_back_to_inline_through_configure() {
    let (_helper, peer, page) = page();
    peer.wait_named("configure", 1);
    // Slots smaller than stride x height.
    let size = 64u64 * 4;
    let fds = [memfd_sealed(100), memfd_sealed(100)];
    let raws: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    let announcement = json!({"generation": 1, "kind": "shm", "width": 8, "height": 4, "scale": 1.0,
        "stride": 64, "format": "BGRA8888",
        "slots": [{"slot": 0, "size": size}, {"slot": 1, "size": size}]});
    peer.send_with_fds(kind::POOL, 1, announcement.to_string().as_bytes(), &raws);
    let configures = peer.wait_named("configure", 2);
    assert_eq!(configures[0]["transfer"], "shm");
    assert_eq!(configures[1]["transfer"], "inline");
    assert!(page.pool().is_none());
    assert_eq!(
        page.transfer(),
        vespertine_webview::protocol::Transfer::Inline
    );
    // Only one fallback configure is sent.
    peer.assert_no_more("configure", 2);
}

#[test]
fn buffer_frames_with_bad_damage_or_unknown_pool_are_discarded_and_released() {
    let (_helper, peer, page) = page();
    let _fds = announce(&peer, 1, 40, 30, 192, 2);
    assert!(wait_until(Duration::from_secs(5), || page.pool().is_some()));
    // Damage reaching outside the frame.
    buffer(&peer, 1, 0, 1, 40, 30, [30, 0, 20, 10]);
    // Slot out of range, unknown generation, size mismatch.
    buffer(&peer, 1, 5, 2, 40, 30, [0, 0, 1, 1]);
    buffer(&peer, 9, 1, 3, 40, 30, [0, 0, 1, 1]);
    buffer(&peer, 1, 1, 4, 41, 30, [0, 0, 1, 1]);
    let releases = peer.wait_named("release", 4);
    let pairs: Vec<_> = releases
        .iter()
        .map(|r| {
            (
                r["generation"].as_u64().unwrap(),
                r["slot"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(pairs, [(1, 0), (1, 5), (9, 1), (1, 1)]);
    assert!(!page.has_pending_frame());
}

#[test]
fn dropped_buffer_frames_union_their_damage_into_the_newer_frame() {
    let (_helper, peer, page) = page();
    let _fds = announce(&peer, 1, 40, 30, 192, 2);
    assert!(wait_until(Duration::from_secs(5), || page.pool().is_some()));
    buffer(&peer, 1, 0, 1, 40, 30, [0, 0, 10, 10]);
    buffer(&peer, 1, 1, 2, 40, 30, [20, 20, 10, 10]);
    // The earlier frame was dropped at once and its slot released.
    let releases = peer.wait_named("release", 1);
    assert_eq!(releases[0]["slot"], 0);
    assert!(wait_until(Duration::from_secs(5), || {
        matches!(page.take_frame(), Some(Frame::Buffer(frame)) if {
            let damage = frame.damage;
            assert_eq!((damage.x, damage.y, damage.width, damage.height), (0, 0, 30, 30));
            assert_eq!(frame.seq, 2);
            true
        })
    }));
}

#[test]
fn a_new_generation_invalidates_the_old_pool() {
    let (_helper, peer, page) = page();
    let _first = announce(&peer, 1, 40, 30, 192, 2);
    assert!(wait_until(Duration::from_secs(5), || page
        .pool()
        .is_some_and(|p| p.generation == 1)));
    let _second = announce(&peer, 2, 20, 10, 128, 2);
    assert!(wait_until(Duration::from_secs(5), || page
        .pool()
        .is_some_and(|p| p.generation == 2)));
    // A frame of the old generation is discarded and released.
    buffer(&peer, 1, 0, 1, 40, 30, [0, 0, 1, 1]);
    peer.wait_named("release", 1);
    assert!(!page.has_pending_frame());
    buffer(&peer, 2, 0, 1, 20, 10, [0, 0, 20, 10]);
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
}
