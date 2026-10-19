//! The demotion and re-promotion policy of scroll layers (PRF-004 R9, R10), kept apart from
//! the window so that every rule is a deterministic function of the frames it is fed.

use std::collections::VecDeque;

/// Frames of content-change history (R9) and how many of them demote.
const CHANGE_WINDOW: usize = 16;
const CHANGE_THRESHOLD: usize = 8;
/// Completed layer frames the cost estimate looks at (R9), the repaint multiplier, the upkeep
/// added to every sample and the clamp of the per-frame work.
const COST_WINDOW: usize = 32;
pub(super) const REPAINT_COST: f32 = 2.0;
const UPKEEP: f32 = 0.25;
pub(super) const WORK_CLAMP: f32 = 64.0;
/// The recent-excess window: samples looked at, and the sum they must exceed (R9).
const RECENT_WINDOW: usize = 6;
const RECENT_SUM: f32 = 6.0;
/// Repaint work above which a content refresh is expensive, and the distance in frames within
/// which two expensive refreshes demote (R9).
const EXPENSIVE_REFRESH: f32 = 4.0;
const EXPENSIVE_PAIR_FRAMES: u64 = 120;
/// Stable frames before the first re-promotion, the cap of the doubling, the update extension
/// and the quiet frames that reset the backoff (R10).
const BASE_COOLDOWN: u32 = 60;
const MAX_COOLDOWN: u32 = 1920;
const UPDATE_EXTENSION: u32 = 60;
const QUIET_RESET: u32 = 1920;
/// Doublings of the cooldown: 60, 120, 240, 480, 960, then 1,920 at most.
const MAX_DOUBLINGS: u32 = 5;

/// The rule that demoted a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Demotion {
    /// The content changed on at least 8 of the last 16 frames.
    ChangedFrames,
    /// The mean cost of the last 32 completed frames reached the direct-draw cost.
    CostWindow,
    /// At least 3 of the last 6 samples exceeded 1.0 and their sum exceeded 6.0.
    RecentExcess,
    /// Two expensive content refreshes within 120 frames.
    ExpensiveRefreshes,
}

/// The cost history of one layer, in units of one direct draw of the visible content.
#[derive(Debug)]
pub(crate) struct CostHistory {
    /// The first frame after promotion constructs the layer and adds no sample.
    fresh: bool,
    changed: VecDeque<bool>,
    samples: VecDeque<f32>,
    last_expensive: Option<u64>,
}

impl Default for CostHistory {
    fn default() -> Self {
        Self {
            fresh: true,
            changed: VecDeque::new(),
            samples: VecDeque::new(),
            last_expensive: None,
        }
    }
}

impl CostHistory {
    /// Records one completed layer frame, number `frame` of the window. `repainted` says the
    /// content was painted into the layer, `refresh` that the repaint answers a content or
    /// input change rather than the retained region moving, and `work` is the repaint work in
    /// units of one direct draw (0 for a pure composite frame). Returns the rule that demotes
    /// the layer after this frame, if any.
    pub(crate) fn complete_frame(
        &mut self,
        frame: u64,
        repainted: bool,
        refresh: bool,
        work: f32,
    ) -> Option<Demotion> {
        let content_refresh = repainted && !self.fresh && refresh;
        let work = work.clamp(0.0, WORK_CLAMP);
        self.changed.push_back(content_refresh);
        if self.changed.len() > CHANGE_WINDOW {
            self.changed.pop_front();
        }
        if self.fresh {
            self.fresh = false;
        } else {
            self.samples.push_back(work + UPKEEP);
            if self.samples.len() > COST_WINDOW {
                self.samples.pop_front();
            }
        }
        let mut demotion = None;
        if self.changed.iter().filter(|changed| **changed).count() >= CHANGE_THRESHOLD {
            demotion = Some(Demotion::ChangedFrames);
        }
        if demotion.is_none()
            && self.samples.len() >= COST_WINDOW
            && self.samples.iter().sum::<f32>() >= COST_WINDOW as f32
        {
            demotion = Some(Demotion::CostWindow);
        }
        if demotion.is_none() && self.samples.len() >= RECENT_WINDOW {
            let recent = self.samples.iter().rev().take(RECENT_WINDOW);
            let above = recent.clone().filter(|sample| **sample > 1.0).count();
            if above >= RECENT_WINDOW / 2 && recent.sum::<f32>() > RECENT_SUM {
                demotion = Some(Demotion::RecentExcess);
            }
        }
        if content_refresh && work > EXPENSIVE_REFRESH {
            if demotion.is_none()
                && self
                    .last_expensive
                    .is_some_and(|last| frame.saturating_sub(last) <= EXPENSIVE_PAIR_FRAMES)
            {
                demotion = Some(Demotion::ExpensiveRefreshes);
            }
            self.last_expensive = Some(frame);
        }
        demotion
    }

    /// The number of cost samples in the window.
    #[cfg(test)]
    pub(crate) fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// The cooldown a block waits out after its layer was demoted, and its backoff (R10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Backoff {
    /// Demotions so far: the exponent of the cooldown.
    pub(crate) demotions: u32,
    /// Stable frames still required before a layer may be promoted again.
    pub(crate) cooldown: u32,
    /// Frames since the last demotion or content update.
    quiet: u32,
}

impl Backoff {
    /// Starts the cooldown of a demotion: 60 frames, doubling on every further demotion up to
    /// 1,920.
    pub(crate) fn demote(&mut self) {
        self.cooldown = (BASE_COOLDOWN << self.demotions.min(MAX_DOUBLINGS)).min(MAX_COOLDOWN);
        self.demotions = (self.demotions + 1).min(MAX_DOUBLINGS + 1);
        self.quiet = 0;
    }

    /// Advances one frame. `updated` says content changed in the frame for a reason that is
    /// not a scroll, `layer` that the block has a layer. A content update during a cooldown
    /// extends it by 60 frames; 1,920 quiet frames reset the backoff.
    pub(crate) fn frame(&mut self, updated: bool, layer: bool) {
        self.quiet = if updated {
            0
        } else {
            self.quiet.saturating_add(1)
        };
        if self.quiet >= QUIET_RESET {
            self.demotions = 0;
        }
        if self.cooldown > 0 && !layer {
            if updated {
                self.cooldown = (self.cooldown + UPDATE_EXTENSION).min(MAX_COOLDOWN);
            }
            self.cooldown -= 1;
        }
    }

    /// Whether a layer may be promoted.
    pub(crate) fn ready(&self) -> bool {
        self.cooldown == 0
    }

    /// Whether the block still has a cooldown or a backoff to remember.
    pub(crate) fn pending(&self) -> bool {
        self.cooldown > 0 || self.demotions > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of a layer showing a 5-viewport region repaints at 2 * 5 = 10 units.
    const FULL_REPAINT: f32 = 10.0;

    #[test]
    fn construction_adds_no_sample_and_composites_alone_never_demote() {
        let mut history = CostHistory::default();
        assert_eq!(history.complete_frame(1, true, false, FULL_REPAINT), None);
        assert_eq!(history.sample_count(), 0);
        for frame in 2..400 {
            assert_eq!(history.complete_frame(frame, false, false, 0.0), None);
        }
        assert_eq!(history.sample_count(), COST_WINDOW);
    }

    #[test]
    fn eight_content_changes_in_sixteen_frames_demote() {
        let mut history = CostHistory::default();
        history.complete_frame(0, true, false, 0.0);
        // Cheap changes (work 0) isolate the change window from the cost rules.
        for frame in 1..=7 {
            assert_eq!(history.complete_frame(frame, true, true, 0.0), None);
            assert_eq!(history.complete_frame(frame + 100, false, false, 0.0), None);
        }
        assert_eq!(
            history.complete_frame(200, true, true, 0.0),
            Some(Demotion::ChangedFrames)
        );
    }

    #[test]
    fn seven_changes_in_sixteen_frames_do_not_demote() {
        let mut history = CostHistory::default();
        history.complete_frame(0, true, false, 0.0);
        for round in 0..20u64 {
            for step in 0..16u64 {
                let changed = step < 7;
                let frame = 1 + round * 16 + step;
                assert_eq!(history.complete_frame(frame, changed, changed, 0.0), None);
            }
        }
    }

    #[test]
    fn the_cost_window_demotes_when_the_mean_reaches_one() {
        // Work 0.75 per frame: sample 1.0 each, a mean of exactly 1.0 over 32 samples.
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        for frame in 1..32 {
            assert_eq!(history.complete_frame(frame, false, false, 0.75), None);
        }
        assert_eq!(history.sample_count(), 31);
        assert_eq!(
            history.complete_frame(32, false, false, 0.75),
            Some(Demotion::CostWindow)
        );
        // Work 0.74: a mean of 0.99 stays.
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        for frame in 1..200 {
            assert_eq!(history.complete_frame(frame, false, false, 0.74), None);
        }
    }

    #[test]
    fn repaints_spread_over_the_window_demote_through_its_sum() {
        // Three full repaints and 29 composites: 3 * 10.25 + 29 * 0.25 = 38 >= 32, but never
        // three samples above 1.0 within six frames.
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        let mut demoted = None;
        for frame in 1..=32u64 {
            let repaint = frame.is_multiple_of(10);
            let work = if repaint { FULL_REPAINT } else { 0.0 };
            let demotion = history.complete_frame(frame, repaint, false, work);
            assert!(
                demotion.is_none() || frame == 32,
                "demoted early at {frame}"
            );
            demoted = demoted.or(demotion);
        }
        assert_eq!(demoted, Some(Demotion::CostWindow));
    }

    #[test]
    fn the_construction_frame_is_excluded_from_the_cost() {
        let mut history = CostHistory::default();
        // Only the construction frame is expensive; the 32 samples after it are pure upkeep.
        history.complete_frame(0, true, false, 64.0);
        for frame in 1..=40 {
            assert_eq!(history.complete_frame(frame, false, false, 0.0), None);
        }
    }

    #[test]
    fn three_expensive_samples_among_six_demote_promptly() {
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        for (frame, repaint) in [(1, true), (2, false), (3, true), (4, false), (5, false)] {
            let work = if repaint { FULL_REPAINT } else { 0.0 };
            assert_eq!(history.complete_frame(frame, repaint, false, work), None);
        }
        assert_eq!(
            history.complete_frame(6, true, false, FULL_REPAINT),
            Some(Demotion::RecentExcess)
        );
        // Two of six above 1.0 stay.
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        for (frame, repaint) in [(1, true), (2, false), (3, true), (4, false)] {
            let work = if repaint { FULL_REPAINT } else { 0.0 };
            assert_eq!(history.complete_frame(frame, repaint, false, work), None);
        }
        for frame in 5..=12 {
            assert_eq!(history.complete_frame(frame, false, false, 0.0), None);
        }
    }

    #[test]
    fn work_is_clamped_to_sixty_four() {
        let mut history = CostHistory::default();
        history.complete_frame(0, false, false, 0.0);
        // A clamped sample is 64.25; unclamped it would be far larger. Either way one sample
        // alone never demotes, and the clamp keeps five composites from hiding it.
        assert_eq!(history.complete_frame(1, true, false, 1.0e9), None);
        assert!(
            history
                .samples
                .iter()
                .all(|sample| *sample <= WORK_CLAMP + UPKEEP)
        );
    }

    #[test]
    fn two_expensive_refreshes_within_120_frames_demote() {
        let mut history = CostHistory::default();
        history.complete_frame(0, true, false, 0.0);
        assert_eq!(history.complete_frame(10, true, true, FULL_REPAINT), None);
        assert_eq!(
            history.complete_frame(130, true, true, FULL_REPAINT),
            Some(Demotion::ExpensiveRefreshes)
        );
        // 121 frames apart: no demotion.
        let mut history = CostHistory::default();
        history.complete_frame(0, true, false, 0.0);
        assert_eq!(history.complete_frame(10, true, true, FULL_REPAINT), None);
        assert_eq!(history.complete_frame(131, true, true, FULL_REPAINT), None);
    }

    #[test]
    fn a_refresh_of_four_units_is_not_expensive() {
        let mut history = CostHistory::default();
        history.complete_frame(0, true, false, 0.0);
        assert_eq!(history.complete_frame(10, true, true, 4.0), None);
        assert_eq!(history.complete_frame(20, true, true, 4.0), None);
    }

    #[test]
    fn cooldown_doubles_from_60_to_1920_and_stays() {
        let mut backoff = Backoff::default();
        let mut cooldowns = Vec::new();
        for _ in 0..8 {
            backoff.demote();
            cooldowns.push(backoff.cooldown);
            // One content update in the cooldown restarts the quiet count, so that the longest
            // cooldown does not itself amount to 1,920 quiet frames.
            let mut spent = 0;
            while !backoff.ready() {
                backoff.frame(spent == 1, false);
                spent += 1;
            }
        }
        assert_eq!(cooldowns, [60, 120, 240, 480, 960, 1920, 1920, 1920]);
    }

    #[test]
    fn a_content_update_during_the_cooldown_extends_it_by_sixty_frames() {
        let mut backoff = Backoff::default();
        backoff.demote();
        for _ in 0..10 {
            backoff.frame(false, false);
        }
        assert_eq!(backoff.cooldown, 50);
        backoff.frame(true, false);
        assert_eq!(backoff.cooldown, 50 + 60 - 1);
        // No layer means the cooldown runs; with a layer it does not.
        let before = backoff.cooldown;
        backoff.frame(false, true);
        assert_eq!(backoff.cooldown, before);
    }

    #[test]
    fn the_extension_is_capped_at_the_longest_cooldown() {
        let mut backoff = Backoff::default();
        for _ in 0..6 {
            backoff.demote();
        }
        assert_eq!(backoff.cooldown, 1920);
        backoff.demote();
        assert_eq!(backoff.cooldown, 1920);
        backoff.frame(true, false);
        assert_eq!(backoff.cooldown, 1919);
    }

    #[test]
    fn quiet_frames_reset_the_backoff_to_the_initial_cooldown() {
        let mut backoff = Backoff::default();
        for _ in 0..3 {
            backoff.demote();
            while !backoff.ready() {
                backoff.frame(false, false);
            }
        }
        assert_eq!(backoff.demotions, 3);
        // 1,919 more quiet frames in total since the last demotion do not reset ...
        let spent = 240;
        for _ in 0..(QUIET_RESET - spent - 1) {
            backoff.frame(false, true);
        }
        assert_eq!(backoff.demotions, 3);
        // ... the 1,920th does, and the next demotion waits 60 again.
        backoff.frame(false, true);
        assert_eq!(backoff.demotions, 0);
        backoff.demote();
        assert_eq!(backoff.cooldown, 60);
    }

    #[test]
    fn an_update_restarts_the_quiet_count() {
        let mut backoff = Backoff::default();
        backoff.demote();
        for _ in 0..1000 {
            backoff.frame(false, true);
        }
        backoff.frame(true, true);
        for _ in 0..1000 {
            backoff.frame(false, true);
        }
        assert_eq!(backoff.demotions, 1);
    }
}
