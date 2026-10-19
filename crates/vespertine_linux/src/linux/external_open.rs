//! Handing URLs and paths to the desktop: the default handler, the file manager, and the
//! OpenURI portal as the fallback.
//!
//! Everything runs on the background executor; failures are logged and never reach the UI. The
//! command runners and the portal sit behind [`DesktopOpener`] so the control flow can be
//! exercised with a scripted opener.

use anyhow::Result;
use futures::future::BoxFuture;
use std::{
    ffi::OsStr,
    fs::File,
    os::fd::AsFd as _,
    path::{Path, PathBuf},
    sync::Arc,
};
use vespertine::{BackgroundExecutor, GraphicalEnvironment};

/// The desktop services an open or reveal request can use.
pub(super) trait DesktopOpener: Send + Sync + 'static {
    /// Runs the desktop's opener commands in order and reports whether one succeeded. The
    /// target is passed exactly as given.
    fn run_opener(
        &self,
        target: &OsStr,
        activation_token: Option<&str>,
    ) -> BoxFuture<'static, bool>;

    /// The OpenURI portal's `OpenURI` request.
    fn portal_open_uri(
        &self,
        uri: &str,
        activation_token: Option<String>,
    ) -> BoxFuture<'static, Result<()>>;

    /// The OpenURI portal's `OpenDirectory` request on a file descriptor of the item to reveal.
    fn portal_open_directory(
        &self,
        file: File,
        activation_token: Option<String>,
    ) -> BoxFuture<'static, Result<()>>;
}

/// The real desktop, reached through the opener commands and the session bus.
pub(super) struct SystemOpener {
    pub(super) environment: GraphicalEnvironment,
}

impl DesktopOpener for SystemOpener {
    fn run_opener(
        &self,
        target: &OsStr,
        activation_token: Option<&str>,
    ) -> BoxFuture<'static, bool> {
        let target = target.to_owned();
        let activation_token = activation_token.map(str::to_owned);
        let environment = self.environment.clone();
        Box::pin(async move {
            for mut command in open::commands(&target) {
                environment.apply_to(&mut command);
                if let Some(token) = activation_token.as_deref() {
                    command.env("XDG_ACTIVATION_TOKEN", token);
                }
                let program = format!("{:?}", command.get_program());
                match smol::process::Command::from(command).spawn() {
                    Ok(mut child) => match child.status().await {
                        Ok(status) if status.success() => return true,
                        Ok(status) => {
                            log::error!("Command {program} exited with status: {status}")
                        }
                        Err(error) => {
                            log::error!("Failed to get status from {program}: {error}")
                        }
                    },
                    Err(error) => log::error!("Failed to open with {program}: {error}"),
                }
            }
            false
        })
    }

    fn portal_open_uri(
        &self,
        uri: &str,
        activation_token: Option<String>,
    ) -> BoxFuture<'static, Result<()>> {
        let uri = ashpd::Uri::parse(uri).map_err(anyhow::Error::from);
        Box::pin(async move {
            let uri = uri?;
            match ashpd::desktop::open_uri::OpenFileRequest::default()
                .activation_token(activation_token.map(ashpd::ActivationToken::from))
                .send_uri(&uri)
                .await
                .and_then(|request| request.response())
            {
                Ok(()) => Ok(()),
                // The user dismissed the application chooser.
                Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn portal_open_directory(
        &self,
        file: File,
        activation_token: Option<String>,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                .activation_token(activation_token.map(ashpd::ActivationToken::from))
                .send(&file.as_fd())
                .await?
                .response()?;
            Ok(())
        })
    }
}

/// Opens `url` with the desktop's default handler: the opener commands first, the OpenURI
/// portal if none succeeds. The URL string is handed over exactly as written.
pub(super) async fn open_url(
    opener: &dyn DesktopOpener,
    url: &str,
    activation_token: Option<String>,
) {
    if opener
        .run_opener(url.as_ref(), activation_token.as_deref())
        .await
    {
        return;
    }
    if let Err(error) = opener.portal_open_uri(url, activation_token).await {
        log::error!("Failed to open {url:?} with the OpenURI portal: {error:#}");
    }
}

/// Opens a local path with its default application.
pub(super) async fn open_path(
    opener: &dyn DesktopOpener,
    path: &Path,
    activation_token: Option<String>,
) {
    if !opener
        .run_opener(path.as_os_str(), activation_token.as_deref())
        .await
    {
        log::error!("Failed to open {path:?} with the system's default application");
    }
}

/// Shows `path` in the file manager. The OpenURI portal highlights the item itself; when it
/// fails, the opener runs on the directory (the path when it is one, otherwise its parent).
pub(super) async fn reveal_path(
    opener: &dyn DesktopOpener,
    path: &Path,
    activation_token: Option<String>,
) {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::error!("Can't reveal {path:?}: it can't be opened for reading: {error}");
            return;
        }
    };
    match opener
        .portal_open_directory(file, activation_token.clone())
        .await
    {
        Ok(()) => return,
        Err(error) => {
            log::error!("Failed to reveal {path:?} with the OpenURI portal: {error:#}")
        }
    }
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new(""))
    };
    if !opener
        .run_opener(directory.as_os_str(), activation_token.as_deref())
        .await
    {
        log::error!("Failed to open {directory:?} in the file manager");
    }
}

pub(super) fn spawn_open_url(
    executor: &BackgroundExecutor,
    opener: Arc<dyn DesktopOpener>,
    url: String,
    activation_token: Option<String>,
) {
    executor
        .spawn(async move { open_url(opener.as_ref(), &url, activation_token).await })
        .detach();
}

pub(super) fn spawn_open_path(
    executor: &BackgroundExecutor,
    opener: Arc<dyn DesktopOpener>,
    path: PathBuf,
    activation_token: Option<String>,
) {
    executor
        .spawn(async move { open_path(opener.as_ref(), &path, activation_token).await })
        .detach();
}

pub(super) fn spawn_reveal_path(
    executor: &BackgroundExecutor,
    opener: Arc<dyn DesktopOpener>,
    path: PathBuf,
    activation_token: Option<String>,
) {
    executor
        .spawn(async move { reveal_path(opener.as_ref(), &path, activation_token).await })
        .detach();
}

pub(super) fn open_uri_internal(
    executor: BackgroundExecutor,
    uri: &str,
    activation_token: Option<String>,
    environment: GraphicalEnvironment,
) {
    spawn_open_url(
        &executor,
        Arc::new(SystemOpener { environment }),
        uri.to_owned(),
        activation_token,
    );
}

pub(super) fn reveal_path_internal(
    executor: BackgroundExecutor,
    path: PathBuf,
    activation_token: Option<String>,
    environment: GraphicalEnvironment,
) {
    spawn_reveal_path(
        &executor,
        Arc::new(SystemOpener { environment }),
        path,
        activation_token,
    );
}

pub(super) fn open_path_with_system(
    executor: &BackgroundExecutor,
    path: PathBuf,
    environment: GraphicalEnvironment,
) {
    spawn_open_path(executor, Arc::new(SystemOpener { environment }), path, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::{LinuxDispatcher, PriorityQueueCalloopReceiver};
    use parking_lot::Mutex;
    use std::{sync::mpsc, thread::ThreadId, time::Duration};

    /// Records every call with the thread it ran on and answers from fixed outcomes.
    struct RecordingOpener {
        opener_succeeds: bool,
        portal_uri_fails: bool,
        portal_directory_fails: bool,
        calls: Mutex<Vec<(String, ThreadId)>>,
        done: Mutex<Option<mpsc::Sender<()>>>,
    }

    impl RecordingOpener {
        fn new(
            opener_succeeds: bool,
            portal_uri_fails: bool,
            portal_directory_fails: bool,
        ) -> (Arc<Self>, mpsc::Receiver<()>) {
            let (tx, rx) = mpsc::channel();
            (
                Arc::new(Self {
                    opener_succeeds,
                    portal_uri_fails,
                    portal_directory_fails,
                    calls: Default::default(),
                    done: Mutex::new(Some(tx)),
                }),
                rx,
            )
        }

        fn record(&self, call: String) {
            self.calls.lock().push((call, std::thread::current().id()));
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .iter()
                .map(|(call, _)| call.clone())
                .collect()
        }
    }

    impl DesktopOpener for RecordingOpener {
        fn run_opener(
            &self,
            target: &OsStr,
            activation_token: Option<&str>,
        ) -> BoxFuture<'static, bool> {
            self.record(format!(
                "opener {} token={}",
                target.to_string_lossy(),
                activation_token.unwrap_or("-")
            ));
            let succeeds = self.opener_succeeds;
            Box::pin(std::future::ready(succeeds))
        }

        fn portal_open_uri(
            &self,
            uri: &str,
            _activation_token: Option<String>,
        ) -> BoxFuture<'static, Result<()>> {
            self.record(format!("portal-uri {uri}"));
            let fails = self.portal_uri_fails;
            Box::pin(std::future::ready(if fails {
                Err(anyhow::anyhow!("portal unavailable"))
            } else {
                Ok(())
            }))
        }

        fn portal_open_directory(
            &self,
            _file: File,
            _activation_token: Option<String>,
        ) -> BoxFuture<'static, Result<()>> {
            self.record("portal-directory".to_owned());
            let fails = self.portal_directory_fails;
            Box::pin(std::future::ready(if fails {
                Err(anyhow::anyhow!("portal unavailable"))
            } else {
                Ok(())
            }))
        }
    }

    /// A background executor backed by the real thread pool, so "off the UI thread" is real.
    fn background_executor() -> BackgroundExecutor {
        let (main_sender, _main_receiver) = PriorityQueueCalloopReceiver::new();
        BackgroundExecutor::new(Arc::new(LinuxDispatcher::new(main_sender)))
    }

    /// Signals once the spawned work has run to completion.
    async fn finished(opener: &RecordingOpener) {
        if let Some(done) = opener.done.lock().take() {
            done.send(()).ok();
        }
    }

    fn wait(done: mpsc::Receiver<()>) {
        done.recv_timeout(Duration::from_secs(10))
            .expect("background work did not finish");
    }

    #[test]
    fn url_opens_on_a_background_thread_with_the_original_string() {
        let (opener, done) = RecordingOpener::new(true, false, false);
        let executor = background_executor();
        let ui_thread = std::thread::current().id();
        let url = "HTTPS://Example.COM/a b?x=%zz#frag".to_owned();
        let signal = opener.clone();
        spawn_open_url(
            &executor,
            opener.clone(),
            url.clone(),
            Some("token-1".to_owned()),
        );
        executor
            .spawn(async move {
                // Queued behind the open on the same pool; poll until the call is recorded.
                while signal.calls.lock().is_empty() {
                    smol::future::yield_now().await;
                }
                finished(&signal).await;
            })
            .detach();
        wait(done);
        let calls = opener.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, format!("opener {url} token=token-1"));
        assert_ne!(
            calls[0].1, ui_thread,
            "the opener ran on the calling thread"
        );
    }

    #[test]
    fn failing_opener_falls_back_to_the_portal_and_failures_do_not_panic() {
        let (opener, _done) = RecordingOpener::new(false, true, true);
        smol::block_on(open_url(opener.as_ref(), "https://example.com", None));
        assert_eq!(
            opener.calls(),
            vec![
                "opener https://example.com token=-".to_owned(),
                "portal-uri https://example.com".to_owned()
            ]
        );
    }

    #[test]
    fn successful_opener_does_not_touch_the_portal() {
        let (opener, _done) = RecordingOpener::new(true, false, false);
        smol::block_on(open_url(opener.as_ref(), "https://example.com", None));
        assert_eq!(opener.calls(), vec!["opener https://example.com token=-"]);
    }

    #[test]
    fn open_path_uses_the_opener_on_the_path() {
        let (opener, _done) = RecordingOpener::new(true, false, false);
        smol::block_on(open_path(opener.as_ref(), Path::new("/tmp/a b.txt"), None));
        assert_eq!(opener.calls(), vec!["opener /tmp/a b.txt token=-"]);
        let (failing, _done) = RecordingOpener::new(false, false, false);
        smol::block_on(open_path(failing.as_ref(), Path::new("/tmp/a.txt"), None));
    }

    #[test]
    fn reveal_prefers_the_directory_portal() {
        let directory = tempfile_dir();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"x").unwrap();
        let (opener, _done) = RecordingOpener::new(true, false, false);
        smol::block_on(reveal_path(opener.as_ref(), &file, None));
        assert_eq!(opener.calls(), vec!["portal-directory"]);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn reveal_falls_back_to_the_parent_directory_of_a_file() {
        let directory = tempfile_dir();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"x").unwrap();
        let (opener, _done) = RecordingOpener::new(true, false, true);
        smol::block_on(reveal_path(opener.as_ref(), &file, Some("t".to_owned())));
        assert_eq!(
            opener.calls(),
            vec![
                "portal-directory".to_owned(),
                format!("opener {} token=t", directory.display())
            ]
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn reveal_falls_back_to_the_directory_itself() {
        let directory = tempfile_dir();
        let (opener, _done) = RecordingOpener::new(true, false, true);
        smol::block_on(reveal_path(opener.as_ref(), &directory, None));
        assert_eq!(
            opener.calls(),
            vec![
                "portal-directory".to_owned(),
                format!("opener {} token=-", directory.display())
            ]
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn reveal_of_an_unreadable_path_does_nothing() {
        let (opener, _done) = RecordingOpener::new(true, false, false);
        smol::block_on(reveal_path(
            opener.as_ref(),
            Path::new("/nonexistent/vespertine/reveal"),
            None,
        ));
        assert!(opener.calls().is_empty());
    }

    #[test]
    fn reveal_runs_on_a_background_thread_and_survives_total_failure() {
        let directory = tempfile_dir();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"x").unwrap();
        let (opener, done) = RecordingOpener::new(false, false, true);
        let executor = background_executor();
        let ui_thread = std::thread::current().id();
        spawn_reveal_path(&executor, opener.clone(), file, None);
        let signal = opener.clone();
        executor
            .spawn(async move {
                while signal.calls.lock().len() < 2 {
                    smol::future::yield_now().await;
                }
                finished(&signal).await;
            })
            .detach();
        wait(done);
        let calls = opener.calls.lock();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(_, thread)| *thread != ui_thread));
        std::fs::remove_dir_all(directory).ok();
    }

    fn tempfile_dir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "vespertine-external-open-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }
}
