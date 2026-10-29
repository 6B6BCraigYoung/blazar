//! 断线时页面怎么判断、怎么说：机器连不上、机器在重连、和 hub 的实时连接停了。
//! 纯逻辑，原生也能编译，方便单测。

/// 一台机器对一个工作区来说现在是什么状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeLink {
    Reachable,
    /// 机器掉线了，但活还在那边跑：橙点“重连中”，不是红色“连不上”。
    Reconnecting,
    Unreachable,
}

impl NodeLink {
    #[must_use]
    pub const fn offline(self) -> bool {
        !matches!(self, Self::Reachable)
    }
}

/// hub 在工作区的当前一步里写 `reconnecting`（`text` 是机器名）：跟读断了、agent 还在远端跑。
pub const HUB_RECONNECTING_STEP: &str = "reconnecting";

#[must_use]
pub fn hub_says_reconnecting(step_kind: Option<&str>) -> bool {
    step_kind == Some(HUB_RECONNECTING_STEP)
}

/// 工作区（或会话）的活动状态算不算“还在跑”。
#[must_use]
pub fn running_activity(activity: &str) -> bool {
    matches!(activity, "running" | "starting")
}

/// `status` 是机器的在线状态（不知道这台机器时为 `None`），`running` 是这个工作区的活还在跑，
/// `hub_reconnecting` 是 hub 明确说“这个会话的机器在重连”。
#[must_use]
pub fn node_link(
    node: &str,
    status: Option<&str>,
    running: bool,
    hub_reconnecting: bool,
) -> NodeLink {
    if node == "local" {
        return NodeLink::Reachable;
    }
    if hub_reconnecting {
        return NodeLink::Reconnecting;
    }
    match status {
        Some(s) if s != "online" => {
            if running {
                NodeLink::Reconnecting
            } else {
                NodeLink::Unreachable
            }
        }
        _ => NodeLink::Reachable,
    }
}

/// 链路慢到值得提一句的门槛。
pub const SLOW_MS: f64 = 300.0;

/// 知道延迟、而且超过 300ms 才算“较慢”；不知道就不说。
#[must_use]
pub fn slow(latency_ms: Option<f64>) -> bool {
    latency_ms.is_some_and(|ms| ms.is_finite() && ms > SLOW_MS)
}

/// 机器表「状态」列里离线的说法：`ago` 是「5 小时前」这样的相对时间，得到「5 小时前离线」；
/// 不知道上次在线是什么时候（空串）就只说「离线」。
#[must_use]
pub fn offline_text(ago: &str) -> String {
    if ago.is_empty() {
        "离线".into()
    } else {
        format!("{ago}离线")
    }
}

/// 实时连接的心跳：每 15 秒问一次 hub，连着两次没回应就当断了，重连。
pub const HEARTBEAT_EVERY_MS: u32 = 15_000;
/// 一次心跳最多等多久。
pub const HEARTBEAT_TIMEOUT_MS: u32 = 5_000;
/// 连着几次没回应算断。
pub const HEARTBEAT_MISSES: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heartbeat {
    misses: u32,
}

impl Heartbeat {
    /// 记一次心跳的结果；返回 `true` 表示该断开重连了（计数随之清零）。
    pub fn beat(&mut self, answered: bool) -> bool {
        if answered {
            self.misses = 0;
            return false;
        }
        self.misses += 1;
        if self.misses >= HEARTBEAT_MISSES {
            self.misses = 0;
            return true;
        }
        false
    }

    pub fn reset(&mut self) {
        self.misses = 0;
    }

    #[cfg(test)]
    #[must_use]
    pub const fn misses(self) -> u32 {
        self.misses
    }
}

/// 和 hub 断开时正文顶部那条横条的字。桌面端告诉了机器名就带上。
#[must_use]
pub fn banner_text(machine: Option<&str>) -> String {
    match machine.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => format!("正在重新连接 {m}…"),
        None => "正在重新连接…".to_owned(),
    }
}

/// 断开多久才显示横条：刚打开页面、或者一闪而过的重连不打扰。
pub const BANNER_GRACE_MS: u32 = 2_000;

/// 机器状态从“连不上”变回“在线”的那一刻：要把排队的消息发出去、让终端重连。
#[must_use]
pub fn came_back(was_offline: Option<bool>, offline: bool) -> bool {
    was_offline == Some(true) && !offline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_on_an_offline_machine_is_reconnecting_not_failed() {
        assert_eq!(
            node_link("gpu-1", Some("offline"), true, false),
            NodeLink::Reconnecting
        );
        assert_eq!(
            node_link("gpu-1", Some("offline"), false, false),
            NodeLink::Unreachable
        );
        assert_eq!(
            node_link("gpu-1", Some("online"), true, false),
            NodeLink::Reachable
        );
    }

    #[test]
    fn this_machine_and_unknown_machines_are_reachable() {
        assert_eq!(
            node_link("local", Some("offline"), false, true),
            NodeLink::Reachable
        );
        assert_eq!(node_link("gpu-1", None, false, false), NodeLink::Reachable);
    }

    #[test]
    fn the_hub_flag_wins_even_before_the_status_flips() {
        assert_eq!(
            node_link("gpu-1", Some("online"), false, true),
            NodeLink::Reconnecting
        );
        assert!(NodeLink::Reconnecting.offline());
        assert!(NodeLink::Unreachable.offline());
        assert!(!NodeLink::Reachable.offline());
    }

    #[test]
    fn the_hub_marks_reconnecting_in_the_current_step() {
        assert!(hub_says_reconnecting(Some("reconnecting")));
        assert!(!hub_says_reconnecting(Some("tool")));
        assert!(!hub_says_reconnecting(None));
    }

    #[test]
    fn only_running_and_starting_count_as_running() {
        assert!(running_activity("running"));
        assert!(running_activity("starting"));
        assert!(!running_activity("awaiting_approval"));
        assert!(!running_activity("idle"));
    }

    #[test]
    fn offline_says_how_long_ago() {
        assert_eq!(offline_text("5 小时前"), "5 小时前离线");
        assert_eq!(offline_text("刚刚"), "刚刚离线");
        assert_eq!(offline_text(""), "离线");
    }

    #[test]
    fn slow_needs_a_known_latency_over_300ms() {
        assert!(!slow(None));
        assert!(!slow(Some(300.0)));
        assert!(slow(Some(301.0)));
        assert!(!slow(Some(f64::NAN)));
    }

    #[test]
    fn two_missed_pongs_in_a_row_mean_reconnect() {
        let mut hb = Heartbeat::default();
        assert!(!hb.beat(false));
        assert_eq!(hb.misses(), 1);
        assert!(!hb.beat(true));
        assert_eq!(hb.misses(), 0);
        assert!(!hb.beat(false));
        assert!(hb.beat(false));
        assert_eq!(hb.misses(), 0, "counting starts over after a reconnect");
        assert!(!hb.beat(false));
        hb.reset();
        assert!(!hb.beat(false));
    }

    #[test]
    fn banner_names_the_machine_when_known() {
        assert_eq!(banner_text(Some("gpu-1")), "正在重新连接 gpu-1…");
        assert_eq!(banner_text(Some("  ")), "正在重新连接…");
        assert_eq!(banner_text(None), "正在重新连接…");
    }

    #[test]
    fn came_back_only_on_the_offline_to_online_edge() {
        assert!(came_back(Some(true), false));
        assert!(!came_back(None, false));
        assert!(!came_back(Some(false), false));
        assert!(!came_back(Some(true), true));
    }
}
