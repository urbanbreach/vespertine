//! Click-versus-drag activation of image chips (CMP-010 R9).

use super::reference::ReferenceDescriptor;
use std::ops::Range;

/// The pointer may move at most this many logical pixels (Euclidean) between press and release
/// for the release to count as a click.
pub const CLICK_DRAG_THRESHOLD: f32 = 4.0;

/// What the owner's activation callback receives (`ActivateReference(number)`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationTarget {
    /// The activated chip's source range.
    pub range: Range<usize>,
    /// The attachment number the chip references.
    pub attachment: Option<u32>,
}

/// Press/release tracking for activatable (image) chips.
#[derive(Clone, Debug, Default)]
pub struct ImageChipActivation {
    pressed: Option<(ActivationTarget, (f32, f32))>,
}

impl ImageChipActivation {
    /// No press recorded.
    pub fn new() -> Self {
        Self::default()
    }

    /// A primary press at `position` (logical pixels) over `chip` (`None`: not over a chip).
    /// Only a plain press — no Shift, a click count of one — that begins on an activatable chip
    /// records it; every other press clears the record and is just selection.
    pub fn press(
        &mut self,
        chip: Option<&ReferenceDescriptor>,
        position: (f32, f32),
        shift: bool,
        click_count: u32,
    ) {
        self.pressed = match chip {
            Some(chip) if chip.kind.is_activatable() && !shift && click_count == 1 => Some((
                ActivationTarget {
                    range: chip.range.clone(),
                    attachment: chip.attachment,
                },
                position,
            )),
            _ => None,
        };
    }

    /// The primary button was released at `position` over `chip`. Returns the target to activate
    /// only if the press began on an activatable chip, the release is over the same chip (same
    /// source range) and the pointer moved no more than 4 px from the press point.
    pub fn release(
        &mut self,
        chip: Option<&ReferenceDescriptor>,
        position: (f32, f32),
    ) -> Option<ActivationTarget> {
        let (target, origin) = self.pressed.take()?;
        let chip = chip?;
        if chip.range != target.range {
            return None;
        }
        let distance = (position.0 - origin.0).hypot(position.1 - origin.1);
        (distance <= CLICK_DRAG_THRESHOLD).then_some(target)
    }

    /// Forgets a recorded press (focus lost, capture cancelled, edit during the press).
    pub fn cancel(&mut self) {
        self.pressed = None;
    }

    /// Whether a press on an activatable chip is pending.
    pub fn is_pending(&self) -> bool {
        self.pressed.is_some()
    }
}

/// Whether the pointer cursor over `chip` is the pointing hand (activatable chips).
pub fn shows_pointing_hand(chip: &ReferenceDescriptor) -> bool {
    chip.kind.is_activatable()
}

#[cfg(test)]
mod tests {
    use super::super::reference::{ReferenceKind, test_support::descriptor};
    use super::*;

    fn image(range: Range<usize>, number: u32) -> ReferenceDescriptor {
        descriptor(
            range,
            ReferenceKind::Image,
            "Image 1.png",
            "p",
            Some(number),
        )
    }

    fn click(distance: (f32, f32)) -> Option<ActivationTarget> {
        let chip = image(10..20, 1);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&chip), (100.0, 50.0), false, 1);
        activation.release(Some(&chip), (100.0 + distance.0, 50.0 + distance.1))
    }

    #[test]
    fn release_without_movement_activates() {
        let target = click((0.0, 0.0)).unwrap();
        assert_eq!(target.attachment, Some(1));
        assert_eq!(target.range, 10..20);
    }

    #[test]
    fn three_and_four_pixels_activate() {
        assert!(click((3.0, 0.0)).is_some());
        assert!(click((4.0, 0.0)).is_some());
        assert!(click((0.0, -4.0)).is_some());
        assert!(click((-3.0, 2.0)).is_some());
    }

    #[test]
    fn five_pixels_do_not_activate() {
        assert!(click((5.0, 0.0)).is_none());
        assert!(click((3.0, 4.0)).is_none());
        assert!(click((3.0, 3.0)).is_none());
    }

    #[test]
    fn release_over_a_different_chip_does_not_activate() {
        let first = image(10..20, 1);
        let second = image(22..32, 2);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&first), (0.0, 0.0), false, 1);
        assert!(activation.release(Some(&second), (0.0, 0.0)).is_none());
        activation.press(Some(&first), (0.0, 0.0), false, 1);
        assert!(activation.release(None, (0.0, 0.0)).is_none());
    }

    #[test]
    fn shift_press_does_not_activate() {
        let chip = image(10..20, 1);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&chip), (0.0, 0.0), true, 1);
        assert!(!activation.is_pending());
        assert!(activation.release(Some(&chip), (0.0, 0.0)).is_none());
    }

    #[test]
    fn multi_click_does_not_activate() {
        let chip = image(10..20, 1);
        let mut activation = ImageChipActivation::new();
        for clicks in [2, 3] {
            activation.press(Some(&chip), (0.0, 0.0), false, clicks);
            assert!(activation.release(Some(&chip), (0.0, 0.0)).is_none());
        }
    }

    #[test]
    fn non_image_chips_are_not_activatable() {
        let file = descriptor(10..20, ReferenceKind::File, "a.rs", "src/a.rs", None);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&file), (0.0, 0.0), false, 1);
        assert!(activation.release(Some(&file), (0.0, 0.0)).is_none());
        assert!(!shows_pointing_hand(&file));
        assert!(shows_pointing_hand(&image(0..1, 1)));
    }

    #[test]
    fn a_new_press_replaces_the_old_one_and_release_consumes_it() {
        let chip = image(10..20, 1);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&chip), (0.0, 0.0), false, 1);
        activation.press(None, (0.0, 0.0), false, 1);
        assert!(activation.release(Some(&chip), (0.0, 0.0)).is_none());
        activation.press(Some(&chip), (0.0, 0.0), false, 1);
        assert!(activation.release(Some(&chip), (0.0, 0.0)).is_some());
        assert!(activation.release(Some(&chip), (0.0, 0.0)).is_none());
    }

    #[test]
    fn cancel_forgets_the_press() {
        let chip = image(10..20, 1);
        let mut activation = ImageChipActivation::new();
        activation.press(Some(&chip), (0.0, 0.0), false, 1);
        activation.cancel();
        assert!(activation.release(Some(&chip), (0.0, 0.0)).is_none());
    }
}
