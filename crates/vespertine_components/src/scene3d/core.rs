//! Result identity shared by the asynchronous image surfaces (CMP-025 R7, CMP-026 R10).
//!
//! [`FrameCore`] tracks the sequence numbers of a surface, how many frames are in flight and
//! the displayed frame together with the key it was rendered for. It decides, for every
//! arriving result, whether it becomes the displayed frame or is discarded; the caller releases
//! whatever the core hands back. The key type is the surface's own (the 3-D surface keys on
//! camera, size, quality, palette and generation; the shader surface on its uniform inputs).

/// The displayed frame of a surface.
#[derive(Clone, Debug)]
pub struct Displayed<K, P> {
    /// The key the frame was rendered for.
    pub key: K,
    /// Sequence number of the request that produced it.
    pub sequence: u64,
    /// The frame itself.
    pub payload: P,
}

/// Why a result was not displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discard {
    /// The surface no longer exists.
    Removed,
    /// The result belongs to another identity of the surface (the surface was replaced).
    Replaced,
    /// A frame at least as new is already displayed (results arrived out of order).
    Older,
}

/// The fate of a delivered result.
#[derive(Debug)]
pub enum Delivery<P> {
    /// The result is now the displayed frame; `replaced` is the previous frame to release.
    Displayed {
        /// The frame that was displayed before, if any.
        replaced: Option<P>,
    },
    /// The result was dropped and must be released by the caller.
    Discarded {
        /// The reason.
        reason: Discard,
        /// The dropped payload.
        payload: P,
    },
}

impl<P> Delivery<P> {
    /// Whether the result became the displayed frame.
    pub fn is_displayed(&self) -> bool {
        matches!(self, Delivery::Displayed { .. })
    }
}

/// Sequence, in-flight and displayed-frame bookkeeping of one surface identity.
#[derive(Debug)]
pub struct FrameCore<K, P> {
    identity: u64,
    next_sequence: u64,
    in_flight: usize,
    limit: usize,
    removed: bool,
    displayed: Option<Displayed<K, P>>,
}

impl<K: PartialEq, P> FrameCore<K, P> {
    /// A core for surface `identity` allowing one frame in flight.
    pub fn new(identity: u64) -> Self {
        Self {
            identity,
            next_sequence: 1,
            in_flight: 0,
            limit: 1,
            removed: false,
            displayed: None,
        }
    }

    /// The identity of the surface this core belongs to.
    pub fn identity(&self) -> u64 {
        self.identity
    }

    /// Starts a new identity in place: results of requests made under the old identity are
    /// discarded on arrival while the displayed frame stays (a replacement that keeps the
    /// previous frame visible until the first frame of the new one lands).
    pub fn reidentify(&mut self, identity: u64) {
        self.identity = identity;
        self.in_flight = 0;
    }

    /// Sets how many frames may be in flight (at least one).
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    /// Frames in flight.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// The displayed frame.
    pub fn displayed(&self) -> Option<&Displayed<K, P>> {
        self.displayed.as_ref()
    }

    /// The key of the displayed frame.
    pub fn displayed_key(&self) -> Option<&K> {
        self.displayed.as_ref().map(|d| &d.key)
    }

    /// Whether the surface was removed.
    pub fn is_removed(&self) -> bool {
        self.removed
    }

    /// Whether a new request is due: below the in-flight limit and the displayed frame was not
    /// rendered for `current` (a frame recorded for an older key is shown but is never current).
    pub fn needs_request(&self, current: &K) -> bool {
        !self.removed
            && self.in_flight < self.limit
            && self.displayed.as_ref().is_none_or(|d| &d.key != current)
    }

    /// Starts a frame: returns its sequence number.
    pub fn begin(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.in_flight += 1;
        sequence
    }

    fn arrived(&mut self, identity: u64) -> Result<(), Discard> {
        if self.removed {
            return Err(Discard::Removed);
        }
        if identity != self.identity {
            return Err(Discard::Replaced);
        }
        self.in_flight = self.in_flight.saturating_sub(1);
        Ok(())
    }

    /// Delivers a result: it replaces the displayed frame unless the surface was removed or
    /// replaced, or a frame with a sequence number at least as new is already displayed.
    pub fn complete(&mut self, identity: u64, sequence: u64, key: K, payload: P) -> Delivery<P> {
        if let Err(reason) = self.arrived(identity) {
            return Delivery::Discarded { reason, payload };
        }
        if self
            .displayed
            .as_ref()
            .is_some_and(|d| sequence <= d.sequence)
        {
            return Delivery::Discarded {
                reason: Discard::Older,
                payload,
            };
        }
        let replaced = self
            .displayed
            .replace(Displayed {
                key,
                sequence,
                payload,
            })
            .map(|d| d.payload);
        Delivery::Displayed { replaced }
    }

    /// A result that carries no frame (an error): ends the frame in flight. Returns whether it
    /// belonged to this surface.
    pub fn complete_without_frame(&mut self, identity: u64) -> bool {
        self.arrived(identity).is_ok()
    }

    /// Takes the displayed frame out (an explicit resource release); the surface keeps working.
    pub fn take_displayed(&mut self) -> Option<P> {
        self.displayed.take().map(|d| d.payload)
    }

    /// Removes the surface: returns the displayed payload to release; later results are
    /// discarded.
    pub fn remove(&mut self) -> Option<P> {
        self.removed = true;
        self.in_flight = 0;
        self.displayed.take().map(|d| d.payload)
    }
}
