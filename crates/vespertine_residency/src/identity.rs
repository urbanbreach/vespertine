//! Identity tokens for asynchronous work (PRF-017 R9).
//!
//! A task captures a [`Ticket`] when it is requested. Before it publishes anything it asks the
//! ticket whether the target still has the identity captured at request time: the same row, the
//! same content hash and the same theme. A mismatching result is discarded. The ticket holds only
//! weak references, so a pending task never keeps a removed row, its media or its views alive.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

/// What an asynchronous result is about: the row (or source) identity, the content hash and the
/// theme identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetIdentity {
    /// Identity of the row or source the result is for.
    pub row: u64,
    /// Hash of the source content the result was computed from.
    pub content: u64,
    /// Identity of the theme the result bakes in (0 for theme-neutral work).
    pub theme: u64,
}

/// The current theme identity, shared by every [`IdentitySource`] created from it.
#[derive(Clone, Debug, Default)]
pub struct ThemeIdentity(Arc<AtomicU64>);

impl ThemeIdentity {
    /// A theme identity starting at `id`.
    pub fn new(id: u64) -> Self {
        Self(Arc::new(AtomicU64::new(id)))
    }

    /// The current theme identity.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Changes the theme; tickets captured under another identity stop being current.
    pub fn set(&self, id: u64) {
        self.0.store(id, Ordering::Release);
    }
}

#[derive(Debug)]
struct SourceState {
    row: AtomicU64,
    content: AtomicU64,
}

/// The identity of one target, owned by the row that asynchronous results are published to.
///
/// The row keeps its `IdentitySource`; dropping it (the row was removed) invalidates every
/// outstanding [`Ticket`] without the tickets having kept anything alive.
#[derive(Debug)]
pub struct IdentitySource {
    state: Arc<SourceState>,
    theme: ThemeIdentity,
}

impl IdentitySource {
    /// A source for `row` showing content with hash `content`, under `theme`.
    pub fn new(theme: &ThemeIdentity, row: u64, content: u64) -> Self {
        Self {
            state: Arc::new(SourceState {
                row: AtomicU64::new(row),
                content: AtomicU64::new(content),
            }),
            theme: theme.clone(),
        }
    }

    /// The identity a result must carry to be published now.
    pub fn current(&self) -> TargetIdentity {
        TargetIdentity {
            row: self.state.row.load(Ordering::Acquire),
            content: self.state.content.load(Ordering::Acquire),
            theme: self.theme.get(),
        }
    }

    /// Points the source at another row and content (a recycled row element rebound to new data).
    pub fn rebind(&self, row: u64, content: u64) {
        self.state.row.store(row, Ordering::Release);
        self.state.content.store(content, Ordering::Release);
    }

    /// Changes only the content hash.
    pub fn set_content(&self, content: u64) {
        self.state.content.store(content, Ordering::Release);
    }

    /// Captures the current identity for a request.
    pub fn ticket(&self) -> Ticket {
        Ticket {
            state: Arc::downgrade(&self.state),
            theme: Arc::downgrade(&self.theme.0),
            captured: self.current(),
        }
    }
}

/// The identity captured when work was requested, to be verified before publishing.
#[derive(Clone, Debug)]
pub struct Ticket {
    state: Weak<SourceState>,
    theme: Weak<AtomicU64>,
    captured: TargetIdentity,
}

impl Ticket {
    /// The identity captured at request time.
    pub fn captured(&self) -> TargetIdentity {
        self.captured
    }

    /// Whether the target still exists and still has the captured row, content and theme.
    pub fn is_current(&self) -> bool {
        let (Some(state), Some(theme)) = (self.state.upgrade(), self.theme.upgrade()) else {
            return false;
        };
        state.row.load(Ordering::Acquire) == self.captured.row
            && state.content.load(Ordering::Acquire) == self.captured.content
            && theme.load(Ordering::Acquire) == self.captured.theme
    }

    /// Runs `publish` and returns its result when the ticket is current; otherwise discards
    /// the result without running it.
    ///
    /// Check and publication are not atomic across threads: call this on the thread that owns
    /// the target (the foreground thread), where identity changes also happen.
    pub fn publish<R>(&self, publish: impl FnOnce() -> R) -> Option<R> {
        self.is_current().then(publish)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_is_current_until_row_content_or_theme_changes() {
        let theme = ThemeIdentity::new(1);
        let source = IdentitySource::new(&theme, 7, 100);
        let ticket = source.ticket();
        assert!(ticket.is_current());
        assert_eq!(
            ticket.captured(),
            TargetIdentity {
                row: 7,
                content: 100,
                theme: 1
            }
        );

        source.set_content(101);
        assert!(!ticket.is_current(), "content hash changed");
        source.set_content(100);
        assert!(
            ticket.is_current(),
            "identity equals the captured one again"
        );

        source.rebind(8, 100);
        assert!(!ticket.is_current(), "row identity changed");
        source.rebind(7, 100);

        theme.set(2);
        assert!(!ticket.is_current(), "theme changed");
        assert!(ticket.publish(|| ()).is_none());
        theme.set(1);
        assert_eq!(ticket.publish(|| 5), Some(5));
    }

    #[test]
    fn dropping_the_source_invalidates_tickets_without_being_kept_alive() {
        let theme = ThemeIdentity::new(0);
        let source = IdentitySource::new(&theme, 1, 1);
        let ticket = source.ticket();
        let state = Arc::downgrade(&source.state);
        drop(source);
        assert!(
            state.upgrade().is_none(),
            "the ticket holds no strong reference"
        );
        assert!(!ticket.is_current());
        let mut ran = false;
        assert!(
            ticket
                .publish(|| {
                    ran = true;
                })
                .is_none()
        );
        assert!(!ran);
    }
}
