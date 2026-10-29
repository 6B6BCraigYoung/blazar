//! Plain-language text for the bot list and detail page: no raw scopes, no
//! internal nouns.

use blazar_core_types::api::machine_label;
use serde_json::Value;

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

fn machine(node: &str) -> String {
    machine_label(node).to_owned()
}

fn ascii_edge(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Joins two phrases, putting a space only where a Latin word meets Chinese.
fn join(left: &str, right: &str) -> String {
    let gap = ascii_edge(left.chars().last()) || ascii_edge(right.chars().next());
    if gap {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

/// `gpu:0,1` → `GPU 0、1`, `dir:/a/b/runs` → `目录 runs`, `port:8766` → `端口 8766`.
#[must_use]
pub fn scope(raw: &str) -> String {
    let (kind, rest) = raw.split_once(':').unwrap_or((raw, ""));
    let rest = rest.trim();
    let list = || {
        rest.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("、")
    };
    let named = |noun: &str, what: String| {
        if what.is_empty() {
            noun.to_owned()
        } else {
            format!("{noun} {what}")
        }
    };
    match kind.trim() {
        "gpu" => named("GPU", list()),
        "dir" => {
            let trimmed = rest.trim_end_matches('/');
            let leaf = trimmed.rsplit('/').next().unwrap_or(trimmed);
            named(
                "目录",
                if leaf.is_empty() {
                    rest.to_owned()
                } else {
                    leaf.to_owned()
                },
            )
        }
        "port" => named("端口", list()),
        "repo" => named("仓库", rest.to_owned()),
        "device" => named("设备", list()),
        _ => raw.to_owned(),
    }
}

/// One claim as a phrase: `本机的 GPU 0、1`, `gpu-1 的端口 8766`.
#[must_use]
pub fn claim(c: &Value) -> String {
    let machine = machine(&s(c, "node"));
    join(&join(&machine, "的"), &scope(&s(c, "scope")))
}

/// What a claim is, verbatim, for tooltips: `gpu-1 · gpu:0,1 · 独占`.
#[must_use]
pub fn claim_detail(c: &Value) -> String {
    let mut out = format!("{} · {}", machine(&s(c, "node")), s(c, "scope"));
    let raw_mode = s(c, "mode");
    let mode = crate::vocab::term(&raw_mode);
    if !mode.is_empty() {
        out.push_str(" · ");
        out.push_str(mode);
    }
    let note = s(c, "note");
    if !note.is_empty() {
        out.push_str(" · ");
        out.push_str(&note);
    }
    out
}

/// `占着本机的 GPU 0、1` for one claim, `占着 2 项资源` for several.
#[must_use]
pub fn claims(claims: &[Value]) -> Option<String> {
    match claims {
        [] => None,
        [one] => Some(join("占着", &claim(one))),
        many => Some(format!("占着 {} 项资源", many.len())),
    }
}

/// The one grey line under a bot's name: what it is doing now, or what it
/// did last.
#[must_use]
pub fn now(b: &Value, ago_of: impl Fn(&str) -> String) -> String {
    let state = s(b, "state");
    let last = s(b, "last_instruction");
    let pending = b["pending_mail"].as_u64().unwrap_or(0);
    let ago = b["last_active_at"].as_str().map(&ago_of);
    let with_ago = |text: String| match &ago {
        Some(t) => format!("{text} · {t}"),
        None => text,
    };
    if b["archived"].as_bool() == Some(true) {
        return format!(
            "归档于 {}",
            b["archived_at"].as_str().map(&ago_of).unwrap_or_default()
        );
    }
    match state.as_str() {
        "awaiting" => return "有一步操作等你批准".into(),
        "lost" => {
            let machine = machine(&s(b, "node"));
            return join(&machine, "连不上了，等它回来接着做");
        }
        "running" if !last.is_empty() => return format!("在做：{last}"),
        "running" => return "正在工作".into(),
        "paused" => return with_ago("已暂停".into()),
        _ => {}
    }
    if pending > 0 {
        return format!("{pending} 封邮件待处理");
    }
    if !last.is_empty() {
        return with_ago(format!("上次：{last}"));
    }
    ago.map_or_else(|| "还没派过活".to_owned(), |t| format!("最近活动 {t}"))
}

/// Badges worth showing: only the settings that differ from the defaults
/// (执行, P1, 固定).
#[must_use]
pub fn badges(b: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if s(b, "role") == "lead" {
        out.push(("总控".to_owned(), "可以给其他 bot 派活".to_owned()));
    }
    match s(b, "priority").as_str() {
        "P0" => out.push(("P0".to_owned(), "优先级最高：真机这类不能等的".to_owned())),
        "P2" => out.push(("P2".to_owned(), "额度紧张时让路".to_owned())),
        _ => {}
    }
    if b["temporary"].as_bool() == Some(true) {
        let hours = b["idle_ttl_secs"].as_i64().unwrap_or(86_400) / 3600;
        out.push(("临时".to_owned(), format!("闲置 {hours} 小时后自动归档")));
    }
    if b["hot"].as_bool() == Some(true) {
        out.push(("保持在线".to_owned(), "新消息会立即送达".to_owned()));
    }
    out
}

/// Tooltip for the grey line: every claim spelled out.
#[must_use]
pub fn claims_title(b: &Value) -> Option<String> {
    let list = b["claims"].as_array().filter(|l| !l.is_empty())?;
    Some(list.iter().map(claim_detail).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scopes_read_as_words() {
        assert_eq!(scope("gpu:0,1"), "GPU 0、1");
        assert_eq!(scope("dir:/tmp/blazar-ui-demo/quasar-ml/runs"), "目录 runs");
        assert_eq!(scope("dir:/data/queue/"), "目录 queue");
        assert_eq!(scope("port:18731"), "端口 18731");
        assert_eq!(scope("repo"), "仓库");
        assert_eq!(scope("device:can1"), "设备 can1");
        assert_eq!(scope("weird"), "weird");
    }

    #[test]
    fn claims_summarise() {
        let one = json!({ "node": "local", "scope": "gpu:0,1", "mode": "exclusive" });
        let two = json!({ "node": "gpu-1", "scope": "port:8766", "mode": "shared" });
        assert_eq!(claims(&[]), None);
        assert_eq!(
            claims(std::slice::from_ref(&one)).as_deref(),
            Some("占着本机的 GPU 0、1")
        );
        assert_eq!(claim(&two), "gpu-1 的端口 8766");
        assert_eq!(claims(&[one, two]).as_deref(), Some("占着 2 项资源"));
    }

    #[test]
    fn defaults_have_no_badges() {
        let plain = json!({ "role": "worker", "priority": "P1", "temporary": false });
        assert!(badges(&plain).is_empty());
        let odd =
            json!({ "role": "lead", "priority": "P0", "temporary": true, "idle_ttl_secs": 14_400 });
        let names: Vec<_> = badges(&odd).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["总控", "P0", "临时"]);
    }

    #[test]
    fn status_line_prefers_what_needs_you() {
        let b = json!({ "state": "awaiting", "pending_mail": 3, "last_instruction": "跑评测" });
        assert_eq!(now(&b, str::to_owned), "有一步操作等你批准");
        let b = json!({ "state": "running", "last_instruction": "跑评测" });
        assert_eq!(now(&b, str::to_owned), "在做：跑评测");
        let b = json!({ "state": "idle", "pending_mail": 2 });
        assert_eq!(now(&b, str::to_owned), "2 封邮件待处理");
        let b = json!({ "state": "idle" });
        assert_eq!(now(&b, str::to_owned), "还没派过活");
    }
}
