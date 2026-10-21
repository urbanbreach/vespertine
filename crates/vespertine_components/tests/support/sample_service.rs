//! A deterministic fake message service for the history tests, shared by the unit tests of
//! `streaming::history` and the CMP-029 integration tests.

use vespertine_components::streaming::history::{Direction, Page, Request, around_start};

/// A deterministic fake message service of `total` messages keyed by index (the sample fixture of
/// the spec: 400 messages, variable heights from a seeded generator). Ids are indices, so the
/// cursor of a page is the identity of its first or last message.
#[derive(Clone, Copy, Debug)]
pub struct SampleService {
    /// Total number of messages.
    pub total: usize,
    /// Seed of the height generator.
    pub seed: u64,
}

impl SampleService {
    /// The default fixture: 400 messages.
    pub fn new(seed: u64) -> Self {
        Self { total: 400, seed }
    }

    fn page(&self, start: usize, end: usize) -> Page<usize> {
        let end = end.min(self.total);
        let start = start.min(end);
        Page {
            items: (start..end).collect(),
            previous_cursor: (start > 0).then_some(start),
            next_cursor: (end < self.total && end > start).then_some(end - 1),
        }
    }

    /// The synchronous initial page: the newest page, or the page around `target`.
    pub fn initial_page(&self, page_size: usize, target: Option<usize>) -> Page<usize> {
        match target {
            None => self.page(self.total.saturating_sub(page_size), self.total),
            Some(target) => {
                let start = around_start(
                    target.min(self.total.saturating_sub(1)),
                    page_size,
                    self.total,
                );
                self.page(start, start + page_size)
            }
        }
    }

    /// The asynchronous fetch (answered immediately here; the consumer adds the latency).
    pub fn fetch_page(&self, page_size: usize, request: &Request<usize>) -> Page<usize> {
        match (request.direction, request.cursor) {
            (Direction::Previous, Some(cursor)) => {
                self.page(cursor.saturating_sub(page_size), cursor)
            }
            (Direction::Next, Some(cursor)) => self.page(cursor + 1, cursor + 1 + page_size),
            (Direction::Around, Some(target)) => self.initial_page(page_size, Some(target)),
            (_, None) => self.initial_page(page_size, None),
        }
    }

    /// The deterministic variable height of message `id`, keyed by the seed.
    pub fn height(&self, id: usize) -> f32 {
        let mut x = self
            .seed
            .wrapping_add(id as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        x ^= x >> 29;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 32;
        30.0 + (x % 14) as f32 * 11.0
    }
}
