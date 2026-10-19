//! Field typing and validation, and selection and toggle changes (R4).

use super::{BORDER, GREEN, INK, RED, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use std::collections::BTreeSet;
use vespertine::prelude::FluentBuilder as _;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px,
};

const FIELD_COUNT: usize = 6;
const LABELS: [&str; FIELD_COUNT] = [
    "Full name",
    "Email",
    "Postal code",
    "Phone",
    "Company",
    "Notes",
];

/// The validation rule of field `index`; `None` when the text is acceptable.
fn validate(index: usize, text: &str) -> Option<&'static str> {
    match index {
        0 if text.trim().len() < 3 => Some("Enter at least three characters"),
        1 if !text.contains('@') => Some("An email address needs an @"),
        2 if text.len() != 5 || !text.chars().all(|ch| ch.is_ascii_digit()) => {
            Some("Five digits expected")
        }
        3 if text.chars().filter(char::is_ascii_digit).count() < 7 => {
            Some("At least seven digits expected")
        }
        4 if text.is_empty() => Some("Required"),
        5 if text.len() > 40 => Some("At most 40 characters"),
        _ => None,
    }
}

/// The character typed into field `index` at frame `frame`.
fn typed(index: usize, frame: u64) -> char {
    match index {
        1 if frame % 9 == 4 => '@',
        2 | 3 => char::from(b'0' + (frame % 10) as u8),
        _ => char::from(b'a' + (frame % 26) as u8),
    }
}

struct Field {
    index: usize,
    text: String,
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let error = validate(self.index, &self.text);
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .child(LABELS[self.index]),
            )
            .child(
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(hex(if error.is_some() { RED } else { BORDER }))
                    .rounded(px(4.0))
                    .child(SharedString::from(format!("{}|", self.text))),
            )
            .child(
                div()
                    .h(px(16.0))
                    .text_size(px(12.0))
                    .text_color(hex(if error.is_some() { RED } else { GREEN }))
                    .child(error.unwrap_or("Looks good")),
            )
    }
}

struct Summary {
    fields: Vec<Entity<Field>>,
}

impl Render for Summary {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let valid = self
            .fields
            .iter()
            .filter(|field| {
                let field = field.read(cx);
                validate(field.index, &field.text).is_none()
            })
            .count();
        div()
            .h(px(32.0))
            .flex()
            .items_center()
            .child(format!("{valid} of {FIELD_COUNT} fields valid"))
    }
}

struct Form {
    fields: Vec<Entity<Field>>,
    summary: Entity<Summary>,
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(self.summary.clone())
            .child(
                div().flex().flex_row().flex_wrap().gap(px(16.0)).children(
                    self.fields
                        .iter()
                        .cloned()
                        .map(|field| div().w(px(260.0)).child(field)),
                ),
            )
    }
}

struct TypingDriver {
    fields: Vec<Entity<Field>>,
}

impl Driver for TypingDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let index = (frame % FIELD_COUNT as u64) as usize;
        let field = &self.fields[index];
        stage.mutate(|app| {
            field.update(app, |field, cx| {
                if frame % 11 == 10 {
                    field.text.pop();
                } else {
                    field.text.push(typed(index, frame));
                }
                if field.text.len() > 48 {
                    field.text.clear();
                }
                cx.notify();
            });
        });
    }
}

fn typing_setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let mut fields = Vec::new();
    harness.open_window(|_, cx| {
        fields = (0..FIELD_COUNT)
            .map(|index| {
                cx.new(|_| Field {
                    index,
                    text: String::new(),
                })
            })
            .collect();
        // The summary reads every field while rendering, so a keystroke rebuilds it.
        let summary = cx.new(|_| Summary {
            fields: fields.clone(),
        });
        let form_fields = fields.clone();
        AnyView::from(cx.new(|_| Form {
            fields: form_fields,
            summary,
        }))
    })?;
    Ok(Box::new(TypingDriver { fields }))
}

const ITEMS: usize = 60;

struct Item {
    index: usize,
    checked: bool,
    selected: bool,
}

impl Render for Item {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(26.0))
            .w_full()
            .px(px(12.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .when(self.selected, |row| row.bg(hex(0xddf4ff)))
            .child(
                div()
                    .size(px(14.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(hex(BORDER))
                    .when(self.checked, |box_| box_.bg(hex(GREEN))),
            )
            .child(format!("Item {:02}", self.index))
    }
}

struct Counts {
    items: Vec<Entity<Item>>,
}

impl Render for Counts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let checked = self
            .items
            .iter()
            .filter(|item| item.read(cx).checked)
            .count();
        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .bg(hex(SURFACE))
            .child(format!("{checked} of {ITEMS} checked"))
    }
}

struct Selector {
    items: Vec<Entity<Item>>,
    counts: Entity<Counts>,
}

impl Render for Selector {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex_col()
            .flex()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(self.counts.clone())
            .children(self.items.iter().cloned())
    }
}

struct SelectionDriver {
    items: Vec<Entity<Item>>,
    checked: BTreeSet<usize>,
    selected: usize,
}

impl Driver for SelectionDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let next = (frame as usize * 7 + 3) % ITEMS;
        let toggled = (frame as usize * 13 + 5) % ITEMS;
        let previous = self.selected;
        self.selected = next;
        if !self.checked.remove(&toggled) {
            self.checked.insert(toggled);
        }
        let checked_now = self.checked.contains(&toggled);
        let items = &self.items;
        stage.mutate(|app| {
            items[previous].update(app, |item, cx| {
                item.selected = false;
                cx.notify();
            });
            items[next].update(app, |item, cx| {
                item.selected = true;
                cx.notify();
            });
            items[toggled].update(app, |item, cx| {
                item.checked = checked_now;
                cx.notify();
            });
        });
    }
}

fn selection_setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let mut items = Vec::new();
    harness.open_window(|_, cx| {
        items = (0..ITEMS)
            .map(|index| {
                cx.new(|_| Item {
                    index,
                    checked: false,
                    selected: index == 0,
                })
            })
            .collect();
        let counts = cx.new(|_| Counts {
            items: items.clone(),
        });
        let selector_items = items.clone();
        AnyView::from(cx.new(|_| Selector {
            items: selector_items,
            counts,
        }))
    })?;
    Ok(Box::new(SelectionDriver {
        items,
        checked: BTreeSet::new(),
        selected: 0,
    }))
}

/// The field-typing and selection workloads.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "typing-validation",
            family: Family::Fields,
            description: "one character typed per frame into six validated fields",
            warmup: 30,
            frames: 200,
            setup: typing_setup,
        },
        Workload {
            name: "selection-toggle",
            family: Family::Selection,
            description: "the selection moves and one checkbox toggles every frame",
            warmup: 30,
            frames: 200,
            setup: selection_setup,
        },
    ]
}
