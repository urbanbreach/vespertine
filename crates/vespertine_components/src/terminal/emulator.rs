//! The terminal emulator wrapper (CMP-021 R1-R4, R17): a pure fold of output bytes into a cell
//! grid, built on the `alacritty_terminal` state machine.
//!
//! [`Emulator::feed`] advances the state and returns the bytes the terminal wants written back to
//! the session (for example the answer to a cursor-position query). There is no I/O, timing or
//! rendering in this module; every behavior is testable with scripted byte strings.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side as AlacrittySide};
use alacritty_terminal::selection::{Selection, SelectionType as AlacrittySelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

/// Scrollback history kept above the viewport, in lines (R2).
pub const SCROLLBACK_LINES: usize = 10_000;
/// Smallest grid width (R2).
pub const MIN_COLS: usize = 2;
/// Smallest grid height (R2).
pub const MIN_ROWS: usize = 1;

/// The size of a grid in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    /// Columns.
    pub cols: usize,
    /// Rows.
    pub rows: usize,
}

impl GridSize {
    /// A size clamped to the minimum grid (2 x 1).
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// A cell color (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellColor {
    /// The default foreground.
    DefaultForeground,
    /// The default background.
    DefaultBackground,
    /// An indexed color: 0-15 ANSI, 16-231 cube, 232-255 grayscale.
    Indexed(u8),
    /// A direct 24-bit color.
    Rgb(u8, u8, u8),
}

/// A snapshot of one grid cell (R2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellSnapshot {
    /// The character; a space for an empty cell.
    pub ch: char,
    /// Zero-width combining marks attached to the character.
    pub marks: Vec<char>,
    /// Inside the current selection.
    pub selected: bool,
    /// The second half of a double-width character (including a leading spacer at a line wrap):
    /// never shaped, only background-painted.
    pub wide_spacer: bool,
    /// The first half of a double-width character.
    pub wide: bool,
    /// Hidden text.
    pub hidden: bool,
    /// Inverse video.
    pub inverse: bool,
    /// Any underline style.
    pub underline: bool,
    /// Italic.
    pub italic: bool,
    /// Dim.
    pub dim: bool,
    /// Bold.
    pub bold: bool,
    /// The background color.
    pub bg: CellColor,
    /// The foreground color.
    pub fg: CellColor,
}

impl CellSnapshot {
    /// An empty cell with default colors.
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            marks: Vec::new(),
            fg: CellColor::DefaultForeground,
            bg: CellColor::DefaultBackground,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
            hidden: false,
            wide: false,
            wide_spacer: false,
            selected: false,
        }
    }

    /// The effective paint colors `(foreground, background)` after inverse and hidden
    /// resolution: inverse swaps them, hidden paints the foreground as the background color.
    pub fn display_colors(&self) -> (CellColor, CellColor) {
        let (mut fg, bg) = if self.inverse {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        };
        if self.hidden {
            fg = bg;
        }
        (fg, bg)
    }
}

/// The cursor in viewport coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorSnapshot {
    /// Viewport row.
    pub row: usize,
    /// Column.
    pub col: usize,
}

/// A point of the grid: `line` is negative inside the scrollback, `0` is the top live row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    /// Grid line.
    pub line: i32,
    /// Column.
    pub column: usize,
}

/// The kind of a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionType {
    /// Characters between the anchors.
    Simple,
    /// Whole words around the anchors.
    Semantic,
    /// Whole lines; the copied text includes the trailing newline.
    Lines,
}

/// The side of a cell a pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The left half (including the exact midpoint).
    Left,
    /// The right half.
    Right,
}

/// What the terminal state machine reports besides grid changes.
#[derive(Default)]
struct Reports {
    /// Bytes to write back to the session.
    responses: Vec<u8>,
    title: Option<String>,
    bell: bool,
}

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Reports>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut reports = self.0.borrow_mut();
        match event {
            Event::PtyWrite(text) => reports.responses.extend_from_slice(text.as_bytes()),
            Event::Title(title) => reports.title = Some(title),
            Event::ResetTitle => reports.title = None,
            Event::Bell => reports.bell = true,
            // Grid-change notifications, clipboard and color requests, size requests and
            // process lifecycle are not part of this surface's contract.
            _ => {}
        }
    }
}

/// The terminal emulator: a pure fold of bytes into a grid (R1).
pub struct Emulator {
    term: Term<Listener>,
    processor: Processor,
    reports: Rc<RefCell<Reports>>,
}

impl Emulator {
    /// An emulator of `cols` x `rows` cells, clamped to at least 2 x 1.
    pub fn new(cols: usize, rows: usize) -> Self {
        let listener = Listener::default();
        let reports = listener.0.clone();
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            // Programs cannot write the host clipboard through the terminal.
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(config, &GridSize::new(cols, rows), listener);
        Self {
            term,
            processor: Processor::new(),
            reports,
        }
    }

    /// Folds `bytes` into the grid and returns the bytes the terminal wants written back to
    /// the session. A UTF-8 character split across two calls is reassembled.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.processor.advance(&mut self.term, bytes);
        std::mem::take(&mut self.reports.borrow_mut().responses)
    }

    /// Resizes the grid (clamped to at least 2 x 1). Content is preserved and the cursor
    /// reflows; the selection is dropped.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = GridSize::new(cols, rows);
        self.term.selection = None;
        self.term.resize(size);
    }

    /// Grid height in rows.
    pub fn rows(&self) -> usize {
        self.term.screen_lines()
    }

    /// Grid width in columns.
    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    /// The window title set by the program, or `None` after a reset.
    pub fn title(&self) -> Option<String> {
        self.reports.borrow().title.clone()
    }

    /// Whether a bell rang since the last call; reading clears it.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.reports.borrow_mut().bell)
    }

    /// Application-cursor mode (private mode 1).
    pub fn app_cursor_mode(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Bracketed-paste mode (private mode 2004).
    pub fn bracketed_paste_mode(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Whether the alternate screen is active (private mode 1049).
    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// The number of scrollback lines above the viewport.
    pub fn history_lines(&self) -> usize {
        self.term.history_size()
    }

    /// The scrollback offset: 0 is pinned to the live bottom.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scrolls the view: positive moves up into history, negative toward the live bottom.
    /// Over-scroll clamps to the history size.
    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    /// Scrolls to an absolute offset from the live bottom, clamped to the history size.
    pub fn scroll_to_offset(&mut self, offset: usize) {
        let target = offset.min(self.history_lines());
        let current = self.display_offset();
        self.term
            .scroll_display(Scroll::Delta(target as i32 - current as i32));
    }

    /// Pins the view to the live bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// The grid point of a viewport cell: the grid line is the viewport row minus the
    /// scrollback offset, so anchors stay on their text while the view scrolls. Rows and
    /// columns clamp into the grid.
    pub fn grid_point(&self, viewport_row: usize, col: usize) -> GridPoint {
        let row = viewport_row.min(self.rows() - 1);
        GridPoint {
            line: row as i32 - self.display_offset() as i32,
            column: col.min(self.cols() - 1),
        }
    }

    fn to_alacritty(&self, point: GridPoint) -> Point {
        let line = point
            .line
            .clamp(-(self.history_lines() as i32), self.rows() as i32 - 1);
        Point::new(Line(line), Column(point.column.min(self.cols() - 1)))
    }

    /// Starts a selection of `selection_type` at `point` on `side`, replacing any existing one.
    pub fn start_selection(&mut self, selection_type: SelectionType, point: GridPoint, side: Side) {
        let ty = match selection_type {
            SelectionType::Simple => AlacrittySelectionType::Simple,
            SelectionType::Semantic => AlacrittySelectionType::Semantic,
            SelectionType::Lines => AlacrittySelectionType::Lines,
        };
        self.term.selection = Some(Selection::new(ty, self.to_alacritty(point), side.into()));
    }

    /// Moves the end of the current selection; does nothing without one.
    pub fn update_selection(&mut self, point: GridPoint, side: Side) {
        let point = self.to_alacritty(point);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side.into());
        }
    }

    /// Drops the selection.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Whether a non-empty selection exists.
    pub fn has_selection(&self) -> bool {
        self.term
            .selection
            .as_ref()
            .is_some_and(|selection| selection.to_range(&self.term).is_some())
    }

    /// The selected text: rows joined with a newline, wide spacers contributing nothing; a
    /// line selection ends with a newline. `None` without a (non-empty) selection.
    pub fn selection_text(&self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        self.term.selection_to_string()
    }

    fn snapshot_cell(cell: &Cell, selected: bool) -> CellSnapshot {
        let (fg, fg_dim) = map_color(cell.fg);
        let (bg, _) = map_color(cell.bg);
        let flags = cell.flags;
        CellSnapshot {
            ch: cell.c,
            marks: cell
                .zerowidth()
                .map(|marks| marks.to_vec())
                .unwrap_or_default(),
            fg,
            bg,
            bold: flags.contains(Flags::BOLD),
            dim: flags.contains(Flags::DIM) || fg_dim,
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            inverse: flags.contains(Flags::INVERSE),
            hidden: flags.contains(Flags::HIDDEN),
            wide: flags.contains(Flags::WIDE_CHAR),
            wide_spacer: flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            selected,
        }
    }

    /// The cells of viewport row `row` (the scrollback offset is honored); an empty vector
    /// for a row outside the viewport.
    pub fn line(&self, row: usize) -> Vec<CellSnapshot> {
        if row >= self.rows() {
            return Vec::new();
        }
        let line = Line(row as i32 - self.display_offset() as i32);
        let range = self
            .term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term));
        let grid_row = &self.term.grid()[line];
        (0..self.cols())
            .map(|column| {
                let selected =
                    range.is_some_and(|range| range.contains(Point::new(line, Column(column))));
                Self::snapshot_cell(&grid_row[Column(column)], selected)
            })
            .collect()
    }

    /// All viewport rows.
    pub fn lines(&self) -> Vec<Vec<CellSnapshot>> {
        (0..self.rows()).map(|row| self.line(row)).collect()
    }

    /// The cursor in viewport coordinates; `None` when hidden or scrolled out of view.
    pub fn cursor(&self) -> Option<CursorSnapshot> {
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let point = self.term.grid().cursor.point;
        let row = point.line.0 + self.display_offset() as i32;
        if row < 0 || row as usize >= self.rows() {
            return None;
        }
        Some(CursorSnapshot {
            row: row as usize,
            col: point.column.0.min(self.cols() - 1),
        })
    }

    /// The text of viewport row `row` with wide spacers skipped and trailing blanks trimmed.
    pub fn row_text(&self, row: usize) -> String {
        let mut text = String::new();
        for cell in self.line(row) {
            if cell.wide_spacer {
                continue;
            }
            text.push(cell.ch);
            text.extend(cell.marks.iter());
        }
        text.truncate(text.trim_end().len());
        text
    }
}

impl From<Side> for AlacrittySide {
    fn from(side: Side) -> Self {
        match side {
            Side::Left => AlacrittySide::Left,
            Side::Right => AlacrittySide::Right,
        }
    }
}

/// The named dim colors, whose base index is their normal counterpart's.
const DIM_NAMED_COLORS: [NamedColor; 8] = [
    NamedColor::DimWhite,
    NamedColor::DimCyan,
    NamedColor::DimMagenta,
    NamedColor::DimBlue,
    NamedColor::DimYellow,
    NamedColor::DimGreen,
    NamedColor::DimRed,
    NamedColor::DimBlack,
];

/// Maps a state-machine color to a cell color plus whether it was a "dim" named color (R2):
/// named colors below 16 become indexed, the named background becomes the default background,
/// other named colors become the default foreground, dim variants fold to their base index.
fn map_color(color: Color) -> (CellColor, bool) {
    match color {
        Color::Spec(Rgb { r, g, b }) => (CellColor::Rgb(r, g, b), false),
        Color::Indexed(index) => (CellColor::Indexed(index), false),
        Color::Named(named) => {
            let index = named as usize;
            match named {
                NamedColor::Background => (CellColor::DefaultBackground, false),
                dim if DIM_NAMED_COLORS.contains(&dim) => (CellColor::Indexed(dim_base(dim)), true),
                _ if index < 16 => (CellColor::Indexed(index as u8), false),
                _ => (CellColor::DefaultForeground, false),
            }
        }
    }
}

fn dim_base(named: NamedColor) -> u8 {
    match named {
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        _ => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cols: usize, rows: usize, script: &str) -> Emulator {
        let mut emulator = Emulator::new(cols, rows);
        emulator.feed(script.as_bytes());
        emulator
    }

    fn cell(emulator: &Emulator, row: usize, col: usize) -> CellSnapshot {
        emulator.line(row)[col].clone()
    }

    #[test]
    fn carriage_return_overwrites_and_lines_wrap() {
        let mut emulator = run(20, 5, "one\r\ntwo\r\nthree");
        assert_eq!(emulator.row_text(0), "one");
        assert_eq!(emulator.row_text(1), "two");
        assert_eq!(emulator.row_text(2), "three");
        emulator.feed(b"\rXX");
        assert_eq!(emulator.row_text(2), "XXree");

        let wrapped = run(10, 3, "abcdefghijKLM");
        assert_eq!(wrapped.row_text(0), "abcdefghij");
        assert_eq!(wrapped.row_text(1), "KLM");
    }

    #[test]
    fn sgr_colors_map_to_indexed_and_direct_colors() {
        let emulator = run(
            10,
            2,
            "\x1b[31ma\x1b[95mb\x1b[38;5;196mc\x1b[38;2;10;20;30md",
        );
        assert_eq!(cell(&emulator, 0, 0).fg, CellColor::Indexed(1));
        assert_eq!(cell(&emulator, 0, 1).fg, CellColor::Indexed(13));
        assert_eq!(cell(&emulator, 0, 2).fg, CellColor::Indexed(196));
        assert_eq!(cell(&emulator, 0, 3).fg, CellColor::Rgb(10, 20, 30));
        let plain = run(10, 2, "x");
        assert_eq!(cell(&plain, 0, 0).fg, CellColor::DefaultForeground);
        assert_eq!(cell(&plain, 0, 0).bg, CellColor::DefaultBackground);
    }

    #[test]
    fn inverse_swaps_and_hidden_matches_background() {
        let emulator = run(10, 2, "\x1b[31;44;7ma\x1b[0;31;44;8mb\x1b[0;8mc");
        let inverse = cell(&emulator, 0, 0);
        assert!(inverse.inverse);
        assert_eq!(
            inverse.display_colors(),
            (CellColor::Indexed(4), CellColor::Indexed(1))
        );
        let hidden = cell(&emulator, 0, 1);
        assert!(hidden.hidden);
        let (fg, bg) = hidden.display_colors();
        assert_eq!(fg, bg);
        assert_eq!(bg, CellColor::Indexed(4));
        let default_hidden = cell(&emulator, 0, 2).display_colors();
        assert_eq!(default_hidden.0, default_hidden.1);
    }

    #[test]
    fn attributes_are_reported() {
        let emulator = run(10, 2, "\x1b[1;2;3;4ma\x1b[0mb");
        let first = cell(&emulator, 0, 0);
        assert!(first.bold && first.dim && first.italic && first.underline);
        let second = cell(&emulator, 0, 1);
        assert!(!second.bold && !second.dim && !second.italic && !second.underline);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let emulator = run(10, 2, "世w");
        let wide = cell(&emulator, 0, 0);
        assert!(wide.wide && !wide.wide_spacer);
        assert!(cell(&emulator, 0, 1).wide_spacer);
        assert_eq!(cell(&emulator, 0, 2).ch, 'w');
        assert_eq!(emulator.cursor(), Some(CursorSnapshot { row: 0, col: 3 }));
        assert_eq!(emulator.row_text(0), "世w");
    }

    #[test]
    fn split_utf8_reassembles() {
        let mut emulator = Emulator::new(10, 2);
        let bytes = "世".as_bytes();
        emulator.feed(&bytes[..1]);
        emulator.feed(&bytes[1..]);
        assert_eq!(emulator.row_text(0), "世");
    }

    #[test]
    fn combining_marks_attach_without_advancing() {
        let emulator = run(10, 2, "e\u{301}x");
        let first = cell(&emulator, 0, 0);
        assert_eq!(first.ch, 'e');
        assert_eq!(first.marks, vec!['\u{301}']);
        assert_eq!(cell(&emulator, 0, 1).ch, 'x');
    }

    #[test]
    fn scrollback_scrolling_and_cursor_visibility() {
        let mut script = String::new();
        for n in 1..=8 {
            script.push_str(&format!("line{n}\r\n"));
        }
        let mut emulator = run(10, 3, &script);
        assert_eq!(emulator.history_lines(), 6);
        assert_eq!(emulator.row_text(0), "line7");
        assert!(emulator.cursor().is_some());
        emulator.scroll(2);
        assert_eq!(emulator.display_offset(), 2);
        assert_eq!(emulator.row_text(0), "line5");
        assert!(emulator.cursor().is_none());
        emulator.scroll(100);
        assert_eq!(emulator.display_offset(), 6);
        assert_eq!(emulator.row_text(0), "line1");
        emulator.scroll_to_bottom();
        assert_eq!(emulator.display_offset(), 0);
        emulator.scroll_to_offset(100);
        assert_eq!(emulator.display_offset(), 6);
        emulator.scroll(-3);
        assert_eq!(emulator.display_offset(), 3);
    }

    #[test]
    fn resize_preserves_content_and_cursor_addressing_is_one_based() {
        let mut emulator = run(20, 5, "keepme\r\nsecond");
        emulator.resize(30, 3);
        assert_eq!((emulator.cols(), emulator.rows()), (30, 3));
        assert_eq!(emulator.row_text(0), "keepme");
        assert_eq!(emulator.row_text(1), "second");

        let mut emulator = run(20, 5, "\x1b[3;5Hx");
        assert_eq!(emulator.row_text(2), "    x");
        assert_eq!(cell(&emulator, 2, 4).ch, 'x');
        assert_eq!(emulator.cursor(), Some(CursorSnapshot { row: 2, col: 5 }));
        emulator.feed(b"\x1b[2;3H");
        let response = emulator.feed(b"\x1b[6n");
        assert_eq!(response, b"\x1b[2;3R");
    }

    #[test]
    fn grid_is_clamped_to_the_minimum() {
        let emulator = Emulator::new(0, 0);
        assert_eq!((emulator.cols(), emulator.rows()), (2, 1));
    }

    #[test]
    fn relative_cursor_motion_and_erase() {
        let mut emulator = run(20, 5, "abcdef\x1b[3D\x1b[1AX");
        assert_eq!(emulator.row_text(0), "abcXef");
        emulator.feed(b"\x1b[2J\x1b[H");
        assert_eq!(emulator.row_text(0), "");
        emulator.feed(b"hello world\x1b[6D\x1b[K");
        assert_eq!(emulator.row_text(0), "hello");
    }

    #[test]
    fn private_modes_toggle_booleans_and_cursor() {
        let mut emulator = Emulator::new(10, 2);
        assert!(!emulator.app_cursor_mode() && !emulator.bracketed_paste_mode());
        emulator.feed(b"\x1b[?1h\x1b[?2004h");
        assert!(emulator.app_cursor_mode() && emulator.bracketed_paste_mode());
        emulator.feed(b"\x1b[?1l\x1b[?2004l");
        assert!(!emulator.app_cursor_mode() && !emulator.bracketed_paste_mode());
        emulator.feed(b"\x1b[?25l");
        assert!(emulator.cursor().is_none());
        emulator.feed(b"\x1b[?25h");
        assert!(emulator.cursor().is_some());
    }

    #[test]
    fn alternate_screen_restores_primary_content() {
        let mut emulator = run(10, 3, "primary");
        emulator.feed(b"\x1b[?1049h\x1b[2J\x1b[Halt");
        assert!(emulator.alt_screen());
        assert_eq!(emulator.row_text(0), "alt");
        emulator.feed(b"\x1b[?1049l");
        assert_eq!(emulator.row_text(0), "primary");
    }

    #[test]
    fn title_and_bell_reports() {
        let mut emulator = Emulator::new(10, 2);
        emulator.feed(b"\x1b]0;my title\x07");
        assert_eq!(emulator.title().as_deref(), Some("my title"));
        // The title terminator is not a bell.
        assert!(!emulator.take_bell());
        emulator.feed(b"\x07");
        assert!(emulator.take_bell());
        assert!(!emulator.take_bell());
    }

    #[test]
    fn selection_text_across_rows_and_unchanged_by_scrolling_output() {
        let mut emulator = run(10, 3, "ab\r\ncd");
        let start = emulator.grid_point(0, 0);
        emulator.start_selection(SelectionType::Simple, start, Side::Left);
        let end = emulator.grid_point(1, 1);
        emulator.update_selection(end, Side::Right);
        assert_eq!(emulator.selection_text().as_deref(), Some("ab\ncd"));
        assert!(emulator.line(0)[0].selected && emulator.line(1)[1].selected);

        let mut emulator = run(12, 3, "target\r\n");
        let start = emulator.grid_point(0, 0);
        emulator.start_selection(SelectionType::Simple, start, Side::Left);
        let end = emulator.grid_point(0, 5);
        emulator.update_selection(end, Side::Right);
        assert_eq!(emulator.selection_text().as_deref(), Some("target"));
        emulator.feed(b"x\r\ny\r\nz\r\n");
        assert_eq!(emulator.selection_text().as_deref(), Some("target"));
        emulator.clear_selection();
        assert!(!emulator.has_selection());
    }

    #[test]
    fn resize_drops_the_selection() {
        let mut emulator = run(10, 3, "hello");
        let start = emulator.grid_point(0, 0);
        emulator.start_selection(SelectionType::Simple, start, Side::Left);
        let end = emulator.grid_point(0, 2);
        emulator.update_selection(end, Side::Right);
        assert!(emulator.has_selection());
        emulator.resize(10, 2);
        assert!(!emulator.has_selection());
    }

    #[test]
    fn semantic_and_line_selection() {
        let mut emulator = run(20, 3, "alpha beta gamma");
        let point = emulator.grid_point(0, 7);
        emulator.start_selection(SelectionType::Semantic, point, Side::Left);
        assert_eq!(emulator.selection_text().as_deref(), Some("beta"));

        let mut emulator = run(20, 3, "first row\r\nsecond row");
        let point = emulator.grid_point(1, 3);
        emulator.start_selection(SelectionType::Lines, point, Side::Left);
        assert_eq!(emulator.selection_text().as_deref(), Some("second row\n"));
    }
}
