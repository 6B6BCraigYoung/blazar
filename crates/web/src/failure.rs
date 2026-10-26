use blazar_core_types::api::WorkspaceView;
use blazar_core_types::failure::FailureCause;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api::{self, Accounts};
use crate::app_state::AppData;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::{fmt, vocab};

pub const CONTINUE: &str = "继续";
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

    pub fn line(&self) -> String {
        format!("{}，{}", self.headline(), self.cause.advice())
    }

    pub fn primary(&self) -> &'static str {
        vocab::next_step(self.cause)
    }

    pub fn node_href(&self) -> Option<String> {
        (self.cause == FailureCause::Node).then(|| format!("/nodes/{}", api::enc(&self.node)))
    }
}

pub fn clock(iso: &str) -> String {
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
    let day = match days {
        0 => "今天".to_owned(),
        1 => "明天".to_owned(),
        _ => {
            let names = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
            names[at.get_day() as usize % 7].to_owned()
        }
    };
    format!("{day} {:02}:{:02}", at.get_hours(), at.get_minutes())
}

pub fn mark_handled(app: AppData, ws: String, session: Option<String>) {
    spawn_local(async move {
        match api::send::<Value>(
            "POST",
            &format!("/api/workspaces/{ws}/failure/handled"),
            &json!({ "session_id": session }),
        )
        .await
        {
            Ok(_) => {
                toast("已标为已处理");
                app.load_state();
            }
            Err(e) => toast(format!("没能标为已处理：{e}")),
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
            Err(e) => toast(format!("{failed}：{e}")),
        }
    });
}

pub fn continue_thread(app: AppData, ws: String, thread: String) {
    let body =
        json!({ "text": CONTINUE, "resume": true, "resume_session": thread, "wait_secs": 0 });
    prompt(app, ws, body, "接着做了", "没能继续");
}

pub fn probe_node(app: AppData, node: String) {
    spawn_local(async move {
        let probed = api::send::<Value>(
            "POST",
            &format!("/api/nodes/{}/probe", api::enc(&node)),
            &json!({}),
        )
        .await;
        app.load_state();
        match probed {
            Ok(_) => toast(format!("{} 连上了", fmt::machine(&node))),
            Err(e) => toast(format!(
                "还是连不上：{}",
                vocab::error(&e.to_string(), None)
            )),
        }
    });
}

async fn accounts() -> Option<Accounts> {
    api::get::<Accounts>("/api/accounts").await.ok()
}

pub fn login(runtime: String, account: Option<String>) {
    spawn_local(async move {
        let Some(list) = accounts().await else {
            toast("读取账号失败，请到运行时页面登录。");
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
            None => toast("没有这个运行时的账号，请先到运行时页面添加。"),
        }
    });
}

pub async fn pick_other_account(runtime: &str, exclude: Option<&str>) -> Option<String> {
    let Some(list) = accounts().await else {
        toast("读取账号失败，请稍后再试。");
        return None;
    };
    let others: Vec<_> = list
        .accounts
        .iter()
        .filter(|a| a.provider == runtime && Some(a.id.as_str()) != exclude && a.usable())
        .cloned()
        .collect();
    if others.is_empty() {
        toast("没有其他可用账号，请先在运行时页面添加或登录。");
        return None;
    }
    let mut choices = vec![Choice::plain("取消")];
    choices.extend(others.iter().map(|a| Choice::plain(a.label.clone())));
    let i = dialog::ask("换个账号继续", "保留这段对话，换一个账号接着做。", choices).await?;
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
        let Some(account) = pick_other_account(&runtime, exclude.as_deref()).await else {
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
        FailureCause::Node => probe_node(app, f.node.clone()),
        FailureCause::Login => login(f.runtime.clone(), f.account.clone()),
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
