//! 对话记录的数据模型：把一串事件整理成界面上的条目，跟 Claude Code 插件的呈现一致——
//! 一轮里连续的中间步骤（工具调用、思考、后台任务）收进一个可展开的「折叠行」，
//! 助手说的话、错误单独成行；子 agent 的步骤挂在派它的那个 Task 下面；被回退的历史单独成组。
//!
//! 每次有新事件都整个重建（纯计算，很快）；每个条目带一个签名，界面只重绘签名变了的条目。

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use blazar_core_types::{
    ApprovalDecision, EntryKind, McpServerStatus, NormalizedEntry, Outcome, RateLimitWindow,
    TokenUsage,
};
use serde::Deserialize;
use serde_json::Value;

/// 一条事件（历史接口和实时推送统一成这个样子）。
#[derive(Debug, Clone)]
pub struct Row {
    pub session_id: String,
    pub seq: u64,
    /// 毫秒时间戳，解析不了是 0。
    pub ts: i64,
    pub parent: Option<String>,
    pub rewound: bool,
    pub kind: EntryKind,
}

#[derive(Deserialize)]
struct HistRow {
    session_id: String,
    seq: u64,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    rewound: bool,
    kind: Value,
}

fn parse_ts(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map_or(0, |t| t.timestamp_millis())
}

impl Row {
    /// 历史接口的一行；认不出的事件类型（新版 hub 多出来的）返回 None。
    pub fn from_history(v: Value) -> Option<Self> {
        let h: HistRow = serde_json::from_value(v).ok()?;
        Some(Self {
            ts: parse_ts(&h.ts),
            kind: serde_json::from_value(h.kind).ok()?,
            session_id: h.session_id,
            seq: h.seq,
            parent: h.parent,
            rewound: h.rewound,
        })
    }

    pub fn from_live(session_id: String, e: NormalizedEntry) -> Self {
        Self {
            session_id,
            seq: e.seq,
            ts: e.ts.timestamp_millis(),
            parent: e.parent_tool_use_id.map(|t| t.0),
            rewound: false,
            kind: e.kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum St {
    Run,
    Ok,
    Err,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Ok => "ok",
            Self::Err => "err",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Todo {
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Extra {
    None,
    /// shell 命令全文
    Cmd(String),
    Todos(Vec<Todo>),
    /// 改动的行：('+' / '-' / '⋯', 内容)；`more` 是没显示的行数
    Diff {
        rows: Vec<(char, String)>,
        more: usize,
    },
    Files(Vec<String>),
    Plan(String),
}

/// 工具结果的显示：默认只露几行，其余点开。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Res {
    pub shown: String,
    pub full: String,
    pub more: usize,
    /// "" / "sum"（摘要）/ "err"
    pub cls: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub label: String,
    pub arg: String,
    pub state: St,
    pub extra: Extra,
    pub result: Option<Res>,
    /// 子 agent（Task）里的步骤。
    pub sub: Vec<Step>,
    pub sub_calls: usize,
    pub sub_last: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Thinking {
        text: String,
        secs: i64,
    },
    Tool(Box<Tool>),
    /// 子 agent 说的话
    Text(String),
    /// 后台任务开始
    Bg(String),
    /// 审批的结果，例如「已允许 · Bash」
    Note(String),
    /// 找不到对应调用的工具结果
    Orphan(Res),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    pub steps: Vec<Step>,
    pub live: bool,
    pub summary: String,
    pub state: St,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    User {
        sid: String,
        seq: u64,
        text: String,
        first: bool,
        long: bool,
    },
    Assistant {
        text: String,
        cost: Option<String>,
    },
    Fold(Fold),
    Error {
        text: String,
        by_account: bool,
    },
    Warn {
        denied: Vec<String>,
    },
    Meta {
        text: String,
        bad: bool,
    },
    Rewound {
        turns: usize,
        items: Vec<Item>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// 稳定的身份：开头那条事件的 session:seq
    pub key: String,
    /// 内容签名：变了就重绘
    pub sig: u64,
    pub body: Body,
}

/// 在等你处理的审批 / 提问。
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub id: String,
    pub sid: String,
    pub request: Value,
    pub ask: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub items: Vec<Item>,
    pub pending: Vec<Pending>,
    pub todos: Option<Vec<Todo>>,
    pub usage: Option<TokenUsage>,
    pub model: String,
    pub mcp: Vec<McpServerStatus>,
    pub rate: Vec<RateLimitWindow>,
    /// 最后一条事件所在的会话（中断、实时控制都发给它）
    pub last_session: Option<String>,
}

// ───────────────────────── 工具的标题、结果、改动 ─────────────────────────

pub const SHELL_TOOLS: [&str; 5] = ["Bash", "Shell", "shell", "exec", "command_execution"];

pub fn tool_name(n: &str) -> String {
    n.trim_start_matches("mcp__blazar__").to_owned()
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        s.to_owned()
    }
}

pub fn rel_path(p: &str, root: &str) -> String {
    let root = root.trim_end_matches('/');
    match p.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        Some(r) if !root.is_empty() => r.to_owned(),
        _ => p.to_owned(),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn command_of(input: &Value) -> String {
    match input.get("command") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        Some(Value::String(c)) => c.clone(),
        _ => String::new(),
    }
}

/// (标题, 参数)
pub fn tool_title(name: &str, input: &Value, root: &str) -> (String, String) {
    let rp = |k: &str| rel_path(s(input, k), root);
    let arg = if SHELL_TOOLS.contains(&name) {
        let d = s(input, "description");
        if d.is_empty() {
            short(command_of(input).lines().next().unwrap_or(""), 140)
        } else {
            short(d, 140)
        }
    } else {
        match name {
            "Read" => {
                let off = input
                    .get("offset")
                    .and_then(Value::as_i64)
                    .filter(|o| *o > 0);
                format!(
                    "{}{}",
                    rp("file_path"),
                    off.map(|o| format!(":{o}")).unwrap_or_default()
                )
            }
            "Write" | "Edit" | "MultiEdit" => rp("file_path"),
            "NotebookEdit" => rp("notebook_path"),
            "Glob" => {
                let p = s(input, "path");
                format!(
                    "{}{}",
                    s(input, "pattern"),
                    if p.is_empty() {
                        String::new()
                    } else {
                        format!(" in {}", rel_path(p, root))
                    }
                )
            }
            "Grep" => {
                let p = s(input, "path");
                format!(
                    "\"{}\"{}",
                    short(s(input, "pattern"), 60),
                    if p.is_empty() {
                        String::new()
                    } else {
                        format!(" in {}", rel_path(p, root))
                    }
                )
            }
            "LS" => rp("path"),
            "WebFetch" => s(input, "url").to_owned(),
            "WebSearch" => s(input, "query").to_owned(),
            "Task" | "Agent" => s(input, "description").to_owned(),
            "TodoWrite" | "ExitPlanMode" => String::new(),
            _ => short(&input.to_string(), 100),
        }
    };
    let label = match name {
        "TodoWrite" => "Update Todos",
        "Shell" => "Bash",
        "Agent" => "Task",
        "ExitPlanMode" => "计划",
        other => other,
    };
    (label.to_owned(), arg)
}

pub fn todos_of(input: &Value) -> Vec<Todo> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|t| Todo {
                    content: [s(t, "content"), s(t, "activeForm")]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .unwrap_or("")
                        .to_owned(),
                    status: [s(t, "status")]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .unwrap_or("pending")
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Write 要写的内容，按「新增的行」显示。
pub fn write_preview(input: &Value) -> Extra {
    let content = s(input, "content");
    if content.is_empty() {
        return Extra::None;
    }
    const MAX: usize = 14;
    let lines: Vec<&str> = content.trim_end_matches('\n').split('\n').collect();
    Extra::Diff {
        rows: lines
            .iter()
            .take(MAX)
            .map(|l| ('+', (*l).to_owned()))
            .collect(),
        more: lines.len().saturating_sub(MAX),
    }
}

pub fn edit_diff(input: &Value, root: &str) -> Extra {
    let edits: Vec<&Value> = match input.get("edits").and_then(Value::as_array) {
        Some(a) => a.iter().collect(),
        None if input.get("old_string").is_some() || input.get("new_string").is_some() => {
            vec![input]
        }
        None => {
            let files: Vec<String> = input
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            [s(c, "path"), s(c, "file")]
                                .into_iter()
                                .find(|x| !x.is_empty())
                                .unwrap_or("")
                        })
                        .filter(|x| !x.is_empty())
                        .map(|f| rel_path(f, root))
                        .collect()
                })
                .unwrap_or_default();
            return if files.is_empty() {
                Extra::None
            } else {
                Extra::Files(files)
            };
        }
    };
    const MAX: usize = 14;
    let mut rows = Vec::new();
    for e in &edits {
        let old = s(e, "old_string");
        let new = s(e, "new_string");
        if !old.is_empty() {
            rows.extend(old.split('\n').map(|l| ('-', l.to_owned())));
        }
        if !new.is_empty() {
            rows.extend(new.split('\n').map(|l| ('+', l.to_owned())));
        }
        if edits.len() > 1 {
            rows.push(('⋯', String::new()));
        }
    }
    let more = rows.len().saturating_sub(MAX);
    rows.truncate(MAX);
    Extra::Diff { rows, more }
}

fn res(text: &str, cls: &'static str, max: usize) -> Res {
    let lines: Vec<&str> = text.split('\n').collect();
    Res {
        shown: lines
            .iter()
            .take(max)
            .copied()
            .collect::<Vec<_>>()
            .join("\n"),
        full: text.to_owned(),
        more: lines.len().saturating_sub(max),
        cls,
    }
}

pub fn result_of(name: &str, ok: bool, content: &str, input: &Value, root: &str) -> Option<Res> {
    let text = content.trim_end();
    let n = if text.is_empty() {
        0
    } else {
        text.split('\n').count()
    };
    if !ok {
        return Some(res(if text.is_empty() { "失败" } else { text }, "err", 6));
    }
    Some(match name {
        "Read" => res(&format!("读取了 {n} 行"), "sum", 4),
        "TodoWrite" => return None,
        "ExitPlanMode" => res("计划已批准", "sum", 4),
        "Edit" | "MultiEdit" => {
            let f = rel_path(s(input, "file_path"), root);
            res(
                &format!(
                    "已修改 {}",
                    if f.is_empty() { "文件".to_owned() } else { f }
                ),
                "sum",
                4,
            )
        }
        "Write" => {
            let w = s(input, "content").split('\n').count();
            res(
                &format!("写入 {w} 行 → {}", rel_path(s(input, "file_path"), root)),
                "sum",
                4,
            )
        }
        "Glob" | "LS" => res(
            &if n > 0 {
                format!("找到 {n} 项\n{text}")
            } else {
                "没有匹配".to_owned()
            },
            "sum",
            1,
        ),
        "Grep" => res(
            &if n > 0 {
                format!("找到 {n} 处\n{text}")
            } else {
                "没有匹配".to_owned()
            },
            "sum",
            1,
        ),
        "Task" | "Agent" => res(if text.is_empty() { "完成" } else { text }, "", 3),
        _ => res(
            if text.is_empty() { "(无输出)" } else { text },
            if text.is_empty() { "sum" } else { "" },
            4,
        ),
    })
}

pub fn decision_text(d: &ApprovalDecision) -> String {
    match d {
        ApprovalDecision::AutoAllowed { rule } if !rule.is_empty() => format!("自动批准 · {rule}"),
        ApprovalDecision::AutoAllowed { .. } => "自动批准".into(),
        ApprovalDecision::Allow => "已允许".into(),
        ApprovalDecision::Deny { .. } => "已拒绝".into(),
        ApprovalDecision::Cancelled => "agent 撤回了这次询问".into(),
        ApprovalDecision::Aborted => "会话已结束，作废".into(),
    }
}

fn appr_kind_label(request: &Value) -> String {
    let tn = tool_name(s(request, "tool_name"));
    if tn == "AskUserQuestion" {
        "问题".into()
    } else if SHELL_TOOLS.contains(&tn.as_str()) {
        "Bash".into()
    } else if tn.is_empty() {
        "工具".into()
    } else {
        tn
    }
}

/// 失败原因像是账号的问题（额度、权限、认证）：给「换个账号继续」。
pub fn looks_like_account_problem(why: &str) -> bool {
    let w = why.to_lowercase();
    [
        "ratelimit",
        "rate limit",
        "rate_limit",
        "usage",
        "quota",
        "credits",
        "429",
        "401",
        "authenticat",
        "额度",
        "限流",
        "not logged in",
    ]
    .iter()
    .any(|k| w.contains(k))
}

// ───────────────────────── 构建 ─────────────────────────

/// 构建过程中的条目：工具放在 `tools` 里，用下标引用，结果、子 agent 到达时直接找到它改。
enum BStep {
    Thinking { text: String, secs: i64 },
    Tool(usize),
    Text(String),
    Bg(String),
    Note(String),
    Orphan(Res),
}

struct BTool {
    tool: Tool,
    sub: Vec<BStep>,
}

enum BBody {
    User {
        sid: String,
        seq: u64,
        text: String,
        first: bool,
        long: bool,
    },
    Assistant {
        text: String,
        cost: Option<String>,
    },
    Fold(Vec<BStep>),
    Error {
        text: String,
        by_account: bool,
    },
    Warn {
        denied: Vec<String>,
    },
    Meta {
        text: String,
        bad: bool,
    },
    Rewound {
        turns: usize,
        items: Vec<BItem>,
    },
}

struct BItem {
    key: String,
    body: BBody,
}

#[derive(Default)]
struct Builder<'a> {
    root: &'a str,
    top: Vec<BItem>,
    tools: Vec<BTool>,
    by_id: HashMap<String, usize>,
    /// 工具下标 → 它的输入（算结果摘要要用）
    inputs: HashMap<usize, Value>,
    /// 当前容器（顶层或最后一个「已回退」组）末尾的折叠行是不是还开着
    fold_open: bool,
    in_rewound: bool,
    last_ts: i64,
    last_text: String,
    usage: Option<TokenUsage>,
    user_seen: HashSet<String>,
    bg_shown: HashMap<String, String>,
    resolved: HashMap<String, String>,
    out: Transcript,
}

const FOLDED: [&str; 3] = ["thinking", "tool_use", "tool_result"];

fn kind_name(k: &EntryKind) -> &'static str {
    match k {
        EntryKind::SessionStarted { .. } => "session_started",
        EntryKind::UserMessage { .. } => "user_message",
        EntryKind::AssistantMessage { .. } => "assistant_message",
        EntryKind::Thinking { .. } => "thinking",
        EntryKind::ToolUse { .. } => "tool_use",
        EntryKind::ToolResult { .. } => "tool_result",
        EntryKind::Approval { .. } => "approval",
        EntryKind::ApprovalResolved { .. } => "approval_resolved",
        EntryKind::InputConsumed { .. } => "input_consumed",
        EntryKind::TokenUsage(_) => "token_usage",
        EntryKind::RateLimit(_) => "rate_limit",
        EntryKind::Error { .. } => "error",
        EntryKind::BackgroundTask { .. } => "background_task",
        EntryKind::Finished(_) => "finished",
    }
}

impl Builder<'_> {
    fn container(&mut self) -> &mut Vec<BItem> {
        let rw = self.in_rewound
            && matches!(
                self.top.last(),
                Some(BItem {
                    body: BBody::Rewound { .. },
                    ..
                })
            );
        if !rw {
            return &mut self.top;
        }
        match self.top.last_mut() {
            Some(BItem {
                body: BBody::Rewound { items, .. },
                ..
            }) => items,
            _ => unreachable!("上面刚确认过"),
        }
    }

    fn push(&mut self, key: String, body: BBody) {
        self.fold_open = false;
        self.container().push(BItem { key, body });
    }

    /// 当前开着的折叠行（没有就新开一个）。
    fn fold(&mut self, key: &str) -> &mut Vec<BStep> {
        let open = self.fold_open
            && matches!(
                self.container().last(),
                Some(BItem {
                    body: BBody::Fold(_),
                    ..
                })
            );
        if !open {
            self.container().push(BItem {
                key: key.to_owned(),
                body: BBody::Fold(Vec::new()),
            });
            self.fold_open = true;
        }
        match self.container().last_mut() {
            Some(BItem {
                body: BBody::Fold(steps),
                ..
            }) => steps,
            _ => unreachable!("刚放进去的就是折叠行"),
        }
    }

    fn row(&mut self, r: Row) {
        let key = format!("{}:{}", r.session_id, r.seq);
        let name = kind_name(&r.kind);

        // 被回退的历史单独成组。
        if r.rewound != self.in_rewound {
            self.fold_open = false;
            self.in_rewound = r.rewound;
            if r.rewound {
                self.top.push(BItem {
                    key: format!("rw-{key}"),
                    body: BBody::Rewound {
                        turns: 0,
                        items: Vec::new(),
                    },
                });
            }
        }
        if r.rewound
            && r.parent.is_none()
            && matches!(r.kind, EntryKind::UserMessage { .. })
            && let Some(BItem {
                body: BBody::Rewound { turns, .. },
                ..
            }) = self.top.last_mut()
        {
            *turns += 1;
        }

        // 子 agent 的步骤挂到派它的 Task 下面。
        let sub_of = r
            .parent
            .as_ref()
            .filter(|_| {
                matches!(
                    name,
                    "user_message" | "assistant_message" | "thinking" | "tool_use" | "tool_result"
                )
            })
            .and_then(|p| self.by_id.get(p).copied());
        if let Some(parent) = sub_of {
            self.sub_row(parent, r);
            return;
        }

        let quiet = matches!(
            name,
            "token_usage"
                | "rate_limit"
                | "input_consumed"
                | "approval_resolved"
                | "session_started"
                | "approval"
        ) || matches!(&r.kind, EntryKind::AssistantMessage { text } if text.trim().is_empty())
            || matches!(&r.kind, EntryKind::BackgroundTask { status, .. } if status == "started");
        if !quiet && !FOLDED.contains(&name) {
            self.fold_open = false;
        }
        let prev_ts = self.last_ts;
        if r.ts > 0 && name != "thinking" {
            self.last_ts = r.ts;
        }

        match r.kind {
            EntryKind::UserMessage { text } => {
                let first = !self.user_seen.contains(&r.session_id) && !r.rewound;
                self.user_seen.insert(r.session_id.clone());
                let long = text.split('\n').count() > 12 || text.chars().count() > 900;
                self.push(
                    key,
                    BBody::User {
                        sid: r.session_id,
                        seq: r.seq,
                        text,
                        first,
                        long,
                    },
                );
            }
            EntryKind::AssistantMessage { text } => {
                if text.trim().is_empty() {
                    return;
                }
                text.trim().clone_into(&mut self.last_text);
                self.push(key, BBody::Assistant { text, cost: None });
            }
            EntryKind::Thinking { text } => {
                let secs = if r.ts > 0 && prev_ts > 0 {
                    ((r.ts - prev_ts + 500) / 1000).max(1)
                } else {
                    0
                };
                self.fold(&key).push(BStep::Thinking { text, secs });
            }
            EntryKind::ToolUse { id, name, input } => {
                let t = self.new_tool(id.0, &name, input);
                if self.tools[t].tool.name == "TodoWrite"
                    && let Extra::Todos(list) = &self.tools[t].tool.extra
                {
                    self.out.todos = Some(list.clone());
                }
                self.fold(&key).push(BStep::Tool(t));
            }
            EntryKind::ToolResult {
                id, ok, content, ..
            } => {
                if let Some(&t) = self.by_id.get(&id.0) {
                    self.finish_tool(t, ok, &content);
                } else if let Some(res) = result_of("", ok, &content, &Value::Null, self.root) {
                    self.fold(&key).push(BStep::Orphan(res));
                }
            }
            EntryKind::Approval { id, request } => {
                let id = id.to_string();
                if let Some(done) = self.resolved.get(&id).cloned() {
                    let note = format!("{done} · {}", appr_kind_label(&request));
                    self.fold(&key).push(BStep::Note(note));
                } else {
                    let ask = tool_name(s(&request, "tool_name")) == "AskUserQuestion";
                    self.out.pending.push(Pending {
                        id,
                        sid: r.session_id,
                        request,
                        ask,
                    });
                }
            }
            EntryKind::ApprovalResolved { .. } | EntryKind::InputConsumed { .. } => {}
            EntryKind::SessionStarted {
                model, mcp_servers, ..
            } => {
                if let Some(m) = model {
                    self.out.model = m;
                }
                self.out.mcp = mcp_servers;
            }
            EntryKind::TokenUsage(u) => self.usage = Some(u),
            EntryKind::RateLimit(rl) => self.out.rate = rl.windows,
            EntryKind::Error { message } => self.push(
                key,
                BBody::Error {
                    text: message,
                    by_account: false,
                },
            ),
            EntryKind::BackgroundTask {
                task_id,
                status,
                description,
                ..
            } => {
                let what = short(
                    description
                        .as_deref()
                        .filter(|d| !d.is_empty())
                        .unwrap_or(&task_id),
                    100,
                );
                if status == "started" {
                    self.fold(&key).push(BStep::Bg(what));
                } else if self.bg_shown.get(&task_id) != Some(&status) {
                    // CLI 对同一个任务会先后发 task_updated 和 task_notification，结局只说一次。
                    self.bg_shown.insert(task_id, status.clone());
                    let bad = matches!(status.as_str(), "killed" | "stopped" | "failed");
                    let end = if status == "completed" {
                        "已完成".to_owned()
                    } else if bad {
                        format!("已停止（{status}）")
                    } else {
                        status
                    };
                    self.push(
                        key,
                        BBody::Meta {
                            text: format!("后台任务「{what}」{end}"),
                            bad,
                        },
                    );
                }
            }
            EntryKind::Finished(out) => self.finished(key, out),
        }
    }

    fn new_tool(&mut self, id: String, raw_name: &str, input: Value) -> usize {
        let name = tool_name(raw_name);
        let (label, arg) = tool_title(&name, &input, self.root);
        let extra = if SHELL_TOOLS.contains(&name.as_str()) {
            let c = command_of(&input);
            if c.is_empty() {
                Extra::None
            } else {
                Extra::Cmd(c)
            }
        } else if name == "TodoWrite" {
            Extra::Todos(todos_of(&input))
        } else if name == "Edit" || name == "MultiEdit" {
            edit_diff(&input, self.root)
        } else if name == "ExitPlanMode" && !s(&input, "plan").is_empty() {
            Extra::Plan(s(&input, "plan").to_owned())
        } else {
            Extra::None
        };
        let t = Tool {
            id: id.clone(),
            name,
            label,
            arg,
            state: St::Run,
            extra,
            result: None,
            sub: Vec::new(),
            sub_calls: 0,
            sub_last: String::new(),
        };
        self.tools.push(BTool {
            tool: t,
            sub: Vec::new(),
        });
        let i = self.tools.len() - 1;
        self.inputs.insert(i, input);
        self.by_id.insert(id, i);
        i
    }

    fn finish_tool(&mut self, t: usize, ok: bool, content: &str) {
        let input = self.inputs.get(&t).cloned().unwrap_or(Value::Null);
        let name = self.tools[t].tool.name.clone();
        let tool = &mut self.tools[t].tool;
        tool.state = if ok { St::Ok } else { St::Err };
        tool.result = result_of(&name, ok, content, &input, self.root);
    }

    fn sub_row(&mut self, parent: usize, r: Row) {
        match r.kind {
            EntryKind::ToolUse { id, name, input } => {
                let t = self.new_tool(id.0, &name, input);
                let (label, arg) = (
                    self.tools[t].tool.label.clone(),
                    self.tools[t].tool.arg.clone(),
                );
                let p = &mut self.tools[parent];
                p.sub.push(BStep::Tool(t));
                p.tool.sub_calls += 1;
                p.tool.sub_last = if arg.is_empty() {
                    label
                } else {
                    format!("{label} {}", short(&arg, 40))
                };
            }
            EntryKind::ToolResult {
                id, ok, content, ..
            } => {
                if let Some(&t) = self.by_id.get(&id.0) {
                    self.finish_tool(t, ok, &content);
                }
            }
            EntryKind::AssistantMessage { text } if !text.trim().is_empty() => {
                self.tools[parent].sub.push(BStep::Text(text))
            }
            EntryKind::Thinking { text } => self.tools[parent]
                .sub
                .push(BStep::Thinking { text, secs: 0 }),
            _ => {}
        }
    }

    fn finished(&mut self, key: String, out: Outcome) {
        let interrupted = matches!(out, Outcome::Interrupted);
        // 还挂着的工具：中断算失败，其余算完成。
        for t in &mut self.tools {
            if t.tool.state == St::Run {
                t.tool.state = if interrupted { St::Err } else { St::Ok };
            }
        }
        match out {
            Outcome::Interrupted => self.push(
                key,
                BBody::Meta {
                    text: "已中断".into(),
                    bad: false,
                },
            ),
            Outcome::Failed { message } => {
                let by_account = looks_like_account_problem(&message);
                self.push(
                    key,
                    BBody::Error {
                        text: message,
                        by_account,
                    },
                );
            }
            Outcome::Success {
                text,
                usage,
                denied,
            } => {
                if let Some(u) = usage {
                    self.usage = Some(u);
                }
                if let Some(t) = text.filter(|t| !t.trim().is_empty() && t.trim() != self.last_text)
                {
                    self.push(
                        format!("{key}:text"),
                        BBody::Assistant {
                            text: t,
                            cost: None,
                        },
                    );
                }
                if !denied.is_empty() {
                    self.push(format!("{key}:denied"), BBody::Warn { denied });
                }
                // 这一轮的花费挂在最后一条回答上。
                if let Some(cost) = self
                    .usage
                    .as_ref()
                    .and_then(|u| u.cost_usd)
                    .filter(|c| *c > 0.0)
                {
                    let cost = format!("${cost:.2}");
                    if let Some(BItem {
                        body: BBody::Assistant { cost: c @ None, .. },
                        ..
                    }) = self
                        .container()
                        .iter_mut()
                        .rev()
                        .find(|i| matches!(i.body, BBody::Assistant { .. }))
                    {
                        *c = Some(cost);
                    }
                }
            }
        }
        self.last_text.clear();
    }
}

// ───────────────────────── 收尾：变成界面用的结构 ─────────────────────────

fn take_step(tools: &mut Vec<Option<BTool>>, st: BStep) -> Step {
    match st {
        BStep::Thinking { text, secs } => Step::Thinking { text, secs },
        BStep::Text(t) => Step::Text(t),
        BStep::Bg(t) => Step::Bg(t),
        BStep::Note(t) => Step::Note(t),
        BStep::Orphan(r) => Step::Orphan(r),
        BStep::Tool(i) => {
            let BTool { mut tool, sub } = tools[i].take().expect("每个工具只出现一次");
            tool.sub = sub.into_iter().map(|x| take_step(tools, x)).collect();
            Step::Tool(Box::new(tool))
        }
    }
}

fn fold_summary(steps: &[Step], live: bool, waiting: Option<bool>) -> (String, St) {
    let tools: Vec<&Tool> = steps
        .iter()
        .filter_map(|s| {
            if let Step::Tool(t) = s {
                Some(&**t)
            } else {
                None
            }
        })
        .collect();
    let errs = tools.iter().filter(|t| t.state == St::Err).count();
    let running = tools.iter().rev().find(|t| t.state == St::Run);
    let thinks: Vec<i64> = steps
        .iter()
        .filter_map(|s| {
            if let Step::Thinking { secs, .. } = s {
                Some(*secs)
            } else {
                None
            }
        })
        .collect();
    let text = match (live, waiting) {
        (true, Some(true)) => "等待你的回答…".to_owned(),
        (true, Some(false)) => "等待你的批准…".to_owned(),
        _ if live && running.is_some() => {
            format!("正在运行 {}…", running.map_or("", |t| t.label.as_str()))
        }
        _ if live && tools.is_empty() => "思考中…".to_owned(),
        _ if !tools.is_empty() => format!(
            "{} 个工具调用{}",
            tools.len(),
            if errs > 0 {
                format!(" · {errs} 个失败")
            } else {
                String::new()
            }
        ),
        _ if !thinks.is_empty() => {
            let secs: i64 = thinks.iter().sum();
            if secs > 0 {
                format!("思考了 {secs}s")
            } else {
                "思考".to_owned()
            }
        }
        _ => format!("{} 个步骤", steps.len()),
    };
    let st = if live {
        St::Run
    } else if errs > 0 {
        St::Err
    } else {
        St::Ok
    };
    (text, st)
}

fn sig_step(st: &Step, h: &mut impl Hasher) {
    match st {
        Step::Thinking { text, secs } => (0u8, text.len(), secs).hash(h),
        Step::Text(t) | Step::Bg(t) | Step::Note(t) => (1u8, t).hash(h),
        Step::Orphan(r) => (2u8, r).hash(h),
        Step::Tool(t) => {
            (3u8, &t.id, t.state, &t.result, t.sub_calls, &t.arg).hash(h);
            for x in &t.sub {
                sig_step(x, h);
            }
        }
    }
}

fn sig_body(b: &Body) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match b {
        Body::User { text, first, .. } => (0u8, text, first).hash(&mut h),
        Body::Assistant { text, cost } => (1u8, text, cost).hash(&mut h),
        Body::Fold(f) => {
            (2u8, f.live, &f.summary, f.state, f.steps.len()).hash(&mut h);
            for s in &f.steps {
                sig_step(s, &mut h);
            }
        }
        Body::Error { text, by_account } => (3u8, text, by_account).hash(&mut h),
        Body::Warn { denied } => (4u8, denied).hash(&mut h),
        Body::Meta { text, bad } => (5u8, text, bad).hash(&mut h),
        Body::Rewound { turns, items } => {
            (6u8, turns, items.len()).hash(&mut h);
            for i in items {
                i.sig.hash(&mut h);
            }
        }
    }
    h.finish()
}

/// `live_fold` 是最后一个顶层折叠行在不在跑（工作区在跑、它是最后开着的那个）。
fn finish_items(
    items: Vec<BItem>,
    tools: &mut Vec<Option<BTool>>,
    live_last: bool,
    waiting: Option<bool>,
) -> Vec<Item> {
    let n = items.len();
    items
        .into_iter()
        .enumerate()
        .map(|(i, it)| {
            let body = match it.body {
                BBody::User {
                    sid,
                    seq,
                    text,
                    first,
                    long,
                } => Body::User {
                    sid,
                    seq,
                    text,
                    first,
                    long,
                },
                BBody::Assistant { text, cost } => Body::Assistant { text, cost },
                BBody::Error { text, by_account } => Body::Error { text, by_account },
                BBody::Warn { denied } => Body::Warn { denied },
                BBody::Meta { text, bad } => Body::Meta { text, bad },
                BBody::Rewound { turns, items } => Body::Rewound {
                    turns,
                    items: finish_items(items, tools, false, None),
                },
                BBody::Fold(steps) => {
                    let steps: Vec<Step> = steps.into_iter().map(|s| take_step(tools, s)).collect();
                    let live = live_last && i + 1 == n;
                    let (summary, state) = fold_summary(&steps, live, waiting);
                    Body::Fold(Fold {
                        steps,
                        live,
                        summary,
                        state,
                    })
                }
            };
            Item {
                key: it.key,
                sig: sig_body(&body),
                body,
            }
        })
        .collect()
}

/// 把一串事件整理成对话记录。`root` 是工作区路径（工具参数里的绝对路径显示成相对路径）；
/// `running` 是工作区现在是不是在跑（决定最后一个折叠行写「正在运行…」还是「N 个工具调用」）。
pub fn build(rows: &[Row], root: &str, running: bool) -> Transcript {
    let mut b = Builder {
        root,
        ..Builder::default()
    };
    for r in rows {
        if let EntryKind::ApprovalResolved { id, decision } = &r.kind {
            b.resolved.insert(id.to_string(), decision_text(decision));
        }
    }
    for r in rows {
        b.row(r.clone());
    }
    b.out.last_session = rows.last().map(|r| r.session_id.clone());
    b.out.usage = b.usage.take();
    // 顶层最后一个是还开着的折叠行，而且工作区在跑：它就是「正在进行」的那一组。
    let live_last = running && b.fold_open && !b.in_rewound;
    let waiting = b.out.pending.first().map(|p| p.ask);
    let mut tools: Vec<Option<BTool>> =
        std::mem::take(&mut b.tools).into_iter().map(Some).collect();
    let top = std::mem::take(&mut b.top);
    b.out.items = finish_items(top, &mut tools, live_last, waiting);
    b.out
}

/// 审批卡片的标题。
pub fn approval_title(request: &Value, root: &str, remote_node: Option<&str>) -> String {
    let tn = tool_name(s(request, "tool_name"));
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    if SHELL_TOOLS.contains(&tn.as_str()) {
        return match remote_node {
            Some(n) if s(request, "tool_name").starts_with("mcp__blazar__") => {
                format!("在 {n} 上执行这个 Bash 命令？")
            }
            _ => "允许执行这个 Bash 命令？".to_owned(),
        };
    }
    if matches!(tn.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
        let f = [s(&input, "file_path"), s(&input, "notebook_path")]
            .into_iter()
            .find(|x| !x.is_empty())
            .map(|f| rel_path(f, root));
        return format!("允许修改 {}？", f.unwrap_or_else(|| "这个文件".to_owned()));
    }
    if tn == "WebFetch" {
        return "允许抓取这个网页？".to_owned();
    }
    let shown = [s(request, "display_name"), tn.as_str()]
        .into_iter()
        .find(|x| !x.is_empty())
        .unwrap_or("这个工具")
        .to_owned();
    format!("允许使用 {shown}？")
}

/// 审批卡片正文里展示的「要做什么」。
pub fn approval_what(request: &Value, root: &str) -> String {
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let cmd = command_of(&input);
    if !cmd.is_empty() {
        return cmd;
    }
    if !s(&input, "url").is_empty() {
        return s(&input, "url").to_owned();
    }
    if !s(&input, "file_path").is_empty() {
        return rel_path(s(&input, "file_path"), root);
    }
    let pretty = serde_json::to_string_pretty(&input).unwrap_or_default();
    pretty.chars().take(800).collect()
}

/// 「以后不再询问」用的规则：工具名，加上 shell 命令的头一两个词。
pub fn always_rule(request: &Value) -> (String, String) {
    let tn = tool_name(s(request, "tool_name"));
    let cmd = request
        .get("input")
        .and_then(|i| i.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let w: Vec<&str> = cmd.split_whitespace().collect();
    let pattern = match w.as_slice() {
        [a, b, ..] if !b.starts_with('-') => format!("{a} {b}"),
        [a, ..] => (*a).to_owned(),
        [] => String::new(),
    };
    (tn, pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(sid: &str, seq: u64, ts: i64, kind: Value) -> Row {
        Row {
            session_id: sid.into(),
            seq,
            ts,
            parent: None,
            rewound: false,
            kind: serde_json::from_value(kind).unwrap(),
        }
    }

    #[test]
    fn folds_intermediate_steps_and_pairs_results() {
        let rows = vec![
            row("s", 1, 1000, json!({"type":"user_message","text":"修一下"})),
            row("s", 2, 2000, json!({"type":"thinking","text":"想想"})),
            row(
                "s",
                3,
                3000,
                json!({"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la","description":"列目录"}}),
            ),
            row(
                "s",
                4,
                3500,
                json!({"type":"tool_result","id":"t1","ok":true,"content":"a\nb\nc\nd\ne\nf"}),
            ),
            row(
                "s",
                5,
                4000,
                json!({"type":"assistant_message","text":"好了"}),
            ),
            row(
                "s",
                6,
                4100,
                json!({"type":"token_usage","input":10,"output":5,"cost_usd":0.12}),
            ),
            row(
                "s",
                7,
                4200,
                json!({"type":"finished","status":"success","text":"好了"}),
            ),
        ];
        let t = build(&rows, "/w", false);
        assert_eq!(t.items.len(), 3, "{:#?}", t.items);
        assert!(matches!(&t.items[0].body, Body::User { first: true, .. }));
        let Body::Fold(f) = &t.items[1].body else {
            panic!()
        };
        assert_eq!(f.summary, "1 个工具调用");
        assert!(matches!(&f.steps[0], Step::Thinking { secs: 1, .. }));
        let Step::Tool(tool) = &f.steps[1] else {
            panic!()
        };
        assert_eq!(
            (tool.label.as_str(), tool.arg.as_str(), tool.state),
            ("Bash", "列目录", St::Ok)
        );
        assert_eq!(tool.result.as_ref().map(|r| r.more), Some(2));
        assert!(
            matches!(&t.items[2].body, Body::Assistant { cost: Some(c), .. } if c == "$0.12"),
            "最终文本和上一条回答一样，不重复；花费挂在回答上"
        );
    }

    #[test]
    fn live_fold_and_pending_approval() {
        let rows = vec![
            row("s", 1, 0, json!({"type":"user_message","text":"x"})),
            row(
                "s",
                2,
                0,
                json!({"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"}}),
            ),
            row(
                "s",
                3,
                0,
                json!({"type":"approval","id":"01a0fa80-9ab8-7320-bd31-48d9174e2bcd","request":{"tool_name":"Edit","input":{"file_path":"/w/src/a.rs"}}}),
            ),
        ];
        let t = build(&rows, "/w", true);
        let Body::Fold(f) = &t.items[1].body else {
            panic!()
        };
        assert!(f.live);
        assert_eq!(f.summary, "等待你的批准…");
        assert_eq!(t.pending.len(), 1);
        assert_eq!(
            approval_title(&t.pending[0].request, "/w", None),
            "允许修改 src/a.rs？"
        );
        let Step::Tool(tool) = &f.steps[0] else {
            panic!()
        };
        assert_eq!(
            tool.extra,
            Extra::Diff {
                rows: vec![('-', "a".into()), ('+', "b".into())],
                more: 0
            }
        );

        // 批准之后：不再挂着，折叠行里留一条记录。
        let mut rows2 = rows.clone();
        rows2.push(row("s", 4, 0, json!({"type":"approval_resolved","id":"01a0fa80-9ab8-7320-bd31-48d9174e2bcd","decision":{"kind":"allow"}})));
        let t2 = build(&rows2, "/w", true);
        assert!(t2.pending.is_empty());
        let Body::Fold(f2) = &t2.items[1].body else {
            panic!()
        };
        assert!(matches!(&f2.steps[1], Step::Note(n) if n == "已允许 · Edit"));
        assert_ne!(t.items[1].sig, t2.items[1].sig, "内容变了签名要变");
        assert_eq!(t.items[0].sig, t2.items[0].sig, "没变的条目签名不变");
    }

    #[test]
    fn subagent_steps_nest_under_task_and_rewound_turns_group() {
        let mut sub = row(
            "s",
            3,
            0,
            json!({"type":"tool_use","id":"c1","name":"Read","input":{"file_path":"/w/x.rs"}}),
        );
        sub.parent = Some("task1".into());
        let mut rw = row("s", 1, 0, json!({"type":"user_message","text":"旧的"}));
        rw.rewound = true;
        let rows = vec![
            rw,
            row(
                "s",
                2,
                0,
                json!({"type":"tool_use","id":"task1","name":"Task","input":{"description":"查一下"}}),
            ),
            sub,
        ];
        let t = build(&rows, "/w", false);
        assert!(matches!(&t.items[0].body, Body::Rewound { turns: 1, items } if items.len() == 1));
        let Body::Fold(f) = &t.items[1].body else {
            panic!("{:#?}", t.items)
        };
        let Step::Tool(task) = &f.steps[0] else {
            panic!()
        };
        assert_eq!((task.sub_calls, task.sub_last.as_str()), (1, "Read x.rs"));
        assert_eq!(task.sub.len(), 1);
    }

    #[test]
    fn failure_offers_account_switch() {
        let rows = vec![row(
            "s",
            1,
            0,
            json!({"type":"finished","status":"failed","message":"API Error: 429 rate_limit_error"}),
        )];
        let t = build(&rows, "", false);
        assert!(matches!(
            &t.items[0].body,
            Body::Error {
                by_account: true,
                ..
            }
        ));
        assert_eq!(
            always_rule(&json!({"tool_name":"Bash","input":{"command":"cargo test -p x"}})),
            ("Bash".into(), "cargo test".into())
        );
    }
}
