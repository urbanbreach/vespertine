//! Fakes shared by the capture tests: portal, X server, filesystem, clock,
//! enrichment hook.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::{FutureExt as _, future::BoxFuture};
use parking_lot::Mutex;

use super::{
    Clock, ContextEnricher, EnrichedContext, OwnWindowProbe, WindowIdentity,
    metadata::DesktopFs,
    portal::{PortalError, PortalProbe, ScreenshotPortal, ScreenshotRequest},
    x11::{RawImage, TitleProperties, WindowGeometry, WindowId, X11Error, X11Server},
};

/// Ordered log of boundary calls.
#[derive(Clone, Default)]
pub struct CallLog(pub Arc<Mutex<Vec<String>>>);

impl CallLog {
    pub fn push(&self, entry: impl Into<String>) {
        self.0.lock().push(entry.into());
    }

    pub fn entries(&self) -> Vec<String> {
        self.0.lock().clone()
    }

    pub fn count(&self, entry: &str) -> usize {
        self.0.lock().iter().filter(|e| *e == entry).count()
    }
}

/// Scripted Screenshot portal. Probe results are consumed in order; the last
/// one repeats.
pub struct FakePortal {
    pub log: CallLog,
    pub probes: Mutex<Vec<Result<PortalProbe, PortalError>>>,
    pub response: Mutex<Result<String, PortalError>>,
    pub requests: Mutex<Vec<ScreenshotRequest>>,
    /// When set, `screenshot` waits for this signal before answering.
    pub gate: Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
}

impl FakePortal {
    pub fn new(log: &CallLog, probe: Result<PortalProbe, PortalError>) -> Arc<Self> {
        Arc::new(Self {
            log: log.clone(),
            probes: Mutex::new(vec![probe]),
            response: Mutex::new(Err(PortalError::Failed("no response scripted".into()))),
            requests: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
        })
    }

    pub fn script_probes(&self, probes: Vec<Result<PortalProbe, PortalError>>) {
        *self.probes.lock() = probes;
    }

    pub fn respond_with(&self, response: Result<String, PortalError>) {
        *self.response.lock() = response;
    }

    pub fn requests(&self) -> Vec<ScreenshotRequest> {
        self.requests.lock().clone()
    }
}

impl ScreenshotPortal for FakePortal {
    fn probe(&self) -> BoxFuture<'_, Result<PortalProbe, PortalError>> {
        self.log.push("portal.probe");
        let mut probes = self.probes.lock();
        let result = if probes.len() > 1 {
            probes.remove(0)
        } else {
            probes[0].clone()
        };
        async move { result }.boxed()
    }

    fn screenshot(&self, request: ScreenshotRequest) -> BoxFuture<'_, Result<String, PortalError>> {
        self.log.push("portal.screenshot");
        self.requests.lock().push(request);
        let gate = self.gate.lock().take();
        let response = self.response.lock().clone();
        async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            response
        }
        .boxed()
    }
}

pub struct FakeOwnWindows(pub AtomicBool);

impl OwnWindowProbe for FakeOwnWindows {
    fn own_window_is_active(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Default)]
pub struct FakeWindow {
    pub pid: Option<u32>,
    pub geometry: Option<WindowGeometry>,
    pub title: TitleProperties,
    pub class: Option<Vec<u8>>,
    pub icon: Option<Vec<u32>>,
    pub image: Option<RawImage>,
}

impl FakeWindow {
    pub fn titled(pid: u32, title: &str) -> Self {
        Self {
            pid: Some(pid),
            title: TitleProperties {
                net_wm_name: Some(title.as_bytes().to_vec()),
                wm_name: None,
            },
            geometry: Some(WindowGeometry {
                width: 2,
                height: 2,
            }),
            image: Some(test_image(2, 2)),
            ..Default::default()
        }
    }
}

/// A 32 bit `0x00RRGGBB` image whose pixel `i` has red = `i * 10`.
pub fn test_image(width: u32, height: u32) -> RawImage {
    let mut data = Vec::new();
    for index in 0..width * height {
        data.extend_from_slice(&[0x30, 0x20, (index * 10) as u8, 0]);
    }
    RawImage {
        width,
        height,
        bits_per_pixel: 32,
        bytes_per_line: width as usize * 4,
        msb_first: false,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
        data,
    }
}

/// Scripted X server.
pub struct FakeX11 {
    pub log: CallLog,
    pub active: Mutex<Option<WindowId>>,
    pub windows: Mutex<HashMap<WindowId, FakeWindow>>,
    pub client_list: Mutex<Vec<WindowId>>,
    /// Applied after `get_image` is called: the active window changes.
    pub switch_active_after_image: Mutex<Option<Option<WindowId>>>,
    /// Applied when the enrichment hook is invoked.
    pub image_reads: AtomicUsize,
}

impl FakeX11 {
    pub fn new(log: &CallLog) -> Arc<Self> {
        Arc::new(Self {
            log: log.clone(),
            active: Mutex::new(None),
            windows: Mutex::new(HashMap::new()),
            client_list: Mutex::new(Vec::new()),
            switch_active_after_image: Mutex::new(None),
            image_reads: AtomicUsize::new(0),
        })
    }

    pub fn add_window(&self, id: WindowId, window: FakeWindow) {
        self.windows.lock().insert(id, window);
        self.client_list.lock().push(id);
    }

    fn window<T>(&self, id: WindowId, read: impl FnOnce(&FakeWindow) -> T) -> Result<T, X11Error> {
        self.windows
            .lock()
            .get(&id)
            .map(read)
            .ok_or_else(|| X11Error::bad_window(format!("bad window {id}")))
    }
}

impl X11Server for FakeX11 {
    fn active_window(&self) -> Result<Option<WindowId>, X11Error> {
        self.log.push("x11.active_window");
        Ok(*self.active.lock())
    }

    fn window_pid(&self, window: WindowId) -> Result<Option<u32>, X11Error> {
        self.log.push("x11.window_pid");
        self.window(window, |w| w.pid)
    }

    fn window_geometry(&self, window: WindowId) -> Result<WindowGeometry, X11Error> {
        self.log.push("x11.window_geometry");
        self.window(window, |w| w.geometry)?
            .ok_or_else(|| X11Error::new("no geometry"))
    }

    fn window_title(&self, window: WindowId) -> Result<TitleProperties, X11Error> {
        self.window(window, |w| w.title.clone())
    }

    fn window_class(&self, window: WindowId) -> Result<Option<Vec<u8>>, X11Error> {
        self.window(window, |w| w.class.clone())
    }

    fn window_icon(&self, window: WindowId) -> Result<Option<Vec<u32>>, X11Error> {
        self.window(window, |w| w.icon.clone())
    }

    fn client_list(&self) -> Result<Vec<WindowId>, X11Error> {
        Ok(self.client_list.lock().clone())
    }

    fn get_image(&self, window: WindowId, _width: u32, _height: u32) -> Result<RawImage, X11Error> {
        self.log.push("x11.get_image");
        self.image_reads.fetch_add(1, Ordering::SeqCst);
        let image = self
            .window(window, |w| w.image.clone())?
            .ok_or_else(|| X11Error::new("no image"))?;
        if let Some(next) = self.switch_active_after_image.lock().take() {
            *self.active.lock() = next;
        }
        Ok(image)
    }
}

/// In-memory filesystem; `reported_len` overrides `file_len` to simulate a
/// file that grows between the length check and the read.
#[derive(Default)]
pub struct MemoryFs {
    pub home: Option<PathBuf>,
    pub files: HashMap<PathBuf, Vec<u8>>,
    pub reported_len: HashMap<PathBuf, u64>,
    pub opened: Mutex<Vec<PathBuf>>,
}

impl MemoryFs {
    pub fn with(mut self, path: &str, data: impl Into<Vec<u8>>) -> Self {
        self.files.insert(PathBuf::from(path), data.into());
        self
    }
}

impl DesktopFs for MemoryFs {
    fn file_len(&self, path: &Path) -> io::Result<u64> {
        if let Some(len) = self.reported_len.get(path) {
            return Ok(*len);
        }
        self.files
            .get(path)
            .map(|data| data.len() as u64)
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    fn read_bounded(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        self.opened.lock().push(path.to_owned());
        self.files
            .get(path)
            .map(|data| data[..data.len().min(limit as usize)].to_vec())
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let entries: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|file| file.parent() == Some(path))
            .cloned()
            .collect();
        if entries.is_empty() {
            Err(io::ErrorKind::NotFound.into())
        } else {
            Ok(entries)
        }
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// Clock with a fixed time; `sleep` is either immediately ready or never.
pub struct FakeClock {
    pub now: SystemTime,
    pub sleeps_expire: AtomicBool,
}

impl FakeClock {
    pub fn new(sleeps_expire: bool) -> Arc<Self> {
        Arc::new(Self {
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            sleeps_expire: AtomicBool::new(sleeps_expire),
        })
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        self.now
    }

    fn sleep(&self, _duration: Duration) -> BoxFuture<'static, ()> {
        if self.sleeps_expire.load(Ordering::SeqCst) {
            async {}.boxed()
        } else {
            futures::future::pending().boxed()
        }
    }
}

/// Enrichment hook with scripted behavior.
pub struct FakeEnricher {
    pub available: bool,
    pub calls: Mutex<Vec<WindowIdentity>>,
    /// `None`: the hook future never completes.
    pub result: Mutex<Option<Option<EnrichedContext>>>,
    pub dropped: Arc<AtomicBool>,
    /// Runs when the hook future completes with a result.
    pub on_complete: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl FakeEnricher {
    pub fn new(result: Option<Option<EnrichedContext>>) -> Arc<Self> {
        Arc::new(Self {
            available: true,
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(result),
            dropped: Arc::new(AtomicBool::new(false)),
            on_complete: Mutex::new(None),
        })
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl ContextEnricher for FakeEnricher {
    fn is_available(&self) -> BoxFuture<'static, bool> {
        let available = self.available;
        async move { available }.boxed()
    }

    fn enrich(&self, identity: &WindowIdentity) -> BoxFuture<'static, Option<EnrichedContext>> {
        self.calls.lock().push(identity.clone());
        let result = self.result.lock().clone();
        let flag = DropFlag(self.dropped.clone());
        let on_complete = self.on_complete.lock().take();
        async move {
            let _flag = flag;
            match result {
                Some(result) => {
                    if let Some(on_complete) = on_complete {
                        on_complete();
                    }
                    result
                }
                None => futures::future::pending().await,
            }
        }
        .boxed()
    }
}
