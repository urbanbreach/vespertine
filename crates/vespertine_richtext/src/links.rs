//! Link rules of TXT-008 R16-R21 independent of painting: activation, the actions menu model,
//! destination disclosure geometry and the file-destination anchor grammar.

use vespertine::{MouseButton, Pixels, Point, Size, point, px};

/// A release farther than this (Euclidean, logical px) from the press is not a click.
pub const CLICK_SLOP: f32 = 4.0;

/// Width of the link actions menu.
pub const MENU_WIDTH: f32 = 260.0;

/// A link: a byte range of a run's displayed text with a destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Byte range in the run's displayed text.
    pub range: std::ops::Range<usize>,
    /// The complete original destination.
    pub destination: String,
}

/// A press that may become a link activation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkPress {
    /// Press position.
    pub position: Point<Pixels>,
    /// Pressed button.
    pub button: MouseButton,
    /// Click count of the press.
    pub click_count: usize,
    /// Whether the press was over the link hitbox.
    pub over_link: bool,
}

/// Whether a pointer release activates the link (R17). ALL of: left button; click count 1 on the
/// press; release within 4 logical px (Euclidean, `<=`) of the press; press and release over the
/// link hitbox; no non-empty selection at release time (a drag ending on a link is a selection,
/// not a navigation).
pub fn pointer_activates(
    press: &LinkPress,
    release_button: MouseButton,
    release: Point<Pixels>,
    release_over_link: bool,
    selection_has_text: bool,
) -> bool {
    if press.button != MouseButton::Left
        || release_button != MouseButton::Left
        || press.click_count != 1
        || !press.over_link
        || !release_over_link
        || selection_has_text
    {
        return false;
    }
    let dx = f32::from(release.x - press.position.x);
    let dy = f32::from(release.y - press.position.y);
    (dx * dx + dy * dy).sqrt() <= CLICK_SLOP
}

/// An entry of the link actions menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkActionKind {
    /// Open with the default handler (the primary action).
    Open,
    /// Open externally.
    OpenExternally,
    /// Copy the destination text.
    CopyDestination,
}

/// One row of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuRow {
    /// The action.
    pub action: LinkActionKind,
    /// Whether the row can be chosen.
    pub enabled: bool,
}

/// Keyboard model of the link actions menu (R18): open with the default handler, open externally
/// and copy; copy is always enabled, the others only when navigation is allowed for the
/// destination's scheme.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkMenu {
    rows: [MenuRow; 3],
    selected: usize,
    /// Where the menu opens: the pointer for a secondary press, the bottom-left of the range for
    /// Shift+F10.
    pub origin: Point<Pixels>,
}

impl LinkMenu {
    /// A menu for a destination; `navigation_allowed` is the scheme policy's verdict.
    pub fn new(navigation_allowed: bool, origin: Point<Pixels>) -> Self {
        let rows = [
            MenuRow {
                action: LinkActionKind::Open,
                enabled: navigation_allowed,
            },
            MenuRow {
                action: LinkActionKind::OpenExternally,
                enabled: navigation_allowed,
            },
            MenuRow {
                action: LinkActionKind::CopyDestination,
                enabled: true,
            },
        ];
        let selected = rows.iter().position(|row| row.enabled).unwrap_or(2);
        Self {
            rows,
            selected,
            origin,
        }
    }

    /// The rows.
    pub fn rows(&self) -> &[MenuRow; 3] {
        &self.rows
    }

    /// Index of the highlighted row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Moves to the next enabled row with wraparound (Tab, Down).
    pub fn next(&mut self) {
        self.step(1);
    }

    /// Moves to the previous enabled row with wraparound (Up, Shift+Tab).
    pub fn previous(&mut self) {
        self.step(self.rows.len() - 1);
    }

    fn step(&mut self, by: usize) {
        for _ in 0..self.rows.len() {
            self.selected = (self.selected + by) % self.rows.len();
            if self.rows[self.selected].enabled {
                return;
            }
        }
    }

    /// The highlighted action.
    pub fn selected_action(&self) -> LinkActionKind {
        self.rows[self.selected].action
    }
}

/// Size limits of the destination disclosure card (R19).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisclosureSize {
    /// Maximum width: `clamp(viewport width - 24, 1, 360)`.
    pub max_width: f32,
    /// Maximum height: `clamp(viewport height - 80, 1, 160)`.
    pub max_height: f32,
    /// Card width: `ceil(natural shaped width) + 14`, capped at the maximum.
    pub width: f32,
}

/// Text size of the disclosure card.
pub const DISCLOSURE_TEXT_SIZE: f32 = 11.0;
/// Line height of the disclosure card.
pub const DISCLOSURE_LINE_HEIGHT: f32 = 14.0;
/// Horizontal padding of the card.
pub const DISCLOSURE_PADDING_X: f32 = 6.0;
/// Vertical padding of the card.
pub const DISCLOSURE_PADDING_Y: f32 = 4.0;
/// Margin kept between a keyboard-focused card and the window edge.
pub const DISCLOSURE_WINDOW_MARGIN: f32 = 8.0;

impl DisclosureSize {
    /// Computes the card size for a viewport and the natural shaped width of the destination.
    pub fn compute(viewport: Size<Pixels>, natural_width: f32) -> Self {
        let max_width = (f32::from(viewport.width) - 24.0).clamp(1.0, 360.0);
        let max_height = (f32::from(viewport.height) - 80.0).clamp(1.0, 160.0);
        Self {
            max_width,
            max_height,
            width: (natural_width.ceil() + 14.0).min(max_width),
        }
    }
}

/// Positions a keyboard-focused card below `anchor_rect`'s bottom-left, snapped inside the window
/// with an 8 px margin.
pub fn snap_card(
    below: Point<Pixels>,
    card: Size<Pixels>,
    window: Size<Pixels>,
    margin: f32,
) -> Point<Pixels> {
    let max_x = f32::from(window.width - card.width) - margin;
    let max_y = f32::from(window.height - card.height) - margin;
    point(
        px(f32::from(below.x).min(max_x).max(margin)),
        px(f32::from(below.y).min(max_y).max(margin)),
    )
}

/// Unit of a file link's column (R21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnUnit {
    /// A one-based character column in the line's text.
    Character,
    /// A one-based byte column (byte offset + 1) in the source line.
    Byte,
}

/// A one-based column with its unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// One-based column.
    pub value: u32,
    /// What the value counts.
    pub unit: ColumnUnit,
}

impl Column {
    /// A character column.
    pub fn character(value: u32) -> Self {
        Self {
            value,
            unit: ColumnUnit::Character,
        }
    }

    /// Converts to `unit` using the text of the line the column refers to. Returns `None` when a
    /// byte column does not lie on a character boundary or the column is past the line.
    pub fn convert(self, line_text: &str, unit: ColumnUnit) -> Option<Column> {
        if self.unit == unit {
            return Some(self);
        }
        let index = (self.value as usize).checked_sub(1)?;
        let value = match self.unit {
            ColumnUnit::Character => {
                let (offset, _) = line_text.char_indices().nth(index).or_else(|| {
                    (line_text.chars().count() == index).then_some((line_text.len(), ' '))
                })?;
                offset as u32 + 1
            }
            ColumnUnit::Byte => {
                if index > line_text.len() || !line_text.is_char_boundary(index) {
                    return None;
                }
                line_text[..index].chars().count() as u32 + 1
            }
        };
        Some(Column { value, unit })
    }
}

/// The payload of a link action for a file (R21).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    /// The path, without anchors.
    pub path: String,
    /// One-based line.
    pub line: Option<u32>,
    /// One-based column with its unit.
    pub column: Option<Column>,
}

struct Anchor {
    line: Option<u32>,
    column: Option<u32>,
}

fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Splits `L12`/`C5` style `(line, column)` pairs; `None` when `text` is not of that form.
fn line_col(text: &str, with_l_prefix: bool) -> Option<(u32, Option<u32>)> {
    let text = if with_l_prefix {
        text.strip_prefix('L')?
    } else {
        text.strip_prefix('L').unwrap_or(text)
    };
    match text.split_once('C') {
        Some((line, column)) => Some((digits(line)?, Some(digits(column)?))),
        None => Some((digits(text)?, None)),
    }
}

enum Fragment {
    /// Not an `L<digits>` anchor: dropped, the path stays.
    Dropped,
    /// Parsed; `None` inside is a zero line/column or a backwards range.
    Anchor(Option<Anchor>),
}

/// `#L12`, `#L12C5`, `#L12-L20`, `#L12-20`, `#L12C1-L20C3`: open at the first line and column.
fn parse_fragment(fragment: &str) -> Fragment {
    let starts_like_anchor = fragment
        .strip_prefix('L')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if !starts_like_anchor {
        return Fragment::Dropped;
    }
    let (first, second) = match fragment.split_once('-') {
        Some((first, second)) => (first, Some(second)),
        None => (fragment, None),
    };
    let Some((line, column)) = line_col(first, true) else {
        return Fragment::Dropped;
    };
    let end = match second {
        Some(second) => match line_col(second, false) {
            Some(end) => Some(end),
            None => return Fragment::Dropped,
        },
        None => None,
    };
    Fragment::Anchor(validate(line, column, end))
}

fn validate(line: u32, column: Option<u32>, end: Option<(u32, Option<u32>)>) -> Option<Anchor> {
    if line == 0 || column == Some(0) {
        return None;
    }
    if let Some((end_line, end_column)) = end {
        if end_line == 0 || end_column == Some(0) || end_line < line {
            return None;
        }
        if end_line == line
            && let (Some(start), Some(end)) = (column, end_column)
            && end < start
        {
            return None;
        }
    }
    Some(Anchor {
        line: Some(line),
        column,
    })
}

/// Trailing `:L`, `:L:C` and `:L-L2` (a range opens at its first line).
fn split_suffix(path: &str) -> Option<(&str, Option<Anchor>)> {
    let Some((rest, last)) = path.rsplit_once(':') else {
        return Some((path, None));
    };
    if let Some((first, second)) = last.split_once('-')
        && let (Some(line), Some(end)) = (digits(first), digits(second))
        && !rest.is_empty()
    {
        return Some((rest, Some(validate(line, None, Some((end, None)))?)));
    }
    let Some(last_number) = digits(last) else {
        return Some((path, None));
    };
    if rest.is_empty() {
        return Some((path, None));
    }
    if let Some((rest2, middle)) = rest.rsplit_once(':')
        && let Some(line) = digits(middle)
        && !rest2.is_empty()
    {
        return Some((rest2, Some(validate(line, Some(last_number), None)?)));
    }
    Some((rest, Some(validate(last_number, None, None)?)))
}

/// Parses a file destination with textual anchors (R21).
///
/// Accepted on a path: trailing `:L`, `:L:C`, `:L-L2` and fragment anchors `#L12`, `#L12C5`,
/// `#L12-L20`, `#L12-20`, `#L12C1-L20C3`; all open at the first line and column given. A fragment
/// that does not start with `L` plus digits is dropped, leaving the path; a fragment containing `/`
/// is part of the path; when both a suffix and a fragment provide a field the fragment wins it. A
/// line of 0, a column of 0 or a backwards range makes the whole text not a file link (`None`).
/// Columns parsed from text are character columns.
pub fn parse_file_destination(text: &str) -> Option<FileLink> {
    let (before, fragment) = match text.rfind('#') {
        Some(ix) if !text[ix + 1..].contains('/') => (&text[..ix], Some(&text[ix + 1..])),
        _ => (text, None),
    };
    let mut anchor = None;
    if let Some(fragment) = fragment {
        match parse_fragment(fragment) {
            Fragment::Dropped => {}
            Fragment::Anchor(parsed) => anchor = Some(parsed?),
        }
    }
    let (path, suffix) = split_suffix(before)?;
    if path.is_empty() {
        return None;
    }
    let line = anchor
        .as_ref()
        .and_then(|a| a.line)
        .or(suffix.as_ref().and_then(|a| a.line));
    let column = anchor
        .as_ref()
        .and_then(|a| a.column)
        .or(suffix.as_ref().and_then(|a| a.column));
    Some(FileLink {
        path: path.to_string(),
        line,
        column: column.map(Column::character),
    })
}

/// The default scheme policy: navigation is allowed for `http`, `https`, `mailto` and `file`
/// destinations and for scheme-less file paths.
pub fn default_navigation_allowed(destination: &str) -> bool {
    let destination = destination.trim();
    match destination.split_once(':') {
        Some((scheme, rest))
            if scheme.len() > 1
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) =>
        {
            let scheme = scheme.to_ascii_lowercase();
            // `name:123` is a path with a line suffix, not a scheme.
            if digits(rest.split(['-', ':']).next().unwrap_or("")).is_some()
                && !rest.starts_with("//")
                && !matches!(scheme.as_str(), "mailto")
            {
                return true;
            }
            matches!(scheme.as_str(), "http" | "https" | "mailto" | "file")
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::size;

    fn press(button: MouseButton, count: usize) -> LinkPress {
        LinkPress {
            position: point(px(100.), px(100.)),
            button,
            click_count: count,
            over_link: true,
        }
    }

    #[test]
    fn four_pixels_activate_five_do_not() {
        let p = press(MouseButton::Left, 1);
        let release = |dx: f32, dy: f32| point(px(100. + dx), px(100. + dy));
        assert!(pointer_activates(
            &p,
            MouseButton::Left,
            release(4., 0.),
            true,
            false
        ));
        assert!(pointer_activates(
            &p,
            MouseButton::Left,
            release(0., -4.),
            true,
            false
        ));
        assert!(!pointer_activates(
            &p,
            MouseButton::Left,
            release(5., 0.),
            true,
            false
        ));
        // Euclidean: (3, 3) is 4.24 px away.
        assert!(!pointer_activates(
            &p,
            MouseButton::Left,
            release(3., 3.),
            true,
            false
        ));
        assert!(pointer_activates(
            &p,
            MouseButton::Left,
            release(2.8, 2.8),
            true,
            false
        ));
    }

    #[test]
    fn other_gestures_never_navigate() {
        let at = point(px(100.), px(100.));
        assert!(!pointer_activates(
            &press(MouseButton::Left, 2),
            MouseButton::Left,
            at,
            true,
            false
        ));
        assert!(!pointer_activates(
            &press(MouseButton::Left, 3),
            MouseButton::Left,
            at,
            true,
            false
        ));
        assert!(!pointer_activates(
            &press(MouseButton::Right, 1),
            MouseButton::Right,
            at,
            true,
            false
        ));
        assert!(!pointer_activates(
            &press(MouseButton::Left, 1),
            MouseButton::Left,
            at,
            false,
            false
        ));
        assert!(
            !pointer_activates(
                &press(MouseButton::Left, 1),
                MouseButton::Left,
                at,
                true,
                true
            ),
            "a drag ending on a link is a selection"
        );
        let mut off = press(MouseButton::Left, 1);
        off.over_link = false;
        assert!(!pointer_activates(&off, MouseButton::Left, at, true, false));
    }

    #[test]
    fn menu_cycles_with_wraparound_and_skips_disabled_rows() {
        let origin = point(px(0.), px(0.));
        let mut menu = LinkMenu::new(true, origin);
        assert_eq!(menu.selected_action(), LinkActionKind::Open);
        menu.next();
        assert_eq!(menu.selected_action(), LinkActionKind::OpenExternally);
        menu.next();
        menu.next();
        assert_eq!(
            menu.selected_action(),
            LinkActionKind::Open,
            "wraps forward"
        );
        menu.previous();
        assert_eq!(
            menu.selected_action(),
            LinkActionKind::CopyDestination,
            "wraps backward"
        );

        let mut blocked = LinkMenu::new(false, origin);
        assert_eq!(blocked.selected_action(), LinkActionKind::CopyDestination);
        blocked.next();
        assert_eq!(blocked.selected_action(), LinkActionKind::CopyDestination);
        assert!(blocked.rows()[2].enabled && !blocked.rows()[0].enabled);
    }

    #[test]
    fn disclosure_size_clamps_to_the_viewport() {
        let large = DisclosureSize::compute(size(px(800.), px(600.)), 120.2);
        assert_eq!(large.max_width, 360.0);
        assert_eq!(large.max_height, 160.0);
        assert_eq!(large.width, 121.0 + 14.0);
        let small = DisclosureSize::compute(size(px(200.), px(100.)), 500.0);
        assert_eq!(small.max_width, 176.0);
        assert_eq!(small.max_height, 20.0);
        assert_eq!(small.width, 176.0);
        let tiny = DisclosureSize::compute(size(px(10.), px(10.)), 5.0);
        assert_eq!((tiny.max_width, tiny.max_height), (1.0, 1.0));
    }

    #[test]
    fn card_snaps_inside_the_window_with_a_margin() {
        let window = size(px(400.), px(300.));
        let card = size(px(150.), px(60.));
        assert_eq!(
            snap_card(point(px(10.), px(20.)), card, window, 8.0),
            point(px(10.), px(20.))
        );
        assert_eq!(
            snap_card(point(px(2.), px(2.)), card, window, 8.0),
            point(px(8.), px(8.))
        );
        assert_eq!(
            snap_card(point(px(390.), px(290.)), card, window, 8.0),
            point(px(242.), px(232.))
        );
    }

    fn file(path: &str, line: Option<u32>, column: Option<u32>) -> Option<FileLink> {
        Some(FileLink {
            path: path.to_string(),
            line,
            column: column.map(Column::character),
        })
    }

    #[test]
    fn suffix_anchors() {
        assert_eq!(
            parse_file_destination("src/main.rs"),
            file("src/main.rs", None, None)
        );
        assert_eq!(
            parse_file_destination("src/main.rs:12"),
            file("src/main.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("src/main.rs:12:5"),
            file("src/main.rs", Some(12), Some(5))
        );
        assert_eq!(
            parse_file_destination("src/main.rs:12-20"),
            file("src/main.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("a.rs:7"),
            file("a.rs", Some(7), None)
        );
    }

    #[test]
    fn fragment_anchors() {
        assert_eq!(
            parse_file_destination("a.rs#L12"),
            file("a.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("a.rs#L12C5"),
            file("a.rs", Some(12), Some(5))
        );
        assert_eq!(
            parse_file_destination("a.rs#L12-L20"),
            file("a.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("a.rs#L12-20"),
            file("a.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("a.rs#L12C1-L20C3"),
            file("a.rs", Some(12), Some(1))
        );
    }

    #[test]
    fn dropped_and_path_fragments() {
        assert_eq!(
            parse_file_destination("a.rs#section"),
            file("a.rs", None, None)
        );
        assert_eq!(parse_file_destination("a.rs#"), file("a.rs", None, None));
        assert_eq!(parse_file_destination("a.rs#L"), file("a.rs", None, None));
        assert_eq!(parse_file_destination("a.rs#Lx"), file("a.rs", None, None));
        assert_eq!(
            parse_file_destination("a.rs#L12x"),
            file("a.rs", None, None)
        );
        // A fragment containing a slash belongs to the path.
        assert_eq!(
            parse_file_destination("dir#frag/a.rs"),
            file("dir#frag/a.rs", None, None)
        );
        assert_eq!(
            parse_file_destination("dir#L1/a.rs:9"),
            file("dir#L1/a.rs", Some(9), None)
        );
    }

    #[test]
    fn fragment_wins_each_field_over_the_suffix() {
        assert_eq!(
            parse_file_destination("a.rs:3:4#L12"),
            file("a.rs", Some(12), Some(4))
        );
        assert_eq!(
            parse_file_destination("a.rs:3:4#L12C9"),
            file("a.rs", Some(12), Some(9))
        );
        assert_eq!(
            parse_file_destination("a.rs:3#section"),
            file("a.rs", Some(3), None)
        );
    }

    #[test]
    fn invalid_anchors_are_not_file_links() {
        for text in [
            "a.rs:0",
            "a.rs:12:0",
            "a.rs:0:5",
            "a.rs#L0",
            "a.rs#L12C0",
            "a.rs#L20-L12",
            "a.rs#L12C5-L12C2",
            "a.rs:20-12",
            "a.rs#L12-L0",
            "#L12",
        ] {
            assert_eq!(parse_file_destination(text), None, "{text}");
        }
        // Equal endpoints are not backwards.
        assert_eq!(
            parse_file_destination("a.rs#L12-L12"),
            file("a.rs", Some(12), None)
        );
        assert_eq!(
            parse_file_destination("a.rs#L12C5-L12C5"),
            file("a.rs", Some(12), Some(5))
        );
    }

    #[test]
    fn columns_convert_between_units_exactly() {
        let line = "é = 日本語;";
        let column = Column::character(8);
        // The eighth character is the final ';'.
        let byte = column.convert(line, ColumnUnit::Byte).unwrap();
        assert_eq!(byte.unit, ColumnUnit::Byte);
        assert_eq!(&line[byte.value as usize - 1..], ";");
        assert_eq!(byte.convert(line, ColumnUnit::Character), Some(column));
        // A byte column inside a character cannot be converted.
        assert_eq!(
            Column {
                value: 2,
                unit: ColumnUnit::Byte
            }
            .convert(line, ColumnUnit::Character),
            None
        );
        // One past the last character converts too (end of line).
        let end = Column::character(line.chars().count() as u32 + 1);
        assert_eq!(
            end.convert(line, ColumnUnit::Byte).unwrap().value as usize,
            line.len() + 1
        );
    }

    #[test]
    fn default_policy_allows_web_mail_file_and_paths() {
        for allowed in [
            "https://example.com",
            "http://x",
            "mailto:a@b.c",
            "file:///tmp/a",
            "src/a.rs",
            "src/a.rs:12",
            "a.rs:12:3",
            "/abs/path",
        ] {
            assert!(default_navigation_allowed(allowed), "{allowed}");
        }
        for blocked in [
            "javascript:alert(1)",
            "data:text/html,x",
            "vbscript:x",
            "custom-scheme://x",
        ] {
            assert!(!default_navigation_allowed(blocked), "{blocked}");
        }
    }
}
