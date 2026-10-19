use serde_json::{Value, json};

pub type HttpCall = (&'static str, String, Option<Value>);

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub build: fn(&Value) -> anyhow::Result<HttpCall>,
}

fn s(t: &str, desc: &str) -> Value {
    json!({ "type": t, "description": desc })
}
fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}
fn arg<'a>(v: &'a Value, k: &str) -> anyhow::Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("缺少必填参数 {k}"))
}
fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

const WAIT_CAP_SECS: u64 = 50;

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "list_workspaces",
        description: "列出所有工作区（跨机器），含活动状态、所在机器、项目归属。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/state".into(), None)),
    },
    Tool {
        name: "list_nodes",
        description: "列出 mesh 里的所有机器，含在线状态与链路延迟。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/state".into(), None)),
    },
    Tool {
        name: "discover_agents",
        description: "发现某台机器上装了哪些 AI coding CLI，含版本与登录态。",
        schema: || obj(json!({ "node": s("string", "机器名") }), &["node"]),
        build: |v| {
            Ok((
                "GET",
                format!("/api/nodes/{}/agents", enc(arg(v, "node")?)),
                None,
            ))
        },
    },
    Tool {
        name: "probe_node",
        description: "采集一台机器的 CPU/内存/磁盘/GPU 与 AI API 出口可达性。调度前必须先体检。",
        schema: || obj(json!({ "node": s("string", "机器名") }), &["node"]),
        build: |v| {
            Ok((
                "POST",
                format!("/api/nodes/{}/probe", enc(arg(v, "node")?)),
                None,
            ))
        },
    },
    Tool {
        name: "create_workspace",
        description: "在某台机器的某个目录上建一个工作区。",
        schema: || {
            obj(
                json!({
                    "node": s("string", "机器名，本机用 local"),
                    "path": s("string", "该机器上的绝对路径"),
                    "name": s("string", "显示名，留空取目录名"),
                    "project": s("string", "项目名；同名项目下的多个工作区即多机副本"),
                }),
                &["node", "path"],
            )
        },
        build: |v| Ok(("POST", "/api/workspaces".into(), Some(v.clone()))),
    },
    Tool {
        name: "read_file",
        description: "读取工作区里的一个文件。路径相对工作区根目录。",
        schema: || {
            obj(
                json!({ "workspace_id": s("string", "工作区 id"), "path": s("string", "相对路径") }),
                &["workspace_id", "path"],
            )
        },
        build: |v| {
            Ok((
                "GET",
                format!(
                    "/api/workspaces/{}/file?path={}",
                    enc(arg(v, "workspace_id")?),
                    enc(arg(v, "path")?)
                ),
                None,
            ))
        },
    },
    Tool {
        name: "list_files",
        description: "列出工作区的文件树，标注哪些文件相对基线有改动。",
        schema: || {
            obj(
                json!({ "workspace_id": s("string", "工作区 id") }),
                &["workspace_id"],
            )
        },
        build: |v| {
            Ok((
                "GET",
                format!("/api/workspaces/{}/tree", enc(arg(v, "workspace_id")?)),
                None,
            ))
        },
    },
    Tool {
        name: "search_code",
        description: "跨所有机器并行全文搜索，只回传匹配行。",
        schema: || obj(json!({ "query": s("string", "搜索词") }), &["query"]),
        build: |v| {
            Ok((
                "GET",
                format!("/api/search?q={}", enc(arg(v, "query")?)),
                None,
            ))
        },
    },
    Tool {
        name: "get_diff",
        description: "看工作区相对基线的改动。",
        schema: || {
            obj(
                json!({ "workspace_id": s("string", "工作区 id") }),
                &["workspace_id"],
            )
        },
        build: |v| {
            Ok((
                "GET",
                format!("/api/workspaces/{}/diff", enc(arg(v, "workspace_id")?)),
                None,
            ))
        },
    },
    Tool {
        name: "copy_files",
        description: "把一个工作区里的文件或文件夹拷到另一个工作区，两个工作区可以在不同机器上，数据经本机中转。文件夹按 git 规则跳过被忽略的内容；单次合计不超过 32MB。",
        schema: || {
            obj(
                json!({
                    "from_workspace_id": s("string", "来源工作区 id"),
                    "paths": { "type": "array", "items": { "type": "string" },
                        "description": "来源工作区里的相对路径，文件或文件夹都行" },
                    "to_workspace_id": s("string", "目标工作区 id"),
                    "dest": s("string", "目标工作区里的相对目录，留空放到根目录；每一项都会放进这个目录下同名的位置"),
                }),
                &["from_workspace_id", "paths", "to_workspace_id"],
            )
        },
        build: |v| {
            let body = json!({
                "to": arg(v, "to_workspace_id")?,
                "paths": v.get("paths").cloned().unwrap_or(json!([])),
                "dest": v.get("dest").and_then(Value::as_str).unwrap_or(""),
            });
            Ok((
                "POST",
                format!("/api/workspaces/{}/copy", enc(arg(v, "from_workspace_id")?)),
                Some(body),
            ))
        },
    },
    Tool {
        name: "send_prompt",
        description: "在某个工作区里让 agent 干活。会话在后台跑，此调用立即返回。",
        schema: || {
            obj(
                json!({
                    "workspace_id": s("string", "工作区 id"),
                    "text": s("string", "给 agent 的指令"),
                    "agent": s("string", "claude / codex 等；默认 claude"),
                    "resume": { "type": "boolean", "description": "是否续接上一轮上下文" },
                    "permission_mode": s("string", "acceptEdits / default / bypassPermissions / plan"),
                    "wait_secs": { "type": "integer",
                        "description": "等 agent 真的开始干活的秒数，最多 50。送达不等于开工 —— 建议填 15，否则 agent 起不来时你会以为成功了" },
                }),
                &["workspace_id", "text"],
            )
        },
        build: |v| {
            let mut body = v.clone();
            body.as_object_mut().map(|m| m.remove("workspace_id"));
            if let Some(w) = body.get("wait_secs").and_then(Value::as_u64) {
                body["wait_secs"] = json!(w.min(WAIT_CAP_SECS));
            }
            Ok((
                "POST",
                format!("/api/workspaces/{}/prompt", enc(arg(v, "workspace_id")?)),
                Some(body),
            ))
        },
    },
    Tool {
        name: "get_history",
        description: "读取工作区的对话与执行记录。默认只给最近 200 条；带 after 从某条之后往后翻，mode=summary 只看消息和工具名。",
        schema: || {
            obj(
                json!({
                    "workspace_id": s("string", "工作区 id"),
                    "session": s("string", "只看这个对话（thread id）"),
                    "after": s("string", "上一页最后一条的 cursor，从它之后开始"),
                    "limit": s("integer", "最多几条，默认 200，上限 5000"),
                    "mode": s("string", "summary（只要消息、工具名和审批）或 full"),
                }),
                &["workspace_id"],
            )
        },
        build: |v| {
            let mut qs = Vec::new();
            for k in ["session", "after", "mode"] {
                if let Some(x) = v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    qs.push(format!("{k}={}", enc(x)));
                }
            }
            qs.push(format!(
                "limit={}",
                v.get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(200)
                    .clamp(1, 5000)
            ));
            Ok((
                "GET",
                format!(
                    "/api/workspaces/{}/history?{}",
                    enc(arg(v, "workspace_id")?),
                    qs.join("&")
                ),
                None,
            ))
        },
    },
    Tool {
        name: "list_bots",
        description: "列出 hub 里的常驻 bot：handle、职责、状态、认领摘要、待处理邮件数。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/bots".into(), None)),
    },
    Tool {
        name: "spawn_bot",
        description: "临时拉一个 bot 来干一件事：给一句职责和第一条指令，它会在 hub 上醒来处理，闲置超过 idle_ttl_secs（默认 1 天）自动归档；干得久了可以转成固定 bot。总控专用。",
        schema: || {
            obj(
                json!({
                    "duty": s("string", "它负责什么，一句话到几行都行"),
                    "text": s("string", "第一条指令"),
                    "handle": s("string", "想固定一个名字就填，留空自动生成 tmp-xxxx"),
                    "runtime": s("string", "claude（默认）/ codex"),
                    "idle_ttl_secs": s("integer", "闲置多少秒后归档，默认 86400"),
                    "priority": s("string", "P0 / P1（默认）/ P2"),
                }),
                &["duty", "text"],
            )
        },
        build: |v| {
            arg(v, "duty")?;
            arg(v, "text")?;
            Ok(("POST", "/api/bots/spawn".into(), Some(v.clone())))
        },
    },
    Tool {
        name: "bot_status",
        description: "看一个 bot 的现状：状态、当前线程、最后一条指令、最近几步操作、待处理邮件、认领的资源。",
        schema: || {
            obj(
                json!({ "bot": s("string", "bot 的 handle 或 id") }),
                &["bot"],
            )
        },
        build: |v| {
            Ok((
                "GET",
                format!("/api/bots/{}/status", enc(arg(v, "bot")?)),
                None,
            ))
        },
    },
    Tool {
        name: "fleet_status",
        description: "一次拿到所有 bot 的现状，加上机器在线情况、账号额度、最近告警、失联的事实。总控看全局用这个。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/fleet/status".into(), None)),
    },
    Tool {
        name: "send_message",
        description: "给另一个 bot 发一封邮件。对方空闲时会被唤醒处理；urgent 会打断它正在跑的一轮。返回邮件 id，可用 wait_reply 等回复。",
        schema: || {
            obj(
                json!({
                    "to": s("string", "对方 bot 的 handle"),
                    "body": s("string", "正文"),
                    "subject": s("string", "主题，可省"),
                    "thread": s("string", "投到对方的哪个线程（bot_threads id），省略投到 main"),
                    "priority": s("string", "normal（默认）或 urgent"),
                    "expires_in_secs": s("integer", "多少秒内没处理就作废"),
                }),
                &["to", "body"],
            )
        },
        build: |v| {
            let mut body = v.clone();
            body.as_object_mut().map(|m| m.remove("to"));
            Ok((
                "POST",
                format!("/api/bots/{}/mail", enc(arg(v, "to")?)),
                Some(body),
            ))
        },
    },
    Tool {
        name: "reply",
        description: "回复一封收到的邮件。回复自动投到发件人的同一线程；发件人是用户时进用户的收件箱。",
        schema: || {
            obj(
                json!({ "mail_id": s("string", "邮件 id"), "body": s("string", "回复正文") }),
                &["mail_id", "body"],
            )
        },
        build: |v| {
            Ok((
                "POST",
                format!("/api/mail/{}/reply", enc(arg(v, "mail_id")?)),
                Some(json!({ "body": arg(v, "body")? })),
            ))
        },
    },
    Tool {
        name: "wait_reply",
        description: "等一封邮件得到回复，或者对方处理完它。每次最多等 50 秒（调用方一分钟就会断开），没等到就返回 timed_out，再调一次接着等。",
        schema: || {
            obj(
                json!({
                    "mail_id": s("string", "邮件 id"),
                    "timeout_secs": s("integer", "这次最多等多少秒，默认也是上限 50"),
                }),
                &["mail_id"],
            )
        },
        build: |v| {
            let t = v
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(WAIT_CAP_SECS)
                .clamp(1, WAIT_CAP_SECS);
            Ok((
                "GET",
                format!("/api/mail/{}/wait?timeout={t}", enc(arg(v, "mail_id")?)),
                None,
            ))
        },
    },
    Tool {
        name: "facts_get",
        description: "读一条事实（含状态 ok/stale/lost 和负责人）。",
        schema: || {
            obj(
                json!({ "key": s("string", "如 run/q01"), "history": { "type": "boolean", "description": "带上历史值" } }),
                &["key"],
            )
        },
        build: |v| {
            let h = v.get("history").and_then(Value::as_bool).unwrap_or(false);
            Ok((
                "GET",
                format!("/api/facts/{}?history={h}", enc(arg(v, "key")?)),
                None,
            ))
        },
    },
    Tool {
        name: "facts_list",
        description: "按前缀列出事实，比如 run/ 或 gpu/gpu-1/。",
        schema: || obj(json!({ "prefix": s("string", "key 前缀，留空全部") }), &[]),
        build: |v| {
            let p = v.get("prefix").and_then(Value::as_str).unwrap_or("");
            Ok(("GET", format!("/api/facts?prefix={}", enc(p)), None))
        },
    },
    Tool {
        name: "facts_put",
        description: "写一条你负责的事实。给了 ttl_secs 就要按这个间隔更新，超过 2 倍标 stale、5 倍标 lost 并告警。别人负责的事实写不了。",
        schema: || {
            obj(
                json!({
                    "key": s("string", "如 run/q01、gpu/gpu-1/0、node/robot-1/online"),
                    "value": { "description": "任意 JSON" },
                    "kind": s("string", "run / gpu / node / dataset / custom，省略按 key 前缀猜"),
                    "ttl_secs": s("integer", "心跳间隔秒数"),
                }),
                &["key", "value"],
            )
        },
        build: |v| {
            let mut body = v.clone();
            body.as_object_mut().map(|m| m.remove("key"));
            Ok((
                "PUT",
                format!("/api/facts/{}", enc(arg(v, "key")?)),
                Some(body),
            ))
        },
    },
    Tool {
        name: "claim",
        description: "认领一台机器上的资源：gpu:0,1 / dir:/data/queue / repo / port:8766 / device:can1。和别人独占的范围重叠会被拒，并告诉你找谁。",
        schema: || {
            obj(
                json!({
                    "scope": s("string", "范围"),
                    "node": s("string", "机器名；给了 workspace_id 就不用"),
                    "workspace_id": s("string", "认领整个工作区时填，会给你挂一个对应的远程手"),
                    "mode": s("string", "exclusive（默认）/ shared / read"),
                    "note": s("string", "用途"),
                    "expires_in_secs": s("integer", "多久后自动过期"),
                }),
                &["scope"],
            )
        },
        build: |v| Ok(("POST", "/api/claims".into(), Some(v.clone()))),
    },
    Tool {
        name: "release",
        description: "释放一条自己的认领。",
        schema: || obj(json!({ "claim_id": s("string", "认领 id") }), &["claim_id"]),
        build: |v| {
            Ok((
                "DELETE",
                format!("/api/claims/{}", enc(arg(v, "claim_id")?)),
                None,
            ))
        },
    },
    Tool {
        name: "claims",
        description: "列出认领：默认全部，给了 bot 只看它的。",
        schema: || obj(json!({ "bot": s("string", "handle，留空看全部") }), &[]),
        build: |v| {
            let b = v.get("bot").and_then(Value::as_str).unwrap_or("");
            Ok(("GET", format!("/api/claims?bot={}", enc(b)), None))
        },
    },
    Tool {
        name: "memory_list",
        description: "列出共享 memory 和你自己的 memory 的索引（名字和描述）。",
        schema: || {
            obj(
                json!({ "scope": s("string", "shared 或 bot:<id>，留空看你能看的全部") }),
                &[],
            )
        },
        build: |v| {
            let sc = v.get("scope").and_then(Value::as_str).unwrap_or("");
            Ok(("GET", format!("/api/memories?scope={}", enc(sc)), None))
        },
    },
    Tool {
        name: "memory_read",
        description: "读一条 memory 的正文。",
        schema: || {
            obj(
                json!({ "memory": s("string", "memory 的 id 或名字") }),
                &["memory"],
            )
        },
        build: |v| {
            Ok((
                "GET",
                format!("/api/memories/{}", enc(arg(v, "memory")?)),
                None,
            ))
        },
    },
    Tool {
        name: "memory_write",
        description: "写一条 memory。同名或描述很接近的已有条目会被拦下并返回它；要更新就带上它的 version。",
        schema: || {
            obj(
                json!({
                    "name": s("string", "kebab-case 名字"),
                    "description": s("string", "一句话描述"),
                    "body": s("string", "正文"),
                    "type": s("string", "user / feedback / project / reference"),
                    "scope": s("string", "shared（默认）或 bot:<你的 id>"),
                    "version": s("integer", "更新已有条目时带上当前版本号"),
                }),
                &["name", "description", "body"],
            )
        },
        build: |v| Ok(("POST", "/api/memories".into(), Some(v.clone()))),
    },
    Tool {
        name: "list_tasks",
        description: "列出看板上的任务，可按工作区、状态、关键词筛选。状态：backlog/todo/in_progress/in_review/done/cancelled。",
        schema: || {
            obj(
                json!({ "workspace_id": s("string", "只看这个工作区的"), "status": s("string", "只看这个状态的"),
                        "q": s("string", "标题 / 描述 / 编号里的关键词") }),
                &[],
            )
        },
        build: |v| {
            let mut qs = Vec::new();
            for (k, p) in [
                ("workspace_id", "workspace"),
                ("status", "status"),
                ("q", "q"),
            ] {
                if let Some(x) = v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    qs.push(format!("{p}={}", enc(x)));
                }
            }
            let tail = if qs.is_empty() {
                String::new()
            } else {
                format!("?{}", qs.join("&"))
            };
            Ok(("GET", format!("/api/tasks{tail}"), None))
        },
    },
    Tool {
        name: "get_task",
        description: "取一个任务的详情：描述、评论、执行日志（每次运行）、子任务。",
        schema: || obj(json!({ "task_id": s("string", "任务 id") }), &["task_id"]),
        build: |v| {
            Ok((
                "GET",
                format!("/api/tasks/{}", enc(arg(v, "task_id")?)),
                None,
            ))
        },
    },
    Tool {
        name: "create_task",
        description: "在看板上建一个任务。给了 workspace_id 和智能体（agent_profile）或运行时（runtime）才能开始。",
        schema: || {
            obj(
                json!({ "title": s("string", "标题"), "description": s("string", "要做什么，越具体越好"),
                        "priority": s("string", "urgent/high/medium/low/none"),
                        "workspace_id": s("string", "在哪个工作区做"),
                        "agent_profile": s("string", "指派给哪个智能体（id）"),
                        "runtime": s("string", "或直接用运行时：claude / codex / …"),
                        "parent_id": s("string", "作为哪个任务的子任务") }),
                &["title"],
            )
        },
        build: |v| {
            arg(v, "title")?;
            Ok(("POST", "/api/tasks".into(), Some(v.clone())))
        },
    },
    Tool {
        name: "update_task",
        description: "改任务的标题、描述、状态、优先级、工作区或指派。只改给了的字段。",
        schema: || {
            obj(
                json!({ "task_id": s("string", "任务 id"), "title": s("string", ""), "description": s("string", ""),
                        "status": s("string", "backlog/todo/in_progress/in_review/done/cancelled"),
                        "priority": s("string", "urgent/high/medium/low/none"),
                        "workspace_id": s("string", ""), "agent_profile": s("string", ""), "runtime": s("string", "") }),
                &["task_id"],
            )
        },
        build: |v| {
            let id = enc(arg(v, "task_id")?);
            let mut body = v.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("task_id");
            }
            Ok(("PUT", format!("/api/tasks/{id}"), Some(body)))
        },
    },
    Tool {
        name: "start_task",
        description: "开始（或继续）一个任务：在它的工作区里让指派的智能体开工。第一次用标题 + 描述开新对话，之后续接同一段对话。",
        schema: || {
            obj(
                json!({ "task_id": s("string", "任务 id"), "text": s("string", "附加说明；不给就用标题 + 描述") }),
                &["task_id"],
            )
        },
        build: |v| {
            let id = enc(arg(v, "task_id")?);
            let body = json!({ "text": v.get("text").cloned().unwrap_or(Value::Null) });
            Ok(("POST", format!("/api/tasks/{id}/start"), Some(body)))
        },
    },
    Tool {
        name: "wait_workspace",
        description: "等一个工作区停下来：跑完（idle），或者停下来等人裁决（blocked），返回最后一句回复。每次最多等 50 秒（调用方一分钟就会断开），还没停下来就返回 reached=false、timed_out=true 和当前状态，再调一次接着等。很久没动静会以 agent_prompt_stalled 失败。",
        schema: || {
            obj(
                json!({
                    "workspace_id": s("string", "工作区 id"),
                    "until": s("string", "idle（默认，跑完为止）或 blocked（跑完或等人裁决，先到哪个算哪个）"),
                    "timeout_secs": s("integer", "这次最多等多少秒，默认也是上限 50"),
                }),
                &["workspace_id"],
            )
        },
        build: |v| {
            let id = enc(arg(v, "workspace_id")?);
            let until = v
                .get("until")
                .and_then(Value::as_str)
                .filter(|u| *u == "blocked")
                .unwrap_or("idle");
            let timeout = v
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(WAIT_CAP_SECS)
                .clamp(1, WAIT_CAP_SECS);
            Ok((
                "GET",
                format!("/api/workspaces/{id}/wait?until={until}&timeout={timeout}"),
                None,
            ))
        },
    },
    Tool {
        name: "list_autopilots",
        description: "列出自动化：触发方式、下一次运行时间、最近一次结果。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/autopilots".into(), None)),
    },
    Tool {
        name: "run_autopilot",
        description: "手动触发一次自动化（暂停着的也能跑）。",
        schema: || {
            obj(
                json!({ "autopilot_id": s("string", "自动化 id") }),
                &["autopilot_id"],
            )
        },
        build: |v| {
            let id = enc(arg(v, "autopilot_id")?);
            Ok(("POST", format!("/api/autopilots/{id}/run"), None))
        },
    },
    Tool {
        name: "list_inbox",
        description: "收件箱里没读的：跑完 / 失败、等人裁决、agent 在提问、自动化被暂停、额度告警。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/inbox?filter=unread".into(), None)),
    },
    Tool {
        name: "get_analytics",
        description: "一段时间里的运行统计：按天趋势、成功率、费用、各智能体排行、失败原因。",
        schema: || {
            obj(
                json!({ "days": s("integer", "统计最近多少天，默认 30") }),
                &[],
            )
        },
        build: |v| {
            let days = v
                .get("days")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 365);
            Ok(("GET", format!("/api/analytics?days={days}"), None))
        },
    },
    Tool {
        name: "get_usage",
        description: "token 消耗与费用统计，按 agent、机器、工作区分组。",
        schema: || obj(json!({}), &[]),
        build: |_| Ok(("GET", "/api/usage".into(), None)),
    },
];

#[must_use]
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

#[must_use]
pub fn manifest() -> Value {
    json!({
        "tools": TOOLS.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "inputSchema": (t.schema)(),
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_unique_name_and_object_schema() {
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "工具名重复: {}", t.name);
            let sc = (t.schema)();
            assert_eq!(sc["type"], "object", "{} 的 schema 必须是 object", t.name);
            assert!(!t.description.is_empty());
        }
    }

    #[test]
    fn missing_required_arg_is_rejected() {
        let t = find("read_file").unwrap();
        assert!(
            (t.build)(&json!({ "workspace_id": "w1" })).is_err(),
            "缺 path 应报错"
        );
        assert!((t.build)(&json!({})).is_err());
    }

    #[test]
    fn path_params_are_percent_encoded() {
        let t = find("discover_agents").unwrap();
        let (_, url, _) = (t.build)(&json!({ "node": "gpu 机/1" })).unwrap();
        assert!(!url.contains(' '), "空格必须转义: {url}");

        assert_eq!(url, "/api/nodes/gpu%20%E6%9C%BA%2F1/agents");

        assert_eq!(url.matches('/').count(), 4, "只应有结构性分隔符: {url}");
    }

    #[test]
    fn send_prompt_moves_id_to_path_not_body() {
        let t = find("send_prompt").unwrap();
        let (m, url, body) = (t.build)(&json!({
            "workspace_id": "w1", "text": "hi", "agent": "codex"
        }))
        .unwrap();
        assert_eq!(m, "POST");
        assert!(url.contains("/w1/prompt"));
        let b = body.unwrap();
        assert!(
            b.get("workspace_id").is_none(),
            "id 已在路径里，不该再出现在请求体"
        );
        assert_eq!(b["agent"], "codex");
    }

    #[test]
    fn waits_answer_before_the_caller_gives_up() {
        for (name, args) in [
            (
                "wait_workspace",
                json!({ "workspace_id": "w1", "timeout_secs": 600 }),
            ),
            ("wait_workspace", json!({ "workspace_id": "w1" })),
            (
                "wait_reply",
                json!({ "mail_id": "m1", "timeout_secs": 3600 }),
            ),
            ("wait_reply", json!({ "mail_id": "m1" })),
        ] {
            let (_, url, _) = (find(name).unwrap().build)(&args).unwrap();
            assert!(url.ends_with(&format!("timeout={WAIT_CAP_SECS}")), "{url}");
        }
        let (_, url, _) = (find("wait_workspace").unwrap().build)(&json!({
            "workspace_id": "w1", "timeout_secs": 5
        }))
        .unwrap();
        assert!(url.ends_with("timeout=5"), "{url}");
        let (_, _, body) = (find("send_prompt").unwrap().build)(&json!({
            "workspace_id": "w1", "text": "hi", "wait_secs": 300
        }))
        .unwrap();
        assert_eq!(body.unwrap()["wait_secs"], WAIT_CAP_SECS);
    }

    #[test]
    fn manifest_is_wellformed() {
        let m = manifest();
        assert_eq!(m["tools"].as_array().unwrap().len(), TOOLS.len());
        assert!(m["tools"][0]["inputSchema"].is_object());
    }
}
