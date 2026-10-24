use blazar_core_types::api::{WorkspaceStep, WorkspaceView};

use crate::vocab::{self, Tone};

#[must_use]
pub fn retrying(step: Option<&WorkspaceStep>) -> Option<(u64, u64)> {
    let s = step.filter(|s| s.kind == "retry")?;
    Some((
        s.input["attempt"].as_u64().unwrap_or(0),
        s.input["max_retries"].as_u64().unwrap_or(0),
    ))
}

#[must_use]
pub fn status(activity: &str, step: Option<&WorkspaceStep>) -> (String, Tone) {
    if matches!(activity, "running" | "starting")
        && let Some((n, max)) = retrying(step)
    {
        return (format!("重试中 {}", vocab::retry_round(n, max)), Tone::Run);
    }
    let (label, tone) = vocab::state(activity);
    (label.to_owned(), tone)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Needs,
    Running,
    Recent,
}

impl Group {
    pub const ALL: [Self; 3] = [Self::Needs, Self::Running, Self::Recent];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Needs => "需要你处理",
            Self::Running => "运行中",
            Self::Recent => "最近",
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Needs => "needs",
            Self::Running => "running",
            Self::Recent => "recent",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub ws: WorkspaceView,
    pub group: Group,
    pub state: String,
    pub child: Option<WorkspaceView>,
    pub urgency: u8,
}

impl Row {
    #[must_use]
    pub fn subject(&self) -> &WorkspaceView {
        self.child.as_ref().unwrap_or(&self.ws)
    }
}

fn needs(state: &str) -> Option<u8> {
    match state {
        "awaiting_approval" => Some(0),
        "errored" => Some(2),
        _ => None,
    }
}

fn running(state: &str) -> bool {
    matches!(state, "running" | "starting")
}

fn last_active(w: &WorkspaceView) -> &str {
    w.last_active_at.as_deref().unwrap_or("")
}

fn classify(
    w: WorkspaceView,
    all: &[WorkspaceView],
    shown: &impl Fn(&WorkspaceView) -> String,
) -> Row {
    let state = shown(&w);
    let kids: Vec<&WorkspaceView> = all
        .iter()
        .filter(|c| c.parent.as_deref() == Some(w.id.as_str()))
        .collect();
    if let Some(urgency) = needs(&state) {
        return Row {
            ws: w,
            group: Group::Needs,
            state,
            child: None,
            urgency,
        };
    }
    let mut attention: Vec<(u8, &WorkspaceView, String)> = kids
        .iter()
        .filter_map(|c| {
            let s = shown(c);
            needs(&s).map(|u| (u + 1, *c, s))
        })
        .collect();
    attention.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| last_active(b.1).cmp(last_active(a.1)))
    });
    if let Some((urgency, c, s)) = attention.into_iter().next() {
        return Row {
            ws: w,
            group: Group::Needs,
            state: s,
            child: Some(c.clone()),
            urgency,
        };
    }
    if running(&state) {
        return Row {
            ws: w,
            group: Group::Running,
            state,
            child: None,
            urgency: 0,
        };
    }
    if let Some(c) = kids.iter().find(|c| running(&c.activity)) {
        return Row {
            ws: w,
            group: Group::Running,
            state: "running".into(),
            child: Some((*c).clone()),
            urgency: 1,
        };
    }
    Row {
        ws: w,
        group: Group::Recent,
        state,
        child: None,
        urgency: 0,
    }
}

#[must_use]
pub fn group_rows(
    top: Vec<WorkspaceView>,
    all: &[WorkspaceView],
    shown: impl Fn(&WorkspaceView) -> String,
) -> Vec<Row> {
    let mut rows: Vec<Row> = top.into_iter().map(|w| classify(w, all, &shown)).collect();
    rows.sort_by(|a, b| {
        let ga = Group::ALL.iter().position(|g| *g == a.group);
        let gb = Group::ALL.iter().position(|g| *g == b.group);
        ga.cmp(&gb)
            .then_with(|| a.urgency.cmp(&b.urgency))
            .then_with(|| {
                let (ta, tb) = (last_active(a.subject()), last_active(b.subject()));
                match (ta.is_empty(), tb.is_empty()) {
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    _ => tb.cmp(ta),
                }
            })
            .then_with(|| a.ws.name.cmp(&b.ws.name))
    });
    rows
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StepText {
    pub label: String,
    pub code: String,
}

impl StepText {
    fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            code: String::new(),
        }
    }
}

#[must_use]
pub fn step_text(step: Option<&WorkspaceStep>, state: &str, root: &str) -> StepText {
    let Some(step) = step else {
        return match state {
            "running" | "starting" => StepText::plain("正在开始…"),
            "errored" => StepText::plain("失败"),
            _ => StepText::default(),
        };
    };
    match step.kind.as_str() {
        "tool" => {
            let (label, arg) = crate::chat_model::tool_title(&step.tool, &step.input, root);
            StepText { label, code: arg }
        }
        "prompt" => StepText {
            label: "正在开始".into(),
            code: step.text.clone(),
        },
        "retry" => StepText::plain(match retrying(Some(step)) {
            Some((n, _)) if n > 0 => format!("重试中（第 {n} 次）"),
            _ => "重试中".to_owned(),
        }),
        "error" => StepText::plain(format!("失败：{}", vocab::cause(&step.text))),
        _ => StepText::plain(step.text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str, activity: &str, at: Option<&str>, parent: Option<&str>) -> WorkspaceView {
        WorkspaceView {
            id: id.into(),
            name: id.into(),
            node: "local".into(),
            path: format!("/srv/{id}"),
            project: None,
            activity: activity.into(),
            last_active_at: at.map(str::to_owned),
            session_id: None,
            diff: None,
            diff_at: None,
            parent: parent.map(str::to_owned),
            step: None,
        }
    }

    fn shown(w: &WorkspaceView) -> String {
        w.activity.clone()
    }

    #[test]
    fn rows_fall_into_needs_running_and_recent_in_that_order() {
        let all = vec![
            ws("old", "idle", Some("2026-10-01T00:00:00+00:00"), None),
            ws("busy", "running", Some("2026-10-22T10:00:00+00:00"), None),
            ws(
                "ask",
                "awaiting_approval",
                Some("2026-10-22T09:00:00+00:00"),
                None,
            ),
            ws("broken", "errored", Some("2026-10-22T11:00:00+00:00"), None),
            ws(
                "fresh",
                "completed",
                Some("2026-10-22T12:00:00+00:00"),
                None,
            ),
            ws("never", "idle", None, None),
        ];
        let rows = group_rows(all.clone(), &all, shown);
        let order: Vec<(&str, Group)> = rows.iter().map(|r| (r.ws.id.as_str(), r.group)).collect();
        assert_eq!(
            order,
            vec![
                ("ask", Group::Needs),
                ("broken", Group::Needs),
                ("busy", Group::Running),
                ("fresh", Group::Recent),
                ("old", Group::Recent),
                ("never", Group::Recent),
            ]
        );
    }

    #[test]
    fn a_child_waiting_for_you_lifts_its_parent_and_names_the_child() {
        let all = vec![
            ws("parent", "idle", Some("2026-10-20T00:00:00+00:00"), None),
            ws(
                "kid-run",
                "running",
                Some("2026-10-22T00:00:00+00:00"),
                Some("parent"),
            ),
            ws(
                "kid-ask",
                "awaiting_approval",
                Some("2026-10-21T00:00:00+00:00"),
                Some("parent"),
            ),
            ws(
                "solo",
                "awaiting_approval",
                Some("2026-10-19T00:00:00+00:00"),
                None,
            ),
        ];
        let top = vec![all[0].clone(), all[3].clone()];
        let rows = group_rows(top, &all, shown);
        assert_eq!(rows[0].ws.id, "solo");
        assert_eq!(rows[0].urgency, 0);
        assert_eq!(rows[1].ws.id, "parent");
        assert_eq!(rows[1].group, Group::Needs);
        assert_eq!(rows[1].urgency, 1);
        assert_eq!(
            rows[1].child.as_ref().map(|c| c.id.as_str()),
            Some("kid-ask")
        );
        assert_eq!(rows[1].state, "awaiting_approval");
    }

    #[test]
    fn a_child_running_puts_an_idle_parent_in_the_running_group() {
        let all = vec![
            ws("parent", "idle", None, None),
            ws(
                "kid",
                "running",
                Some("2026-10-22T00:00:00+00:00"),
                Some("parent"),
            ),
        ];
        let rows = group_rows(vec![all[0].clone()], &all, shown);
        assert_eq!(rows[0].group, Group::Running);
        assert_eq!(rows[0].child.as_ref().map(|c| c.id.as_str()), Some("kid"));
        assert_eq!(rows[0].subject().id, "kid");
    }

    #[test]
    fn rows_follow_the_state_they_are_shown_with() {
        let all = vec![ws(
            "quiet",
            "errored",
            Some("2026-10-22T00:00:00+00:00"),
            None,
        )];
        let rows = group_rows(all.clone(), &all, |_| "idle".to_owned());
        assert_eq!(rows[0].group, Group::Recent);
        assert_eq!(rows[0].state, "idle");
    }

    #[test]
    fn retries_read_as_running_and_failures_name_their_cause() {
        let retry = WorkspaceStep {
            kind: "retry".into(),
            text: "server_error".into(),
            input: serde_json::json!({ "attempt": 2, "max_retries": 10, "status": 529 }),
            ..WorkspaceStep::default()
        };
        assert_eq!(
            status("running", Some(&retry)),
            ("重试中 2/10".to_owned(), Tone::Run)
        );
        assert_eq!(status("errored", Some(&retry)).1, Tone::Failed);
        assert_eq!(status("idle", None), ("空闲".to_owned(), Tone::Other));
        assert_eq!(
            step_text(Some(&retry), "running", "").label,
            "重试中（第 2 次）"
        );
        let failed = WorkspaceStep {
            kind: "error".into(),
            text: "API Error: 529 Overloaded".into(),
            ..WorkspaceStep::default()
        };
        assert_eq!(
            step_text(Some(&failed), "errored", "").label,
            "失败：服务繁忙（529）"
        );
    }

    #[test]
    fn steps_read_as_a_label_and_a_code_argument() {
        let tool = WorkspaceStep {
            kind: "tool".into(),
            tool: "Bash".into(),
            input: serde_json::json!({ "command": "python -m pytest -q" }),
            text: String::new(),
        };
        assert_eq!(
            step_text(Some(&tool), "running", "/srv/app"),
            StepText {
                label: "Bash".into(),
                code: "python -m pytest -q".into()
            }
        );
        let edit = WorkspaceStep {
            kind: "tool".into(),
            tool: "Edit".into(),
            input: serde_json::json!({ "file_path": "/srv/app/src/main.rs" }),
            text: String::new(),
        };
        assert_eq!(
            step_text(Some(&edit), "running", "/srv/app").code,
            "src/main.rs"
        );
        let retry = WorkspaceStep {
            kind: "retry".into(),
            text: "server_error".into(),
            ..WorkspaceStep::default()
        };
        assert_eq!(step_text(Some(&retry), "running", "").label, "重试中");
        assert_eq!(step_text(None, "errored", "").label, "失败");
        assert_eq!(step_text(None, "running", "").label, "正在开始…");
        assert_eq!(step_text(None, "idle", ""), StepText::default());
        let said = WorkspaceStep {
            kind: "message".into(),
            text: "改好了。".into(),
            ..WorkspaceStep::default()
        };
        assert_eq!(step_text(Some(&said), "running", "").label, "改好了。");
    }
}
