use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use blazar_core_types::{EntryKind, McpServerStatus, Outcome, RateLimitWindow, TokenUsage};
use serde_json::Value;

use super::approvals::{appr_kind_label, decision_text};
use super::finish::finish_items;
use super::tools::{
    SHELL_TOOLS, command_of, edit_diff, fleet_target, result_of, s, shell_display, shell_result,
    short, todos_of, tool_name, tool_title, write_preview,
};
use super::{Extra, Item, Pending, Res, Row, St, Todo, Tool, Transcript};

fn est_tokens(s: &str) -> u64 {
    let (wide, narrow) = s.chars().fold((0u64, 0u64), |(w, n), c| {
        if (c as u32) >= 0x2E80 {
            (w + 1, n)
        } else {
            (w, n + 1)
        }
    });
    wide + narrow.div_ceil(4)
}

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

pub fn human_error(message: &str) -> String {
    let head = crate::vocab::error(message, None);
    if head == message {
        head
    } else {
        format!("{head}\n{}", message.trim())
    }
}

pub(super) enum BStep {
    Thinking { text: String, secs: i64 },
    Tool(usize),
    Text(String),
    Bg(String),
    Note(String),
    Orphan(Res),
}

pub(super) struct BTool {
    pub(super) tool: Tool,
    pub(super) sub: Vec<BStep>,
    start: i64,
}

pub(super) enum BBody {
    User {
        sid: String,
        seq: u64,
        text: String,
        first: bool,
        long: bool,
    },
    Assistant {
        text: String,
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

pub(super) struct BItem {
    pub(super) key: String,
    pub(super) body: BBody,
}

#[derive(Clone, Default)]
struct Carry {
    last_ts: i64,
    last_text: String,
    usage: Option<TokenUsage>,
    turn_tokens: HashMap<String, u64>,
    turn_est: u64,
    todos: Option<Vec<Todo>>,
    model: String,
    mcp: Vec<McpServerStatus>,
    rate: Vec<RateLimitWindow>,
}

#[derive(Default)]
struct Frozen {
    fed: usize,
    last: Option<(String, u64)>,
    items: Vec<Rc<Item>>,
    index: Rc<HashMap<String, usize>>,
    tools: HashSet<String>,
    approvals: HashSet<String>,
    resolved: HashMap<String, String>,
    seen: HashSet<String>,
    bg: HashMap<String, String>,
    carry: Carry,
}

struct Mark {
    row: usize,
    items: usize,
    tools: usize,
    seen: usize,
    bg: usize,
    carry: Option<Carry>,
}

struct Promotion {
    row: usize,
    items: usize,
    tools: Vec<String>,
    seen: Vec<String>,
    bg: Vec<(String, String)>,
    carry: Option<Carry>,
}

const LOOKBACK: usize = 256;

struct Builder<'a> {
    root: &'a str,
    base: &'a Frozen,
    len: usize,
    top: Vec<BItem>,
    tools: Vec<BTool>,
    by_id: HashMap<String, usize>,
    running: usize,
    fold_open: bool,
    in_rewound: bool,
    carry: Carry,
    seen: HashSet<String>,
    seen_order: Vec<String>,
    bg: HashMap<String, String>,
    bg_order: Vec<(String, String)>,
    resolved: HashMap<String, String>,
    pending: Vec<Pending>,
    bail: bool,
    mark: Option<Mark>,
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
        EntryKind::Notice { .. } => "notice",
        EntryKind::BackgroundTask { .. } => "background_task",
        EntryKind::Finished(_) => "finished",
        EntryKind::Progress { .. } => "progress",
    }
}

impl<'a> Builder<'a> {
    fn new(root: &'a str, base: &'a Frozen) -> Self {
        Self {
            root,
            base,
            len: 0,
            top: Vec::new(),
            tools: Vec::new(),
            by_id: HashMap::new(),
            running: 0,
            fold_open: false,
            in_rewound: false,
            carry: base.carry.clone(),
            seen: HashSet::new(),
            seen_order: Vec::new(),
            bg: HashMap::new(),
            bg_order: Vec::new(),
            resolved: HashMap::new(),
            pending: Vec::new(),
            bail: false,
            mark: None,
        }
    }

    fn prepare(&mut self, rows: &[Row]) {
        self.len = rows.len();
        for r in rows {
            if let EntryKind::ApprovalResolved { id, decision } = &r.kind {
                let id = id.to_string();
                if self.base.approvals.contains(&id) {
                    self.bail = true;
                }
                self.resolved.insert(id, decision_text(decision));
            }
        }
    }

    fn boundary(&mut self, row: usize) {
        if row > 0
            && !self.fold_open
            && !self.in_rewound
            && self.running == 0
            && self.pending.is_empty()
        {
            self.mark = Some(Mark {
                row,
                items: self.top.len(),
                tools: self.tools.len(),
                seen: self.seen_order.len(),
                bg: self.bg_order.len(),
                carry: (row + LOOKBACK >= self.len).then(|| self.carry.clone()),
            });
        }
    }

    fn tool_index(&mut self, id: &str) -> Option<usize> {
        let found = self.by_id.get(id).copied();
        if found.is_none() && self.base.tools.contains(id) {
            self.bail = true;
        }
        found
    }

    fn resolution(&self, id: &str) -> Option<&String> {
        self.resolved.get(id).or_else(|| self.base.resolved.get(id))
    }

    fn seen_session(&self, sid: &str) -> bool {
        self.seen.contains(sid) || self.base.seen.contains(sid)
    }

    fn bg_status(&self, id: &str) -> Option<&String> {
        self.bg.get(id).or_else(|| self.base.bg.get(id))
    }

    fn set_state(&mut self, t: usize, st: St) {
        let tool = &mut self.tools[t].tool;
        if tool.state == St::Run && st != St::Run {
            self.running -= 1;
        }
        tool.state = st;
    }

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

    fn row(&mut self, r: &Row) {
        let key = format!("{}:{}", r.session_id, r.seq);
        let name = kind_name(&r.kind);

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

        let nested = matches!(
            name,
            "user_message" | "assistant_message" | "thinking" | "tool_use" | "tool_result"
        );
        let sub_of = match &r.parent {
            Some(p) if nested => self.tool_index(p),
            _ => None,
        };
        if self.bail {
            return;
        }
        if let Some(parent) = sub_of {
            self.sub_row(parent, r);
            return;
        }

        let quiet = matches!(
            name,
            "token_usage"
                | "progress"
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
        let prev_ts = self.carry.last_ts;
        if r.ts > 0 && name != "thinking" {
            self.carry.last_ts = r.ts;
        }

        match &r.kind {
            EntryKind::UserMessage { text } => {
                let text = shell_display(text);
                self.carry.turn_tokens.clear();
                self.carry.turn_est = 0;
                let first = !self.seen_session(&r.session_id) && !r.rewound;
                if self.seen.insert(r.session_id.clone()) {
                    self.seen_order.push(r.session_id.clone());
                }
                let long = text.split('\n').count() > 12 || text.chars().count() > 900;
                self.push(
                    key,
                    BBody::User {
                        sid: r.session_id.clone(),
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
                self.carry.turn_est += est_tokens(text);
                text.trim().clone_into(&mut self.carry.last_text);
                self.push(key, BBody::Assistant { text: text.clone() });
            }
            EntryKind::Thinking { text } => {
                self.carry.turn_est += est_tokens(text);
                let secs = if r.ts > 0 && prev_ts > 0 {
                    ((r.ts - prev_ts + 500) / 1000).max(1)
                } else {
                    0
                };
                self.fold(&key).push(BStep::Thinking {
                    text: text.clone(),
                    secs,
                });
            }
            EntryKind::ToolUse { id, name, input } => {
                self.carry.turn_est += est_tokens(&input.to_string()) + 8;
                let t = self.new_tool(id.0.clone(), name, input.clone(), r.ts);
                if self.tools[t].tool.name == "TodoWrite"
                    && let Extra::Todos(list) = &self.tools[t].tool.extra
                {
                    self.carry.todos = Some(list.clone());
                }
                self.fold(&key).push(BStep::Tool(t));
            }
            EntryKind::ToolResult {
                id, ok, content, ..
            } => {
                if let Some(t) = self.tool_index(&id.0) {
                    self.finish_tool(t, *ok, content, r.ts);
                } else if !self.bail
                    && let Some(res) = result_of("", *ok, content)
                {
                    self.fold(&key).push(BStep::Orphan(res));
                }
            }
            EntryKind::Approval { id, request } => {
                let id = id.to_string();
                if let Some(done) = self.resolution(&id).cloned() {
                    let note = format!("{done} · {}", appr_kind_label(request));
                    self.fold(&key).push(BStep::Note(note));
                } else {
                    let ask = tool_name(s(request, "tool_name")) == "AskUserQuestion";
                    self.pending.push(Pending {
                        id,
                        sid: r.session_id.clone(),
                        request: request.clone(),
                        ask,
                    });
                }
            }
            EntryKind::ApprovalResolved { .. } | EntryKind::InputConsumed { .. } => {}
            EntryKind::SessionStarted {
                model, mcp_servers, ..
            } => {
                if let Some(m) = model {
                    self.carry.model = m.clone();
                }
                self.carry.mcp = mcp_servers.clone();
            }
            EntryKind::TokenUsage(u) => self.carry.usage = Some(u.clone()),
            EntryKind::Progress {
                message_id,
                output_tokens,
            } => {
                let n = self
                    .carry
                    .turn_tokens
                    .entry(message_id.clone())
                    .or_default();
                *n = (*n).max(*output_tokens);
            }
            EntryKind::RateLimit(rl) => self.carry.rate = rl.windows.clone(),
            EntryKind::Notice { message } => self.push(
                key,
                BBody::Meta {
                    text: message.clone(),
                    bad: false,
                },
            ),
            EntryKind::Error { message } => self.push(
                key,
                BBody::Error {
                    text: message.clone(),
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
                        .unwrap_or(task_id),
                    100,
                );
                if status == "started" {
                    self.fold(&key).push(BStep::Bg(what));
                } else if self.bg_status(task_id) != Some(status) {
                    self.bg.insert(task_id.clone(), status.clone());
                    self.bg_order.push((task_id.clone(), status.clone()));
                    let bad = matches!(status.as_str(), "killed" | "stopped" | "failed");
                    let end = match status.as_str() {
                        "completed" => "已完成",
                        "killed" => "已被结束",
                        "failed" => "失败",
                        "stopped" => "已停止",
                        _ => "状态有变化",
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

    fn new_tool(&mut self, id: String, raw_name: &str, input: Value, ts: i64) -> usize {
        let name = tool_name(raw_name);
        let (label, arg) = tool_title(&name, &input, self.root);
        let extra = if SHELL_TOOLS.contains(&name.as_str()) {
            let c = command_of(&input);
            if c.trim().contains('\n') {
                Extra::Cmd(c)
            } else {
                Extra::None
            }
        } else if name == "TodoWrite" {
            Extra::Todos(todos_of(&input))
        } else if name == "Edit" || name == "MultiEdit" {
            edit_diff(&input, self.root)
        } else if name == "Write" {
            write_preview(&input)
        } else if name == "ExitPlanMode" && !s(&input, "plan").is_empty() {
            Extra::Plan(s(&input, "plan").to_owned())
        } else if let Some(ws) = fleet_target(&name, &input) {
            Extra::Fleet(ws)
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
            ms: None,
            sub: Vec::new(),
            sub_calls: 0,
            sub_last: String::new(),
            sub_elided: false,
        };
        self.tools.push(BTool {
            tool: t,
            sub: Vec::new(),
            start: ts,
        });
        self.running += 1;
        let i = self.tools.len() - 1;
        self.by_id.insert(id, i);
        i
    }

    fn finish_tool(&mut self, t: usize, ok: bool, content: &str, ts: i64) {
        let b = &self.tools[t];
        let ms = (b.start > 0 && ts >= b.start).then(|| (ts - b.start) as u64);
        let res = if SHELL_TOOLS.contains(&b.tool.name.as_str()) {
            Some(shell_result(ok, content, ms))
        } else {
            result_of(&b.tool.name, ok, content)
        };
        self.set_state(t, if ok { St::Ok } else { St::Err });
        self.tools[t].tool.result = res;
        self.tools[t].tool.ms = ms;
    }

    fn sub_row(&mut self, parent: usize, r: &Row) {
        if r.elided {
            self.tools[parent].tool.sub_elided = true;
        }
        match &r.kind {
            EntryKind::ToolUse { id, name, input } => {
                let t = self.new_tool(id.0.clone(), name, input.clone(), r.ts);
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
            EntryKind::ToolResult { id, ok, .. } if r.elided => {
                if let Some(t) = self.tool_index(&id.0) {
                    self.set_state(t, if *ok { St::Ok } else { St::Err });
                }
            }
            EntryKind::ToolResult {
                id, ok, content, ..
            } => {
                if let Some(t) = self.tool_index(&id.0) {
                    self.finish_tool(t, *ok, content, r.ts);
                }
            }
            EntryKind::AssistantMessage { text } if !text.trim().is_empty() => {
                self.tools[parent].sub.push(BStep::Text(text.clone()))
            }
            EntryKind::Thinking { .. } if r.elided => {}
            EntryKind::Thinking { text } => self.tools[parent].sub.push(BStep::Thinking {
                text: text.clone(),
                secs: 0,
            }),
            _ => {}
        }
    }

    fn finished(&mut self, key: String, out: &Outcome) {
        let interrupted = matches!(out, Outcome::Interrupted);

        for t in &mut self.tools {
            if t.tool.state == St::Run {
                t.tool.state = if interrupted { St::Err } else { St::Ok };
            }
        }
        self.running = 0;
        match out {
            Outcome::Interrupted => self.push(
                key,
                BBody::Meta {
                    text: "已中断".into(),
                    bad: false,
                },
            ),
            Outcome::Failed { message } => {
                let by_account = looks_like_account_problem(message);
                self.push(
                    key,
                    BBody::Error {
                        text: human_error(message),
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
                    self.carry.usage = Some(u.clone());
                }
                if let Some(t) = text
                    .as_ref()
                    .filter(|t| !t.trim().is_empty() && t.trim() != self.carry.last_text)
                {
                    self.push(format!("{key}:text"), BBody::Assistant { text: t.clone() });
                }
                if !denied.is_empty() {
                    self.push(
                        format!("{key}:denied"),
                        BBody::Warn {
                            denied: denied.clone(),
                        },
                    );
                }
            }
        }
        self.carry.last_text.clear();
    }

    fn finish(self, rows: &[Row], running: bool) -> (Transcript, Option<Promotion>) {
        let Self {
            base,
            len,
            top,
            tools,
            pending,
            carry,
            fold_open,
            in_rewound,
            mark,
            seen_order,
            bg_order,
            ..
        } = self;
        let promotion = mark.map(|m| Promotion {
            row: m.row,
            items: m.items,
            tools: tools[..m.tools].iter().map(|t| t.tool.id.clone()).collect(),
            seen: seen_order[..m.seen].to_vec(),
            bg: bg_order[..m.bg].to_vec(),
            carry: if m.row == len {
                Some(carry.clone())
            } else {
                m.carry
            },
        });
        let live_last = running && fold_open && !in_rewound;
        let mut tools: Vec<Option<BTool>> = tools.into_iter().map(Some).collect();
        let fresh = finish_items(top, &mut tools, live_last);
        let turn_tokens = carry.turn_tokens.values().sum::<u64>().max(carry.turn_est);
        let offset = base.items.len();
        let index = fresh
            .iter()
            .enumerate()
            .map(|(i, it)| (it.key.clone(), offset + i))
            .collect();
        let mut items = base.items.clone();
        items.extend(fresh.into_iter().map(Rc::new));
        let out = Transcript {
            items,
            frozen: base.index.clone(),
            index,
            pending,
            todos: carry.todos,
            usage: carry.usage,
            model: carry.model,
            mcp: carry.mcp,
            rate: carry.rate,
            last_session: rows.last().map(|r| r.session_id.clone()),
            turn_tokens,
        };
        (out, promotion)
    }
}

pub struct Model {
    root: String,
    epoch: u64,
    frozen: Frozen,
}

impl Model {
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_owned(),
            epoch: 0,
            frozen: Frozen::default(),
        }
    }

    #[cfg(test)]
    pub fn frozen_rows(&self) -> usize {
        self.frozen.fed
    }

    pub fn update(&mut self, rows: &[Row], epoch: u64, running: bool) -> Transcript {
        let fed = self.frozen.fed;
        let kept = epoch == self.epoch
            && rows.len() >= fed
            && self.frozen.last.as_ref().is_none_or(|(sid, seq)| {
                rows.get(fed.wrapping_sub(1))
                    .is_some_and(|r| r.session_id == *sid && r.seq == *seq)
            });
        if !kept {
            self.frozen = Frozen::default();
            self.epoch = epoch;
        }
        loop {
            if let Some(t) = self.advance(rows, running) {
                return t;
            }
            self.frozen = Frozen::default();
        }
    }

    fn advance(&mut self, rows: &[Row], running: bool) -> Option<Transcript> {
        let tail = &rows[self.frozen.fed..];
        let mut b = Builder::new(&self.root, &self.frozen);
        b.prepare(tail);
        if b.bail {
            return None;
        }
        for (i, r) in tail.iter().enumerate() {
            b.boundary(i);
            b.row(r);
            if b.bail {
                return None;
            }
        }
        b.boundary(tail.len());
        let (out, promotion) = b.finish(rows, running);
        if let Some(p) = promotion {
            self.freeze(p, rows, &out);
        }
        Some(out)
    }

    fn replay(&self, rows: &[Row], upto: usize) -> Carry {
        let tail = &rows[self.frozen.fed..];
        let mut b = Builder::new(&self.root, &self.frozen);
        b.prepare(tail);
        for r in &tail[..upto] {
            b.row(r);
        }
        b.carry
    }

    fn freeze(&mut self, p: Promotion, rows: &[Row], out: &Transcript) {
        let carry = match p.carry {
            Some(c) => c,
            None => self.replay(rows, p.row),
        };
        let f = &mut self.frozen;
        let start = f.fed;
        let end = start + p.row;
        let had = f.items.len();
        let n = had + p.items;
        let mut index = (*f.index).clone();
        for (i, it) in out.items[had..n].iter().enumerate() {
            index.insert(it.key.clone(), had + i);
        }
        f.items = out.items[..n].to_vec();
        f.index = Rc::new(index);
        f.tools.extend(p.tools);
        for r in &rows[start..end] {
            match &r.kind {
                EntryKind::Approval { id, .. } => {
                    f.approvals.insert(id.to_string());
                }
                EntryKind::ApprovalResolved { id, decision } => {
                    f.resolved.insert(id.to_string(), decision_text(decision));
                }
                _ => {}
            }
        }
        f.seen.extend(p.seen);
        f.bg.extend(p.bg);
        f.carry = carry;
        f.fed = end;
        f.last = rows
            .get(end.wrapping_sub(1))
            .map(|r| (r.session_id.clone(), r.seq));
    }
}

#[cfg(test)]
pub fn build(rows: &[Row], root: &str, running: bool) -> Transcript {
    Model::new(root).update(rows, 0, running)
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
