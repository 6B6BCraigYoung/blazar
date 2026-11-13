#[must_use]
pub fn others_note(paused: usize, queued: usize, also: bool) -> String {
    let place = if paused + queued == 1 {
        "another conversation"
    } else {
        "other conversations"
    };
    let what = match (paused, queued) {
        (0, q) => format!("{q} queued"),
        (p, 0) => format!("{p} paused"),
        (p, q) => format!("{p} paused, {q} queued"),
    };
    if also {
        format!("Also {what} in {place}")
    } else {
        format!("{what} in {place}")
    }
}

#[cfg(test)]
mod tests {
    use super::others_note;

    #[test]
    fn the_band_says_whether_other_conversations_are_paused() {
        assert_eq!(others_note(1, 0, false), "1 paused in another conversation");
        assert_eq!(others_note(0, 1, false), "1 queued in another conversation");
        assert_eq!(others_note(0, 3, false), "3 queued in other conversations");
        assert_eq!(
            others_note(1, 2, false),
            "1 paused, 2 queued in other conversations"
        );
        assert_eq!(
            others_note(2, 0, true),
            "Also 2 paused in other conversations"
        );
    }
}
