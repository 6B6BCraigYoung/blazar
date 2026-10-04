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
            toast("当前对话没有正在运行的任务");
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
                Ok(r) => toast(r["reason"].as_str().unwrap_or("未能停止").to_owned()),
                Err(e) => toast(format!("停止失败：{e}")),
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
                    toast(format!("已为当前轮次调整 {what}"))
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
                        .unwrap_or("决定未送达，请重试")
                        .to_owned(),
                ),
                Err(error) => toast(format!("发送失败，输入已保留：{error}")),
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
                &format!(
                    "在当前工作区始终允许 \"{what}\"，不再询问？\n\n可在「设置 → 自动审批」中关闭。"
                ),
                vec![Choice::plain("取消"), Choice::plain("始终允许")],
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
                toast("这条消息含有不能在此编辑的设置，已保留在队列中");
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
                        let r = api::send::<Value>("POST", &format!("{base}/send"), &json!({})).await?;
                        self.note_sent_in(&context, r["session_id"].as_str().map(str::to_owned), r["thread_id"].as_str().map(str::to_owned));
                    }
                    _ => {
                        let Some(sid) = steer_session else {
                            toast("当前没有可补充消息的运行中会话");
                            return Ok(());
                        };
                        let Some(request) = q.request.as_ref() else {
                            toast("这条消息的设置无法用于插话，已保留在队列中");
                            return Ok(());
                        };
                        if dialog::ask("发送插话？", "附件和文件上下文会一并发送；模型、账号和权限沿用当前轮次，原排队消息的启动设置不会应用。", vec![Choice::plain("取消"), Choice::plain("发送插话")]).await != Some(1) {
                            return Ok(());
                        }
                        if !self.context_is_current(&context) {
                            toast("对话已切换，消息仍保留在原队列中");
                            return Ok(());
                        }
                        let i = api::send::<Value>("POST", &format!("/api/sessions/{sid}/input"), &steer_request(request)).await?;
                        if i["accepted"].as_bool() != Some(true) {
                            toast(i["reason"].as_str().unwrap_or("当前运行时不支持中途补充，将在本轮结束后发送。").to_owned());
                            return Ok(());
                        }
                        api::send::<Value>("DELETE", &base, &json!({"expected": request})).await?;
                        toast("已补充到当前轮次");
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
            toast("请先停止当前运行，再重试。");
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
                "修改消息后重试"
            };
            let tail = if later > 0 {
                format!("及之后的 {later} 轮对话")
            } else {
                String::new()
            };
            let body = format!(
                "· 将文件还原到这条消息之前，先备份当前状态\n· 这条消息{tail}将移出上下文\n· 非 Git 目录没有检查点，仅重新开始对话"
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
                toast("对话已切换，未重新发送");
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
                    toast(r["reason"].as_str().unwrap_or("工作区正在处理其他操作").to_owned())
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
                    toast(format!("重试失败：{e}"));
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
                    "还原文件",
                    "将文件还原到这条消息之前？\n对话记录保留；先备份当前状态，可撤销还原。",
                    vec![Choice::plain("取消"), Choice::plain("还原文件")],
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
                        toast("已撤销还原");
                    } else if let Some(u) = r["undo"].as_str() {
                        let u = u.to_owned();
                        if dialog::ask(
                            "已还原",
                            "文件已回到这条消息之前。要撤销还原吗？",
                            vec![Choice::plain("保留"), Choice::plain("撤销")],
                        )
                        .await
                            == Some(1)
                        {
                            self.rewind(u, true);
                        }
                    } else {
                        toast("已还原");
                    }
                }
                Err(e) => toast(format!("还原失败：{e}")),
            }
        });
    }
}
