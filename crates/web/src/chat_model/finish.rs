use std::hash::{Hash, Hasher};

use super::builder::{BBody, BItem, BStep, BTool};
use super::{Body, Fold, Item, Step};

fn take_step(tools: &mut Vec<Option<BTool>>, st: BStep) -> Step {
    match st {
        BStep::Thinking { text, secs } => Step::Thinking { text, secs },
        BStep::Text(t) => Step::Text(t),
        BStep::Bg(t) => Step::Bg(t),
        BStep::Note(t) => Step::Note(t),
        BStep::Orphan(r) => Step::Orphan(r),
        BStep::Tool(i) => {
            let BTool { mut tool, sub, .. } = tools[i].take().expect("每个工具只出现一次");
            tool.sub = sub.into_iter().map(|x| take_step(tools, x)).collect();
            Step::Tool(Box::new(tool))
        }
    }
}

fn sig_step(st: &Step, h: &mut impl Hasher) {
    match st {
        Step::Thinking { text, secs } => (0u8, text.len(), secs).hash(h),
        Step::Text(t) | Step::Bg(t) | Step::Note(t) => (1u8, t).hash(h),
        Step::Orphan(r) => (2u8, r).hash(h),
        Step::Tool(t) => {
            (
                3u8,
                &t.id,
                t.state,
                &t.result,
                t.ms,
                t.sub_calls,
                t.sub_elided,
                &t.arg,
            )
                .hash(h);
            for x in &t.sub {
                sig_step(x, h);
            }
        }
    }
}

pub fn step_sig(st: &Step) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sig_step(st, &mut h);
    h.finish()
}

fn sig_body(b: &Body) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match b {
        Body::User { text, first, .. } => (0u8, text, first).hash(&mut h),
        Body::Assistant { text } => (1u8, text).hash(&mut h),
        Body::Fold(f) => {
            (2u8, f.live, f.steps.len()).hash(&mut h);
            for s in &f.steps {
                sig_step(s, &mut h);
            }
        }
        Body::Error { text, by_account } => (3u8, text, by_account).hash(&mut h),
        Body::Warn { denied } => (4u8, denied).hash(&mut h),
        Body::Meta { text, bad } => (5u8, text, bad).hash(&mut h),
        Body::Retry { text } => (7u8, text).hash(&mut h),
        Body::Rewound { turns, items } => {
            (6u8, turns, items.len()).hash(&mut h);
            for i in items {
                i.sig.hash(&mut h);
            }
        }
    }
    h.finish()
}

pub(super) fn finish_items(
    items: Vec<BItem>,
    tools: &mut Vec<Option<BTool>>,
    live_last: bool,
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
                BBody::Assistant { text } => Body::Assistant { text },
                BBody::Error { text, by_account } => Body::Error { text, by_account },
                BBody::Warn { denied } => Body::Warn { denied },
                BBody::Meta { text, bad } => Body::Meta { text, bad },
                BBody::Retry { text } => Body::Retry { text },
                BBody::Rewound { turns, items } => Body::Rewound {
                    turns,
                    items: finish_items(items, tools, false),
                },
                BBody::Fold(steps) => {
                    let steps: Vec<Step> = steps.into_iter().map(|s| take_step(tools, s)).collect();
                    Body::Fold(Fold {
                        steps,
                        live: live_last && i + 1 == n,
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
