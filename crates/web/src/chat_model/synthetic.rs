use serde_json::{Value, json};

use super::Row;

pub struct Prng(u64);

impl Prng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }

    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }

    pub fn words(&mut self, n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            if i > 0 {
                s.push(if i % 11 == 10 { '\n' } else { ' ' });
            }
            let w: &&str = self.pick(&WORDS[..]);
            s.push_str(w);
        }
        s
    }

    pub fn words_between(&mut self, lo: usize, hi: usize) -> String {
        let n = self.range(lo, hi);
        self.words(n)
    }

    pub fn lines(&mut self, lo: usize, hi: usize) -> String {
        let n = self.range(lo, hi);
        (0..n)
            .map(|i| {
                let w = self.words_between(2, 9);
                format!("{:>6}\t{w}", i + 1)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

const WORDS: [&str; 28] = [
    "the",
    "build",
    "rows",
    "model",
    "clone",
    "signal",
    "effect",
    "wasm",
    "tool",
    "result",
    "cargo",
    "clippy",
    "warning",
    "workspace",
    "session",
    "history",
    "event",
    "stream",
    "fold",
    "summary",
    "token",
    "index",
    "memo",
    "render",
    "scroll",
    "snapshot",
    "timing",
    "dev",
];

pub fn row(
    sid: &str,
    seq: u64,
    ts: i64,
    parent: Option<&str>,
    rewound: bool,
    elided: bool,
    kind: Value,
) -> Row {
    Row {
        session_id: sid.to_owned(),
        seq,
        ts,
        parent: parent.map(str::to_owned),
        rewound,
        elided,
        kind: serde_json::from_value(kind).expect("synthetic row must deserialize"),
    }
}

pub fn appr_id(n: usize) -> String {
    format!("01a0fa80-9ab8-7320-bd31-{n:012x}")
}

struct Turn {
    sid: String,
    seq: u64,
    rewound: bool,
}

pub struct Realistic {
    rng: Prng,
    pub rows: Vec<Row>,
    ts: i64,
    tools: usize,
    apprs: usize,
    msgs: usize,
}

impl Realistic {
    fn push(&mut self, t: &mut Turn, parent: Option<&str>, elided: bool, kind: Value) {
        t.seq += 1;
        self.ts += i64::try_from(self.rng.range(50, 4000)).unwrap_or(0);
        self.rows
            .push(row(&t.sid, t.seq, self.ts, parent, t.rewound, elided, kind));
    }

    fn tool_call(&mut self) -> (&'static str, Value, String) {
        let roll = self.rng.below(100);
        let file = format!("/w/src/{}.rs", self.rng.pick(&WORDS[..]));
        let (name, input) = if roll < 30 {
            let cmd = format!(
                "cargo {} -p blazar-{}",
                self.rng.pick(&WORDS[..]),
                self.rng.pick(&WORDS[..])
            );
            let desc = self.rng.words(4);
            ("Bash", json!({"command": cmd, "description": desc}))
        } else if roll < 55 {
            ("Read", json!({"file_path": file}))
        } else if roll < 70 {
            let old = self.rng.words(12);
            let new = self.rng.words(14);
            (
                "Edit",
                json!({"file_path": file, "old_string": old, "new_string": new}),
            )
        } else if roll < 80 {
            let pat = self.rng.words(1);
            ("Grep", json!({"pattern": pat, "path": "/w/crates"}))
        } else if roll < 85 {
            ("Glob", json!({"pattern": "**/*.rs"}))
        } else if roll < 90 {
            let content = self.rng.lines(5, 40);
            ("Write", json!({"file_path": file, "content": content}))
        } else if roll < 94 {
            let a = self.rng.words(5);
            let b = self.rng.words(4);
            (
                "TodoWrite",
                json!({"todos": [{"content": a, "status": "in_progress"}, {"content": b, "status": "pending"}]}),
            )
        } else if roll < 97 {
            let d = self.rng.words(3);
            let p = self.rng.words(60);
            ("Task", json!({"description": d, "prompt": p}))
        } else if roll < 99 {
            (
                "WebFetch",
                json!({"url": "https://example.com/docs", "prompt": "summarize"}),
            )
        } else {
            let text = self.rng.words(30);
            (
                "mcp__blazar-fleet__send_prompt",
                json!({"workspace_id": "w1", "text": text}),
            )
        };
        (name, input, file)
    }

    fn tool(&mut self, t: &mut Turn, parent: Option<&str>, thin: bool) -> String {
        self.tools += 1;
        let id = format!("toolu_{:06}", self.tools);
        let (name, input, file) = self.tool_call();
        let kind = json!({"type":"tool_use","id":id,"name":name,"input":input});
        self.push(t, parent, false, kind);
        if name == "Task" {
            let sub_thin = self.rng.chance(0.8);
            let n = self.rng.range(3, 15);
            for _ in 0..n {
                if self.rng.chance(0.3) {
                    let text = if sub_thin {
                        String::new()
                    } else {
                        self.rng.words(40)
                    };
                    self.push(
                        t,
                        Some(&id),
                        sub_thin,
                        json!({"type":"thinking","text": text}),
                    );
                }
                if self.rng.chance(0.2) {
                    let text = self.rng.words(30);
                    self.push(
                        t,
                        Some(&id),
                        false,
                        json!({"type":"assistant_message","text": text}),
                    );
                }
                self.tool(t, Some(&id), sub_thin);
            }
        }
        if name != "Task" && parent.is_none() && self.rng.chance(0.03) {
            self.apprs += 1;
            let aid = appr_id(self.apprs);
            let kind = json!({"type":"approval","id":aid,"request":{"tool_name":name,"input":input,"tool_use_id":id}});
            self.push(t, None, false, kind);
            let decision = if self.rng.chance(0.85) {
                "allow"
            } else {
                "deny"
            };
            let kind = json!({"type":"approval_resolved","id":aid,"decision":{"kind": decision}});
            self.push(t, None, false, kind);
        }
        let ok = !self.rng.chance(0.08);
        let content = if thin {
            String::new()
        } else if !ok {
            format!("Error: {}", self.rng.words(12))
        } else {
            match name {
                "Read" => self.rng.lines(30, 300),
                "Bash" => self.rng.lines(1, 60),
                "Grep" => self.rng.lines(1, 40),
                "Glob" => self.rng.lines(1, 20),
                "Edit" => format!("The file {file} has been updated."),
                "Write" => format!("File created successfully at: {file}"),
                "TodoWrite" => "Todos have been modified successfully".to_owned(),
                "Task" => self.rng.words(120),
                _ => self.rng.words(40),
            }
        };
        let kind = json!({"type":"tool_result","id":id,"ok":ok,"content":content});
        self.push(t, parent, thin, kind);
        id
    }

    fn turn(&mut self, no: usize, rewound: bool, live: bool) {
        let mut t = Turn {
            sid: format!("sess-{no:05}"),
            seq: 0,
            rewound,
        };
        let kind = json!({"type":"session_started","provider_session_id":format!("p{no}"),"model":"claude-opus-5-5","cwd":"/w","permission_mode":"default","mcp_servers":[{"name":"blazar","status":"connected"}]});
        self.push(&mut t, None, false, kind);
        let long = self.rng.chance(0.05);
        let text = if long {
            self.rng.words(200)
        } else {
            self.rng.words_between(8, 60)
        };
        self.push(
            &mut t,
            None,
            false,
            json!({"type":"user_message","text": text}),
        );
        let mut n = 1;
        while self.rng.chance(0.88) && n < 60 {
            n += 1;
        }
        let steer = self.rng.chance(0.05).then(|| self.rng.below(n));
        let bg = self.rng.chance(0.02);
        let mut last_text = String::new();
        for i in 0..n {
            if live && i + 1 == n {
                break;
            }
            if steer == Some(i) {
                let text = self.rng.words(10);
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"user_message","text": text}),
                );
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"input_consumed","text": "steer"}),
                );
            }
            if self.rng.chance(0.4) {
                let text = self.rng.words_between(15, 120);
                self.push(&mut t, None, false, json!({"type":"thinking","text": text}));
            }
            if self.rng.chance(0.6) {
                self.msgs += 1;
                let mid = format!("msg_{:06}", self.msgs);
                let n = self.rng.range(20, 900);
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"progress","message_id":mid,"output_tokens": n}),
                );
            }
            if self.rng.chance(0.15) {
                last_text = self.rng.words_between(15, 90);
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"assistant_message","text": last_text}),
                );
            }
            if bg && i == 0 {
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"background_task","task_id":format!("bg{no}"),"status":"started","description":"run the suite"}),
                );
            }
            self.tool(&mut t, None, false);
            if bg && i + 1 == n {
                self.push(
                    &mut t,
                    None,
                    false,
                    json!({"type":"background_task","task_id":format!("bg{no}"),"status":"completed","description":"run the suite"}),
                );
            }
        }
        if live {
            let text = self.rng.words(30);
            self.push(&mut t, None, false, json!({"type":"thinking","text": text}));
            self.tools += 1;
            let id = format!("toolu_{:06}", self.tools);
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"tool_use","id":id,"name":"Bash","input":{"command":"cargo test --workspace","description":"run tests"}}),
            );
            if self.rng.chance(0.3) {
                self.apprs += 1;
                let kind = json!({"type":"approval","id":appr_id(self.apprs),"request":{"tool_name":"Bash","input":{"command":"cargo test --workspace"},"tool_use_id":id}});
                self.push(&mut t, None, false, kind);
            }
            return;
        }
        let roll = self.rng.below(100);
        if roll < 4 {
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"finished","status":"interrupted"}),
            );
        } else if roll < 5 {
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"finished","status":"failed","message":"API Error: 529 overloaded"}),
            );
        } else {
            let text = self.rng.words_between(20, 120);
            let dup = self.rng.chance(0.8);
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"assistant_message","text": text}),
            );
            let input = self.rng.range(1000, 90000);
            let output = self.rng.range(100, 9000);
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"token_usage","input":input,"output":output,"cost_usd":0.4}),
            );
            let fin = if dup { text } else { last_text + " done" };
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"finished","status":"success","text": fin}),
            );
        }
        if self.rng.chance(0.03) {
            self.push(
                &mut t,
                None,
                false,
                json!({"type":"notice","message":"Context compacted"}),
            );
        }
    }
}

pub fn realistic(seed: u64, at_least: usize) -> Vec<Row> {
    let mut g = Realistic {
        rng: Prng::new(seed),
        rows: Vec::new(),
        ts: 1_700_000_000_000,
        tools: 0,
        apprs: 0,
        msgs: 0,
    };
    let mut turns = Vec::new();
    let mut no = 0;
    while g.rows.len() < at_least {
        no += 1;
        g.turn(no, false, false);
        turns.push(g.rows.len());
    }
    let from = g.rng.range(turns.len() / 3, turns.len() * 2 / 3);
    let to = (from + g.rng.range(1, 4)).min(turns.len().saturating_sub(1));
    let start = from.checked_sub(1).map_or(0, |i| turns[i]);
    let end = turns[to.max(from)];
    for r in &mut g.rows[start..end] {
        r.rewound = true;
    }
    g.turn(no + 1, false, true);
    g.rows
}

fn chaos_kind(rng: &mut Prng, i: usize, tools: &mut Vec<String>, elided: bool) -> Value {
    let existing = |rng: &mut Prng, tools: &[String]| {
        if !tools.is_empty() && rng.chance(0.6) {
            rng.pick(tools).clone()
        } else {
            format!("missing{}", rng.below(5))
        }
    };
    let text = |rng: &mut Prng| {
        if rng.chance(0.2) {
            "  ".to_owned()
        } else {
            rng.words_between(1, 30)
        }
    };
    match rng.below(19) {
        0 | 1 => json!({"type":"user_message","text": text(rng)}),
        2 | 3 => json!({"type":"assistant_message","text": text(rng)}),
        4 | 5 => json!({"type":"thinking","text": text(rng)}),
        6..=8 => {
            let id = if rng.chance(0.05) && !tools.is_empty() {
                rng.pick(tools).clone()
            } else {
                format!("t{i}")
            };
            tools.push(id.clone());
            let name = *rng.pick(
                &[
                    "Bash",
                    "Read",
                    "Edit",
                    "Task",
                    "TodoWrite",
                    "ExitPlanMode",
                    "mcp__blazar-fleet__wait_workspace",
                    "Unknown",
                ][..],
            );
            let input = match name {
                "Bash" => json!({"command": rng.words(3)}),
                "Read" | "Edit" => {
                    json!({"file_path": "/w/a.rs", "old_string": "a", "new_string": "b"})
                }
                "TodoWrite" => json!({"todos": [{"content": "x", "status": "pending"}]}),
                "ExitPlanMode" => json!({"plan": "do it"}),
                "mcp__blazar-fleet__wait_workspace" => json!({"workspace_id": "w1"}),
                _ => json!({"description": rng.words(2)}),
            };
            json!({"type":"tool_use","id":id,"name":name,"input":input})
        }
        9..=11 => {
            let id = existing(rng, tools);
            let ok = rng.chance(0.8);
            let content = if elided {
                String::new()
            } else if !ok && rng.chance(0.3) {
                "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Destructive Cleanup]. Continue with other work.".to_owned()
            } else {
                text(rng)
            };
            json!({"type":"tool_result","id": id,"ok": ok,"content": content})
        }
        12 => {
            let tn = *rng.pick(&["Bash", "Edit", "AskUserQuestion"][..]);
            json!({"type":"approval","id": appr_id(rng.below(6)),"request":{"tool_name": tn,"input":{"command":"ls"}}})
        }
        13 => {
            let decision = match rng.below(5) {
                0 => json!({"kind":"allow"}),
                1 => json!({"kind":"deny","message":"no"}),
                2 => json!({"kind":"auto_allowed","rule":"Bash(ls)"}),
                3 => json!({"kind":"cancelled"}),
                _ => json!({"kind":"aborted"}),
            };
            json!({"type":"approval_resolved","id": appr_id(rng.below(6)),"decision": decision})
        }
        14 => {
            json!({"type":"progress","message_id": format!("m{}", rng.below(4)),"output_tokens": rng.below(2000)})
        }
        15 => match rng.below(3) {
            0 => {
                let t = if rng.chance(0.5) {
                    Value::Null
                } else {
                    json!(text(rng))
                };
                let (denied, denials) = if rng.chance(0.3) {
                    let id = existing(rng, tools);
                    (
                        json!(["Bash: rm -rf build"]),
                        json!([{"tool_name":"Bash","tool_use_id": id,"input":{"command":"rm -rf build"}}]),
                    )
                } else {
                    (json!([]), json!([]))
                };
                json!({"type":"finished","status":"success","text": t,"usage":{"input":10,"output":5},"denied": denied,"denials": denials})
            }
            1 => json!({"type":"finished","status":"interrupted"}),
            _ => {
                let m = if rng.chance(0.5) {
                    "rate limit hit"
                } else {
                    "boom"
                };
                json!({"type":"finished","status":"failed","message": m})
            }
        },
        16 => match rng.below(5) {
            0 => {
                let status =
                    *rng.pick(&["started", "completed", "killed", "running", "stopped"][..]);
                json!({"type":"background_task","task_id": format!("bg{}", rng.below(3)),"status": status,"description": "job"})
            }
            1 => json!({"type":"notice","message":"note"}),
            2 => json!({"type":"error","message":"bad"}),
            3 => {
                let id = existing(rng, tools);
                json!({"type":"denial_decided","tool_use_id": id,"allow": rng.chance(0.6),"by":"demo","what":"rm -rf build"})
            }
            _ => json!({"type":"input_consumed","text":"x"}),
        },
        17 => match rng.below(3) {
            0 => {
                let model = if rng.chance(0.5) {
                    Value::Null
                } else {
                    json!("m1")
                };
                let mode = if rng.chance(0.5) {
                    Value::Null
                } else {
                    json!("auto")
                };
                json!({"type":"session_started","provider_session_id":"p","model": model,"cwd":null,"permission_mode": mode,"mcp_servers":[{"name":"a","status":"connected"}]})
            }
            1 => json!({"type":"token_usage","input": rng.below(500),"output": rng.below(100)}),
            _ => {
                json!({"type":"rate_limit","provider":"anthropic","allowed":true,"windows":[{"name":"five_hour","utilization": 0.5}]})
            }
        },
        _ => {
            json!({"type":"tool_result","id": format!("t{}", rng.below(i.max(1))),"ok": true,"content": text(rng)})
        }
    }
}

pub fn chaos(seed: u64, n: usize) -> Vec<Row> {
    let mut rng = Prng::new(seed);
    let mut rows = Vec::with_capacity(n);
    let mut sessions: Vec<(String, u64)> = vec![("s0".to_owned(), 0)];
    let mut cur = 0;
    let mut tools: Vec<String> = Vec::new();
    let mut rewound = false;
    let mut ts = 1_000;
    for i in 0..n {
        if rng.chance(0.08) {
            if rng.chance(0.7) || sessions.len() == 1 {
                sessions.push((format!("s{}", sessions.len()), 0));
                cur = sessions.len() - 1;
            } else {
                cur = rng.below(sessions.len());
            }
        }
        if rng.chance(0.05) {
            rewound = !rewound;
        }
        if !rng.chance(0.1) {
            ts += i64::try_from(rng.range(0, 3000)).unwrap_or(0);
        }
        let parent = if rng.chance(0.3) && !tools.is_empty() {
            Some(rng.pick(&tools).clone())
        } else if rng.chance(0.02) {
            Some("ghost".to_owned())
        } else {
            None
        };
        let elided = rng.chance(0.1);
        let kind = chaos_kind(&mut rng, i, &mut tools, elided);
        let (sid, seq) = &mut sessions[cur];
        *seq += 1;
        rows.push(row(sid, *seq, ts, parent.as_deref(), rewound, elided, kind));
    }
    rows
}
