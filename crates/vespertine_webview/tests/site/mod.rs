//! A loopback test site served from a thread: pages with form fields, counters, long content,
//! navigation targets and storage scripts, plus cookie endpoints.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct TestSite {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl TestSite {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(stream) = stream {
                    std::thread::spawn(move || {
                        let _ = serve(stream, port);
                    });
                }
            }
        });
        Self { port, stop }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for TestSite {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(stream: TcpStream, port: u16) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut cookie_header = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("cookie:") {
            let start = line.len() - value.len();
            cookie_header = line[start..].trim().to_owned();
        }
    }
    let (status, headers, body) = route(&path, &cookie_header, port);
    let mut stream = stream;
    let content_type = if headers.contains("Content-Type") {
        ""
    } else {
        "Content-Type: text/html; charset=utf-8\r\n"
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{content_type}{headers}\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())
}

fn route(path: &str, cookies: &str, port: u16) -> (&'static str, String, String) {
    let path_only = path.split('?').next().unwrap_or("/");
    match path_only {
        "/" => ("200 OK", String::new(), index(port)),
        "/solid" => (
            "200 OK",
            String::new(),
            "<!doctype html><title>Solid</title><body style='margin:0;background:#2060c0'>solid</body>".to_owned(),
        ),
        "/second" => ("200 OK", String::new(), "<title>Second</title><body style='background:#fff'>second page</body>".to_owned()),
        "/set-cookies" => (
            "200 OK",
            [
                "Set-Cookie: persist=yes; Max-Age=3600; Path=/\r\n",
                "Set-Cookie: secret=hidden; Max-Age=3600; Path=/; HttpOnly\r\n",
                "Set-Cookie: expired=1; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/\r\n",
                "Set-Cookie: doomed=1; Max-Age=3600; Path=/\r\n",
                "Set-Cookie: doomed=; Max-Age=0; Path=/\r\n",
            ]
            .concat(),
            "<title>cookies set</title>cookies set".to_owned(),
        ),
        "/echo-cookies" => (
            "200 OK",
            String::new(),
            format!("<title>echo</title><pre id=h>{}</pre><pre id=j></pre><script>document.getElementById('j').textContent=document.cookie</script>", html_escape(cookies)),
        ),
        "/storage" => (
            "200 OK",
            String::new(),
            r#"<title>storage</title><script>
localStorage.setItem('vsp','remembered');
const open = indexedDB.open('vsp-db', 1);
open.onupgradeneeded = () => open.result.createObjectStore('s');
open.onsuccess = () => { const tx = open.result.transaction('s','readwrite'); tx.objectStore('s').put('idb-value','k'); tx.oncomplete = () => { document.title = 'stored'; }; };
</script>storing"#.to_owned(),
        ),
        "/read-storage" => (
            "200 OK",
            String::new(),
            r#"<title>reading</title><script>
const result = { ls: localStorage.getItem('vsp'), idb: null };
const open = indexedDB.open('vsp-db', 1);
open.onupgradeneeded = () => open.result.createObjectStore('s');
open.onsuccess = () => {
  const get = open.result.transaction('s').objectStore('s').get('k');
  get.onsuccess = () => { result.idb = get.result === undefined ? null : get.result; document.title = 'read:' + JSON.stringify(result); };
};
</script>reading"#.to_owned(),
        ),
        "/download" => (
            "200 OK",
            "Content-Disposition: attachment; filename=\"x.bin\"\r\nContent-Type: application/octet-stream\r\n".to_owned(),
            "downloaded bytes".to_owned(),
        ),
        _ => ("404 Not Found", String::new(), "not found".to_owned()),
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

fn index(port: u16) -> String {
    format!(
        r##"<!doctype html><html><head><meta charset=utf-8><title>Embedded page</title>
<style>
body {{ margin: 0; background: #fff; font: 16px sans-serif; }}
h1 {{ font-size: 24px; margin: 8px; color: #20242C; }}
#tall {{ height: 3000px; background: linear-gradient(#fff, #ccc); }}
input, button, select {{ margin: 4px; font-size: 16px; }}
#f {{ width: 360px; }}
</style></head><body>
<h1>Embedded page</h1>
<input id=f type=text placeholder="field">
<input id=p type=password value="s3cret">
<button id=inc onclick="window.counter=(window.counter||0)+1;document.getElementById('count').textContent=window.counter">count</button>
<span id=count></span>
<select id=s><option value=a>Alpha</option><option value=b>Beta</option><option value=c>Gamma</option></select>
<div>
<a id=l-js href="javascript:window.jsran=1">js</a>
<a id=l-file href="file:///etc/hostname">file</a>
<a id=l-custom href="x-app://open/thing">custom</a>
<a id=l-cred href="http://user:pw@127.0.0.1:{port}/second">cred</a>
<a id=l-blank target=_blank href="http://127.0.0.1:{port}/second">blank</a>
<a id=l-ok href="http://127.0.0.1:{port}/second">ok</a>
</div>
<script>window.frames_loaded = 0;</script>
<iframe id=if-data onload="window.frames_loaded++" src="data:text/html,<title>d</title>data-frame"></iframe>
<iframe id=if-blank onload="window.frames_loaded++" src="about:blank"></iframe>
<script>
if (location.search.includes('open=1')) setTimeout(() => window.open('http://127.0.0.1:{port}/second'), 100);
window.frameCounter = 0; (function tick() {{ window.frameCounter++; requestAnimationFrame(tick); }})();
</script>
<div id=tall></div>
</body></html>"##
    )
}
