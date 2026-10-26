use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::chat_model;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;

use super::delivery::steer_request;
use super::{Chat, Queued};

impl Chat {
    pub fn stop(self) {
        let Some(sid) = self.transcript.with_untracked(|t| t.last_session.clone()) else {
            toast("这个对话没有在运行");
            return;
        };
        self.spawn(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/sessions/{sid}/interrupt"),
                &json!({}),
            )
            .await
            {
                Ok(r) if r["interrupted"].as_bool() == Some(true) => toast("已停止"),
                Ok(r) => toast(r["reason"].as_str().unwrap_or("没能停止").to_owned()),
                Err(e) => toast(format!("没能停止：{e}")),
            }
        });
    }

    pub fn live(self, body: Value, what: &'static str) {
        if !self.running.get_untracked() {
            return;
        }
        let Some(sid) = self.transcript.with_untracked(|t| t.last_session.clone()) else {
            return;
        };
        self.spawn(async move {
            match api::send::<Value>("POST", &format!("/api/sessions/{sid}/control"), &body).await {
                Ok(r) if r["accepted"].as_bool() == Some(true) => {
                    toast(format!("{what} 已用于当前这一轮"))
                }
                Ok(r) => toast(format!(
                    "{what} 将从下一轮生效（{}）",
                    r["reason"].as_str().unwrap_or("")
                )),
                Err(_) => toast(format!("{what} 将从下一轮生效")),
            }
        });
    }

    pub fn set_mode(self, v: &str) {
        self.perm.set(v.to_owned());
        if !v.is_empty() {
            self.live(json!({ "permission_mode": v }), "permission mode");
        }
    }

    pub fn cycle_mode(self) {
        let list = untrack(move || self.modes());
        if list.is_empty() {
            return;
        }
        let cur = untrack(move || self.effective_mode()).map(|m| m.0);
        let i = list
            .iter()
            .position(|m| Some(m.0) == cur)
            .map_or(0, |i| (i + 1) % list.len());
        self.set_mode(list[i].0);
    }

    pub fn decide(self, id: String, allow: bool, message: String, answers: Option<Value>) {
        if !self.decided.write().begin(&id) {
            return;
        }
        self.spawn(async move {
            let mut body = json!({ "allow": allow, "message": message });
            if let Some(a) = answers {
                body["answers"] = a;
            }
            let result = api::send::<Value>("POST", &format!("/api/approvals/{id}"), &body).await;
            let delivered = result
                .as_ref()
                .is_ok_and(|result| result["delivered"].as_bool() == Some(true));
            let _ = self
                .decided
                .try_update(|decisions| decisions.finish(&id, delivered));
            match result {
                Ok(_) if delivered => {}
                Ok(result) => toast(
                    result["reason"]
                        .as_str()
                        .unwrap_or("没有送达，请重试")
                        .to_owned(),
                ),
                Err(error) => toast(format!("没能发送，内容已保留：{error}")),
            }
        });
    }

    pub fn decide_denial(self, sid: String, tool_use_id: String, allow: bool) {
        let key = format!("{sid}:{tool_use_id}");
        if !self.denials.write().begin(&key) {
            return;
        }
        self.spawn(async move {
            let r = api::send::<Value>(
                "POST",
                &format!("/api/sessions/{sid}/denials"),
                &json!({ "tool_use_id": tool_use_id, "allow": allow }),
            )
            .await;
            let recorded = r
                .as_ref()
                .is_ok_and(|r| r["recorded"].as_bool() == Some(true));
            let _ = self.denials.try_update(|d| d.finish(&key, recorded));
            match r {
                Ok(r) if recorded => {
                    if allow && let Some(text) = r["text"].as_str() {
                        self.send_text(text.to_owned());
                    }
                }
                Ok(r) => toast(
                    r["reason"]
                        .as_str()
                        .unwrap_or("这条操作已经决定过了")
                        .to_owned(),
                ),
                Err(e) => toast(format!("没能记录：{e}")),
            }
        });
    }

    pub fn allow_always(self, id: String, request: Value) {
        let ws = self.ws_id();
        self.spawn(async move {
            let (tool, pattern) = chat_model::always_rule(&request);
            let what = if pattern.is_empty() {
                tool.clone()
            } else {
                format!("{tool} {pattern}…")
            };
            let ok = dialog::ask(
                "不再询问",
                &format!("在这个工作区里总是允许「{what}」？\n\n之后可以在设置里关掉。"),
                vec![Choice::plain("取消"), Choice::plain("总是允许")],
            )
            .await;
            if ok != Some(1) {
                return;
            }
            match api::send::<Value>(
                "POST",
                "/api/approval-rules",
                &json!({ "tool": tool, "pattern": pattern, "workspace_id": ws }),
            )
            .await
            {
                Ok(_) => self.decide(id, true, String::new(), None),
                Err(e) => toast(e.to_string()),
            }
        });
    }

    pub fn queue_act(self, act: &'static str, q: Queued) {
        if act == "edit" {
            if q.request.is_some() {
                self.editing_queue.set(Some(q));
            } else {
                toast("这条消息的设置不能在这里修改，仍在排队");
            }
            return;
        }
        let context = self.send_context(q.thread_id.clone());
        let ws = context.workspace.clone();
        let steer_session = self.transcript.with_untracked(|t| t.last_session.clone());
        let base = format!("/api/workspaces/{ws}/queue/{}", q.id);
        self.spawn(async move {
            let r: Result<(), api::ApiError> = async {
                match act {
                    "drop" => {
                        api::send::<Value>("DELETE", &base, &json!({})).await?;
                        toast("已移除");
                    }
                    "send" => {
                        let r =
                            api::send::<Value>("POST", &format!("{base}/send"), &json!({})).await?;
                        self.note_sent_in(
                            &context,
                            r["session_id"].as_str().map(str::to_owned),
                            r["thread_id"].as_str().map(str::to_owned),
                        );
                    }
                    _ => {
                        let Some(sid) = steer_session else {
                            toast("没有正在运行的会话可以加入");
                            return Ok(());
                        };
                        let Some(request) = q.request.as_ref() else {
                            toast("这条消息的设置不能在一轮中途发送，仍在排队");
                            return Ok(());
                        };
                        if !self.context_is_current(&context) {
                            toast("对话已切换，消息仍在排队");
                            return Ok(());
                        }
                        let i = api::send::<Value>(
                            "POST",
                            &format!("/api/sessions/{sid}/input"),
                            &steer_request(request),
                        )
                        .await?;
                        if i["accepted"].as_bool() != Some(true) {
                            toast(
                                i["reason"]
                                    .as_str()
                                    .unwrap_or(
                                        "这个运行时不能在一轮中途接收消息，会在这一轮结束后发送。",
                                    )
                                    .to_owned(),
                            );
                            return Ok(());
                        }
                        api::send::<Value>("DELETE", &base, &json!({"expected": request})).await?;
                        toast("已加入当前这一轮");
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                toast(e.to_string());
            }
            self.load_queue();
        });
    }

    pub fn retry(self, sid: String, text: Option<String>, later: usize) {
        if self.running.get_untracked() {
            toast("先停止当前这一轮，再重试。");
            return;
        }
        let context = self.send_context(self.view.get_untracked());
        let ws = context.workspace.clone();
        let mut options = self.send_options();
        options.insert("wait_secs".into(), json!(20));
        self.spawn(async move {
            let head = if text.is_none() {
                "重试这一轮"
            } else {
                "编辑并重试"
            };
            let tail = if later > 0 {
                format!("及之后的 {later} 轮")
            } else {
                String::new()
            };
            let body = format!(
                "· 文件会还原到这条消息之前（当前状态会先备份）\n· 这条消息{tail}会移出上下文\n· 不是 Git 仓库的目录没有检查点，只会重开对话"
            );
            if dialog::ask(
                &format!("{head}？"),
                &body,
                vec![Choice::plain("取消"), Choice::danger("重试")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if !self.context_is_current(&context) {
                toast("对话已切换，没有重新发送");
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/workspaces/{ws}/retry"),
                &json!({ "session_id": sid, "text": text, "options": options }),
            )
            .await
            {
                Ok(r) if r["admitted"] == json!(false) => {
                    toast(r["reason"].as_str().unwrap_or("工作区正忙着别的操作").to_owned())
                }
                Ok(r) => {
                    if r["session_id"].is_string() {
                        self.note_sent_in(
                            &context,
                            r["session_id"].as_str().map(str::to_owned),
                            r["thread_id"].as_str().map(str::to_owned),
                        );
                    }
                    let restored = r["files_restored"].as_bool() == Some(true);
                    if restored {
                        self.git.changed.update(|n| *n += 1);
                    }
                    toast(if restored {
                        "文件已还原，从这里继续。"
                    } else {
                        "从这里继续；没有检查点，文件未变。"
                    });
                }
                Err(e) => {
                    toast(format!("没能重试：{e}"));
                    if self.context_is_current(&context) {
                        self.load_history();
                    }
                }
            }
        });
    }

    pub fn rewind(self, cp: String, undo: bool) {
        self.spawn(async move {
            if !undo
                && dialog::ask(
                    "回退代码",
                    "把文件还原到这条消息之前？\n对话会保留，当前状态会先备份，可以撤销。",
                    vec![Choice::plain("取消"), Choice::plain("回退代码")],
                )
                .await
                    != Some(1)
            {
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/checkpoints/{cp}/restore"),
                &json!({}),
            )
            .await
            {
                Ok(r) => {
                    self.git.changed.update(|n| *n += 1);
                    if undo {
                        toast("已撤销回退");
                    } else if let Some(u) = r["undo"].as_str() {
                        let u = u.to_owned();
                        if dialog::ask(
                            "已回退",
                            "文件已回到这条消息之前。要撤销这次回退吗？",
                            vec![Choice::plain("保留"), Choice::plain("撤销")],
                        )
                        .await
                            == Some(1)
                        {
                            self.rewind(u, true);
                        }
                    } else {
                        toast("已回退");
                    }
                }
                Err(e) => toast(format!("没能回退：{e}")),
            }
        });
    }
}
