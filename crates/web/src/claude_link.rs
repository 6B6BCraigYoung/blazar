use blazar_core_types::api::{ClaudeLink, ClaudeLinkState};

#[must_use]
pub fn label(state: ClaudeLinkState) -> &'static str {
    match state {
        ClaudeLinkState::Ok => "正常",
        ClaudeLinkState::Slow => "较慢",
        ClaudeLinkState::Degraded => "不稳定",
        ClaudeLinkState::ProxyDown => "代理不通",
        ClaudeLinkState::EgressDown => "连不上 API",
        ClaudeLinkState::MachineDown => "机器连不上",
        ClaudeLinkState::Unknown => "未测",
    }
}

#[must_use]
pub fn latency_text(ms: Option<u32>) -> Option<String> {
    ms.map(|m| format!("{m} ms"))
}

#[must_use]
pub fn loss_text(loss: f64) -> Option<String> {
    (loss > 0.0).then(|| format!("丢包 {}%", (loss * 100.0).round() as i64))
}

#[must_use]
pub fn cause_label(cause: &str) -> &'static str {
    match cause {
        "network" => "网络",
        "busy" => "服务繁忙",
        "login" => "未登录",
        "quota" => "额度用完",
        "node" => "机器连不上",
        _ => "出错",
    }
}

/// 表格里那一格：状态加一个最能说明问题的数字（正常/较慢看延迟，不稳定看丢包）。
#[must_use]
pub fn cell(link: &ClaudeLink) -> String {
    let tail = match link.state {
        ClaudeLinkState::Ok | ClaudeLinkState::Slow => latency_text(link.latency_ms),
        ClaudeLinkState::Degraded => loss_text(link.loss),
        _ => None,
    };
    match tail {
        Some(t) => format!("{} · {t}", label(link.state)),
        None => label(link.state).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(state: ClaudeLinkState, latency_ms: Option<u32>, loss: f64) -> ClaudeLink {
        ClaudeLink {
            state,
            route: "直连".into(),
            latency_ms,
            loss,
            checked_at: None,
            detail: None,
        }
    }

    #[test]
    fn the_cell_pairs_the_state_with_the_number_that_explains_it() {
        assert_eq!(
            cell(&link(ClaudeLinkState::Ok, Some(128), 0.0)),
            "正常 · 128 ms"
        );
        assert_eq!(
            cell(&link(ClaudeLinkState::Slow, Some(2600), 0.0)),
            "较慢 · 2600 ms"
        );
        assert_eq!(
            cell(&link(ClaudeLinkState::Degraded, Some(130), 0.4)),
            "不稳定 · 丢包 40%"
        );
        assert_eq!(
            cell(&link(ClaudeLinkState::ProxyDown, None, 1.0)),
            "代理不通"
        );
        assert_eq!(
            cell(&link(ClaudeLinkState::MachineDown, None, 1.0)),
            "机器连不上"
        );
    }

    #[test]
    fn loss_only_shows_when_there_is_loss() {
        assert_eq!(loss_text(0.0), None);
        assert_eq!(loss_text(0.2), Some("丢包 20%".into()));
    }
}
