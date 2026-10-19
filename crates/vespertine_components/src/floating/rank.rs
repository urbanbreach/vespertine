//! Search ranking shared by comboboxes, selects and pickers (CMP-002 R25).

/// Rank of `label` for `query`: 0 when it starts with the query, 1 when it merely contains it,
/// `None` when it does not match. The query is trimmed and both sides compare
/// case-insensitively; an empty query matches everything at rank 0.
pub fn match_rank(label: &str, query: &str) -> Option<u8> {
    rank_lowercased(&label.to_lowercase(), &query.trim().to_lowercase())
}

fn rank_lowercased(label: &str, query: &str) -> Option<u8> {
    if label.starts_with(query) {
        Some(0)
    } else if label.contains(query) {
        Some(1)
    } else {
        None
    }
}

/// Filters and ranks `labels` for `query`, returning **original indices**.
///
/// An empty (after trimming) query keeps every item in input order. Otherwise non-matching
/// labels are dropped and the rest is ordered by rank, then by original index (stable within a
/// rank).
pub fn filter_and_rank<S: AsRef<str>>(labels: &[S], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..labels.len()).collect();
    }
    let mut ranked: Vec<(u8, usize)> = labels
        .iter()
        .enumerate()
        .filter_map(|(index, label)| {
            rank_lowercased(&label.as_ref().to_lowercase(), &query).map(|rank| (rank, index))
        })
        .collect();
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ranks_before_substring_and_misses_drop() {
        let labels = ["banana", "an", "Ant", "cana", "xyz"];
        assert_eq!(filter_and_rank(&labels, "an"), vec![1, 2, 0, 3]);
        assert_eq!(match_rank("Ant", "AN"), Some(0));
        assert_eq!(match_rank("banana", "an"), Some(1));
        assert_eq!(match_rank("xyz", "an"), None);
    }

    #[test]
    fn query_is_trimmed_and_case_insensitive() {
        let labels = ["Alpha", "beta"];
        assert_eq!(filter_and_rank(&labels, "  ALP "), vec![0]);
        assert_eq!(filter_and_rank(&labels, "ETA"), vec![1]);
    }

    #[test]
    fn empty_query_returns_input_order() {
        let labels = ["c", "a", "b"];
        assert_eq!(filter_and_rank(&labels, ""), vec![0, 1, 2]);
        assert_eq!(filter_and_rank(&labels, "   "), vec![0, 1, 2]);
        assert_eq!(filter_and_rank::<&str>(&[], "x"), Vec::<usize>::new());
    }

    #[test]
    fn order_is_stable_within_a_rank() {
        let labels = ["zeta1", "zeta0", "a-zeta", "b-zeta"];
        assert_eq!(filter_and_rank(&labels, "zeta"), vec![0, 1, 2, 3]);
    }
}
