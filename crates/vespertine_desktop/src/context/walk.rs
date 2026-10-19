//! Bounded breadth-first traversal and line format (PLT-013 R2).

use super::{
    AccessibilityConnection, AccessibilitySnapshot, ClockSource, ContextBudgets, ObjectRef,
    SNAPSHOT_FORMAT_VERSION,
};
use futures::join;
use std::{collections::VecDeque, time::Instant};

/// Collapses every run of whitespace to one space, trims, and truncates to
/// `max_chars` characters.
pub(crate) fn clean_value(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut chars = 0usize;
    for word in raw.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
            chars += 1;
        }
        for c in word.chars() {
            if chars >= max_chars {
                break;
            }
            out.push(c);
            chars += 1;
        }
        if chars >= max_chars {
            break;
        }
    }
    out
}

/// Walks the tree below `root` breadth-first within `budgets` and returns the
/// outline. Stops (marking the snapshot truncated) when the next node's depth
/// exceeds the depth limit, the node limit has been visited, the output has
/// reached the byte limit, or the deadline has passed. Individual request
/// failures skip that item only.
pub(crate) async fn walk(
    connection: &dyn AccessibilityConnection,
    root: ObjectRef,
    budgets: &ContextBudgets,
    clock: &dyn ClockSource,
    deadline: Instant,
) -> AccessibilitySnapshot {
    let mut content = String::new();
    let mut truncated = false;
    let mut visited = 0usize;
    let mut queue: VecDeque<(ObjectRef, usize)> = VecDeque::new();
    queue.push_back((root, 0));

    while let Some((_, depth)) = queue.front() {
        if *depth > budgets.max_depth
            || visited >= budgets.max_nodes
            || content.len() >= budgets.max_bytes
            || clock.now() >= deadline
        {
            truncated = true;
            break;
        }
        let Some((node, depth)) = queue.pop_front() else {
            break;
        };
        visited += 1;

        let (role, name, description, text_interface, children) = join!(
            connection.role(&node),
            connection.name(&node),
            connection.description(&node),
            connection.implements_text(&node),
            connection.children(&node),
        );

        // Text is read only from a Text-interface node whose role is known not
        // to be a password field.
        let text = match (&role, &text_interface) {
            (Ok(role), Ok(true)) if !role.password => {
                read_text(connection, &node, budgets.max_value_chars).await
            }
            _ => None,
        };

        let max_chars = budgets.max_value_chars;
        let name = name
            .ok()
            .map(|name| clean_value(&name, max_chars))
            .unwrap_or_default();
        let description = description
            .ok()
            .map(|description| clean_value(&description, max_chars))
            .unwrap_or_default();
        let text = text
            .map(|text| clean_value(&text, max_chars))
            .unwrap_or_default();
        // Description and text are emitted only when non-empty and different
        // from the name.
        let mut fields: Vec<String> = Vec::new();
        if !name.is_empty() {
            fields.push(name.clone());
        }
        for value in [description, text] {
            if !value.is_empty() && value != name {
                fields.push(value);
            }
        }

        if !fields.is_empty() {
            let role_name = match &role {
                Ok(role) => clean_value(&role.name, budgets.max_value_chars),
                Err(_) => "unknown".to_owned(),
            };
            let mut line = String::new();
            for _ in 0..depth {
                line.push_str("  ");
            }
            line.push_str(&role_name);
            line.push_str(": ");
            line.push_str(&fields.join(" | "));
            line.push('\n');
            if content.len() + line.len() > budgets.max_bytes {
                truncated = true;
                break;
            }
            content.push_str(&line);
        }

        if let Ok(children) = children {
            for child in children {
                // Nothing beyond the node limit can ever be visited, so the
                // queue never needs more than the remaining budget; dropping
                // the rest still means the outline is incomplete.
                if visited + queue.len() >= budgets.max_nodes {
                    truncated = true;
                    break;
                }
                queue.push_back((child, depth + 1));
            }
        }
    }

    AccessibilitySnapshot {
        version: SNAPSHOT_FORMAT_VERSION,
        content,
        truncated,
    }
}

async fn read_text(
    connection: &dyn AccessibilityConnection,
    node: &ObjectRef,
    max_chars: usize,
) -> Option<String> {
    let count = connection.character_count(node).await.ok()?;
    let end = count.min(max_chars);
    if end == 0 {
        return None;
    }
    connection.text(node, 0, end).await.ok()
}

#[cfg(test)]
mod tests {
    use super::clean_value;

    #[test]
    fn clean_collapses_trims_and_truncates() {
        assert_eq!(clean_value("  a \t\n b\u{a0}c  ", 100), "a b c");
        assert_eq!(clean_value("abcdef", 3), "abc");
        assert_eq!(clean_value("   ", 10), "");
        assert_eq!(clean_value("ab cd", 3), "ab ");
        assert_eq!(clean_value("é é é", 3), "é é");
    }
}
