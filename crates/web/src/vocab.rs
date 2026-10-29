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
        "asking" => ("在问你", Tone::Attention),
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    Machine,
    Remote,
    Busy,
    RateLimit,
    Login,
    Forbidden,
    Billing,
    Server,
    Timeout,
    Offline,
    Missing,
    DiskFull,
    NoTool,
}

#[must_use]
pub fn error_kind(raw: &str, status: Option<u16>) -> Option<ErrorKind> {
    let lower = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    let bare = raw.is_empty() || raw.starts_with("HTTP ");
    Some(
        if has(&["enospc", "no space left on device", "disk quota exceeded"]) {
            ErrorKind::DiskFull
        } else if has(&["command not found", ": not found"]) {
            ErrorKind::NoTool
        } else if has(&[
            "机器连不上",
            "退出码 255",
            "exit code 255",
            "ssh:",
            "could not resolve hostname",
            "no route to host",
            "host is down",
            "connection closed by",
        ]) {
            ErrorKind::Machine
        } else if raw.starts_with("退出码 ") {
            ErrorKind::Remote
        } else if status == Some(529) || has(&["529", "overloaded"]) {
            ErrorKind::Busy
        } else if status == Some(429)
            || has(&["429", "rate limit", "rate_limit", "too many requests"])
        {
            ErrorKind::RateLimit
        } else if status == Some(401)
            || has(&[
                "401 ",
                "unauthorized",
                "invalid api key",
                "not logged in",
                "authenticat",
            ])
        {
            ErrorKind::Login
        } else if status == Some(403)
            || has(&[
                "403 ",
                "forbidden",
                "permission_error",
                "eacces",
                "permission denied",
            ])
        {
            ErrorKind::Forbidden
        } else if has(&["billing_error"]) {
            ErrorKind::Billing
        } else if has(&[
            "api error: 500",
            "internal server error",
            "server_error",
            "api_error",
        ]) {
            ErrorKind::Server
        } else if has(&["timed out", "timeout", "deadline exceeded"]) {
            ErrorKind::Timeout
        } else if has(&[
            "connection refused",
            "failed to fetch",
            "networkerror",
            "network error",
            "econnrefused",
            "econnreset",
            "connection reset",
            "broken pipe",
        ]) {
            ErrorKind::Offline
        } else if status.is_some_and(|c| c >= 500) && bare {
            ErrorKind::Server
        } else if status == Some(404) && bare {
            ErrorKind::Missing
        } else {
            return None;
        },
    )
}

#[must_use]
pub fn error(raw: &str, status: Option<u16>) -> String {
    match error_kind(raw, status) {
        Some(ErrorKind::Machine) => "机器连不上，确认它开着机、网络是通的".into(),
        Some(ErrorKind::Remote) => "远端命令没有成功执行".into(),
        Some(ErrorKind::Busy) => "服务繁忙，稍后再试".into(),
        Some(ErrorKind::RateLimit) => "请求太频繁，稍后再试".into(),
        Some(ErrorKind::Login) => "登录已失效，请重新登录".into(),
        Some(ErrorKind::Forbidden) => "没有权限".into(),
        Some(ErrorKind::Billing) => "额度不足，请检查账号".into(),
        Some(ErrorKind::Server) => "服务出错了，稍后再试".into(),
        Some(ErrorKind::Timeout) => "等待超时，稍后再试".into(),
        Some(ErrorKind::Offline) => "连不上服务，请检查网络".into(),
        Some(ErrorKind::Missing) => "找不到了，可能已被删除".into(),
        Some(ErrorKind::DiskFull) => "机器的磁盘满了，清出空间后再试".into(),
        Some(ErrorKind::NoTool) => "机器上缺少要用的程序".into(),
        None => raw.to_owned(),
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3000}'..='\u{303f}').contains(&c))
}

/// 一句给人看的话，加上可以折进「详情」的原文。认得出的照 [`error`] 翻译；
/// 后台自己写的中文提示照原样；认不出的英文原文（或者“中文前缀：英文原文”的后半截）
/// 不直接给人看，只说“出了点问题”。
#[must_use]
pub fn explain(raw: &str, status: Option<u16>) -> (String, Option<String>) {
    explain_with(raw, status, error, VAGUE)
}

/// 认不出来的错误统一说这一句；[`lead`] 遇到它就只留前缀。
pub const VAGUE: &str = "出了点问题";

/// [`explain`] 的骨架，中英文共用：`translate` 是 [`error`] 或 `vocab_en::error`，
/// `vague` 是认不出来时说的那句话。
pub(crate) fn explain_with(
    raw: &str,
    status: Option<u16>,
    translate: fn(&str, Option<u16>) -> String,
    vague: &str,
) -> (String, Option<String>) {
    let raw = raw.trim();
    let detail = || Some(raw.to_owned()).filter(|r| !r.is_empty() && !r.starts_with("HTTP "));
    let why = translate(raw, status);
    if why != raw {
        return (why, detail());
    }
    if !has_cjk(raw) {
        return (vague.into(), detail());
    }
    if let Some((head, tail)) = raw.split_once('：')
        && has_cjk(head)
        && !has_cjk(tail)
        && !tail.trim().is_empty()
    {
        return (head.trim().to_owned(), Some(tail.trim().to_owned()));
    }
    (raw.to_owned(), None)
}

/// 给一句人话加上“在做什么”的前缀：“没保存上：服务繁忙，稍后再试”。
/// 认不出来的错误只留前缀（“没保存上”），原文还在「详情」里。
#[must_use]
pub fn lead(lead: &str, shown: (String, Option<String>)) -> (String, Option<String>) {
    lead_with(lead, shown, VAGUE, "：")
}

pub(crate) fn lead_with(
    lead: &str,
    (text, detail): (String, Option<String>),
    vague: &str,
    sep: &str,
) -> (String, Option<String>) {
    let lead = lead.trim().trim_end_matches(['：', ':']).trim_end();
    if lead.is_empty() || text.starts_with(lead) {
        (text, detail)
    } else if text == vague || text.is_empty() {
        (lead.to_owned(), detail)
    } else {
        (format!("{lead}{sep}{text}"), detail)
    }
}

/// 用量页的失败原因：原文先翻成一句短话（[`cause`]），说法相同的合成一行、次数相加，
/// 原文都留着放进「详情」。顺序按次数从多到少，次数相同时保持原来的先后。
#[must_use]
pub fn failure_reasons(rows: &[(f64, String)]) -> Vec<(f64, String, Vec<String>)> {
    let mut out: Vec<(f64, String, Vec<String>)> = Vec::new();
    for (n, raw) in rows {
        let why = cause(raw);
        let raw = raw.trim();
        let original = (!raw.is_empty() && raw != why).then(|| raw.to_owned());
        match out.iter_mut().find(|(_, w, _)| *w == why) {
            Some((total, _, originals)) => {
                *total += n;
                originals.extend(original.filter(|o| !originals.contains(o)));
            }
            None => out.push((*n, why, original.into_iter().collect())),
        }
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out
}

pub fn http_code(raw: &str) -> Option<u16> {
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

/// The dot colour of a failure, the one rule the workspace cards and the console rail share:
/// a failure that one click on 「继续」 picks up again (a dropped connection, a busy server)
/// is orange like anything else waiting on you; the rest — machine, login, quota, a lost
/// conversation, anything unknown — are red. Only the dot carries it; the text stays grey.
#[must_use]
pub const fn failure_tone(cause: blazar_core_types::failure::FailureCause) -> Tone {
    if cause.continues() {
        Tone::Attention
    } else {
        Tone::Failed
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
    fn only_failures_you_can_just_continue_are_orange() {
        use blazar_core_types::failure::FailureCause;
        for cause in FailureCause::ALL {
            let want = if matches!(cause, FailureCause::Network | FailureCause::Busy) {
                Tone::Attention
            } else {
                Tone::Failed
            };
            assert_eq!(failure_tone(cause), want, "{cause:?}");
        }
    }

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
    fn unknown_errors_fold_their_original_text() {
        assert_eq!(
            explain("database is locked", Some(500)),
            (
                "出了点问题".to_owned(),
                Some("database is locked".to_owned())
            )
        );
        assert_eq!(explain("", None), ("出了点问题".to_owned(), None));
        assert_eq!(
            explain("HTTP 418", Some(418)),
            ("出了点问题".to_owned(), None)
        );
        assert_eq!(
            explain("这不像一个 Git 地址", Some(400)),
            ("这不像一个 Git 地址".to_owned(), None)
        );
        assert_eq!(
            explain("创建失败：No such file or directory (os error 2)", None),
            (
                "创建失败".to_owned(),
                Some("No such file or directory (os error 2)".to_owned())
            )
        );
        assert_eq!(
            explain(
                "文件夹已存在：/Users/me/Developer/x。要用它的话，请直接选择这个文件夹",
                Some(400)
            )
            .1,
            None
        );
        let (text, detail) = explain("connection refused", None);
        assert_eq!(text, "连不上服务，请检查网络");
        assert_eq!(detail.as_deref(), Some("connection refused"));
    }

    #[test]
    fn explaining_twice_changes_nothing() {
        for raw in [
            "API Error: 529 Overloaded",
            "ssh: connect to host 10.0.0.7 port 22: Operation timed out",
            "rate_limit",
            "invalid api key",
            "permission denied",
            "billing_error",
            "server_error",
            "deadline exceeded",
            "Failed to fetch",
            "HTTP 404",
            "ENOSPC",
            "bash: x: command not found",
            "database is locked",
            "退出码 2：tar",
        ] {
            let once = explain(raw, None).0;
            assert_eq!(explain(&once, None), (once.clone(), None), "{raw}");
        }
    }

    #[test]
    fn leads_say_what_was_being_done() {
        let shown = |raw: &str| explain(raw, None);
        assert_eq!(
            lead("没保存上", shown("API Error: 529 Overloaded")),
            (
                "没保存上：服务繁忙，稍后再试".to_owned(),
                Some("API Error: 529 Overloaded".to_owned())
            )
        );
        assert_eq!(
            lead("无法加载设置：", shown("database is locked")),
            (
                "无法加载设置".to_owned(),
                Some("database is locked".to_owned())
            )
        );
        assert_eq!(
            lead("", shown("工作区不存在")),
            ("工作区不存在".to_owned(), None)
        );
        assert_eq!(
            lead("改名失败", shown("改名失败：名字重复了")),
            ("改名失败：名字重复了".to_owned(), None)
        );
    }

    #[test]
    fn failure_reasons_read_as_causes_and_keep_their_originals() {
        let rows = [
            (2.0, "API Error: 529 overloaded".to_owned()),
            (
                5.0,
                "agent 异常退出（退出码 1）。stderr: Error: ENOSPC: no space left on device, write"
                    .to_owned(),
            ),
            (1.0, "API Error: 529 Overloaded".to_owned()),
            (1.0, "API Error: 529 overloaded".to_owned()),
        ];
        let got = failure_reasons(&rows);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, 5.0);
        assert_eq!(got[0].1, "机器的磁盘满了");
        assert_eq!(got[1].0, 4.0);
        assert_eq!(got[1].1, "服务繁忙（529）");
        assert_eq!(
            got[1].2,
            vec![
                "API Error: 529 overloaded".to_owned(),
                "API Error: 529 Overloaded".to_owned()
            ]
        );
        assert_eq!(
            failure_reasons(&[(1.0, String::new())]),
            vec![(1.0, "请求没有成功".to_owned(), vec![])]
        );
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
        assert_eq!(cause("ECONNRESET"), "连不上服务");
        assert_eq!(
            cause("bash: claude: command not found"),
            "机器上缺少要用的程序"
        );
        assert_eq!(cause(""), "请求没有成功");
        assert_eq!(
            cause(
                "agent 异常退出（退出码 1）。stderr: Error: ENOSPC: no space left on device, write"
            ),
            "机器的磁盘满了"
        );
        assert_eq!(cause(&"x".repeat(70)), format!("{}…", "x".repeat(60)));
        assert_eq!(retry_round(2, 10), "2/10");
        assert_eq!(retry_round(3, 0), "第 3 次");
    }
}
