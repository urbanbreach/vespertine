"""Library of the helper harness: fixture HTTP server, helper process wrapper, pixel helpers."""
import array
import collections
import http.server
import json
import mmap
import os
import signal
import socket
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
# The helper under test: $VSP_HELPER, else the development build in the git-ignored target dir.
DEFAULT_HELPER = os.path.normpath(os.path.join(HERE, "..", "..", "..", "..", "target", "webview-dev", "vsp-helper"))
HELPER = os.environ.get("VSP_HELPER", DEFAULT_HELPER)


# ----------------------------------------------------------------------------------------------
# Fixture web server
# ----------------------------------------------------------------------------------------------
class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=HERE, **kwargs)

    def log_message(self, *args):
        pass

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/set-cookies":
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            expired = "Thu, 01 Jan 1970 00:00:01 GMT"
            future = "Fri, 01 Jan 2100 00:00:00 GMT"
            self.send_header("Set-Cookie", "persist=1; Path=/; Expires=" + future)
            self.send_header("Set-Cookie", "httponly=1; Path=/; HttpOnly; Expires=" + future)
            self.send_header("Set-Cookie", "gone=1; Path=/; Expires=" + future)
            self.send_header("Set-Cookie", "gone=; Path=/; Expires=" + expired)
            self.send_header("Set-Cookie", "stale=1; Path=/; Expires=" + expired)
            self.send_header("Set-Cookie", "session=1; Path=/")
            self.end_headers()
            self.wfile.write(b"<html><body>cookies set</body></html>")
            return
        if path == "/echo-cookies":
            cookie = self.headers.get("Cookie", "")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(cookie.encode())
            return
        if path == "/download":
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", "attachment; filename=x.bin")
            self.end_headers()
            self.wfile.write(b"x" * 1000)
            return
        if path == "/slow":
            time.sleep(1.5)
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.end_headers()
            self.wfile.write(b"<html><body>slow</body></html>")
            return
        if path == "/auth":
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="x"')
            self.end_headers()
            return
        return super().do_GET()


class ThreadingServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


class Site:
    def __init__(self):
        self.server = ThreadingServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def url(self, path="/"):
        return "http://127.0.0.1:%d%s" % (self.port, path)

    def close(self):
        self.server.shutdown()
        self.server.server_close()


# ----------------------------------------------------------------------------------------------
# Helper process wrapper
# ----------------------------------------------------------------------------------------------
class Msg:
    __slots__ = ("kind", "page", "payload", "fds", "t")

    def __init__(self, kind, page, payload, fds):
        self.kind = kind
        self.page = page
        self.payload = payload
        self.fds = fds
        self.t = time.monotonic()

    def json(self):
        return json.loads(self.payload.decode("utf-8"))

    def __repr__(self):
        p = self.payload if len(self.payload) < 200 else self.payload[:200] + b"..."
        return "Msg(%s, page=%d, %r, fds=%d)" % (self.kind, self.page, p, len(self.fds))


class Helper:
    def __init__(self, profile=None, env=None, extra_args=None, helper=None, read_first=True):
        self.sock, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        self.child_fd = child.fileno()
        args = [helper or os.environ.get('VSP_HELPER', DEFAULT_HELPER), "--channel", str(child.fileno())]
        if profile is not None:
            args += ["--profile", profile]
        if extra_args:
            args += extra_args
        full_env = dict(os.environ)
        if env:
            full_env.update(env)
        self.stderr = tempfile.TemporaryFile()
        self.proc = subprocess.Popen(args, pass_fds=[child.fileno()], env=full_env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=self.stderr)
        child.close()
        self.cond = threading.Condition()
        self.msgs = []
        self.eof = False
        self.violation = False
        self.stream = bytearray()
        self.pending_fds = collections.deque()
        self.t0 = time.monotonic()
        self.reader = threading.Thread(target=self._read_loop, daemon=True)
        self.reader.start()
        self.next_page = 1
        self.pools = {}  # page -> {generation: pool}
        self.release_automatically = False
        self.send_lock = threading.Lock()

    # -- reading ------------------------------------------------------------------------------
    def _read_loop(self):
        fds_space = socket.CMSG_LEN(struct.calcsize("i") * 16)
        while True:
            try:
                data, anc, flags, _ = self.sock.recvmsg(1 << 20, fds_space)
            except OSError:
                data = b""
                anc = []
            for level, typ, cdata in anc:
                if level == socket.SOL_SOCKET and typ == socket.SCM_RIGHTS:
                    n = len(cdata) // 4
                    self.pending_fds.extend(struct.unpack("%di" % n, cdata[: n * 4]))
            if not data:
                with self.cond:
                    self.eof = True
                    self.cond.notify_all()
                return
            self.stream += data
            self._parse()

    def _parse(self):
        while len(self.stream) >= 9:
            kind, page, length = struct.unpack("<cII", bytes(self.stream[:9]))
            if len(self.stream) < 9 + length:
                return
            payload = bytes(self.stream[9:9 + length])
            del self.stream[:9 + length]
            kind = kind.decode("latin1")
            fds = []
            if kind == "P":
                n = len(json.loads(payload)["slots"])
                for _ in range(n):
                    fds.append(self.pending_fds.popleft())
            with self.cond:
                self.msgs.append(Msg(kind, page, payload, fds))
                self.cond.notify_all()
            if kind == "B" and self.release_automatically:
                j = json.loads(payload)
                try:
                    self.cmd(page, "release", generation=j["generation"], slot=j["slot"])
                except OSError:
                    pass

    def wait_for(self, pred, timeout=10.0, what="message", start=0):
        """Waits until some message at index >= start satisfies pred; returns (index, msg)."""
        deadline = time.monotonic() + timeout
        i = start
        with self.cond:
            while True:
                while i < len(self.msgs):
                    if pred(self.msgs[i]):
                        return i, self.msgs[i]
                    i += 1
                if self.eof:
                    raise AssertionError("helper closed the channel while waiting for %s\nstderr: %s" % (what, self.stderr_text()))
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError("timeout waiting for %s; got %s" % (what, self.msgs[-8:]))
                self.cond.wait(remaining)

    def mark(self):
        with self.cond:
            return len(self.msgs)

    def stderr_text(self):
        try:
            self.stderr.seek(0)
            return self.stderr.read().decode("utf-8", "replace")
        except Exception:
            return ""

    # -- writing ------------------------------------------------------------------------------
    def send_raw(self, kind, page, payload):
        with self.send_lock:
            self.sock.sendall(struct.pack("<cII", kind.encode(), page, len(payload)) + payload)

    def cmd(self, page, name, **kw):
        kw["cmd"] = name
        self.send_raw("K", page, json.dumps(kw).encode())

    # -- handshake ----------------------------------------------------------------------------
    def handshake(self, timeout=20.0):
        _, m = self.wait_for(lambda m: True, timeout, "first message")
        return m

    def configure(self, transfer):
        self.cmd(0, "configure", transfer=transfer)

    # -- pages --------------------------------------------------------------------------------
    def create(self, width=None, height=None, scale=1.0, visible=True):
        page = self.next_page
        self.next_page += 1
        self.cmd(page, "create")
        if width is not None:
            self.cmd(page, "resize", width=width, height=height, scale=scale)
        return page

    def load(self, page, url, wait=True, timeout=20.0):
        start = self.mark()
        self.cmd(page, "load", url=url)
        if wait:
            self.wait_load(page, start, timeout)

    def wait_load(self, page, start=0, timeout=20.0):
        """Waits for a finished, non-loading state snapshot after `start`."""
        seen_started = [False]

        def pred(m):
            if m.kind != "S" or m.page != page:
                return False
            s = m.json()
            if s.get("error"):
                return True
            if s["loading"]:
                seen_started[0] = True
                return False
            return seen_started[0] and s["url"] is not None
        return self.wait_for(pred, timeout, "load finished", start)[1].json()

    def state(self, page):
        for m in reversed(self.msgs):
            if m.kind == "S" and m.page == page:
                return m.json()
        return None

    def eval(self, page, script, timeout=10.0):
        start = self.mark()
        self.cmd(page, "eval", script=script)
        _, m = self.wait_for(lambda m: m.kind == "J" and m.page == page, timeout, "eval result", start)
        return json.loads(m.payload.decode("utf-8"))

    def eval_until(self, page, script, pred, timeout=10.0, what="eval condition"):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            last = self.eval(page, script)
            if pred(last):
                return last
            time.sleep(0.05)
        raise AssertionError("%s not reached; last value %r" % (what, last))

    # -- frames -------------------------------------------------------------------------------
    def frame_inline(self, page, start=0, timeout=10.0, pred=None):
        def p(m):
            if m.kind != "F" or m.page != page:
                return False
            return pred is None or pred(self.decode_inline(m))
        _, m = self.wait_for(p, timeout, "inline frame", start)
        return self.decode_inline(m)

    @staticmethod
    def decode_inline(m):
        w, h, sbits = struct.unpack("<IIf", m.payload[:12])
        # struct "f" is the IEEE single
        return {"width": w, "height": h, "scale": sbits, "pixels": m.payload[12:]}

    def pool_for(self, page, generation):
        return self.pools[(page, generation)]

    def adopt_pool(self, m):
        j = m.json()
        slots = []
        for i, s in enumerate(j["slots"]):
            fd = m.fds[i]
            size = s["size"]
            mm = mmap.mmap(fd, size, mmap.MAP_SHARED, mmap.PROT_READ)
            slots.append({"fd": fd, "map": mm, "size": size, "slot": s["slot"]})
        self.pools[(m.page, j["generation"])] = {"info": j, "slots": slots}
        return j

    def buffer_frame_pixels(self, m):
        j = m.json()
        pool = self.pools[(m.page, j["generation"])]
        info = pool["info"]
        slot = pool["slots"][j["slot"]]
        stride = info["stride"]
        w, h = j["width"], j["height"]
        rows = []
        for y in range(h):
            rows.append(bytes(slot["map"][y * stride:y * stride + w * 4]))
        return b"".join(rows)

    def release(self, page, generation, slot):
        self.cmd(page, "release", generation=generation, slot=slot)

    # -- shutdown -----------------------------------------------------------------------------
    def close_channel(self):
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.sock.close()

    def finish(self, timeout=10.0):
        """Closes the channel and returns the exit status."""
        self.close_channel()
        try:
            return self.proc.wait(timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            raise AssertionError("helper did not exit after channel close")

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()
        try:
            self.sock.close()
        except OSError:
            pass


# ----------------------------------------------------------------------------------------------
# Pixel helpers
# ----------------------------------------------------------------------------------------------
def pixel(frame, x, y):
    w = frame["width"]
    i = (y * w + x) * 4
    b, g, r, a = frame["pixels"][i:i + 4]
    return (r, g, b, a)


def bgra_pixel(pixels, width, x, y):
    i = (y * width + x) * 4
    b, g, r, a = pixels[i:i + 4]
    return (r, g, b, a)


def mode_bits(path):
    return os.stat(path).st_mode & 0o777




def new_helper(**kw):
    """Spawns the helper and waits for its ready message."""
    h = Helper(**kw)
    first = h.handshake()
    if first.kind != "R":
        raise AssertionError("expected ready, got %r\n%s" % (first, h.stderr_text()))
    return h


# ----------------------------------------------------------------------------------------------
# Input helpers
# ----------------------------------------------------------------------------------------------
def click(h, page, x, y, button=1, mods=0, wait=0.35):
    """Move, press, release. The engine queues pointer and keyboard events separately and does not
    order them across kinds, so tests wait briefly before sending dependent events."""
    h.cmd(page, "move", x=x, y=y, mods=mods)
    h.cmd(page, "down", x=x, y=y, button=button, mods=mods)
    h.cmd(page, "up", x=x, y=y, button=button, mods=mods)
    if wait:
        time.sleep(wait if button != 3 else max(wait, 0.5))


def type_key(h, page, key, text=None, mods=0):
    if text is None:
        text = key if len(key) == 1 else ""
    h.cmd(page, "key_down", key=key, text=text, mods=mods)
    h.cmd(page, "key_up", key=key, text="", mods=mods)


def type_text(h, page, text):
    for ch in text:
        type_key(h, page, ch)


def settle(seconds=0.3):
    time.sleep(seconds)


def children_of(pid):
    """All descendant pids of pid."""
    out = []
    todo = [pid]
    while todo:
        p = todo.pop()
        try:
            with open("/proc/%d/task/%d/children" % (p, p)) as f:
                kids = [int(x) for x in f.read().split()]
        except OSError:
            kids = []
        out += kids
        todo += kids
    return out
