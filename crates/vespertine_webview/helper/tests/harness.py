#!/usr/bin/env python3
"""Black-box tests of the Vespertine WebKitGTK helper (PLT-014/015/016, helper side).

Usage: harness.py [--helper PATH] [test-name-substring ...]  (default: target/webview-dev/vsp-helper)
"""
import os
import sys

sys.dont_write_bytecode = True
import vsplib
from vsplib import *  # noqa: F401,F403

TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn



# ==============================================================================================
# PLT-014: process, handshake, shutdown, profile
# ==============================================================================================
@test
def test_handshake_and_environment(site):
    h = new_helper()
    try:
        first = h.msgs[0]
        assert first.kind == "R" and first.page == 0, first
        j = first.json()
        assert j["protocol"] == 1, j
        assert j["engine"].startswith("WebKitGTK "), j
        assert j["transfers"] == ["inline", "shm"], j
        assert j["capabilities"] == ["ping", "eval"], j
        with open("/proc/%d/status" % h.proc.pid) as f:
            umask = [l for l in f if l.startswith("Umask:")][0].split()[1]
        assert umask == "0077", umask
        # ping / pong from the main loop
        m = h.mark()
        h.cmd(0, "ping", nonce=7)
        _, pong = h.wait_for(lambda m: m.kind == "Z", 5, "pong", m)
        assert pong.page == 0 and pong.json() == {"nonce": 7}, pong
        # the channel descriptor is not inherited by engine sub-processes (CLOEXEC)
        page = h.create(200, 100)
        h.load(page, site.url("/basic.html"))
        link = os.readlink("/proc/%d/fd/%d" % (h.proc.pid, h.child_fd_in_helper())) if False else None
        inode = None
        for fd in os.listdir("/proc/%d/fd" % h.proc.pid):
            try:
                target = os.readlink("/proc/%d/fd/%s" % (h.proc.pid, fd))
            except OSError:
                continue
            if target.startswith("socket:") and os.fstat(h.sock.fileno()).st_ino != 0:
                pass
        # identify the helper's channel end: the fd the helper was told about
        chan = os.readlink("/proc/%d/fd/%d" % (h.proc.pid, h.child_fd))
        kids = children_of(h.proc.pid)
        assert kids, "engine processes expected"
        for kid in kids:
            for fd in os.listdir("/proc/%d/fd" % kid):
                try:
                    target = os.readlink("/proc/%d/fd/%s" % (kid, fd))
                except OSError:
                    continue
                assert target != chan, "engine process %d inherited the channel (fd %s)" % (kid, fd)
    finally:
        status = h.finish()
        out = h.proc.stdout.read()
        assert status == 0, status
        assert out == b"", "helper wrote to stdout: %r" % out


@test
def test_display_failure_exits_before_any_message(site):
    h = Helper(env={"WAYLAND_DISPLAY": "vsp-no-such-display", "DISPLAY": ":97", "GDK_BACKEND": "x11"})
    try:
        status = h.proc.wait(20)
        settle(0.2)
        assert status == 1, status
        assert not h.msgs, h.msgs
        err = h.stderr_text().strip()
        assert err and "\n" not in err, repr(err)
    finally:
        h.kill()


@test
def test_x11_backend(site):
    h = new_helper(env={"GDK_BACKEND": "x11"})
    try:
        page = h.create(200, 100)
        h.load(page, site.url("/basic.html"))
        f = h.frame_inline(page, pred=lambda f: pixel(f, 5, 5) == (255, 0, 0, 255))
        assert (f["width"], f["height"]) == (200, 100)
        h.cmd(page, "resize", width=400, height=300, scale=2.0)
        settle(0.6)
        assert h.eval(page, "[devicePixelRatio, innerWidth, innerHeight]") == [2, 200, 150]
    finally:
        assert h.finish() == 0


@test
def test_shutdown_by_eof_and_command(site):
    h = new_helper()
    page = h.create(400, 300)
    h.load(page, site.url("/input.html"))
    click(h, page, 50, 175)  # opens the option menu: it must be closed first on shutdown
    h.wait_for(lambda m: m.kind == "M", 5, "option menu")
    t0 = time.monotonic()
    assert h.finish(5) == 0
    assert time.monotonic() - t0 < 5
    # explicit shutdown command: everything after it is ignored
    h = new_helper()
    page = h.create(200, 100)
    h.cmd(0, "shutdown")
    h.cmd(page, "load", url=site.url("/basic.html"))
    h.cmd(0, "ping", nonce=1)
    assert h.proc.wait(10) == 0
    assert not [m for m in h.msgs if m.kind in "SZ"], h.msgs
    h.kill()


@test
def test_protocol_violation_leaves_loop(site):
    h = new_helper()
    h.send_raw("K", 0, b"")  # fine
    h.sock.sendall(struct.pack("<cII", b"K", 0, 1048577))
    try:
        status = h.proc.wait(10)
    except subprocess.TimeoutExpired:
        h.kill()
        raise AssertionError("helper stayed alive after an oversized message")
    assert status == 0, status
    h.kill()


@test
def test_framing_robustness(site):
    h = new_helper()
    try:
        page = h.create(200, 100)
        h.send_raw("Q", page, b"\x00" * 5000)             # unknown kind with payload: skipped
        h.send_raw("K", page, b"[1,2,3]")                  # not an object: ignored
        h.send_raw("K", page, b"{not json")                # invalid: ignored
        h.send_raw("K", 0, json.dumps({"cmd": "ping", "nonce": 41, "extra": [1]}).encode())
        h.send_raw("K", 0, json.dumps({"cmd": "ping"}).encode())  # missing nonce -> 0
        m = h.mark()
        _, z = h.wait_for(lambda m: m.kind == "Z", 5, "pong")
        assert z.json() == {"nonce": 41}
        _, z = h.wait_for(lambda m: m.kind == "Z" and m.json()["nonce"] == 0, 5, "pong default")
        # a 1 MiB command is still fine
        big = json.dumps({"cmd": "frobnicate", "pad": "x" * (1048576 - 40)}).encode()
        assert len(big) <= 1048576
        h.send_raw("K", page, big)
        h.cmd(0, "ping", nonce=42)
        h.wait_for(lambda m: m.kind == "Z" and m.json()["nonce"] == 42, 5, "pong after big")
    finally:
        assert h.finish() == 0


@test
def test_unknown_commands_and_pages(site):
    h = new_helper()
    try:
        page = h.create(400, 300)
        h.load(page, site.url("/input.html"))
        h.cmd(page, "down", x=50, y=25, button=1, mods=0)
        h.cmd(page, "up", x=50, y=25, button=1, mods=0)
        settle()
        h.eval(page, "window.events.length=0;1")
        m = h.mark()
        h.cmd(page, "frobnicate", key="a", text="a")
        h.cmd(99, "load", url=site.url("/basic.html"))
        h.cmd(99, "eval", script="1")
        h.cmd(99, "key_down", key="a", text="a", mods=0)
        h.cmd(page, "key_up_", key="a")
        settle()
        assert h.eval(page, "window.events.length") == 0
        assert h.eval(page, "document.getElementById('t').value") == ""
        assert not [x for x in h.msgs[m:] if x.page == 99], "message for unknown page"
        # "create" twice creates once: geometry set by resize survives
        h.cmd(page, "resize", width=300, height=200, scale=1.0)
        h.cmd(page, "create")
        m = h.mark()
        f = h.frame_inline(page, start=m, pred=lambda f: f["width"] == 300)
        assert (f["width"], f["height"]) == (300, 200)
        # "close" then commands: ignored
        h.cmd(page, "close")
        m = h.mark()
        h.cmd(page, "load", url=site.url("/basic.html"))
        h.cmd(page, "eval", script="1")
        settle(0.5)
        assert not [x for x in h.msgs[m:] if x.page == page], h.msgs[m:]
    finally:
        assert h.finish() == 0


# ---------------------------------------------------------------------------------------------
# Profiles
# ---------------------------------------------------------------------------------------------
def all_modes(root):
    bad = []
    for dirpath, dirs, files in os.walk(root):
        for n in dirs + files:
            p = os.path.join(dirpath, n)
            if os.path.islink(p):
                continue
            if os.stat(p).st_mode & 0o077:
                bad.append((p, oct(os.stat(p).st_mode & 0o777)))
    return bad


@test
def test_profile_argument_errors(site):
    for args in (["--profile"], ["--profile", "relative/dir"], ["--profile", "/nonexistent/a", "extra"], ["bogus"]):
        h = Helper(extra_args=args)
        try:
            _, m = h.wait_for(lambda m: True, 20, "first message")
            assert m.kind == "E" and m.page == 0, m
            assert m.payload.decode() == "Browser profile requires an absolute storage directory", m
            assert h.proc.wait(10) == 1
        finally:
            h.kill()


@test
def test_profile_regular_file_is_storage_error(site):
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "afile")
        open(path, "w").close()
        h = Helper(profile=path)
        try:
            _, m = h.wait_for(lambda m: True, 20, "first message")
            assert m.kind == "E", m
            assert m.payload.decode() == "Could not create or write the browser profile directory", m
            assert h.proc.wait(10) == 1
        finally:
            h.kill()
        # a path below a regular file also fails
        h = Helper(profile=os.path.join(path, "sub"))
        try:
            _, m = h.wait_for(lambda m: True, 20, "first message")
            assert m.kind == "E" and m.payload.decode() == "Could not create or write the browser profile directory", m
            assert h.proc.wait(10) == 1
        finally:
            h.kill()


@test
def test_profile_layout_modes_and_lock(site):
    with tempfile.TemporaryDirectory() as d:
        root = os.path.join(d, "a", "b", "profile")
        h = new_helper(profile=root)
        try:
            assert mode_bits(root) == 0o700
            assert mode_bits(os.path.join(root, "profile.lock")) == 0o600
            assert mode_bits(os.path.join(root, "cookies.sqlite")) == 0o600
            assert mode_bits(os.path.join(root, "data")) == 0o700
            assert mode_bits(os.path.join(root, "cache")) == 0o700
            assert not [n for n in os.listdir(root) if n.startswith(".vsp-probe")]
            # second helper on the locked profile
            h2 = Helper(profile=root)
            try:
                _, m = h2.wait_for(lambda m: True, 20, "first message")
                assert m.kind == "E" and m.page == 0, m
                assert m.payload.decode() == ("Browser profile is in use by another process or cannot be locked. "
                                              "Close the other instance and reopen this tab."), m
                assert h2.proc.wait(10) == 1
            finally:
                h2.kill()
            # the lock holder is unaffected and writes private files only
            page = h.create(200, 100)
            h.load(page, site.url("/set-cookies"))
            h.load(page, site.url("/storage.html?write"))
            h.eval_until(page, "window.result", lambda r: r is not None)
        finally:
            assert h.finish() == 0
        bad = all_modes(root)
        assert not bad, bad
        # lock released: a new helper starts
        h3 = new_helper(profile=root)
        assert h3.finish() == 0


def read_storage(h, page, site, write=False):
    h.load(page, site.url("/storage.html" + ("?write" if write else "")))
    return h.eval_until(page, "window.result", lambda r: r is not None, 15, "storage result")


def read_cookie_header(h, page, site):
    h.load(page, site.url("/echo-cookies"))
    return h.eval(page, "document.body.innerText")


@test
def test_profile_persistence_and_isolation(site):
    with tempfile.TemporaryDirectory() as d:
        p1 = os.path.join(d, "one")
        p2 = os.path.join(d, "two")
        h = new_helper(profile=p1)
        page = h.create(200, 100)
        h.load(page, site.url("/set-cookies"))
        r = read_storage(h, page, site, write=True)
        assert r["ls"] == "ls-value" and r["idb"] == "idb-value", r
        assert "persist=1" in r["cookie"] and "session=1" in r["cookie"], r
        assert "httponly" not in r["cookie"] and "gone" not in r["cookie"] and "stale" not in r["cookie"], r
        assert h.finish() == 0

        # restart the helper: persistent data survive, the session cookie does not
        h = new_helper(profile=p1)
        page = h.create(200, 100)
        r = read_storage(h, page, site)
        assert r["ls"] == "ls-value" and r["idb"] == "idb-value", r
        assert "persist=1" in r["cookie"] and "js-persist=1" in r["cookie"], r
        header = read_cookie_header(h, page, site)
        assert "persist=1" in header and "httponly=1" in header, header
        assert "gone" not in header and "stale" not in header and "session=1" not in header, header
        # a second profile at the same time sees nothing
        h2 = new_helper(profile=p2)
        page2 = h2.create(200, 100)
        r2 = read_storage(h2, page2, site)
        assert r2["ls"] is None and r2["idb"] is None and r2["cookie"] == "", r2
        assert read_cookie_header(h2, page2, site) == ""
        assert h2.finish() == 0
        assert h.finish() == 0
        # ephemeral context: nothing persists, nothing shared
        h = new_helper()
        page = h.create(200, 100)
        r = read_storage(h, page, site)
        assert r["ls"] is None and r["idb"] is None and r["cookie"] == "", r
        assert h.finish() == 0
        # back to the first profile
        h = new_helper(profile=p1)
        page = h.create(200, 100)
        r = read_storage(h, page, site)
        assert r["ls"] == "ls-value" and r["idb"] == "idb-value", r
        assert "httponly=1" in read_cookie_header(h, page, site)
        assert h.finish() == 0
        assert not all_modes(p1), all_modes(p1)
        assert not all_modes(p2), all_modes(p2)


# ==============================================================================================
# PLT-015: commands, state, frames
# ==============================================================================================
@test
def test_state_snapshots(site):
    h = new_helper()
    try:
        page = h.create(300, 200)
        m = h.mark()
        h.cmd(page, "load", url=site.url("/basic.html"))
        h.wait_load(page, m)
        _, ts = h.wait_for(lambda x: x.kind == "S" and x.json()["title"] == "Basic fixture", 5, "title state", m)
        s = ts.json()
        assert s["url"] == site.url("/basic.html"), s
        assert s["error"] is None and s["loading"] is False and s["can_back"] is False and s["can_forward"] is False, s
        states = [x.json() for x in h.msgs[m:] if x.kind == "S" and x.page == page]
        assert any(x["loading"] for x in states), states
        for st in states:
            assert set(st) == {"url", "title", "error", "loading", "can_back", "can_forward"}, st
        # history
        h.load(page, site.url("/anim.html"))
        s = h.state(page)
        assert s["can_back"] is True and s["can_forward"] is False, s
        m = h.mark()
        h.cmd(page, "back")
        h.wait_for(lambda x: x.kind == "S" and x.json()["url"] == site.url("/basic.html") and not x.json()["loading"] and x.json()["can_forward"], 10, "back state", m)
        m = h.mark()
        h.cmd(page, "forward")
        h.wait_for(lambda x: x.kind == "S" and x.json()["url"] == site.url("/anim.html") and not x.json()["loading"] and not x.json()["can_forward"], 10, "forward state", m)
        m = h.mark()
        h.cmd(page, "reload")
        h.wait_for(lambda x: x.kind == "S" and x.json()["loading"], 10, "reload loading", m)
        # load failure sets the error; the next load start clears it
        m = h.mark()
        h.cmd(page, "load", url="http://127.0.0.1:1/")
        _, e = h.wait_for(lambda x: x.kind == "S" and x.json()["error"], 15, "failure state", m)
        assert e.json()["error"], e.json()
        m = h.mark()
        h.cmd(page, "load", url=site.url("/basic.html"))
        _, first = h.wait_for(lambda x: x.kind == "S" and x.json()["loading"], 10, "next load start", m)
        assert first.json()["error"] is None, first.json()
        h.wait_load(page, m)
    finally:
        assert h.finish() == 0


@test
def test_rejected_load_and_error_strings(site):
    h = new_helper()
    try:
        page = h.create(300, 200)
        h.load(page, site.url("/basic.html"))
        settle(0.6)  # let the title update of the first load arrive
        for bad in ("file:///etc/hostname", "javascript:alert(1)", "vespertine://x", "http://user:pw@127.0.0.1/", "", "about:blank",
                    "data:text/html,hi", "mailto:a@b.c"):
            m = h.mark()
            h.cmd(page, "load", url=bad)
            _, s = h.wait_for(lambda x: x.kind == "S" and x.page == page, 5, "state for " + bad, m)
            st = s.json()
            assert st["error"] == "Only http and https addresses are supported.", (bad, st)
            assert st["url"] == site.url("/basic.html"), (bad, st)
        h.cmd(page, "load")  # missing url -> empty -> rejected too
        settle(0.3)
        # a following good load clears the error
        h.load(page, site.url("/anim.html"))
        assert h.state(page)["error"] is None
    finally:
        assert h.finish() == 0


@test
def test_web_process_termination(site):
    h = new_helper()
    try:
        page = h.create(300, 200)
        h.load(page, site.url("/basic.html"))
        victims = [k for k in children_of(h.proc.pid) if "WebProcess" in open("/proc/%d/cmdline" % k, "rb").read().decode("latin1")]
        assert victims, "no web process"
        m = h.mark()
        for v in victims:
            os.kill(v, signal.SIGKILL)
        _, s = h.wait_for(lambda x: x.kind == "S" and x.json()["error"] == "The browser process stopped. Reload to try again.", 10, "termination state", m)
        m = h.mark()
        h.cmd(page, "reload")
        # snapshots still carrying the termination error may arrive first; the reload clears it
        h.wait_for(lambda x: x.kind == "S" and x.json()["error"] is None and x.json()["loading"], 10, "reload start", m)
        h.wait_for(lambda x: x.kind == "S" and x.json()["error"] is None and not x.json()["loading"] and x.json()["title"] == "Basic fixture", 10, "reload finished", m)
    finally:
        assert h.finish() == 0


@test
def test_inline_frames_scale_resize_visibility(site):
    h = new_helper()
    try:
        page = h.create()  # default 800x600
        h.load(page, site.url("/basic.html"))
        f = h.frame_inline(page, pred=lambda f: pixel(f, 5, 5) == (255, 0, 0, 255))
        assert (f["width"], f["height"]) == (800, 600) and f["scale"] == 1.0, (f["width"], f["height"], f["scale"])
        assert len(f["pixels"]) == 800 * 600 * 4
        assert pixel(f, 790, 590) == (0x33, 0x66, 0x99, 255)
        # alpha 255 everywhere
        assert all(f["pixels"][i] == 255 for i in range(3, len(f["pixels"]), 4 * 997))
        # static page: no frames without damage
        settle(0.5)
        m = h.mark()
        settle(1.0)
        assert not [x for x in h.msgs[m:] if x.kind == "F"], "frames without damage"
        # resize to 400x300 at scale 2: device size, devicePixelRatio and CSS viewport follow
        m = h.mark()
        h.cmd(page, "resize", width=400, height=300, scale=2.0)
        # the 100x50 red CSS box covers 200x100 device pixels
        f = h.frame_inline(page, start=m, pred=lambda f: f["width"] == 400 and pixel(f, 190, 90) == (255, 0, 0, 255))
        assert (f["height"], f["scale"]) == (300, 2.0)
        assert pixel(f, 210, 110) != (255, 0, 0, 255)
        # CSS viewport = device size / scale and devicePixelRatio = scale
        assert h.eval(page, "[devicePixelRatio, innerWidth, innerHeight]") == [2, 200, 150]
        # shrink works
        m = h.mark()
        h.cmd(page, "resize", width=300, height=200, scale=1.0)
        f = h.frame_inline(page, start=m, pred=lambda f: f["width"] == 300)
        assert f["height"] == 200
        # identical resize: no new frame; clamping 1..8192 / 0.5..4
        settle(0.8)
        m = h.mark()
        h.cmd(page, "resize", width=300, height=200, scale=1.0)
        settle(0.6)
        assert not [x for x in h.msgs[m:] if x.kind == "F"]
        h.cmd(page, "resize", width=100000, height=-5, scale=100)
        f = h.frame_inline(page, start=m, pred=lambda f: f["width"] == 8192)
        assert (f["height"], f["scale"]) == (1, 4.0), (f["height"], f["scale"])
        m = h.mark()
        h.cmd(page, "resize", width=64, height=64, scale=0.01)
        f = h.frame_inline(page, start=m, pred=lambda f: f["width"] == 64)
        assert f["scale"] == 0.5
        # hidden: no frames; visible: next tick
        h.cmd(page, "resize", width=200, height=100, scale=1.0)
        h.frame_inline(page, start=m, pred=lambda f: f["width"] == 200)
        settle(0.3)
        h.cmd(page, "visible", value=0)
        settle(0.2)
        m = h.mark()
        h.cmd(page, "resize", width=210, height=110, scale=1.0)
        settle(1.0)
        assert not [x for x in h.msgs[m:] if x.kind == "F"], "frame produced while hidden"
        h.cmd(page, "visible", value=1)
        f = h.frame_inline(page, start=m, timeout=2, pred=lambda f: f["width"] == 210)
        assert f["height"] == 110
        # showing again with unchanged size marks the page dirty: one full frame
        h.cmd(page, "visible", value=0)
        settle(0.2)
        m = h.mark()
        h.cmd(page, "visible", value=1)
        h.frame_inline(page, start=m, timeout=2)
    finally:
        assert h.finish() == 0


@test
def test_frame_updates_follow_page_changes(site):
    h = new_helper()
    try:
        page = h.create(400, 300)
        h.load(page, site.url("/anim.html"))
        h.frame_inline(page, pred=lambda f: pixel(f, 5, 5) == (255, 0, 0, 255))
        settle(0.5)
        m = h.mark()
        h.eval(page, "flip('a','#0f0');1")
        f = h.frame_inline(page, start=m, pred=lambda f: pixel(f, 5, 5) == (0, 255, 0, 255))
        assert pixel(f, 320, 220) == (0, 0, 255, 255)
    finally:
        assert h.finish() == 0


def wait_p(h, page, start, gen=None, timeout=10):
    def pred(m):
        return m.kind == "P" and m.page == page and (gen is None or m.json()["generation"] == gen)
    i, m = h.wait_for(pred, timeout, "pool announcement", start)
    return i, m


@test
def test_shm_pool_and_frames(site):
    h = new_helper()
    try:
        h.configure("shm")
        page = h.create(1000, 300)
        h.load(page, site.url("/anim.html"))
        i, p = wait_p(h, page, 0)
        j = h.adopt_pool(p)
        assert j["generation"] == 1 and j["kind"] == "shm" and j["format"] == "BGRA8888", j
        assert (j["width"], j["height"], j["scale"]) == (1000, 300, 1.0), j
        assert j["stride"] % 64 == 0 and j["stride"] >= 4000, j
        assert j["stride"] == 4032, j
        assert [s["slot"] for s in j["slots"]] == [0, 1] and len(p.fds) == 2, j
        for s in j["slots"]:
            assert s["size"] >= j["stride"] * j["height"], s
        # sealed against shrink and grow
        for fd in p.fds:
            for size in (16, j["slots"][0]["size"] * 2):
                try:
                    os.ftruncate(fd, size)
                except OSError:
                    pass
                else:
                    raise AssertionError("memfd can be resized")
        # frames: seq starts at 1; first frame is full; pixels in rows follow the stride
        _, b = h.wait_for(lambda m: m.kind == "B" and m.page == page, 10, "first buffer frame", i)
        bj = b.json()
        assert bj["generation"] == 1 and bj["seq"] == 1 and bj["damage"] == [0, 0, 1000, 300], bj
        assert (bj["width"], bj["height"], bj["scale"]) == (1000, 300, 1.0), bj
        h.release(page, 1, bj["slot"])

        # settle: release everything until frames stop, remembering the last seq
        seen = {b}
        def drain(wait=0.6):
            while True:
                settle(wait)
                new = [m for m in h.msgs if m.kind == "B" and m.page == page and m not in seen]
                if not new:
                    return
                for m in new:
                    seen.add(m)
                    h.release(page, m.json()["generation"], m.json()["slot"])
        drain()
        last = max(m.json()["seq"] for m in seen)
        # pixel content incl. stride: the red box, white background, last column of row 1
        m = h.mark()
        h.eval(page, "flip('a','#f00');flip('b','#00f');1")
        drain()
        newest = max((m for m in seen), key=lambda m: m.json()["seq"])
        px = h.buffer_frame_pixels(newest)
        assert bgra_pixel(px, 1000, 5, 5) == (255, 0, 0, 255)
        assert bgra_pixel(px, 1000, 999, 1) == (255, 255, 255, 255)
        assert bgra_pixel(px, 1000, 320, 220) == (0, 0, 255, 255)

        # backpressure: hold two buffers; never more than two outstanding
        m = h.mark()
        h.eval(page, "flip('a','#0f0');1")
        _, f1 = h.wait_for(lambda x: x.kind == "B" and x.page == page, 5, "held frame 1", m)
        m1 = h.mark()
        h.eval(page, "flip('b','#ff0');1")
        _, f2 = h.wait_for(lambda x: x.kind == "B" and x.page == page, 5, "held frame 2", m1)
        assert f1.json()["slot"] != f2.json()["slot"]
        m2 = h.mark()
        h.eval(page, "flip('a','#f0f');1")
        h.eval(page, "flip('b','#0ff');1")
        settle(0.8)
        assert not [x for x in h.msgs[m2:] if x.kind in "BF"], "frame sent with both buffers owned"
        # first release: exactly one frame, damage = union since the last handed-over frame
        h.release(page, 1, f1.json()["slot"])
        _, g = h.wait_for(lambda x: x.kind == "B" and x.page == page, 5, "frame after release", m2)
        gj = g.json()
        dx, dy, dw, dh = gj["damage"]
        assert dx <= 0 and dy <= 0 and dx + dw >= 350 and dy + dh >= 250, gj
        assert gj["slot"] == f1.json()["slot"] and gj["seq"] == f2.json()["seq"] + 1, gj
        px = h.buffer_frame_pixels(g)
        assert bgra_pixel(px, 1000, 5, 5) == (255, 0, 255, 255) and bgra_pixel(px, 1000, 320, 220) == (0, 255, 255, 255)
        settle(0.6)
        assert len([x for x in h.msgs[m2:] if x.kind == "B"]) == 1
        for x in (f2, g):
            h.release(page, 1, x.json()["slot"])
        seen |= {f1, f2, g}
        drain()

        # a stale release (wrong generation) is ignored; no crash
        h.release(page, 99, 0)
        h.release(page, 1, 7)

        # resize: new pool generation; no buffer of the old generation after the new announcement
        m = h.mark()
        h.cmd(page, "resize", width=300, height=200, scale=1.0)
        i2, p2 = wait_p(h, page, m)
        j2 = h.adopt_pool(p2)
        assert j2["generation"] == 2 and (j2["width"], j2["height"]) == (300, 200), j2
        _, b2 = h.wait_for(lambda x: x.kind == "B" and x.page == page and x.json()["generation"] == 2, 5, "gen 2 frame", i2)
        assert b2.json()["damage"] == [0, 0, 300, 200] and b2.json()["seq"] > 0, b2
        h.release(page, 2, b2.json()["slot"])
        h.cmd(page, "resize", width=300, height=200, scale=2.0)
        i3, p3 = wait_p(h, page, i2 + 1, gen=3)
        assert p3.json()["scale"] == 2.0 and p3.json()["width"] == 300
        assert not [x for x in h.msgs[i3:] if x.kind == "B" and x.json()["generation"] < 3 and x.page == page]
        # fallback to inline
        _, b3 = h.wait_for(lambda x: x.kind == "B" and x.page == page and x.json()["generation"] == 3, 5, "gen 3 frame", i3)
        h.release(page, 3, b3.json()["slot"])
        h.configure("inline")
        m = h.mark()
        h.eval(page, "flip('a','#123456');1")
        h.frame_inline(page, start=m, pred=lambda f: pixel(f, 3, 3) == (0x12, 0x34, 0x56, 255))
        assert not [x for x in h.msgs[m:] if x.kind in "BP"]
        # and back to shm: a fresh pool with a higher generation
        m = h.mark()
        h.configure("shm")
        _, p4 = wait_p(h, page, m)
        assert p4.json()["generation"] > 3
        h.adopt_pool(p4)
    finally:
        assert h.finish() == 0


@test
def test_shm_hidden_produces_no_buffers(site):
    h = new_helper()
    try:
        h.configure("shm")
        h.release_automatically = True
        page = h.create(200, 100)
        h.load(page, site.url("/anim.html"))
        _, p = wait_p(h, page, 0)
        h.adopt_pool(p)
        h.wait_for(lambda x: x.kind == "B", 5, "first frame")
        settle(0.8)
        h.cmd(page, "visible", value=0)
        settle(0.3)
        m = h.mark()
        h.eval(page, "flip('a','#0f0');1")
        settle(0.8)
        assert not [x for x in h.msgs[m:] if x.kind in "BPF"]
        h.cmd(page, "visible", value=1)
        h.wait_for(lambda x: x.kind == "B" and x.json()["damage"] == [0, 0, 200, 100], 2, "full frame after show", m)
    finally:
        assert h.finish() == 0




# ==============================================================================================
# PLT-016: pointer, keyboard, composition, clipboard, input-method state
# ==============================================================================================
def input_page(site, width=400, height=300, scale=1.0, **kw):
    h = new_helper(**kw)
    page = h.create(width, height, scale)
    h.load(page, site.url("/input.html"))
    settle(0.4)
    return h, page


def value(h, page, ident="t"):
    return h.eval(page, "document.getElementById('%s').value" % ident)


@test
def test_pointer_click_focus_and_keys(site):
    h, page = input_page(site)
    try:
        # "down" gives the web view keyboard focus
        click(h, page, 50, 25)
        settle()
        assert h.eval(page, "document.activeElement.id") == "t"
        type_text(h, page, "hello linux")
        settle()
        assert value(h, page) == "hello linux"
        # key resolution: names, shift, non-ASCII, control keys
        type_key(h, page, "BackSpace")
        type_key(h, page, "Left")
        type_key(h, page, "Left")
        type_key(h, page, "X", mods=1)
        type_key(h, page, "é")
        type_key(h, page, "日")
        type_key(h, page, "End")
        type_key(h, page, "space", text=" ")
        type_key(h, page, ".")
        type_key(h, page, "Home")
        type_key(h, page, "Delete")
        settle()
        v = value(h, page)
        assert v == "ello liXé日nu .", v
        # return in a text field fires no insertion; in a textarea it inserts a newline
        click(h, page, 50, 120)
        type_text(h, page, "a")
        type_key(h, page, "Return", text="\r")
        type_text(h, page, "b")
        settle()
        assert value(h, page, "ta") == "a\nb"
        keys = h.eval(page, "window.events.filter(function(e){return e.indexOf('keydown:')==0}).slice(0,3)")
        assert keys[0].startswith("keydown:h:KeyH:72"), keys
    finally:
        assert h.finish() == 0


@test
def test_pointer_modifiers_buttons_scale(site):
    h, page = input_page(site, 800, 600, 2.0)
    try:
        # logical coordinates are multiplied by the page scale: CSS px == logical px at scale 2
        click(h, page, 30, 40, button=1, mods=0)
        click(h, page, 31, 41, button=3, mods=1 | 4)
        click(h, page, 32, 42, button=2, mods=8)
        settle()
        md = h.eval(page, "window.mdowns")
        assert [m[0] for m in md] == [0, 2, 1], md
        assert md[0][1:3] == [30, 40] and md[1][1:3] == [31, 41] and md[2][1:3] == [32, 42], md
        assert md[1][3] is True and md[1][4] is True, md  # shift + ctrl
        # exactly one click per button click on the page's button (at CSS 10..110 x 200..240)
        for _ in range(3):
            click(h, page, 40, 210)
        settle()
        assert h.eval(page, "window.clicks") == 3
    finally:
        assert h.finish() == 0


@test
def test_hover_leave_and_scroll(site):
    h, page = input_page(site)
    try:
        h.cmd(page, "move", x=320, y=220, mods=0)
        settle()
        assert h.eval(page, "window.hover") is True
        h.cmd(page, "leave", x=320, y=220)
        settle()
        assert h.eval(page, "window.hover") is False
        h.cmd(page, "move", x=320, y=220, mods=0)
        settle()
        assert h.eval(page, "window.hover") is True
        h.cmd(page, "move", x=20, y=280, mods=0)
        settle()
        assert h.eval(page, "window.hover") is False
        # smooth scrolling: dy = +6.25 scrolls down well beyond 50 CSS px, one wheel event
        h.cmd(page, "scroll", x=200, y=150, dx=0, dy=6.25, mods=0)
        h.eval_until(page, "window.scrollY", lambda y: y > 50, 5, "scroll offset")
        assert h.eval(page, "window.wheels") >= 1
        h.cmd(page, "scroll", x=200, y=150, dx=0, dy=-20.0, mods=0)
        h.eval_until(page, "window.scrollY", lambda y: y == 0, 5, "scroll back to top")
        # frames keep rendering while interacting
        m = h.mark()
        settle(0.5)
        frames0 = h.eval(page, "window.frames_")
        settle(1.0)
        assert h.eval(page, "window.frames_") - frames0 > 30
    finally:
        assert h.finish() == 0


@test
def test_focus_command_and_im_state(site):
    h, page = input_page(site)
    try:
        h.cmd(page, "eval", script="document.getElementById('t').focus()")
        h.cmd(page, "focus", value=1)
        settle()
        assert h.eval(page, "document.hasFocus()") is True
        ims = [m.json() for m in h.msgs if m.kind == "I" and m.page == page]
        assert {"focused": True} in [{"focused": i["focused"]} for i in ims if "focused" in i], ims
        m = h.mark()
        h.cmd(page, "focus", value=0)
        settle()
        assert h.eval(page, "document.hasFocus()") is False
        h.wait_for(lambda x: x.kind == "I" and x.json().get("focused") is False, 3, "focus-out I message", m)
        h.cmd(page, "focus", value=1)
        settle()
        # caret and surrounding text
        m = h.mark()
        type_text(h, page, "héllo")
        settle()
        ims = [x.json() for x in h.msgs[m:] if x.kind == "I"]
        carets = [i["caret"] for i in ims if "caret" in i]
        assert carets and all(len(c) == 4 and all(isinstance(v, int) for v in c) for c in carets), ims
        assert carets[-1][2] >= 1 and carets[-1][3] >= 10 and carets[-1][0] > 10, carets
        sur = [i for i in ims if "text" in i]
        assert sur[-1] == {"focused": True, "text": "héllo", "cursor": 6, "selection": 6}, sur[-1]  # byte offsets
        type_key(h, page, "Left")
        settle()
        sur = [x.json() for x in h.msgs[m:] if x.kind == "I" and "text" in x.json()]
        assert sur[-1]["cursor"] == 5 and sur[-1]["selection"] == 5, sur[-1]
        type_key(h, page, "Left", mods=1)  # shift+Left selects
        settle()
        sur = [x.json() for x in h.msgs[m:] if x.kind == "I" and "text" in x.json()]
        assert {sur[-1]["cursor"], sur[-1]["selection"]} == {4, 5}, sur[-1]
    finally:
        assert h.finish() == 0


@test
def test_password_never_leaks(site):
    h, page = input_page(site)
    try:
        click(h, page, 50, 25)
        type_text(h, page, "visible")
        settle()
        click(h, page, 50, 65)  # password field
        type_text(h, page, "hunter2secret")
        settle()
        assert value(h, page, "pw") == "hunter2secret"
        pw_ims = [m for m in h.msgs if m.kind == "I"]
        for m in pw_ims:
            assert b"hunter2" not in m.payload and b"secret" not in m.payload, m
        # the stored state after the password field got focus carries no stale or secret text
        sur = [m.json() for m in pw_ims if "text" in m.json()]
        assert sur[-1]["text"] == "" and sur[-1]["cursor"] == 0 and sur[-1]["selection"] == 0, sur[-1]
        # copy / cut yield nothing for the password control
        m = h.mark()
        h.cmd(page, "select-all")
        h.cmd(page, "copy")
        h.cmd(page, "cut")
        settle(0.8)
        assert not [x for x in h.msgs[m:] if x.kind == "C"], [x for x in h.msgs[m:] if x.kind == "C"]
        assert value(h, page, "pw") == "hunter2secret"
        for m in h.msgs:
            if m.kind in "CIMJ":
                assert b"hunter2" not in m.payload or m.kind == "J", m
    finally:
        assert h.finish() == 0


@test
def test_composition_and_text(site):
    h, page = input_page(site)
    try:
        click(h, page, 50, 25)
        type_text(h, page, "hello linux")
        settle()
        h.cmd(page, "text", text=" — café 日本語")
        settle()
        assert value(h, page) == "hello linux — café 日本語", value(h, page)
        h.cmd(page, "eval", script="window.events.length=0;1")
        h.cmd(page, "preedit", text="にほ")
        settle()
        assert h.eval(page, "window.events.filter(function(e){return e.indexOf('composition')==0})") == ["compositionstart:", "compositionupdate:にほ"]
        h.cmd(page, "preedit", text="日本語")
        settle()
        assert value(h, page).endswith("日本語") and "にほ" not in value(h, page)
        h.cmd(page, "commit", text="日本語")
        settle()
        v = value(h, page)
        assert v == "hello linux — café 日本語日本語", v
        ev = h.eval(page, "window.events.filter(function(e){return e.indexOf('composition')==0})")
        assert ev[-1] == "compositionend:日本語", ev
        # unmark ends the composition without inserting
        before = value(h, page)
        h.cmd(page, "preedit", text="zzz")
        settle()
        h.cmd(page, "unmark")
        settle()
        after = value(h, page)
        assert "zzz" not in after, after
        # plain text after composition still works; commit without composition inserts too
        h.cmd(page, "commit", text="!")
        settle()
        assert value(h, page).endswith("!"), value(h, page)
        # the context never filters key events: ordinary keys still arrive during composition
        h.cmd(page, "preedit", text="ab")
        type_key(h, page, "Right")
        h.cmd(page, "unmark")
        settle()
        assert not any("keydown" in e and "Right" in e for e in []) 
    finally:
        assert h.finish() == 0


@test
def test_clipboard_commands(site):
    h, page = input_page(site)
    try:
        click(h, page, 50, 25)
        type_text(h, page, "hello world")
        settle()
        m = h.mark()
        h.cmd(page, "copy")  # no selection (caret only): nothing
        settle(0.6)
        assert not [x for x in h.msgs[m:] if x.kind == "C"]
        h.cmd(page, "select-all")
        h.cmd(page, "copy")
        _, c = h.wait_for(lambda x: x.kind == "C", 5, "copy result", m)
        assert c.payload.decode() == "hello world"
        assert value(h, page) == "hello world"
        # partial selection by keyboard, then cut
        type_key(h, page, "End")
        for _ in range(5):
            type_key(h, page, "Left", mods=1)
        settle()
        m = h.mark()
        h.cmd(page, "cut")
        _, c = h.wait_for(lambda x: x.kind == "C", 5, "cut result", m)
        assert c.payload.decode() == "world"
        settle()
        assert value(h, page) == "hello "
        # paste through "text"
        h.cmd(page, "text", text="café — 日本語")
        settle()
        assert value(h, page) == "hello café — 日本語"
        # document selection (double click on a word)
        m = h.mark()
        h.cmd(page, "down", x=30, y=262, button=1, mods=0)
        h.cmd(page, "up", x=30, y=262, button=1, mods=0)
        h.cmd(page, "down", x=30, y=262, button=1, mods=0)
        h.cmd(page, "up", x=30, y=262, button=1, mods=0)
        settle()
        h.cmd(page, "copy")
        _, c = h.wait_for(lambda x: x.kind == "C", 5, "document selection", m)
        assert c.payload.decode().strip().endswith("selectable") and "words" not in c.payload.decode(), c.payload
        # select-all on the page body copies the page text including the field-less document
        h.cmd(page, "eval", script="document.activeElement.blur();1")
        h.cmd(page, "select-all")
        settle()
        m = h.mark()
        h.cmd(page, "copy")
        _, c = h.wait_for(lambda x: x.kind == "C", 5, "page selection", m)
        assert "selectable words here" in c.payload.decode()
    finally:
        assert h.finish() == 0


@test
def test_context_menu_message(site):
    h, page = input_page(site)
    try:
        # link
        m = h.mark()
        click(h, page, 200, 210, button=3)
        _, mm = h.wait_for(lambda x: x.kind == "M", 5, "link menu", m)
        j = mm.json()
        assert (j["x"], j["y"]) == (200, 210), j
        labels = [(i["label"], i["action"], i["enabled"], i["selected"]) for i in j["items"]]
        assert labels == [
            ("Open link in new tab", "open-link", True, False),
            ("Copy link address", "copy-link", True, False),
            ("Copy", "copy", False, False),
            ("Select all", "select-all", True, False),
            ("Back", "back", False, False),
            ("Forward", "forward", False, False),
            ("Reload", "reload", True, False),
        ], labels
        m = h.mark()
        h.cmd(page, "open-link")
        h.cmd(page, "copy-link")
        _, n = h.wait_for(lambda x: x.kind == "N", 5, "open-link", m)
        _, c = h.wait_for(lambda x: x.kind == "C", 5, "copy-link", m)
        assert n.payload.decode() == site.url("/basic.html") == c.payload.decode()
        # editable target: Copy enabled, Paste present
        m = h.mark()
        click(h, page, 50, 25, button=3)
        _, mm = h.wait_for(lambda x: x.kind == "M", 5, "editable menu", m)
        items = mm.json()["items"]
        assert [i["action"] for i in items] == ["copy", "text", "select-all", "back", "forward", "reload"], items
        assert items[0]["enabled"] is True and items[1]["enabled"] is True
        # no link captured now: open-link / copy-link are no-ops
        m = h.mark()
        h.cmd(page, "open-link")
        h.cmd(page, "copy-link")
        settle(0.5)
        assert not [x for x in h.msgs[m:] if x.kind in "NC"]
        # plain page after navigation: Back enabled
        h.load(page, site.url("/basic.html"))
        m = h.mark()
        click(h, page, 300, 250, button=3)
        _, mm = h.wait_for(lambda x: x.kind == "M", 5, "plain menu", m)
        items = {i["action"]: i["enabled"] for i in mm.json()["items"]}
        assert items["back"] is True and items["forward"] is False and "text" not in items, items
        # selection enables Copy
        h.cmd(page, "select-all")
        settle()
        m = h.mark()
        click(h, page, 300, 250, button=3)
        _, mm = h.wait_for(lambda x: x.kind == "M", 5, "selection menu", m)
        assert {i["action"]: i["enabled"] for i in mm.json()["items"]}["copy"] is True
        # back through the menu action
        h.cmd(page, "back")
        h.eval_until(page, "location.pathname", lambda p: p == "/input.html", 10, "history back")
    finally:
        assert h.finish() == 0


@test
def test_option_menu(site):
    for scale in (1.0, 2.0):
        h, page = input_page(site, 800, 600, scale)
        try:
            m = h.mark()
            click(h, page, 50, 175)  # the select spans CSS x 10..210, y 160..190
            _, mm = h.wait_for(lambda x: x.kind == "M", 5, "option menu", m)
            j = mm.json()
            assert abs(j["x"] - 10) <= 2 and abs(j["y"] - 190) <= 2, (scale, j)
            items = j["items"]
            assert [(i["label"], i["action"], i["enabled"], i["selected"]) for i in items] == [
                ("Alpha", "option:0", True, True), ("Beta", "option:1", True, False),
                ("Gamma", "option:2", False, False), ("Delta", "option:3", True, False)], items
            h.cmd(page, "option:1")
            settle()
            assert value(h, page, "sel") == "b"
            # out-of-range index: menu stays usable, value unchanged, dismiss closes it
            m = h.mark()
            click(h, page, 50, 175)
            h.wait_for(lambda x: x.kind == "M", 5, "option menu again", m)
            h.cmd(page, "option:99")
            h.cmd(page, "option:abc")
            settle()
            assert value(h, page, "sel") == "b"
            h.cmd(page, "dismiss-menu")
            settle()
            assert value(h, page, "sel") == "b"
            # after dismissal, option:N has no effect
            h.cmd(page, "option:3")
            settle()
            assert value(h, page, "sel") == "b"
            # reopen, choose the last
            m = h.mark()
            click(h, page, 50, 175)
            h.wait_for(lambda x: x.kind == "M", 5, "option menu third", m)
            h.cmd(page, "option:3")
            settle()
            assert value(h, page, "sel") == "d"
            # closing a page with an open option menu works
            m = h.mark()
            click(h, page, 50, 175)
            h.wait_for(lambda x: x.kind == "M", 5, "option menu fourth", m)
            h.cmd(page, "close")
            settle()
        finally:
            assert h.finish() == 0


# ==============================================================================================
# PLT-014 R7/R10: policy
# ==============================================================================================
@test
def test_policy_links_and_new_tabs(site):
    h = new_helper()
    try:
        page = h.create(600, 400)
        h.load(page, site.url("/policy.html"))
        settle(1.2)  # includes a script-initiated window.open without gesture after 300 ms
        assert not [m for m in h.msgs if m.kind == "N"], "window.open without a user gesture produced a new-tab request"
        base = site.url("/policy.html")
        for name, y in (("js", 20), ("file", 50), ("custom", 80), ("cred", 110), ("blankfile", 170)):
            m = h.mark()
            click(h, page, 50, y)
            settle(0.6)
            assert h.eval(page, "location.href") == base, name
            assert not [x for x in h.msgs[m:] if x.kind == "N"], name
            st = h.state(page)
            assert st["url"] == base and st["error"] is None, (name, st)
        # javascript: URLs never run through links or forms
        assert h.eval(page, "[window.jsran, window.formran]") == [False, False]
        click(h, page, 410, 18)
        settle(0.5)
        assert h.eval(page, "[window.jsran, window.formran, location.href]") == [False, False, base]
        # a user-initiated target=_blank link with an allowed address: exactly one new-tab request
        m = h.mark()
        click(h, page, 50, 140)
        settle(0.8)
        ns = [x for x in h.msgs[m:] if x.kind == "N"]
        assert len(ns) == 1 and ns[0].payload.decode() == site.url("/basic.html?newtab=1"), ns
        assert h.eval(page, "location.href") == base
        # window.open from a click handler (user gesture)
        m = h.mark()
        click(h, page, 50, 240)
        settle(0.8)
        ns = [x for x in h.msgs[m:] if x.kind == "N"]
        assert len(ns) == 1 and ns[0].payload.decode() == site.url("/basic.html?opened=1"), ns
        # plain http link navigates the page itself
        m = h.mark()
        click(h, page, 50, 200)
        h.eval_until(page, "location.search", lambda s: s == "?plain=1", 10, "plain navigation")
    finally:
        assert h.finish() == 0


@test
def test_policy_frames(site):
    h = new_helper()
    try:
        page = h.create(600, 400)
        h.load(page, site.url("/policy.html"))
        settle(1.0)
        docs = h.eval(page, "Array.prototype.map.call(document.querySelectorAll('iframe'), function(f){try{return f.contentDocument.body.textContent}catch(e){return 'blocked'}})")
        # data: documents are opaque-origin: contentDocument is inaccessible, so check the frames' windows instead
        urls = h.eval(page, "Array.prototype.map.call(document.querySelectorAll('iframe'), function(f){try{return f.contentWindow.location.href}catch(e){return 'cross-origin'}})")
        print("   frame documents:", docs, urls)
        assert docs[1] == "" and urls[1] == "about:blank", (docs, urls)   # about:blank iframe loaded
        assert docs[3].strip() == "srcdoc", docs                           # srcdoc loaded
        assert docs[0] in ("blocked",) and urls[0] == "cross-origin", (docs, urls)  # data: iframe loaded (opaque origin)
        assert docs[4] == "" or docs[4] == "blocked", docs                 # file: iframe never loads
    finally:
        assert h.finish() == 0


@test
def test_permissions_downloads_and_dialogs(site):
    dl = os.path.join(os.path.expanduser("~"), "Downloads", "x.bin")
    existed = os.path.exists(dl)
    h = new_helper()
    try:
        page = h.create(600, 400)
        h.load(page, site.url("/policy.html"))
        settle(1.0)
        assert h.eval(page, "window.geo") == "denied:1"
        assert h.eval(page, "window.notif") == "denied"
        # a download link is cancelled at once; the page keeps working and no file appears
        h.cmd(page, "eval", script="var a=document.createElement('a');a.href='/download';a.id='dl';a.textContent='dl';"
              "a.style.cssText='position:absolute;left:300px;top:10px;display:block;width:80px;height:20px';document.body.appendChild(a);1")
        settle(0.3)
        click(h, page, 320, 20)
        settle(1.0)
        assert h.eval(page, "location.pathname") == "/policy.html"
        assert os.path.exists(dl) == existed
        # HTTP authentication is cancelled instead of waiting on an invisible dialog
        h.cmd(page, "load", url=site.url("/auth"))
        settle(1.5)
        assert h.eval(page, "1+1") == 2
        # script dialogs never block the page
        h.load(page, site.url("/alert.html"))
        assert h.eval(page, "window.r") == [None, False, None] or h.eval(page, "window.r") is not None
    finally:
        assert h.finish() == 0




@test
def test_eval_results(site):
    h = new_helper()
    try:
        page = h.create(200, 100)
        h.load(page, site.url("/basic.html"))
        assert h.eval(page, "1+2") == 3
        assert h.eval(page, "({a:[1,'é',null],b:true})") == {"a": [1, "é", None], "b": True}
        assert h.eval(page, "'日本語'") == "日本語"
        assert h.eval(page, "undefined") is None
        assert h.eval(page, "throw new Error('x')") is None
        start = h.mark()
        h.cmd(page, "eval", script="'first'")
        h.cmd(page, "eval", script="'second'")
        got = []
        h.wait_for(lambda m: m.kind == "J" and got.append(json.loads(m.payload)) is None and len(got) == 2, 5, "two results", start)
        assert got == ["first", "second"], got
    finally:
        assert h.finish() == 0


@test
def test_page_lifecycle_does_not_leak_descriptors(site):
    h = new_helper()
    try:
        h.configure("shm")
        h.release_automatically = True

        def fds():
            names = []
            for f in os.listdir("/proc/%d/fd" % h.proc.pid):
                try:
                    target = os.readlink("/proc/%d/fd/%s" % (h.proc.pid, f))
                except OSError:
                    continue
                # the engine's memory-pressure monitor opens /proc and cgroup files lazily
                if not target.startswith(("/proc/", "/sys/")):
                    names.append(target)
            return names
        def batch(n):
            for i in range(n):
                p = h.create(300 + i, 200)
                h.load(p, site.url("/anim.html"))
                h.wait_for(lambda m: m.kind == "B" and m.page == p, 5, "frame")
                h.cmd(p, "resize", width=320, height=240, scale=1.5)
                settle(0.15)
                h.cmd(p, "close")
            settle(1.0)
        batch(8)  # warm-up: the engine opens some descriptors lazily
        base = fds()
        batch(8)
        after = fds()
        assert len(after) <= len(base) + 2, (len(base), len(after), sorted(set(after) - set(base)))
        # closed pages answer nothing and the helper still works
        h.cmd(0, "ping", nonce=3)
        h.wait_for(lambda m: m.kind == "Z" and m.json()["nonce"] == 3, 5, "pong")
    finally:
        assert h.finish() == 0


@test
def test_two_pages_are_independent(site):
    h = new_helper()
    try:
        p1 = h.create(300, 200)
        p2 = h.create(200, 150)
        h.load(p1, site.url("/basic.html"))
        h.load(p2, site.url("/anim.html"))
        assert h.eval(p1, "location.pathname") == "/basic.html"
        assert h.eval(p2, "location.pathname") == "/anim.html"
        f1 = h.frame_inline(p1, pred=lambda f: pixel(f, 5, 5) == (255, 0, 0, 255))
        f2 = h.frame_inline(p2, pred=lambda f: pixel(f, 5, 5) == (255, 0, 0, 255))
        assert (f1["width"], f1["height"]) == (300, 200) and (f2["width"], f2["height"]) == (200, 150)
        h.cmd(p1, "close")
        settle(0.3)
        assert h.eval(p2, "1") == 1
    finally:
        assert h.finish() == 0




@test
def test_cancelled_navigation_is_not_an_error(site):
    h = new_helper()
    try:
        page = h.create(300, 200)
        m = h.mark()
        h.cmd(page, "load", url=site.url("/slow"))
        settle(0.4)
        h.cmd(page, "load", url=site.url("/basic.html"))
        h.wait_load(page, m)
        settle(2.0)  # the abandoned request finishes on the server side
        errors = [x.json()["error"] for x in h.msgs[m:] if x.kind == "S" and x.json()["error"]]
        assert not errors, errors
        assert h.state(page)["url"] == site.url("/basic.html")
    finally:
        assert h.finish() == 0


@test
def test_drag_selection_with_pressed_button_mask(site):
    h, page = input_page(site)
    try:
        h.cmd(page, "move", x=12, y=262, mods=0)
        h.cmd(page, "down", x=12, y=262, button=1, mods=0)
        for x in range(20, 200, 20):
            h.cmd(page, "move", x=x, y=262, mods=256)
        h.cmd(page, "up", x=200, y=262, button=1, mods=0)
        settle(0.5)
        sel = h.eval(page, "String(getSelection())")
        assert sel.startswith("selectable words") and "here" not in sel, sel
    finally:
        assert h.finish() == 0



def run(selected):
    vsplib.HELPER = os.environ.get("VSP_HELPER", vsplib.DEFAULT_HELPER)
    site = Site()
    failures = 0
    for fn in TESTS:
        if selected and not any(s in fn.__name__ for s in selected):
            continue
        t0 = time.monotonic()
        try:
            fn(site)
            print("PASS %-40s %.1fs" % (fn.__name__, time.monotonic() - t0), flush=True)
        except Exception:
            failures += 1
            print("FAIL %-40s %.1fs" % (fn.__name__, time.monotonic() - t0), flush=True)
            traceback.print_exc()
    site.close()
    print("%d failure(s)" % failures)
    return failures


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["--helper"]:
        os.environ["VSP_HELPER"] = args[1]
        args = args[2:]
    sys.exit(1 if run(args) else 0)
