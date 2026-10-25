#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Run,
    Attention,
    Failed,
    Other,
}

impl Tone {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Attention => "attention",
            Self::Failed => "failed",
            Self::Other => "other",
        }
    }
}

#[must_use]
pub fn state(raw: &str) -> (&'static str, Tone) {
    match raw {
        "running" | "in_progress" | "busy" => ("运行中", Tone::Run),
        "starting" => ("启动中", Tone::Run),
        "awaiting_approval" | "awaiting" | "approval" => ("等你批准", Tone::Attention),
        "lost" => ("已失联", Tone::Failed),
        "in_review" => ("待审阅", Tone::Attention),
        "failed" | "errored" | "error" => ("失败", Tone::Failed),
        "paused" => ("已暂停", Tone::Other),
        "offline" => ("离线", Tone::Other),
        "online" => ("在线", Tone::Other),
        "completed" | "done" => ("已完成", Tone::Other),
        "interrupted" => ("已中断", Tone::Other),
        "killed" => ("已结束", Tone::Other),
        "cancelled" => ("已取消", Tone::Other),
        "skipped" => ("已跳过", Tone::Other),
        "pending" => ("等待中", Tone::Other),
        "backlog" => ("待规划", Tone::Other),
        "todo" => ("待办", Tone::Other),
        "active" => ("已启用", Tone::Other),
        "archived" => ("已归档", Tone::Other),
        _ => ("空闲", Tone::Other),
    }
}

#[must_use]
pub fn term(raw: &str) -> &str {
    match raw {
        "exclusive" => "独占",
        "shared" => "共享",
        "read" => "只读",
        "claude_cli" | "claude" => "Claude Code",
        "codex_app_server" | "codex" => "Codex",
        "job_spawn" => "启动后台任务",
        "job_register" => "登记后台任务",
        "node_offline" => "机器离线",
        "quota_high" => "额度快用完",
        "job_orphan" => "后台任务失去依赖",
        "bot_stuck" => "智能体卡住",
        "local" => "本机",
        "lead" => "总控",
        "worker" => "执行",
        other => other,
    }
}

#[must_use]
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    match s {
        0..60 => format!("{s} 秒"),
        60..3600 if s.is_multiple_of(60) => format!("{} 分钟", s / 60),
        60..3600 => format!("{} 分 {} 秒", s / 60, s % 60),
        3600..86_400 if s % 3600 / 60 == 0 => format!("{} 小时", s / 3600),
        3600..86_400 => format!("{} 小时 {} 分", s / 3600, s % 3600 / 60),
        _ if s % 86_400 / 3600 == 0 => format!("{} 天", s / 86_400),
        _ => format!("{} 天 {} 小时", s / 86_400, s % 86_400 / 3600),
    }
}

#[must_use]
pub fn weekdays(days: &[u8]) -> String {
    let mut sorted = days.iter().map(|d| d % 7).collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted.dedup();
    match sorted.as_slice() {
        [1, 2, 3, 4, 5] => "工作日".into(),
        [0, 6] => "周末".into(),
        [0, 1, 2, 3, 4, 5, 6] => "每天".into(),
        list => {
            let names = ["日", "一", "二", "三", "四", "五", "六"];
            let picked = list
                .iter()
                .filter_map(|d| names.get(usize::from(*d)))
                .copied()
                .collect::<Vec<_>>()
                .join("、");
            format!("每周{picked}")
        }
    }
}

#[must_use]
pub fn timezone(tz: &str) -> &str {
    match tz {
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "PRC" => "北京时间",
        "Asia/Hong_Kong" => "香港时间",
        "Asia/Taipei" => "台北时间",
        "Asia/Tokyo" => "东京时间",
        "UTC" | "Etc/UTC" | "GMT" => "UTC",
        other => other,
    }
}

#[must_use]
pub fn error(raw: &str, status: Option<u16>) -> String {
    let lower = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&[
        "机器连不上",
        "退出码 255",
        "ssh:",
        "could not resolve hostname",
        "no route to host",
        "host is down",
        "connection closed by",
    ]) {
        "机器连不上，确认它开着机、网络是通的".into()
    } else if raw.starts_with("退出码 ") {
        "远端命令没有成功执行".into()
    } else if status == Some(529) || has(&["529", "overloaded"]) {
        "服务繁忙，稍后再试".into()
    } else if status == Some(429) || has(&["429", "rate limit", "rate_limit", "too many requests"])
    {
        "请求太频繁，稍后再试".into()
    } else if status == Some(401)
        || has(&[
            "401 ",
            "unauthorized",
            "invalid api key",
            "not logged in",
            "authenticat",
        ])
    {
        "登录已失效，请重新登录".into()
    } else if status == Some(403) || has(&["403 ", "forbidden", "permission_error"]) {
        "没有权限".into()
    } else if has(&["billing_error"]) {
        "额度不足，请检查账号".into()
    } else if has(&[
        "api error: 500",
        "internal server error",
        "server_error",
        "api_error",
    ]) {
        "服务出错了，稍后再试".into()
    } else if has(&["timed out", "timeout", "deadline exceeded"]) {
        "等待超时，稍后再试".into()
    } else if has(&[
        "connection refused",
        "failed to fetch",
        "networkerror",
        "network error",
        "econnrefused",
    ]) {
        "连不上服务，请检查网络".into()
    } else if status.is_some_and(|c| c >= 500) && (raw.is_empty() || raw.starts_with("HTTP ")) {
        "服务出错了，稍后再试".into()
    } else if status == Some(404) && (raw.is_empty() || raw.starts_with("HTTP ")) {
        "找不到了，可能已被删除".into()
    } else {
        raw.to_owned()
    }
}

fn http_code(raw: &str) -> Option<u16> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter_map(|t| t.parse::<u16>().ok())
        .find(|n| (400..600).contains(n))
}

#[must_use]
pub fn cause(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "请求没有成功".into();
    }
    let code = http_code(raw);
    let mut why = error(raw, None);
    if why == raw
        && let Some(c) = code
    {
        why = error("", Some(c));
    }
    let head: String = if why == raw || why.is_empty() {
        let line = raw
            .rsplit("stderr: ")
            .next()
            .unwrap_or(raw)
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let mut cut: String = line.chars().take(60).collect();
        if line.chars().count() > 60 {
            cut.push('…');
        }
        cut
    } else {
        why.split('，').next().unwrap_or(&why).to_owned()
    };
    match code {
        Some(code) => format!("{head}（{code}）"),
        None => head,
    }
}

#[must_use]
pub fn failure(
    cause: blazar_core_types::failure::FailureCause,
    machine: &str,
    runtime: &str,
    resets: Option<&str>,
    message: &str,
) -> String {
    use blazar_core_types::failure::FailureCause;
    match cause {
        FailureCause::Network => "网络断了，没做完".into(),
        FailureCause::Busy => "服务繁忙，没做完".into(),
        FailureCause::Node => format!("{machine} 连不上"),
        FailureCause::Login => format!("{} 未登录", term(runtime)),
        FailureCause::Quota => match resets.filter(|r| !r.is_empty()) {
            Some(at) => format!("额度用完了，{at} 重置"),
            None => "额度用完了".into(),
        },
        FailureCause::Resume | FailureCause::Other => cause_of(message),
    }
}

fn cause_of(message: &str) -> String {
    cause(message)
}

#[must_use]
pub const fn next_step(cause: blazar_core_types::failure::FailureCause) -> &'static str {
    use blazar_core_types::failure::FailureCause;
    match cause {
        FailureCause::Network | FailureCause::Busy => "继续",
        FailureCause::Node => "重试连接",
        FailureCause::Login => "登录…",
        FailureCause::Quota => "换账号…",
        FailureCause::Resume | FailureCause::Other => "重试",
    }
}

#[must_use]
pub fn retry_round(attempt: u64, max: u64) -> String {
    if max > 0 {
        format!("{attempt}/{max}")
    } else {
        format!("第 {attempt} 次")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_and_failed_never_share_a_tone() {
        for raw in ["running", "in_progress", "busy", "starting"] {
            assert_eq!(state(raw).1, Tone::Run);
        }
        for raw in ["failed", "errored"] {
            assert_eq!(state(raw), ("失败", Tone::Failed));
        }
    }

    #[test]
    fn each_state_has_one_name() {
        assert_eq!(state("running").0, state("in_progress").0);
        assert_eq!(state("awaiting").0, "等你批准");
        assert_eq!(state("awaiting_approval").0, "等你批准");
        assert_eq!(state("paused"), ("已暂停", Tone::Other));
        assert_eq!(state("done").0, state("completed").0);
    }

    #[test]
    fn times_read_as_words() {
        assert_eq!(duration(14.0), "14 秒");
        assert_eq!(duration(180.0), "3 分钟");
        assert_eq!(duration(185.0), "3 分 5 秒");
        assert_eq!(duration(7200.0), "2 小时");
        assert_eq!(weekdays(&[1, 2, 3, 4, 5]), "工作日");
        assert_eq!(weekdays(&[1, 3]), "每周一、三");
    }

    #[test]
    fn machine_failures_hide_their_addresses() {
        let raw = "退出码 255：ssh: connect to host 10.0.0.7 port 22: Operation timed out";
        assert_eq!(
            error(raw, Some(502)),
            "机器连不上，确认它开着机、网络是通的"
        );
        assert_eq!(
            error(
                "机器连不上：ssh: Could not resolve hostname gpu-a.invalid",
                None
            ),
            "机器连不上，确认它开着机、网络是通的"
        );
        assert_eq!(error("退出码 2：tar: x", None), "远端命令没有成功执行");
    }

    #[test]
    fn errors_read_as_plain_words() {
        assert_eq!(
            error("API Error: 529 Overloaded", None),
            "服务繁忙，稍后再试"
        );
        assert_eq!(error("HTTP 502", Some(502)), "服务出错了，稍后再试");
        assert_eq!(error("工作区不存在", Some(404)), "工作区不存在");
        assert_eq!(error("server_error", None), "服务出错了，稍后再试");
        assert_eq!(error("billing_error", None), "额度不足，请检查账号");
        assert_eq!(term("exclusive"), "独占");
    }

    #[test]
    fn runtimes_read_as_product_names() {
        assert_eq!(term("claude_cli"), "Claude Code");
        assert_eq!(term("codex_app_server"), "Codex");
        assert_eq!(term("codex"), "Codex");
        assert_eq!(term("local"), "本机");
    }

    #[test]
    fn every_failure_cause_reads_as_one_line_and_one_next_step() {
        use blazar_core_types::failure::FailureCause;
        let line = |c, machine, rt, resets, msg| failure(c, machine, rt, resets, msg);
        assert_eq!(
            line(FailureCause::Network, "本机", "claude", None, "ENOTFOUND"),
            "网络断了，没做完"
        );
        assert_eq!(
            line(FailureCause::Busy, "本机", "claude", None, "529"),
            "服务繁忙，没做完"
        );
        assert_eq!(
            line(FailureCause::Node, "gpu-x", "claude", None, "ssh"),
            "gpu-x 连不上"
        );
        assert_eq!(
            line(FailureCause::Login, "本机", "claude", None, "401"),
            "Claude Code 未登录"
        );
        assert_eq!(
            line(FailureCause::Login, "本机", "codex", None, "401"),
            "Codex 未登录"
        );
        assert_eq!(
            line(FailureCause::Quota, "本机", "claude", None, "429"),
            "额度用完了"
        );
        assert_eq!(
            line(
                FailureCause::Quota,
                "本机",
                "claude",
                Some("周日 08:00"),
                "429"
            ),
            "额度用完了，周日 08:00 重置"
        );
        assert_eq!(
            line(
                FailureCause::Other,
                "本机",
                "claude",
                None,
                "API Error: 400 bad request"
            ),
            "API Error: 400 bad request（400）"
        );
        assert_eq!(next_step(FailureCause::Network), "继续");
        assert_eq!(next_step(FailureCause::Busy), "继续");
        assert_eq!(next_step(FailureCause::Node), "重试连接");
        assert_eq!(next_step(FailureCause::Login), "登录…");
        assert_eq!(next_step(FailureCause::Quota), "换账号…");
        assert_eq!(next_step(FailureCause::Other), "重试");
    }

    #[test]
    fn causes_keep_the_status_code_and_drop_the_advice() {
        assert_eq!(cause("API Error: 529 overloaded"), "服务繁忙（529）");
        assert_eq!(cause("server_error"), "服务出错了");
        assert_eq!(cause("rate_limit"), "请求太频繁");
        assert_eq!(cause("HTTP 502"), "服务出错了（502）");
        assert_eq!(cause("ECONNRESET"), "ECONNRESET");
        assert_eq!(cause(""), "请求没有成功");
        assert_eq!(
            cause(
                "agent 异常退出（退出码 1）。stderr: Error: ENOSPC: no space left on device, write"
            ),
            "Error: ENOSPC: no space left on device, write"
        );
        assert_eq!(cause(&"x".repeat(70)), format!("{}…", "x".repeat(60)));
        assert_eq!(retry_round(2, 10), "2/10");
        assert_eq!(retry_round(3, 0), "第 3 次");
    }
}
