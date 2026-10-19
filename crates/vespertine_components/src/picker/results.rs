//! Sections, limits, caps and footer states of a picker's result list (CMP-006 R9, R12, R15).
//!
//! Owners describe rows and sections with generic descriptors that carry stable keys; the picker
//! is not coupled to a data source. [`Results::build`] applies the per-section limit of the
//! current view, hides empty sections, caps the list at [`DISPLAY_CAP`] rows and derives the
//! list and footer states.

use vespertine::SharedString;

/// Display cap of a list (R15).
pub const DISPLAY_CAP: usize = 300;
/// Skeleton rows reserved by an empty list that is loading (R15).
pub const SKELETON_ROWS: usize = 4;
/// Summary view limit per section (R12).
pub const SUMMARY_LIMIT: usize = 5;

/// Stable identity of a result row.
pub type RowKey = SharedString;

/// A generic result row: command, file, content hit or any entity.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Stable key; highlight and selection follow it, never the index.
    pub key: RowKey,
    /// Primary label (also what local ranking compares).
    pub label: SharedString,
    /// Quieter secondary text.
    pub secondary: Option<SharedString>,
    /// Hint shown after the row hint delay (CMP-004).
    pub hint: Option<SharedString>,
    /// Whether the row can be highlighted and activated.
    pub enabled: bool,
}

impl Row {
    /// An enabled row with only a key and a label.
    pub fn new(key: impl Into<RowKey>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            secondary: None,
            hint: None,
            enabled: true,
        }
    }
}

/// What one section contributes to a build.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionSpec {
    /// Stable section key (scope tabs refer to it).
    pub key: SharedString,
    /// Optional header text.
    pub title: Option<SharedString>,
    /// Rows in display order, before any limit.
    pub rows: Vec<Row>,
    /// Rows shown in the summary view.
    pub summary_limit: usize,
    /// Rows shown in a dedicated scope.
    pub scope_limit: usize,
    /// Status text of the header (`Searching…`, `Indexing…`).
    pub status: Option<SharedString>,
    /// The section's source failed: the message replaces the rows (R10).
    pub error: Option<SharedString>,
    /// The section's source has a request in flight.
    pub loading: bool,
}

impl SectionSpec {
    /// A settled section with the default limits.
    pub fn new(key: impl Into<SharedString>, rows: Vec<Row>) -> Self {
        Self {
            key: key.into(),
            title: None,
            rows,
            summary_limit: SUMMARY_LIMIT,
            scope_limit: 50,
            status: None,
            error: None,
            loading: false,
        }
    }
}

/// The view a list is built for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// Every section, each capped at its summary limit.
    Summary,
    /// A dedicated scope: only the listed sections, each at its long limit.
    Scope(Vec<SharedString>),
}

/// A section as displayed.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Section key.
    pub key: SharedString,
    /// Header text.
    pub title: Option<SharedString>,
    /// Header status.
    pub status: Option<SharedString>,
    /// The visible rows.
    pub rows: Vec<Row>,
    /// An inline error with a retry action replaces the rows.
    pub error: Option<SharedString>,
}

/// Footer of the list (R15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Footer {
    /// No footer.
    None,
    /// More rows were returned than the display cap shows: `Showing X of Y`.
    ShowingOf {
        /// Rows displayed.
        shown: usize,
        /// Rows available.
        total: usize,
    },
    /// The next page is loading.
    LoadingMore,
    /// The next page failed; the footer offers Retry.
    MoreError(SharedString),
}

impl Footer {
    /// The text of the footer, when it has one.
    pub fn text(&self) -> Option<String> {
        match self {
            Footer::None => None,
            Footer::ShowingOf { shown, total } => Some(format!("Showing {shown} of {total}")),
            Footer::LoadingMore => Some("Loading more\u{2026}".to_string()),
            Footer::MoreError(message) => Some(message.to_string()),
        }
    }
}

/// Paging state of the list, supplied by the owner from its sources.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paging {
    /// A page request is in flight.
    pub loading_more: bool,
    /// The last page request failed.
    pub error: Option<SharedString>,
}

/// State of the list as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListState {
    /// There are rows (or sections with errors) to show.
    Rows,
    /// Nothing yet and a request is in flight: room for [`SKELETON_ROWS`] skeleton rows.
    Loading,
    /// Nothing matched: a title and a hint.
    Empty {
        /// Title of the empty state.
        title: SharedString,
        /// Hint below the title.
        hint: SharedString,
    },
}

/// The displayed result list.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    /// Sections with at least one row (or an error), in declared order.
    pub sections: Vec<Section>,
    /// List state.
    pub state: ListState,
    /// Footer.
    pub footer: Footer,
}

impl Results {
    /// Builds the displayed list from the sections of every source (R12, R15). Empty sections
    /// are not shown; `empty_title` and `empty_hint` are the owner's texts for the empty state.
    pub fn build(
        specs: &[SectionSpec],
        view: &View,
        paging: &Paging,
        empty_title: &str,
        empty_hint: &str,
    ) -> Self {
        let mut sections = Vec::new();
        let mut loading = false;
        let mut remaining = DISPLAY_CAP;
        let mut wanted_total = 0;
        for spec in specs {
            let limit = match view {
                View::Summary => spec.summary_limit,
                View::Scope(keys) => {
                    if !keys.contains(&spec.key) {
                        continue;
                    }
                    spec.scope_limit
                }
            };
            loading |= spec.loading;
            if spec.rows.is_empty() && spec.error.is_none() {
                continue;
            }
            let wanted = spec.rows.len().min(limit);
            let take = wanted.min(remaining);
            wanted_total += wanted;
            remaining -= take;
            sections.push(Section {
                key: spec.key.clone(),
                title: spec.title.clone(),
                status: spec.status.clone(),
                rows: spec.rows[..take].to_vec(),
                error: spec.error.clone(),
            });
        }
        let shown: usize = sections.iter().map(|section| section.rows.len()).sum();
        let has_content = !sections.is_empty();
        let state = if has_content {
            ListState::Rows
        } else if loading {
            ListState::Loading
        } else {
            ListState::Empty {
                title: empty_title.to_string().into(),
                hint: empty_hint.to_string().into(),
            }
        };
        let footer = if let Some(message) = &paging.error {
            Footer::MoreError(message.clone())
        } else if paging.loading_more {
            Footer::LoadingMore
        } else if wanted_total > shown {
            Footer::ShowingOf {
                shown,
                total: wanted_total,
            }
        } else {
            Footer::None
        };
        Self {
            sections,
            state,
            footer,
        }
    }

    /// Row count reserved for skeleton rows (4 when the list is empty and loading).
    pub fn skeleton_rows(&self) -> usize {
        if self.state == ListState::Loading {
            SKELETON_ROWS
        } else {
            0
        }
    }

    /// Every row of every section in navigation order; headers are skipped (R13).
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.sections.iter().flat_map(|section| section.rows.iter())
    }

    /// Keys of the flat navigation order.
    pub fn keys(&self) -> Vec<RowKey> {
        self.rows().map(|row| row.key.clone()).collect()
    }

    /// Whether each flat row is enabled.
    pub fn enabled(&self) -> Vec<bool> {
        self.rows().map(|row| row.enabled).collect()
    }

    /// Number of rows in the flat order.
    pub fn len(&self) -> usize {
        self.rows().count()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The section index and the row index inside it of a flat index.
    pub fn locate(&self, flat: usize) -> Option<(usize, usize)> {
        let mut base = 0;
        for (section_ix, section) in self.sections.iter().enumerate() {
            if flat < base + section.rows.len() {
                return Some((section_ix, flat - base));
            }
            base += section.rows.len();
        }
        None
    }

    /// Whether the flat row is the first row of its section (its header scrolls into view with
    /// it, R13).
    pub fn is_section_first(&self, flat: usize) -> bool {
        self.locate(flat).is_some_and(|(_, row)| row == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(prefix: &str, count: usize) -> Vec<Row> {
        (0..count)
            .map(|ix| Row::new(format!("{prefix}{ix}"), format!("{prefix} {ix}")))
            .collect()
    }

    fn build(specs: &[SectionSpec], view: &View) -> Results {
        Results::build(specs, view, &Paging::default(), "No results", "Try another query")
    }

    #[test]
    fn summary_caps_every_section_at_five_and_scope_shows_the_long_limit() {
        let specs = [
            SectionSpec::new("files", rows("f", 12)),
            SectionSpec::new("commands", rows("c", 3)),
        ];
        let summary = build(&specs, &View::Summary);
        assert_eq!(summary.sections[0].rows.len(), 5);
        assert_eq!(summary.sections[1].rows.len(), 3);
        let scope = build(&specs, &View::Scope(vec!["files".into()]));
        assert_eq!(scope.sections.len(), 1);
        assert_eq!(scope.sections[0].rows.len(), 12);
    }

    #[test]
    fn empty_sections_are_hidden_and_order_is_the_declared_order() {
        let specs = [
            SectionSpec::new("a", Vec::new()),
            SectionSpec::new("b", rows("b", 1)),
            SectionSpec::new("c", rows("c", 1)),
        ];
        let results = build(&specs, &View::Summary);
        let keys: Vec<_> = results.sections.iter().map(|s| s.key.to_string()).collect();
        assert_eq!(keys, ["b", "c"]);
    }

    #[test]
    fn flat_order_skips_headers_and_locates_section_firsts() {
        let specs = [
            SectionSpec::new("a", rows("a", 2)),
            SectionSpec::new("b", rows("b", 2)),
        ];
        let results = build(&specs, &View::Summary);
        let keys: Vec<_> = results.keys().iter().map(|k| k.to_string()).collect();
        assert_eq!(keys, ["a0", "a1", "b0", "b1"]);
        assert_eq!(results.locate(3), Some((1, 1)));
        assert!(results.is_section_first(0));
        assert!(!results.is_section_first(1));
        assert!(results.is_section_first(2));
        assert_eq!(results.locate(4), None);
    }

    #[test]
    fn more_rows_than_the_cap_show_the_cap_and_showing_x_of_y() {
        let mut spec = SectionSpec::new("files", rows("f", 450));
        spec.scope_limit = 1000;
        let results = build(&[spec], &View::Scope(vec!["files".into()]));
        assert_eq!(results.len(), DISPLAY_CAP);
        assert_eq!(results.footer, Footer::ShowingOf { shown: 300, total: 450 });
        assert_eq!(results.footer.text().as_deref(), Some("Showing 300 of 450"));
    }

    #[test]
    fn paging_states_replace_the_count_footer() {
        let spec = SectionSpec::new("files", rows("f", 3));
        let loading = Paging {
            loading_more: true,
            error: None,
        };
        let results = Results::build(&[spec.clone()], &View::Summary, &loading, "", "");
        assert_eq!(results.footer, Footer::LoadingMore);
        let failed = Paging {
            loading_more: false,
            error: Some("offline".into()),
        };
        let results = Results::build(&[spec], &View::Summary, &failed, "", "");
        assert_eq!(results.footer, Footer::MoreError("offline".into()));
    }

    #[test]
    fn an_empty_loading_list_reserves_four_skeleton_rows() {
        let mut spec = SectionSpec::new("files", Vec::new());
        spec.loading = true;
        let results = build(&[spec], &View::Summary);
        assert_eq!(results.state, ListState::Loading);
        assert_eq!(results.skeleton_rows(), 4);
    }

    #[test]
    fn an_empty_result_has_a_title_and_a_hint_and_an_error_keeps_its_section() {
        let results = build(&[SectionSpec::new("a", Vec::new())], &View::Summary);
        assert_eq!(
            results.state,
            ListState::Empty {
                title: "No results".into(),
                hint: "Try another query".into()
            }
        );
        let mut failed = SectionSpec::new("a", Vec::new());
        failed.error = Some("boom".into());
        let results = build(&[failed], &View::Summary);
        assert_eq!(results.state, ListState::Rows);
        assert_eq!(results.sections[0].error.as_deref(), Some("boom"));
        assert!(results.is_empty());
    }
}
