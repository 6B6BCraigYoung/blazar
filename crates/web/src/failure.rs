use blazar_core_types::api::WorkspaceView;
use blazar_core_types::failure::FailureCause;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api::{self, Accounts};
use crate::app_state::AppData;
use crate::components::dialog::{self, Choice};
use crate::components::toast::{toast, toast_error, toast_error_en};
use crate::{fmt, vocab};

pub const CONTINUE: &str = "Continue";
pub const SWITCHED: &str =
    "(Switched accounts.) Please continue from where you left off and finish the remaining work.";

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub cause: FailureCause,
    pub message: String,
    pub workspace: String,
    pub session: String,
    pub thread: String,
    pub node: String,
    pub runtime: String,
    pub account: Option<String>,
    pub resets_at: Option<String>,
}

impl Failure {
    pub fn of_workspace(w: &WorkspaceView) -> Option<Self> {
        let f = w.failure.as_ref()?;
        Some(Self {
            cause: f.cause,
            message: f.message.clone(),
            workspace: w.id.clone(),
            session: f.session_id.clone(),
            thread: f.thread_id.clone(),
            node: w.node.clone(),
            runtime: f.runtime.clone(),
            account: f.account.clone(),
            resets_at: f.resets_at.clone(),
        })
    }

    pub fn headline(&self) -> String {
        let resets = self.resets_at.as_deref().map(clock);
        vocab::failure(
            self.cause,
            &fmt::machine(&self.node),
            &self.runtime,
            resets.as_deref(),
            &self.message,
        )
    }

    pub fn headline_en(&self) -> String {
        let resets = self.resets_at.as_deref().map(clock_en);
        crate::vocab_en::failure(
            self.cause,
            &fmt::machine(&self.node),
            &self.runtime,
            resets.as_deref(),
            &self.message,
        )
    }

    /// The machine this run lost has come back: the card stops asking the user to reconnect
    /// it and offers to continue instead (the hub keeps the card when auto-resume is off).
    pub fn node_back(&self) -> bool {
        self.cause == FailureCause::Node
            && leptos::prelude::use_context::<AppData>()
                .is_some_and(|app| app.node_online(&self.node))
    }

    pub fn primary_en(&self) -> &'static str {
        if self.node_back() {
            crate::vocab_en::next_step(FailureCause::Network)
        } else {
            crate::vocab_en::next_step(self.cause)
        }
    }

    pub fn line(&self) -> String {
        if self.node_back() {
            return format!("{} 回来了，可以继续", fmt::machine(&self.node));
        }
        format!("{}，{}", self.headline(), self.cause.advice())
    }

    pub fn line_or(&self, stored: Option<&str>) -> String {
        match stored.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) if matches!(self.cause, FailureCause::Other | FailureCause::Resume) => {
                s.to_owned()
            }
            _ => self.line(),
        }
    }

    pub fn primary(&self) -> &'static str {
        if self.node_back() {
            vocab::next_step(FailureCause::Network)
        } else {
            vocab::next_step(self.cause)
        }
    }

    pub fn node_href(&self) -> Option<String> {
        (self.cause == FailureCause::Node).then(|| format!("/nodes/{}", api::enc(&self.node)))
    }

    /// The dot colour, see [`vocab::failure_tone`].
    pub fn tone(&self) -> vocab::Tone {
        if self.node_back() {
            vocab::failure_tone(FailureCause::Network)
        } else {
            vocab::failure_tone(self.cause)
        }
    }
}

pub fn clock(iso: &str) -> String {
    clock_in(iso, false)
}

pub fn clock_en(iso: &str) -> String {
    clock_in(iso, true)
}

fn clock_in(iso: &str, en: bool) -> String {
    let ms = js_sys::Date::parse(iso);
    if ms.is_nan() {
        return String::new();
    }
    let at = js_sys::Date::new(&ms.into());
    let now = js_sys::Date::new_0();
    let midnight = |d: &js_sys::Date| {
        js_sys::Date::new_with_year_month_day(
            d.get_full_year(),
            d.get_month() as i32,
            d.get_date() as i32,
        )
        .get_time()
    };
    let days = ((midnight(&at) - midnight(&now)) / 86_400_000.0).round() as i64;
    let day = match (days, en) {
        (0, false) => "今天".to_owned(),
        (1, false) => "明天".to_owned(),
        (0, true) => "today".to_owned(),
        (1, true) => "tomorrow".to_owned(),
        (_, true) => {
            let names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            names[at.get_day() as usize % 7].to_owned()
        }
        (_, false) => {
            let names = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
            names[at.get_day() as usize % 7].to_owned()
        }
    };
    format!("{day} {:02}:{:02}", at.get_hours(), at.get_minutes())
}

pub fn mark_handled(app: AppData, ws: String, session: Option<String>, en: bool) {
    spawn_local(async move {
        match api::send::<Value>(
            "POST",
            &format!("/api/workspaces/{ws}/failure/handled"),
            &json!({ "session_id": session }),
        )
        .await
        {
            Ok(_) => {
                toast(if en {
                    "Marked as handled"
                } else {
                    "已标为已处理"
                });
                app.load_state();
            }
            Err(e) if en => toast_error_en("Couldn't mark as handled", &e),
            Err(e) => toast_error("没能标为已处理", &e),
        }
    });
}

fn prompt(app: AppData, ws: String, body: Value, done: &'static str, failed: &'static str) {
    spawn_local(async move {
        match api::send::<Value>("POST", &format!("/api/workspaces/{ws}/prompt"), &body).await {
            Ok(r) if r["admitted"] == json!(false) => toast(
                r["reason"]
                    .as_str()
                    .unwrap_or("工作区正忙着别的操作")
                    .to_owned(),
            ),
            Ok(_) => {
                toast(done);
                app.load_state();
            }
            Err(e) => toast_error(failed, &e),
        }
    });
}

pub fn continue_thread(app: AppData, ws: String, thread: String) {
    let body =
        json!({ "text": CONTINUE, "resume": true, "resume_session": thread, "wait_secs": 0 });
    prompt(app, ws, body, "接着做了", "没能继续");
}

pub fn probe_node(app: AppData, node: String, en: bool) {
    spawn_local(async move {
        let probed = api::send::<Value>(
            "POST",
            &format!("/api/nodes/{}/probe", api::enc(&node)),
            &json!({}),
        )
        .await;
        app.load_state();
        match probed {
            Ok(_) if en => toast(format!("{} is back online", fmt::machine(&node))),
            Err(e) if en => toast_error_en("Still can't connect", &e),
            Ok(_) => toast(format!("{} 连上了", fmt::machine(&node))),
            Err(e) => toast_error("还是连不上", &e),
        }
    });
}

async fn accounts() -> Option<Accounts> {
    api::get::<Accounts>("/api/accounts").await.ok()
}

pub fn login(runtime: String, account: Option<String>, en: bool) {
    spawn_local(async move {
        let Some(list) = accounts().await else {
            toast(if en {
                "Couldn't load accounts. Log in on the Runtimes page."
            } else {
                "读取账号失败，请到「账号与额度」登录。"
            });
            return;
        };
        let wanted = account.unwrap_or_else(|| format!("{runtime}-default"));
        let picked = list
            .accounts
            .iter()
            .find(|a| a.id == wanted)
            .or_else(|| list.accounts.iter().find(|a| a.provider == runtime))
            .cloned();
        match picked {
            Some(a) => crate::pages::runtimes::login(&a),
            None if en => toast("No account for this runtime. Add one on the Runtimes page."),
            None => toast("还没有这个软件的账号，请先到「账号与额度」添加。"),
        }
    });
}

pub async fn pick_other_account(runtime: &str, exclude: Option<&str>, en: bool) -> Option<String> {
    let Some(list) = accounts().await else {
        toast(if en {
            "Couldn't load accounts. Try again later."
        } else {
            "读取账号失败，请稍后再试。"
        });
        return None;
    };
    let others: Vec<_> = list
        .accounts
        .iter()
        .filter(|a| a.provider == runtime && Some(a.id.as_str()) != exclude && a.usable())
        .cloned()
        .collect();
    if others.is_empty() {
        toast(if en {
            "No other account available. Add or log in to one on the Runtimes page."
        } else {
            "没有其他可用账号，请先在「账号与额度」添加或登录。"
        });
        return None;
    }
    let mut choices = vec![Choice::plain(if en { "Cancel" } else { "取消" })];
    choices.extend(others.iter().map(|a| Choice::plain(a.label.clone())));
    let (title, body) = if en {
        (
            "Continue with another account",
            "Keep the conversation and continue on another account.",
        )
    } else {
        ("换个账号继续", "保留这段对话，换一个账号接着做。")
    };
    let i = dialog::ask(title, body, choices).await?;
    (i > 0).then(|| others[i - 1].id.clone())
}

pub fn switch_account(
    app: AppData,
    ws: String,
    thread: String,
    runtime: String,
    exclude: Option<String>,
) {
    spawn_local(async move {
        let Some(account) = pick_other_account(&runtime, exclude.as_deref(), false).await else {
            return;
        };
        let body = json!({
            "text": SWITCHED, "resume": true, "resume_session": thread,
            "account": account, "wait_secs": 0,
        });
        prompt(app, ws, body, "已换账号继续", "没能发送");
    });
}

pub fn act(app: AppData, f: &Failure, retry: impl FnOnce()) {
    match f.cause {
        FailureCause::Network | FailureCause::Busy => {
            continue_thread(app, f.workspace.clone(), f.thread.clone());
        }
        FailureCause::Node if f.node_back() => {
            continue_thread(app, f.workspace.clone(), f.thread.clone());
        }
        FailureCause::Node => probe_node(app, f.node.clone(), false),
        FailureCause::Login => login(f.runtime.clone(), f.account.clone(), false),
        FailureCause::Quota => switch_account(
            app,
            f.workspace.clone(),
            f.thread.clone(),
            f.runtime.clone(),
            f.account.clone(),
        ),
        FailureCause::Resume | FailureCause::Other => retry(),
    }
}
