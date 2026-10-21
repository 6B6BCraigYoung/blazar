#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub head: String,
    pub keys: Vec<String>,
}

pub fn group_turns(items: impl IntoIterator<Item = (String, bool)>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (key, user) in items {
        match turns.last_mut() {
            Some(turn) if !user => turn.keys.push(key),
            _ => turns.push(Turn {
                head: key.clone(),
                keys: vec![key],
            }),
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turns(items: &[(&str, bool)]) -> Vec<(String, Vec<String>)> {
        group_turns(items.iter().map(|(k, u)| ((*k).to_owned(), *u)))
            .into_iter()
            .map(|t| (t.head, t.keys))
            .collect()
    }

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn each_user_message_opens_a_turn_that_holds_everything_until_the_next() {
        assert_eq!(
            turns(&[
                ("u1", true),
                ("a1", false),
                ("f1", false),
                ("u2", true),
                ("a2", false),
            ]),
            vec![
                ("u1".to_owned(), keys(&["u1", "a1", "f1"])),
                ("u2".to_owned(), keys(&["u2", "a2"])),
            ]
        );
    }

    #[test]
    fn rows_before_the_first_user_message_form_a_turn_without_a_head() {
        assert_eq!(
            turns(&[("m0", false), ("f0", false), ("u1", true), ("a1", false)]),
            vec![
                ("m0".to_owned(), keys(&["m0", "f0"])),
                ("u1".to_owned(), keys(&["u1", "a1"])),
            ]
        );
    }

    #[test]
    fn consecutive_user_messages_and_empty_input_stay_separate() {
        assert_eq!(
            turns(&[("u1", true), ("u2", true)]),
            vec![
                ("u1".to_owned(), keys(&["u1"])),
                ("u2".to_owned(), keys(&["u2"])),
            ]
        );
        assert!(turns(&[]).is_empty());
    }
}
