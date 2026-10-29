/// 趋势图上的一格：某一天，或中间连续几天没运行压成的一条细线（首尾两天的下标）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Day(usize),
    Idle(usize, usize),
}

/// 开头没运行的日子整段省略；中间连续 3 天及以上没运行的压成一格；其余每天一格。
pub fn slots(runs: &[f64]) -> Vec<Slot> {
    let idle_from = |i: usize| runs[i..].iter().take_while(|r| **r == 0.0).count();
    let mut out = Vec::new();
    let mut i = idle_from(0);
    while i < runs.len() {
        let idle = idle_from(i);
        if idle >= 3 && i + idle < runs.len() {
            out.push(Slot::Idle(i, i + idle - 1));
            i += idle;
        } else {
            out.push(Slot::Day(i));
            i += 1;
        }
    }
    out
}
