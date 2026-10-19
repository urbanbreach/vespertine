//! Browser contexts: the process-wide registry of shared helpers and asynchronous page startup
//! (PLT-014 R11, R15, R16).

use crate::helper::{self, Helper, HelperConfig};
use crate::page::NativePage;
use crate::profile::BrowserProfile;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

/// Registry key: the profile root and the explicit helper path (contexts that differ in the
/// helper executable never share a process).
type RegistryKey = (PathBuf, Option<PathBuf>);

static REGISTRY: Mutex<Option<HashMap<RegistryKey, Weak<ContextShared>>>> = Mutex::new(None);

struct ContextShared {
    config: HelperConfig,
    /// The live helper, created on demand. The lock also serializes helper creation so that
    /// several surfaces starting at once result in one helper.
    helper: Mutex<Option<Arc<Helper>>>,
}

/// A cloneable handle to a website-data profile and its shared helper process.
///
/// The default context is ephemeral: it owns a private helper that stores nothing on disk. A
/// context built for a profile shares one helper per profile root within the process, across all
/// windows; the helper stays alive for as long as any handle (or page) exists.
#[derive(Clone)]
pub struct BrowserContext {
    shared: Arc<ContextShared>,
}

impl Default for BrowserContext {
    fn default() -> Self {
        Self::with_config(HelperConfig::default())
    }
}

impl BrowserContext {
    /// The default, ephemeral context.
    pub fn ephemeral() -> Self {
        Self::default()
    }

    /// Obtains or joins the process-wide context of `profile`.
    pub fn for_profile(profile: &BrowserProfile) -> Self {
        Self::for_profile_with_config(profile, HelperConfig::default())
    }

    /// Like [`BrowserContext::for_profile`] with an explicit configuration (helper path,
    /// transfers). The profile root of `config` is replaced by `profile`'s.
    pub fn for_profile_with_config(profile: &BrowserProfile, mut config: HelperConfig) -> Self {
        config.profile_root = Some(profile.root().to_path_buf());
        Self::with_config(config)
    }

    /// A context from a full configuration. Contexts with a profile root and the same helper path
    /// share one helper; contexts without a profile root are private and ephemeral.
    pub fn with_config(config: HelperConfig) -> Self {
        let Some(root) = config.profile_root.clone() else {
            return Self {
                shared: Arc::new(ContextShared {
                    config,
                    helper: Mutex::new(None),
                }),
            };
        };
        let key = (root, config.helper_path.clone());
        let mut registry = REGISTRY.lock();
        let map = registry.get_or_insert_with(HashMap::new);
        // Prune entries whose handles are all gone.
        map.retain(|_, weak| weak.strong_count() > 0);
        if let Some(shared) = map.get(&key).and_then(Weak::upgrade) {
            return Self { shared };
        }
        let shared = Arc::new(ContextShared {
            config,
            helper: Mutex::new(None),
        });
        map.insert(key, Arc::downgrade(&shared));
        Self { shared }
    }

    pub fn config(&self) -> &HelperConfig {
        &self.shared.config
    }

    /// The profile root, or `None` for an ephemeral context.
    pub fn profile_root(&self) -> Option<&std::path::Path> {
        self.shared.config.profile_root.as_deref()
    }

    /// The process id of the live helper, if one is running.
    pub fn helper_pid(&self) -> Option<u32> {
        self.shared
            .helper
            .lock()
            .as_ref()
            .map(|helper| helper.pid())
    }

    /// Whether two handles refer to the same context (and therefore the same helper).
    pub fn same_context(&self, other: &BrowserContext) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Returns the live helper, restarting it when the registered one has failed or its child
    /// exited, or spawning the first one. Blocking: only call it from startup threads.
    pub fn acquire_helper(&self) -> Result<Arc<Helper>, String> {
        let mut slot = self.shared.helper.lock();
        if let Some(helper) = slot.as_ref() {
            if !helper.is_unusable() {
                return Ok(helper.clone());
            }
            slot.take();
        }
        if let Some(root) = &self.shared.config.profile_root {
            // A helper of this profile that was just closed may still hold the profile lock.
            helper::wait_for_shutdowns(root);
        }
        let helper = Arc::new(Helper::spawn(&self.shared.config)?);
        *slot = Some(helper.clone());
        Ok(helper)
    }

    /// Starts a page on a dedicated background thread: helper extraction, spawn, handshake and
    /// the `create` command all run off the calling thread. The returned one-shot receiver
    /// delivers the finished page or an error string; dropping the receiver abandons the page,
    /// which is then closed on the startup thread.
    pub fn start_page(&self) -> async_channel::Receiver<Result<NativePage, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        let context = self.clone();
        let spawned = std::thread::Builder::new()
            .name("webview-startup".into())
            .spawn({
                let sender = sender.clone();
                move || {
                    let result = context
                        .acquire_helper()
                        .and_then(|helper| helper.new_page());
                    // Release the context before delivering, so that a caller that drops its
                    // last handle right after receiving the page can immediately reopen the
                    // profile with a fresh helper.
                    drop(context);
                    // A send to a dropped receiver returns the page, which closes it here.
                    let _ = sender.send_blocking(result);
                }
            });
        if let Err(error) = spawned {
            let _ = sender.try_send(Err(format!(
                "Could not start the browser startup thread: {error}"
            )));
        }
        receiver
    }
}
