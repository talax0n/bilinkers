//! Port of `isGroupedReadingLayout` from `src/groupedReading.js`.
//! The browser-side reader `groupedReadingLayoutFromDocument` stays JS and produces this struct.

use std::collections::HashSet;

use serde::Deserialize;

use crate::js;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupedReadingLayout {
    pub pills: Vec<String>,
    /// First character of each visible option button; an empty button gives an empty string.
    pub option_letters: Vec<String>,
}

pub fn is_grouped_reading_layout(layout: &GroupedReadingLayout) -> bool {
    // The set holds the untrimmed pills, as the JS does.
    let numeric_pills: HashSet<&str> =
        layout.pills.iter().map(String::as_str).filter(|pill| js::is_ascii_digits(js::trim(pill))).collect();
    let group_count = layout.option_letters.iter().filter(|letter| js::trim(letter) == "A").count();
    numeric_pills.len() >= 2 && group_count >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(pills: &[&str], option_letters: &[&str]) -> GroupedReadingLayout {
        GroupedReadingLayout {
            pills: pills.iter().map(|s| s.to_string()).collect(),
            option_letters: option_letters.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn recognizes_top_level_grouped_reading_only_with_multiple_numeric_pills_and_a_restarted_option_groups() {
        assert!(is_grouped_reading_layout(&layout(&["1", "2", "3"], &["A", "B", "C", "D", "A", "B", "C", "D"])));
        assert!(
            !is_grouped_reading_layout(&layout(&["1", "2", "3"], &["A", "B", "C", "D"])),
            "an ordinary one-group native exercise is not reading"
        );
        assert!(
            !is_grouped_reading_layout(&layout(&["1"], &["A", "B", "A", "B"])),
            "a single numeric pill is not reading"
        );
    }
}
