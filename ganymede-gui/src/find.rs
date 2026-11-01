//! Literal notebook-source search; offsets always refer to the original UTF-8 text.
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub cell: u64,
    pub range: Range<usize>,
}

pub fn matches(query: &str, sources: &[(u64, String)]) -> Vec<Hit> {
    if query.is_empty() {
        return vec![];
    }
    let Ok(pattern) = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return vec![];
    };
    sources
        .iter()
        .flat_map(|(cell, text)| {
            pattern.find_iter(text).map(move |m| Hit {
                cell: *cell,
                range: m.range(),
            })
        })
        .collect()
}

pub fn step(current: Option<usize>, count: usize, backwards: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (current, backwards) {
        (Some(i), true) => (i + count - 1) % count,
        (Some(i), false) => (i + 1) % count,
        (None, true) => count - 1,
        (None, false) => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_unicode_search_keeps_original_byte_ranges_and_cell_order() {
        let sources = vec![(8, "é CAFÉ a.b".into()), (3, "café A.B".into())];
        let hits = matches("café", &sources);
        assert_eq!(
            hits,
            vec![
                Hit {
                    cell: 8,
                    range: 3..8
                },
                Hit {
                    cell: 3,
                    range: 0..5
                }
            ]
        );
        assert_eq!(matches("a.b", &sources).len(), 2);
        assert!(matches("", &sources).is_empty());
        assert!(matches("missing", &sources).is_empty());
    }
    #[test]
    fn navigation_wraps_in_both_directions() {
        assert_eq!(step(Some(0), 3, true), Some(2));
        assert_eq!(step(Some(2), 3, false), Some(0));
        assert_eq!(step(None, 3, false), Some(0));
        assert_eq!(step(None, 0, true), None);
    }
}
