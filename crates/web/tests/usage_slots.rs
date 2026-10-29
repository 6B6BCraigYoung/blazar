#[path = "../src/pages/usage/slots.rs"]
mod slots;

use slots::Slot::{Day, Idle};
use slots::slots;

#[test]
fn idle_days_fold_into_slots() {
    // 开头没运行整段省略，中间 1–2 天照常占位，3 天及以上压成一格，结尾不压。
    let runs = [
        0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
    ];
    assert_eq!(
        slots(&runs),
        vec![
            Day(2),
            Day(3),
            Day(4),
            Day(5),
            Day(6),
            Day(7),
            Idle(8, 10),
            Day(11),
            Day(12),
            Day(13),
            Day(14)
        ]
    );
}

#[test]
fn nothing_ran_means_no_slots() {
    assert_eq!(slots(&[0.0; 4]), vec![]);
}
