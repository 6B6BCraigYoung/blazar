use std::hash::{Hash, Hasher};

use super::{St, Step, Tool, step_sig};

const READ_ONLY: [&str; 6] = ["Read", "Grep", "Glob", "LS", "WebFetch", "WebSearch"];

#[derive(Debug, Clone, PartialEq)]
pub enum Group {
    Reads {
        tools: Vec<Tool>,
        summary: String,
        state: St,
    },
    One(Step),
}

fn read_only(s: &Step) -> Option<&Tool> {
    match s {
        Step::Tool(t) if READ_ONLY.contains(&t.name.as_str()) => Some(t),
        _ => None,
    }
}

fn verb(name: &str) -> (&'static str, &'static str) {
    match name {
        "Read" => ("读取", "个文件"),
        "Grep" | "Glob" => ("搜索", "次"),
        "LS" => ("列出", "个目录"),
        "WebFetch" => ("抓取", "个网页"),
        _ => ("联网搜索", "次"),
    }
}

fn reads_summary(tools: &[Tool]) -> (String, St) {
    let mut parts: Vec<(&str, &str, usize)> = Vec::new();
    for t in tools {
        let (v, unit) = verb(&t.name);
        match parts.iter_mut().find(|p| p.0 == v) {
            Some(p) => p.2 += 1,
            None => parts.push((v, unit, 1)),
        }
    }
    let running = tools.iter().any(|t| t.state == St::Run);
    let errs = tools.iter().filter(|t| t.state == St::Err).count();
    let done = if running { "" } else { "了" };
    let mut text = parts
        .iter()
        .map(|(v, unit, n)| format!("{v}{done} {n} {unit}"))
        .collect::<Vec<_>>()
        .join("，");
    if running {
        text = format!("正在{text}…");
    }
    if let [only] = tools
        && !only.arg.is_empty()
    {
        text = format!("{text} · {}", only.arg);
    }
    if errs > 0 {
        text = format!("{text} · {errs} 次失败");
    }
    let state = if running {
        St::Run
    } else if errs > 0 {
        St::Err
    } else {
        St::Ok
    };
    (text, state)
}

pub fn group_steps(steps: &[Step]) -> Vec<(usize, Group)> {
    let mut out: Vec<(usize, Group)> = Vec::new();
    let mut run: Vec<Tool> = Vec::new();
    let mut at = 0;
    let flush = |run: &mut Vec<Tool>, at: usize, out: &mut Vec<(usize, Group)>| {
        if run.is_empty() {
            return;
        }
        let tools = std::mem::take(run);
        let (summary, state) = reads_summary(&tools);
        out.push((
            at,
            Group::Reads {
                tools,
                summary,
                state,
            },
        ));
    };
    for (i, s) in steps.iter().enumerate() {
        if let Some(t) = read_only(s) {
            if run.is_empty() {
                at = i;
            }
            run.push(t.clone());
        } else {
            flush(&mut run, at, &mut out);
            out.push((i, Group::One(s.clone())));
        }
    }
    flush(&mut run, at, &mut out);
    out
}

pub fn group_sig(g: &Group) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match g {
        Group::Reads { tools, .. } => {
            0u8.hash(&mut h);
            for t in tools {
                step_sig(&Step::Tool(Box::new(t.clone()))).hash(&mut h);
            }
        }
        Group::One(s) => (1u8, step_sig(s)).hash(&mut h),
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_model::Extra;

    fn tool(name: &str, arg: &str, state: St) -> Step {
        Step::Tool(Box::new(Tool {
            id: format!("{name}{arg}"),
            name: name.into(),
            label: name.into(),
            arg: arg.into(),
            state,
            extra: Extra::None,
            result: None,
            ms: None,
            sub: Vec::new(),
            sub_calls: 0,
            sub_last: String::new(),
            sub_elided: false,
        }))
    }

    fn summary(g: &Group) -> (&str, St) {
        match g {
            Group::Reads { summary, state, .. } => (summary.as_str(), *state),
            Group::One(_) => ("", St::Ok),
        }
    }

    #[test]
    fn consecutive_reads_merge_and_everything_else_stands_alone() {
        let steps = vec![
            tool("Read", "a.rs", St::Ok),
            tool("Read", "b.rs", St::Ok),
            tool("Grep", "\"x\"", St::Ok),
            tool("Glob", "*.rs", St::Ok),
            tool("Edit", "a.rs", St::Ok),
            tool("Read", "c.rs", St::Err),
            Step::Thinking {
                text: String::new(),
                secs: 2,
            },
            tool("Bash", "ls", St::Ok),
            tool("WebFetch", "https://x.invalid", St::Run),
        ];
        let g = group_steps(&steps);
        assert_eq!(
            g.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [0, 4, 5, 6, 7, 8]
        );
        assert_eq!(summary(&g[0].1), ("读取了 2 个文件，搜索了 2 次", St::Ok));
        assert!(matches!(&g[1].1, Group::One(Step::Tool(t)) if t.name == "Edit"));
        assert_eq!(
            summary(&g[2].1),
            ("读取了 1 个文件 · c.rs · 1 次失败", St::Err)
        );
        assert_eq!(
            summary(&g[5].1),
            ("正在抓取 1 个网页… · https://x.invalid", St::Run)
        );
        assert_ne!(group_sig(&g[0].1), group_sig(&g[2].1));
    }
}
