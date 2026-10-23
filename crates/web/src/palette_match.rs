const PREFIX: u32 = 8;
const WORD: u32 = 6;
const CONTAINS: u32 = 4;
const SUBSEQUENCE: u32 = 2;

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

fn tier(q: &str, text: &str, whole: bool) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    if text.starts_with(q) {
        return Some(if whole { PREFIX } else { WORD });
    }
    if text
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '/' | '.' | '·' | '（' | '('))
        .skip(1)
        .any(|w| w.starts_with(q))
    {
        return Some(WORD);
    }
    if text.contains(q) {
        return Some(CONTAINS);
    }
    if q.chars().count() >= 2 && subsequence(q, text) {
        return Some(SUBSEQUENCE);
    }
    None
}

pub fn score(query: &str, title: &str, keys: &str) -> Option<u32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let by_title = tier(&q, &title.to_lowercase(), true).map(|t| t * 2 + 1);
    let by_keys = keys
        .split_whitespace()
        .filter_map(|k| tier(&q, &k.to_lowercase(), false))
        .max()
        .map(|t| t * 2);
    by_title.max(by_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_beats_word_start_beats_contains_beats_subsequence() {
        let prefix = score("in", "inbox", "").unwrap();
        let word = score("in", "mail inbox", "").unwrap();
        let contains = score("in", "mainline", "").unwrap();
        let sub = score("ibx", "inbox", "").unwrap();
        assert!(prefix > word);
        assert!(word > contains);
        assert!(contains > sub);
    }

    #[test]
    fn chinese_titles_match_through_aliases() {
        assert!(score("设", "设置", "settings sz").is_some());
        assert!(score("sz", "设置", "settings sz").is_some());
        assert!(score("sett", "设置", "settings sz").is_some());
        assert_eq!(score("收件", "设置", "settings sz"), None);
    }

    #[test]
    fn a_title_hit_outranks_the_same_tier_through_keys() {
        let title = score("ta", "tasks", "").unwrap();
        let keys = score("ta", "任务", "tasks").unwrap();
        assert!(title > keys);
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        assert_eq!(score(" Set ", "settings", ""), score("set", "settings", ""));
        assert_eq!(score("", "anything", ""), Some(0));
    }

    #[test]
    fn single_characters_do_not_match_as_subsequences() {
        assert_eq!(score("x", "inbox", ""), Some(CONTAINS * 2 + 1));
        assert_eq!(score("q", "inbox", ""), None);
    }
}
