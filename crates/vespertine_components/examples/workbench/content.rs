//! The fixed text of the workbench: session titles, the seeded history, the canned agent replies
//! and the prompts the script types. Everything here is constant, so a scripted run is a pure
//! function of the tick index.

/// One session of the sidebar.
pub struct SessionSeed {
    pub title: &'static str,
    pub subtitle: &'static str,
    /// `(is_user, markdown)` pairs, oldest first.
    pub history: &'static [(bool, &'static str)],
}

pub const SESSIONS: &[SessionSeed] = &[
    SessionSeed {
        title: "Tokenizer refactor",
        subtitle: "crates/lexer",
        history: &[
            (
                true,
                "The tokenizer allocates a `String` for every identifier. Can we borrow from the source instead?",
            ),
            (false, REPLY_BORROWING),
            (
                true,
                "Show me what the call sites look like after the change.",
            ),
            (false, REPLY_CALL_SITES),
            (
                true,
                "Good. Does the benchmark still pass with the borrowed tokens?",
            ),
            (false, REPLY_BENCH),
        ],
    },
    SessionSeed {
        title: "Review: pane resizing",
        subtitle: "pull request 482",
        history: &[
            (
                true,
                "Review the seam drag change and list anything that could regress the resize behavior.",
            ),
            (false, REPLY_REVIEW),
            (true, "Which of those is the riskiest?"),
            (
                false,
                "The **seam hitbox** is the riskiest: it is 20 px wide and consumes pointer events, so a content change near a pane edge can silently lose clicks.\n\n> Keep the seam test that presses just inside and just outside the hitbox.",
            ),
            (true, "Write the regression test for the first finding."),
            (false, REPLY_SEAM_TEST),
            (true, "Does the double-click reset need one as well?"),
            (false, REPLY_RESET_TEST),
        ],
    },
    SessionSeed {
        title: "Release notes",
        subtitle: "draft for 0.9",
        history: &[
            (true, "Draft the release notes from the merged changes."),
            (false, REPLY_NOTES),
            (true, "Add the upgrade steps for people on 0.8."),
            (false, REPLY_UPGRADE),
            (true, "Show the new configuration section in full."),
            (false, REPLY_CONFIG),
        ],
    },
];

const REPLY_SEAM_TEST: &str = "A test that resizes the window first and then drags once:\n\n```rust\n#[test]\nfn first_drag_after_a_window_resize_uses_the_new_limits() {\n    let mut f = Fixture::new(size(px(1100.), px(700.)));\n    f.resize_window(size(px(700.), px(500.)));\n    f.drag_seam(LEADING, px(900.));\n    assert_eq!(f.pane_size(LEADING), px(400.));\n}\n```\n\nThe fixture:\n\n- opens the group at the large size,\n- shrinks the window so the limits change,\n- drags past the old maximum and expects the clamp of the *new* frame.";

const REPLY_RESET_TEST: &str = "Yes, and it is shorter:\n\n```rust\n#[test]\nfn double_click_resets_to_the_default_clamped_to_the_limits() {\n    let mut f = Fixture::new(size(px(800.), px(600.)));\n    f.set_size(LEADING, px(380.));\n    f.double_click_seam(LEADING);\n    assert_eq!(f.pane_size(LEADING), px(256.));\n}\n```\n\nIt also covers the collapsed case: a reset on a closed pane changes nothing and emits no event.";

const REPLY_UPGRADE: &str = "## Upgrading from 0.8\n\n1. Update the dependency.\n2. Replace `Pane::new` with `PaneDescriptor::leading`.\n3. Rebuild and fix the renamed events.\n\n```toml\n[dependencies]\nvespertine = \"0.9\"\n```\n\nThe compiler will name every call that changed, and the old names are gone rather than deprecated, so there is nothing to clean up later.";

const REPLY_CONFIG: &str = "```toml\n[panes]\nleading = 256\ntrailing = 360\n\n[transcript]\nfollow = true\njump_threshold = 320\n\n[input]\nsubmit = \"enter\"\nmax_rows = 6\n```\n\nEvery key is optional. A missing section takes its defaults, and an unknown key is reported once at startup with its line number.";

const REPLY_BORROWING: &str = "## Borrowing from the source\n\nThe lexer can hand out `&str` slices of the input instead of owned strings. The token type gains a lifetime and the interner is no longer needed on the hot path:\n\n```rust\npub struct Token<'src> {\n    pub kind: TokenKind,\n    pub text: &'src str,\n    pub span: Range<usize>,\n}\n\nimpl<'src> Lexer<'src> {\n    fn identifier(&mut self) -> Token<'src> {\n        let start = self.pos;\n        while self.peek().is_some_and(is_ident_continue) {\n            self.bump();\n        }\n        Token { kind: TokenKind::Ident, text: &self.src[start..self.pos], span: start..self.pos }\n    }\n}\n```\n\nThree things change:\n\n- `Token` is no longer `'static`, so the parser stores spans and re-slices on demand.\n- Keywords are matched on the slice without allocating.\n- Error messages copy the text only when they are built.";

const REPLY_CALL_SITES: &str = "Here is the parser entry point after the change:\n\n```rust\nlet source = std::fs::read_to_string(path)?;\nlet tokens: Vec<Token<'_>> = Lexer::new(&source).collect();\nlet tree = Parser::new(&tokens).parse_module()?;\n```\n\nThe `source` string now outlives every token, which is why `parse_module` takes `&[Token<'_>]`. The old `to_owned()` calls are gone, and the compiler points at the one place that stored a token in a long-lived cache.";

const REPLY_BENCH: &str = "Yes. The benchmark group `lex/identifiers` improves and the others are unchanged:\n\n| benchmark | before | after |\n| --- | --- | --- |\n| lex/identifiers | 412 us | 171 us |\n| lex/numbers | 96 us | 95 us |\n| lex/strings | 203 us | 198 us |\n\nAllocation count for the identifier benchmark drops from 18 400 to 12.";

const REPLY_REVIEW: &str = "I read the diff twice. Findings, most important first:\n\n1. The drag handler clamps against the limits of the *previous* frame, so the first move after a window resize can overshoot.\n2. `reset_size` runs inside the double-click handler without checking that the pane is still open.\n3. The seam hitbox shrinks to zero height when the bottom reservation exceeds the group height.\n\n```rust\nlet limits = placement.limits;\nlet clamped = limits.clamp(requested);\nif clamped != self.size {\n    self.set_size(&id, clamped, cx);\n}\n```\n\nNone of them is a blocker, but (1) deserves a test.";

const REPLY_NOTES: &str = "# Release 0.9\n\n## Highlights\n\n- Tabs reorder with a sliding insertion indicator.\n- Panes resize with direct pointer tracking.\n- Streaming transcripts follow the bottom until you scroll away.\n\n## Fixes\n\n- Markdown code blocks keep their language label when streamed.\n- The input clears exactly once per submit.\n\nSee `CHANGELOG.md` for the full list.";

/// A reply the script's prompts receive; they differ by index so every cycle streams new text.
pub const REPLIES: &[&str] = &[
    "## Plan\n\nI will make the change in three steps:\n\n1. Add the failing test.\n2. Fix the root cause.\n3. Run the whole suite.\n\n```python\ndef normalize(path: str) -> str:\n    parts = []\n    for part in path.split(\"/\"):\n        if part in (\"\", \".\"):\n            continue\n        if part == \"..\":\n            if parts:\n                parts.pop()\n            continue\n        parts.append(part)\n    return \"/\" + \"/\".join(parts)\n```\n\nThe function never touches the file system, so the test needs no fixtures. The edge cases are an empty path, a path made only of `..` and a trailing slash. **All three** are covered by the table below.\n\n| input | output |\n| --- | --- |\n| `/a/./b/../c` | `/a/c` |\n| `/../..` | `/` |\n| `/x/` | `/x` |\n\nAfter the suite passes I will describe the change in the pull request.",
    "## Findings\n\nThe slowdown comes from `rebuild_index` being called on every keystroke. The call is cheap for small projects and quadratic for large ones.\n\n```rust\nfn on_edit(&mut self, edit: &Edit) {\n    self.pending.push(edit.clone());\n    if self.pending.len() >= BATCH || self.idle_for() > DEBOUNCE {\n        self.rebuild_index();\n        self.pending.clear();\n    }\n}\n```\n\nBatching removes the quadratic term:\n\n- keystrokes are queued and applied together,\n- the index rebuilds after a 150 ms pause,\n- saving flushes the queue first.\n\nI measured a project with 40 000 files: typing latency went from *380 ms* to *6 ms* per key.",
    "## Summary of the investigation\n\nThe crash only reproduces when the window is resized while a reply is streaming. The stack ends in the layout cache:\n\n```text\nthread 'main' panicked at 'index out of bounds: the len is 41 but the index is 41'\n   0: layout::cache::Cache::get\n   1: layout::measure_rows\n   2: transcript::render\n```\n\nThe cache is sized from the row count of the *previous* frame. A reply that appends a row between two frames reads one past the end.\n\n```rust\nfn row_height(&self, ix: usize) -> f32 {\n    self.heights.get(ix).copied().unwrap_or(self.estimate)\n}\n```\n\nUsing `get` with the running estimate fixes it, and the next measure pass replaces the estimate with the real height.",
    "## Migration notes\n\nThe configuration format moves from a flat table to nested sections. Old files keep working for one release.\n\n```toml\n[editor]\nfont_size = 14\nwrap = \"word\"\n\n[agent]\nmodel = \"default\"\nmax_turns = 40\n```\n\nSteps for users:\n\n1. Run `workbench config migrate`.\n2. Review the generated file.\n3. Delete the `.bak` copy when satisfied.\n\nUnknown keys are preserved under `[extra]` so nothing is lost, and the command is idempotent: running it twice changes nothing.",
];

/// The prompts the script types, one per cycle. The `\n` is typed as a Shift+Enter newline.
pub const PROMPTS: &[&str] = &[
    "Fix the path normalizer.\nAdd a table of edge cases.",
    "Why is typing slow in big projects?\nKeep the answer short.",
    "Explain the resize crash.\nInclude the stack.",
    "Write migration notes for the config change.",
];

/// The text of the plan tab.
pub const PLAN: &str = "# Plan\n\n- [x] Reproduce the problem\n- [ ] Write the failing test\n- [ ] Fix and verify\n\n```sh\ncargo test --workspace\n```\n\nThe plan is a static Markdown document; the transcript shows the live work.";

/// The first text of the notes tab.
pub const NOTES: &str = "Scratch notes\n- remember to squash the fixup commits\n";

/// Splits `reply` into streaming chunks: groups of `words` words, whitespace kept, so
/// concatenating the chunks gives the reply exactly.
pub fn chunks(reply: &str, words: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    let mut previous_space = true;
    for ch in reply.chars() {
        let space = ch.is_whitespace();
        if !space && previous_space {
            if count == words {
                chunks.push(std::mem::take(&mut current));
                count = 0;
            }
            count += 1;
        }
        current.push(ch);
        previous_space = space;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
