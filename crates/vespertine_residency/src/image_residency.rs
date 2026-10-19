//! Image residency with atlas release (PRF-017 R6, R7, R8).
//!
//! [`ImageResidency`] is an entity-side driver for [`ClassedResidency`] over decoded images: it
//! reads the clock of the executor, accounts every image with its decoded bytes, schedules the
//! timer that ends a grace period and, whenever an item is evicted, explicitly releases every
//! frame of its image from the sprite atlas of every window (`App::drop_image`) and drops the
//! decoded data. The release never waits for an unrelated event: the grace timer ticks the
//! tracker itself.

use crate::media_residency::{ClassedResidency, ResidencyConfig, ResidencyStats};
use std::{hash::Hash, sync::Arc, time::Instant};
use vespertine::{Context, RenderImage, Task};

/// Decoded bytes of an image: the pixel bytes of every frame.
pub fn decoded_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .map(|frame| image.as_bytes(frame).map_or(0, <[u8]>::len))
        .sum()
}

/// Resident images per class, with grace timers and atlas release.
pub struct ImageResidency<C, K> {
    classes: ClassedResidency<C, K, Arc<RenderImage>>,
    /// The pending grace timer and the deadline it waits for.
    timer: Option<(Instant, Task<()>)>,
    released_images: u64,
}

impl<C, K> ImageResidency<C, K>
where
    C: 'static + Clone + Eq + Hash,
    K: 'static + Clone + Eq + Hash,
{
    /// A residency whose classes use `default` unless configured otherwise.
    pub fn new(default: ResidencyConfig) -> Self {
        Self {
            classes: ClassedResidency::new(default),
            timer: None,
            released_images: 0,
        }
    }

    /// Gives `class` its own budget and grace period.
    pub fn configure(&mut self, class: C, config: ResidencyConfig) {
        self.classes.configure(class, config);
    }

    /// Requests the image of `key` in `class`: the resident image (no decoding, no uploading)
    /// when it is within its grace period or still requested, otherwise `None` and the caller
    /// decodes and calls [`Self::insert`].
    pub fn request(
        &mut self,
        class: &C,
        key: &K,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let now = cx.background_executor().now();
        let image = self.classes.class_mut(class).request(key, now).cloned();
        self.release_evicted(cx);
        image
    }

    /// Makes a freshly decoded image resident and requested. Images evicted by the budget are
    /// released from the atlas.
    pub fn insert(&mut self, class: &C, key: K, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let bytes = decoded_bytes(&image);
        self.classes.class_mut(class).insert(key, image, bytes, now);
        self.release_evicted(cx);
        self.schedule_next(cx);
    }

    /// Releases the request for `key`: the owning content collapsed or scrolled out. The image
    /// stays resident for the grace period and is released by the timer after it.
    pub fn release(&mut self, class: &C, key: &K, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.classes.class_mut(class).release(key, now);
        self.release_evicted(cx);
        self.schedule_next(cx);
    }

    /// Removes `key` at once (its content was removed) and releases its image.
    pub fn remove(&mut self, class: &C, key: &K, cx: &mut Context<Self>) {
        self.classes.class_mut(class).remove(key);
        self.release_evicted(cx);
    }

    /// Enforces every budget and grace period at the current time.
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        if let Some((_, timer)) = self.timer.take() {
            // This is usually the timer task itself, in the middle of its callback.
            timer.detach();
        }
        let now = cx.background_executor().now();
        self.classes.tick(now);
        self.release_evicted(cx);
        self.schedule_next(cx);
    }

    /// Counters of `class`.
    pub fn stats(&self, class: &C) -> ResidencyStats {
        self.classes
            .class(class)
            .map_or_else(ResidencyStats::default, |tracker| tracker.stats())
    }

    /// Decoded bytes resident in every class.
    pub fn resident_bytes(&self) -> usize {
        self.classes.resident_bytes()
    }

    /// Images released from the atlas so far.
    pub fn released_images(&self) -> u64 {
        self.released_images
    }

    /// Releases every evicted image from the atlas of every window. The release is deferred to
    /// the end of the current update so that the window being drawn is visited too.
    fn release_evicted(&mut self, cx: &mut Context<Self>) {
        for evicted in self.classes.take_evicted() {
            self.released_images += 1;
            let image = evicted.value;
            cx.defer(move |cx| cx.drop_image(image, None));
        }
    }

    /// Makes sure a timer waits for the earliest grace deadline.
    fn schedule_next(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.classes.next_deadline() else {
            return;
        };
        if self.timer.as_ref().is_some_and(|(at, _)| *at <= deadline) {
            return;
        }
        let wait = deadline.saturating_duration_since(cx.background_executor().now());
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| this.tick(cx)).ok();
        });
        self.timer = Some((deadline, task));
    }
}
