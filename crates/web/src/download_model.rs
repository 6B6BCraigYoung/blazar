use std::collections::VecDeque;

/// 速度按最近这么长时间里的增量算，太短会跳得厉害。
const SPEED_WINDOW_MS: f64 = 3000.0;
const SPEED_MIN_SPAN_MS: f64 = 400.0;

#[must_use]
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[must_use]
pub fn percent(received: u64, total: Option<u64>) -> Option<u32> {
    let total = total?;
    if total == 0 {
        return Some(100);
    }
    Some(((received.min(total) as f64 / total as f64) * 100.0).floor() as u32)
}

/// 「12.3 MB / 40.0 MB · 30% · 5.2 MB/s」；不知道总大小时是「已下载 12.3 MB · 5.2 MB/s」。
#[must_use]
pub fn progress_line(received: u64, total: Option<u64>, speed: Option<f64>) -> String {
    let mut parts = match (total, percent(received, total)) {
        (Some(total), Some(pct)) => vec![
            format!("{} / {}", bytes(received), bytes(total)),
            format!("{pct}%"),
        ],
        _ => vec![format!("已下载 {}", bytes(received))],
    };
    if let Some(speed) = speed {
        parts.push(format!("{}/s", bytes(speed.max(0.0) as u64)));
    }
    parts.join(" · ")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Speed {
    samples: VecDeque<(f64, u64)>,
}

impl Speed {
    /// 记一个采样（毫秒时间戳，累计字节），返回每秒字节数；数据不够时为 None。
    pub fn push(&mut self, at_ms: f64, received: u64) -> Option<f64> {
        self.samples.push_back((at_ms, received));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| at_ms - t > SPEED_WINDOW_MS)
            && self.samples.len() > 2
        {
            self.samples.pop_front();
        }
        let (t0, r0) = *self.samples.front()?;
        let span = at_ms - t0;
        (span >= SPEED_MIN_SPAN_MS).then(|| received.saturating_sub(r0) as f64 * 1000.0 / span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(7_600_000), "7.2 MB");
        assert_eq!(bytes(150 * 1024 * 1024), "150 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn progress_lines_with_and_without_a_total() {
        assert_eq!(
            progress_line(
                3 * 1024 * 1024,
                Some(10 * 1024 * 1024),
                Some(1024.0 * 1024.0)
            ),
            "3.0 MB / 10.0 MB · 30% · 1.0 MB/s"
        );
        assert_eq!(progress_line(5 * 1024 * 1024, None, None), "已下载 5.0 MB");
        assert_eq!(percent(0, Some(0)), Some(100));
        assert_eq!(percent(11, Some(10)), Some(100));
        assert_eq!(percent(1, None), None);
    }

    #[test]
    fn speed_needs_a_moment_then_follows_the_recent_window() {
        let mut s = Speed::default();
        assert_eq!(s.push(0.0, 0), None);
        assert_eq!(s.push(200.0, 100_000), None);
        assert_eq!(s.push(1000.0, 1_000_000), Some(1_000_000.0));
        for i in 2u32..=10 {
            s.push(
                f64::from(i) * 1000.0,
                1_000_000 + u64::from(i - 1) * 100_000,
            );
        }
        let recent = s.push(11_000.0, 2_000_000).unwrap();
        assert!((90_000.0..=110_000.0).contains(&recent), "{recent}");
    }
}
