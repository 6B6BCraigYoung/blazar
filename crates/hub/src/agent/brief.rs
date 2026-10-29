use blazar_core_types::api::{Ask, DEFAULT_ASK_OPTIONS};
use blazar_core_types::failure::FailureCause;
use blazar_core_types::{SessionId, WorkspaceId};
use serde_json::Value;
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;
use crate::titles::{Brain, ask_haiku, brain};

pub const SUMMARY_MAX: usize = 60;
const QUESTION_MAX: usize = 80;
const OPTION_MAX: usize = 24;
const PROMPT_CHARS: usize = 800;
const REPLY_CHARS: usize = 2000;
const PARAGRAPH_MAX: usize = 400;
const MODEL_SECS: u64 = 40;
const ASKS_MAX: usize = 4;

const ASK_CUES: &[&str] = &[
    "可以吗",
    "要不要",
    "你选",
    "请确认",
    "哪个",
    "哪种",
    "行不行",
    "好吗",
    "能否",
    "请选择",
    "请决定",
    "你决定",
    "你定",
    "需要你",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Brief {
    pub summary: String,
    pub asks: Vec<Ask>,
}

#[derive(Debug, Clone, Default)]
pub struct Turn {
    pub prompt: String,
    pub reply: String,
    pub failure: String,
    pub cause: Option<FailureCause>,
    pub account: Option<String>,
}

fn head(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(n - max));
    out
}

#[must_use]
pub fn tidy(line: &str, max: usize) -> String {
    let line = line
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .trim_start_matches(|c: char| "-*•#>".contains(c))
        .trim();
    let line = crate::titles::strip_echo(line)
        .trim_matches(|c: char| "\"'“”「」『』《》`".contains(c))
        .trim_end_matches(|c: char| "。．.,，;；".contains(c))
        .trim();
    let joined = line
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    crate::titles::fit(&joined, max)
}

fn paragraphs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join("\n").trim().to_owned());
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    out
}

#[must_use]
pub fn last_paragraph(text: &str) -> String {
    paragraphs(text)
        .into_iter()
        .rev()
        .find(|p| {
            !p.starts_with("```")
                && !p.starts_with('|')
                && !p.starts_with("---")
                && !p.starts_with("<details")
        })
        .unwrap_or_default()
}

fn prose(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty()
        && !l.starts_with('|')
        && !l.starts_with('#')
        && !l.starts_with("```")
        && !l.starts_with("- ")
        && !l.starts_with("* ")
        && !l.starts_with(char::is_numeric)
}

#[must_use]
pub fn last_prose(text: &str) -> String {
    text.lines()
        .rev()
        .find(|l| prose(l))
        .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[must_use]
pub fn asks_user(reply: &str) -> bool {
    let p = last_paragraph(reply);
    if p.is_empty() || p.chars().count() > PARAGRAPH_MAX {
        return false;
    }
    let end = p.trim_end_matches(|c: char| "」』”\"'）)*_ ".contains(c));
    if end.ends_with('？') || end.ends_with('?') {
        return true;
    }
    ASK_CUES.iter().any(|c| p.contains(c))
}

#[must_use]
pub fn question_of(reply: &str) -> String {
    let p = last_paragraph(reply);
    let sentence = p
        .split_inclusive(|c: char| "？?。！!".contains(c))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .rev()
        .find(|s| s.ends_with('？') || s.ends_with('?') || ASK_CUES.iter().any(|c| s.contains(c)))
        .unwrap_or(p.as_str());
    let q = tidy(sentence, QUESTION_MAX);
    if q.is_empty() {
        tidy(&p, QUESTION_MAX)
    } else {
        q
    }
}

fn option_line(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix(|c: char| "-*•".contains(c)).or_else(|| {
        let digits = t.trim_start_matches(|c: char| c.is_ascii_digit() || "abcdABCD".contains(c));
        (digits.len() < t.len())
            .then(|| digits.strip_prefix(|c: char| ".、)）:：".contains(c)))
            .flatten()
    })?;
    let item = tidy(rest, OPTION_MAX + 1);
    (!item.is_empty() && item.chars().count() <= OPTION_MAX).then_some(item)
}

#[must_use]
pub fn local_options(reply: &str) -> Vec<String> {
    let items: Vec<String> = last_paragraph(reply)
        .lines()
        .filter_map(option_line)
        .collect();
    if (2..=4).contains(&items.len()) {
        items
    } else {
        Vec::new()
    }
}

fn or_default(options: Vec<String>) -> Vec<String> {
    if options.is_empty() {
        DEFAULT_ASK_OPTIONS.map(str::to_owned).to_vec()
    } else {
        options
    }
}

fn numbered_question(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == t.len() {
        return None;
    }
    let rest = rest.strip_prefix(|c: char| ".、)）:：".contains(c))?;
    (rest.contains('？') || rest.contains('?')).then(|| tidy(rest, QUESTION_MAX))
}

/// One card per numbered question when the reply closes on several; otherwise one card.
#[must_use]
pub fn local_asks(reply: &str) -> Vec<Ask> {
    let questions: Vec<String> = last_paragraph(reply)
        .lines()
        .filter_map(numbered_question)
        .filter(|q| !q.is_empty())
        .take(ASKS_MAX)
        .collect();
    if questions.len() >= 2 {
        return questions
            .into_iter()
            .map(|question| Ask {
                question,
                options: or_default(Vec::new()),
            })
            .collect();
    }
    vec![Ask {
        question: question_of(reply),
        options: or_default(local_options(reply)),
    }]
}

#[must_use]
pub fn fallback(status: &str, turn: &Turn) -> Brief {
    match status {
        "interrupted" => Brief {
            summary: "被中断了，没做完".into(),
            asks: Vec::new(),
        },
        "failed" => Brief {
            summary: turn.cause.unwrap_or(FailureCause::Other).one_liner(),
            asks: Vec::new(),
        },
        _ => {
            let summary = tidy(&last_prose(&turn.reply), SUMMARY_MAX);
            let asks = if asks_user(&turn.reply) {
                local_asks(&turn.reply)
            } else {
                Vec::new()
            };
            Brief {
                summary: if summary.is_empty() {
                    "跑完了".into()
                } else {
                    summary
                },
                asks,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelReply {
    pub summary: String,
    pub asking: bool,
    pub questions: Vec<Ask>,
}

fn model_options(v: &Value) -> Vec<String> {
    v["options"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|o| tidy(o, OPTION_MAX))
                .filter(|o| !o.is_empty())
                .take(4)
                .collect()
        })
        .unwrap_or_default()
}

fn model_ask(v: &Value) -> Option<Ask> {
    let question = tidy(v["question"].as_str().unwrap_or_default(), QUESTION_MAX);
    (!question.is_empty()).then(|| Ask {
        question,
        options: model_options(v),
    })
}

#[must_use]
pub fn parse_model(text: &str) -> Option<ModelReply> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let v: Value = serde_json::from_str(&text[start..=end]).ok()?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let questions = match v["questions"].as_array() {
        Some(list) => list.iter().filter_map(model_ask).take(ASKS_MAX).collect(),
        None => model_ask(&v).into_iter().collect(),
    };
    Some(ModelReply {
        summary: tidy(&s("summary"), SUMMARY_MAX),
        asking: v["asking"].as_bool() == Some(true),
        questions,
    })
}

#[must_use]
pub fn merge(status: &str, turn: &Turn, model: Option<ModelReply>) -> Brief {
    let base = fallback(status, turn);
    let Some(m) = model else {
        return base;
    };
    let summary = if m.summary.is_empty() || crate::titles::looks_like_error(&m.summary) {
        base.summary
    } else {
        m.summary
    };
    let asks = if status != "done" || !m.asking || base.asks.is_empty() {
        Vec::new()
    } else if m.questions.is_empty() {
        base.asks
    } else {
        let single = m.questions.len() == 1 && base.asks.len() == 1;
        let local = base.asks.into_iter().next().unwrap_or_default().options;
        m.questions
            .into_iter()
            .map(|a| Ask {
                question: a.question,
                options: if !a.options.is_empty() {
                    a.options
                } else if single {
                    local.clone()
                } else {
                    or_default(Vec::new())
                },
            })
            .collect()
    };
    Brief { summary, asks }
}

fn model_prompt(status: &str, turn: &Turn) -> String {
    let prompt = head(turn.prompt.trim(), PROMPT_CHARS);
    if status == "failed" {
        return format!(
            "你在给一个任务看板写卡片。下面是一轮对话的用户指令和它失败的原因。\
             只输出一个 JSON 对象，不要别的内容：\n\
             {{\"summary\": \"\", \"asking\": false, \"questions\": []}}\n\
             summary 用中文写一句话（不超过 30 个字），说清楚为什么失败、下一步可以做什么；\
             只写那句话本身，不要加「摘要」之类的前缀，不要路径、提交号、命令和引号。\
             \n\n用户指令：\n{prompt}\n\n失败原因：\n{}",
            head(turn.failure.trim(), 600)
        );
    }
    format!(
        "你在给一个任务看板写卡片。下面是一轮对话的用户指令和助手的最后回复。\
         只输出一个 JSON 对象，不要别的内容：\n\
         {{\"summary\": \"\", \"asking\": false, \"questions\": [{{\"question\": \"\", \"options\": []}}]}}\n\
         - summary：用中文写一句话（不超过 30 个字），说清楚这一轮做成了什么，或者卡在哪里；\
         只写那句话本身，不要加「摘要」「这段对话在做什么」之类的前缀，不要路径、提交号、命令和引号。\n\
         - asking：只看回复的最后一段，助手是不是在等用户回答或拍板（例如结尾是问句，\
         或者出现「可以吗 / 要不要 / 请确认」）。普通的汇报写 false。\n\
         - questions：asking 为 false 时给空数组。为 true 时，助手每问一件要用户定的事就写一项，\
         最多 4 项（例如「要你定两件事：1. … 2. …」写两项）。\
         question 用一句话（不超过 30 个字）写清楚这一件在问什么；\
         options 列出 2 到 4 个短选项（每个不超过 10 个字），选项写在句子里的也要拆出来\
         （例如「要不要 A，或者 B？」给「A」「B」「都不要」），实在看不出选项时给空数组。\n\n用户指令：\n{prompt}\n\n助手回复：\n{}",
        tail(turn.reply.trim(), REPLY_CHARS)
    )
}

fn wants_model(status: &str, turn: &Turn) -> bool {
    match status {
        "done" => !turn.reply.trim().is_empty(),
        "failed" => {
            matches!(turn.cause, Some(FailureCause::Other) | None)
                && !turn.failure.trim().is_empty()
        }
        _ => false,
    }
}

async fn turn_of(st: &Shared, sid: SessionId) -> Turn {
    let id = sid.to_string();
    let account: Option<String> =
        sqlx::query_scalar::<_, Option<String>>("SELECT account_id FROM sessions WHERE id = ?1")
            .bind(&id)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .flatten();
    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1
           AND (payload LIKE '{\"type\":\"user_message\"%'
             OR payload LIKE '{\"type\":\"assistant_message\"%'
             OR payload LIKE '{\"type\":\"finished\"%')
         ORDER BY seq",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut turn = Turn {
        account,
        ..Turn::default()
    };
    for p in payloads {
        let Ok(v) = serde_json::from_str::<Value>(&p) else {
            continue;
        };
        let text = v["text"].as_str().unwrap_or_default();
        match v["type"].as_str() {
            Some("user_message") if turn.prompt.is_empty() => turn.prompt = text.to_owned(),
            Some("assistant_message") if !text.trim().is_empty() => turn.reply = text.to_owned(),
            Some("finished") => {
                if !text.trim().is_empty() {
                    turn.reply = text.to_owned();
                }
                if let Some(m) = v["message"].as_str() {
                    turn.failure = m.to_owned();
                }
            }
            _ => {}
        }
    }
    turn.cause = crate::failures::cause_of(st, &id).await;
    turn
}

async fn ask_model(brain: &Brain, status: &str, turn: &Turn) -> Option<ModelReply> {
    let text = ask_haiku(brain, &model_prompt(status, turn), MODEL_SECS).await?;
    let parsed = parse_model(&text);
    if parsed.is_none() {
        tracing::warn!(target: "blazar::brief", "摘要模型没有给出 JSON：{}", head(&text, 160));
    }
    parsed
}

pub async fn store(st: &Shared, sid: SessionId, ws: WorkspaceId, brief: &Brief) {
    let ask = brief
        .asks
        .first()
        .and_then(|a| serde_json::to_string(a).ok());
    let _ = sqlx::query("UPDATE sessions SET summary = ?2, ask = COALESCE(ask, ?3) WHERE id = ?1")
        .bind(sid.to_string())
        .bind(&brief.summary)
        .bind(ask)
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::WorkspacesChanged);
    st.emit(ServerEvent::SessionTitled {
        workspace_id: ws,
        session_id: sid,
        title: String::new(),
    });
}

pub async fn on_run_finished(st: Shared, sid: SessionId, ws: WorkspaceId, status: &'static str) {
    let turn = turn_of(&st, sid).await;
    let model = if wants_model(status, &turn) {
        let brain = brain(&st, turn.account.as_deref()).await;
        ask_model(&brain, status, &turn).await
    } else {
        None
    };
    let brief = merge(status, &turn, model);
    store(&st, sid, ws, &brief).await;
    if status == "failed" && crate::auto_resume::takes_over(&st, sid).await {
        return;
    }
    crate::inbox::on_run_finished(st, sid, status, &brief).await;
}

pub async fn ask_of(st: &Shared, sid: &str) -> Option<Ask> {
    sqlx::query_scalar::<_, Option<String>>("SELECT ask FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .flatten()
        .and_then(|a| serde_json::from_str(&a).ok())
}

pub fn ask_from_row(row: &sqlx::sqlite::SqliteRow, column: &str) -> Option<Ask> {
    row.try_get::<Option<String>, _>(column)
        .ok()
        .flatten()
        .and_then(|a| serde_json::from_str(&a).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closing_question_counts_as_asking_but_a_report_does_not() {
        let asking = "改好了分页。\n\n可以吗？正式 hub 要不要一起改？";
        assert!(asks_user(asking));
        assert_eq!(question_of(asking), "正式 hub 要不要一起改？");
        assert!(!asks_user("全部做完了：\n\n- 分页\n- 测试\n\n没有推送。"));
        assert!(!asks_user(
            "先问一句：要不要分页？\n\n已经按默认做完了，测试通过。"
        ));
        assert!(!asks_user(""));
        let long = format!("{}？", "很长".repeat(300));
        assert!(!asks_user(&long));
    }

    #[test]
    fn options_come_from_a_short_list_in_the_last_paragraph() {
        let reply = "有两个方案：\n\n1. 换到本机重跑\n2. 等节点恢复\n\n你选哪个？";
        assert!(asks_user(reply));
        assert!(local_options(reply).is_empty());
        let listed = "你选哪个？\n- 换到本机重跑\n- 等节点恢复";
        assert_eq!(local_options(listed), vec!["换到本机重跑", "等节点恢复"]);
    }

    #[test]
    fn fallbacks_never_show_raw_errors() {
        let turn = Turn {
            failure: "API Error: 529 overloaded".into(),
            cause: Some(FailureCause::Busy),
            ..Turn::default()
        };
        assert_eq!(
            fallback("failed", &turn).summary,
            "服务繁忙，没做完，可以继续"
        );
        assert_eq!(fallback("interrupted", &turn).summary, "被中断了，没做完");
        let done = Turn {
            reply: "已提交：`3f9c2ab feat(users): paginate`。没有推送。".into(),
            ..Turn::default()
        };
        assert_eq!(
            fallback("done", &done).summary,
            "已提交：3f9c2ab feat(users): paginate。没有推送"
        );
        assert_eq!(fallback("done", &Turn::default()).summary, "跑完了");
    }

    #[test]
    fn summaries_drop_echoed_prefixes_and_end_on_whole_words() {
        let model = parse_model(
            "{\"summary\": \"这段对话在做什么：请求在 lab-3.invalid 上为真机试验做准备（方案丙，A/B/C 三组对照，每组各跑五轮再汇总）\", \"asking\": false}",
        )
        .unwrap();
        assert!(model.summary.starts_with("请求在 lab-3.invalid 上"));
        assert!(model.summary.chars().count() <= SUMMARY_MAX);
        assert!(!model.summary.contains("这段对话"));
        assert_eq!(
            model.summary.matches('（').count(),
            model.summary.matches('）').count()
        );
        assert!(!model.summary.ends_with("A/B/C…"));
        assert_eq!(
            tidy("摘要：改完了，测试通过。", SUMMARY_MAX),
            "改完了，测试通过"
        );
    }

    #[test]
    fn the_model_reply_is_merged_conservatively() {
        let turn = Turn {
            reply: "改完了。\n\n正式 hub 要不要一起改？".into(),
            ..Turn::default()
        };
        let model = parse_model(
            "```json\n{\"summary\": \"给 /users 加了分页，测试通过。\", \"asking\": true, \"question\": \"正式 hub 要不要一起改\", \"options\": [\"一起改\", \"先不动\"]}\n```",
        );
        let b = merge("done", &turn, model);
        assert_eq!(b.summary, "给 /users 加了分页，测试通过");
        assert_eq!(b.asks.len(), 1);
        assert_eq!(b.asks[0].question, "正式 hub 要不要一起改");
        assert_eq!(b.asks[0].options, vec!["一起改", "先不动"]);
        let quiet = Turn {
            reply: "改完了，测试通过。".into(),
            ..Turn::default()
        };
        let model = ModelReply {
            summary: "Not logged in · Please run /login".into(),
            asking: true,
            questions: vec![Ask {
                question: "要不要继续".into(),
                options: vec![],
            }],
        };
        let b = merge("done", &quiet, Some(model));
        assert_eq!(b.summary, "改完了，测试通过");
        assert!(b.asks.is_empty());
        assert!(parse_model("no json here").is_none());
    }

    #[test]
    fn several_numbered_questions_become_one_card_each() {
        let reply = "方案整理好了。\n\n要你定两件事：\n1. 产什么：只出报告还是连代码一起改？\n2. 谁来执行：mac-mini 还是笔记本？";
        assert!(asks_user(reply));
        let asks = local_asks(reply);
        assert_eq!(asks.len(), 2);
        assert!(asks[0].question.starts_with("产什么"));
        assert!(asks[1].question.starts_with("谁来执行"));
        assert!(asks.iter().all(|a| a.options == DEFAULT_ASK_OPTIONS));
        let one = local_asks("改完了。\n\n要不要把远端拉下来，或者整理一份上游分析？");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].options, DEFAULT_ASK_OPTIONS);
    }

    #[test]
    fn the_model_can_split_a_reply_into_several_cards() {
        let turn = Turn {
            reply: "要你定两件事：\n1. 产什么？\n2. 谁来执行？".into(),
            ..Turn::default()
        };
        let model = parse_model(
            "{\"summary\": \"方案整理好了\", \"asking\": true, \"questions\": [\
             {\"question\": \"产出什么\", \"options\": [\"只出报告\", \"连代码一起改\"]},\
             {\"question\": \"谁来执行\", \"options\": []}]}",
        );
        let b = merge("done", &turn, model);
        assert_eq!(b.asks.len(), 2);
        assert_eq!(b.asks[0].options, vec!["只出报告", "连代码一起改"]);
        assert_eq!(b.asks[1].question, "谁来执行");
        assert_eq!(b.asks[1].options, DEFAULT_ASK_OPTIONS);
        let legacy =
            parse_model("{\"asking\": true, \"question\": \"推吗\", \"options\": [\"推\"]}")
                .unwrap();
        assert_eq!(legacy.questions.len(), 1);
        let many = format!(
            "{{\"asking\": true, \"questions\": [{}]}}",
            ["{\"question\": \"q\"}"; 6].join(",")
        );
        assert_eq!(parse_model(&many).unwrap().questions.len(), ASKS_MAX);
    }
}
