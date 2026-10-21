# 设计稿：常驻 bot、远程 hub 与飞书入口

状态：草案 v2，待评审。v1 是概念稿；v2 在通读 hub、runtime、transport、mcp、db、web 之后重写，每个设计点都标了它依赖或要改的现有代码。

实施状态（2026-10-13）：bot 分临时 / 固定两种（迁移 0033：`lifetime`、`idle_ttl_secs`）：只给一句职责就能建，档案自动生成为 `bot:<handle>`；临时 bot 闲置到期自动归档、可恢复、可转固定；总控用 `spawn_bot` 临时拉人。P0 的 G10/G11/G12 与部署脚本（`scripts/hub-service.sh`）、P1 的 bot / 线程 / 收件箱 / 调度器 / 多个手 / 调用方身份与范围 / `send_message` `reply` `wait_reply` `fleet_status` `bot_status` `list_bots`、P3 的事实层 / 认领 / 共享 memory（含上报令牌）、热 bot、`get_history` 分页、bot 页面与侧栏已经做完，迁移 0028–0032。还没做：P2 飞书入口、远程手的 ReadImage / Spawn 与后台任务表、规则引擎里除 fact_stale / claim_expired 以外的规则、越界写转审批、P5 路由与交接摘要、P6 远程模式、P7。

## 0. 摘要

- **bot** 是 hub 里的一条持久身份：职责、记忆、收件箱、认领的资源、对话线程。进程只在有事时拉起，干完就退。
- **hub 常驻在一台常在线主机上**（下文称 hub-host），笔记本和飞书都只是客户端。
- **bot 默认在 hub-host 上思考，用 Blazar 现有的“远程手”去操作工作机器**，也就是 `brain=local` 加 remote-hands MCP。这是 Blazar 已经成熟的路径，续接对话、skills、账号、飞书工具都能直接用。少数重 I/O 的 bot 可以设成在工作机器上思考。
- **飞书是主入口**：一个飞书机器人对应总控 bot，消息进来后由 hub 路由、唤醒 bot，产出以卡片回到飞书。
- **事实层、资源认领、规则监控**都放在 hub，状态只存一份；监控按规则跑、不花模型额度，只有异常才唤醒总控。
- **多订阅**：bot 是角色，运行时可以换；按优先级和额度路由，限流时降级，并做交叉复核。

## 1. 背景与验收用例

这份设计来自一次真实的多会话协作：一台笔记本上开了五个 Claude Code 终端，一个当总控，四个分别负责训练、仿真评测、真机、看板。下面这些场景就是验收用例，第 13 节逐条对应：

| # | 场景 | 现在怎么做 | 改造后应该怎么做 |
| --- | --- | --- | --- |
| U1 | 看所有 bot 现状 | 总控用脚本解析别的会话的记录文件 | `fleet_status` 一次拿到每个 bot 的状态、最后指令、最近操作、认领、后台任务 |
| U2 | 发现状态残影 | 人眼发现看板多一条“其他训练” | 事实层心跳超时，规则自动标“失联”并告警 |
| U3 | 派活修复并核实 | 本机会话互发消息，再 ssh 查数据库 | 经 hub 发消息（带发件人），核实时直接查事实层 |
| U4 | 分工与改名 | 逐个终端 `/rename`，各自写 memory，还写重了 | 在 hub 里建 bot 档案；memory 由 hub 统一管理、去重 |
| U5 | 清理遗留 | 手工找出跑了 3 天的轮询和 3 GB 中转文件 | 后台任务登记，规则发现“等待目标已不存在” |
| U6 | 删别人的文件 | bot 坚持要用户亲口确认 | 越过认领边界的写操作必须在飞书里确认 |
| U7 | 用户在手机上派活 | 做不到 | 飞书私聊或话题直接派活、看进度、审批 |
| U8 | 笔记本合盖 | 所有会话都停 | 不受影响 |
| U9 | 工作机器掉线 | bot 自己排查网络 | 规则发现节点离线，挂起相关任务并告警 |
| U10 | 额度紧张 | 用户手动切账号 | 按优先级自动路由，限流时降级并通知 |

## 2. 现状盘点

### 2.1 可以直接复用

| 能力 | 位置 |
| --- | --- |
| 分离运行：断网、hub 重启都不丢输出，可以重新接上、继续输入 | `transport/src/detached.rs`，`hub/src/agent/run.rs` `supervise`、`reattach_all` |
| 插话、中断、审批往返 | `run.rs` `interject`、`interrupt`、`deliver_saved_approval` |
| 远程手：hub 本机的 CLI 通过 SSH 在远端执行 Bash/Read/Write/Edit/Glob/Grep | `mcp/src/remote.rs`，`api/sessions.rs` `local_brain` |
| 续接对话（同机器、同运行时） | `sessions.rs` `--resume`、`--resume-session-at` |
| 智能体档案：职责、模型、权限、MCP、skills、账号 | `agent/agents.rs`，`sessions.rs` `apply_profile` |
| 账号额度与自动切换 | `agent/accounts.rs` `resolve`、`pick`、失败重派 |
| 凭据代理（远端经反向隧道访问模型 API） | `agent/proxy.rs`、`codex_proxy.rs` |
| fleet MCP：派活、等待、看记录、任务看板、收件箱 | `hub/src/fleet.rs`、`mcp/src/tools.rs` |
| 收件箱，并转发到飞书（只能往外发） | `work/inbox.rs`、`office/mod.rs` `forward` |
| 定时、webhook、手动触发的自动化 | `work/autopilot.rs` |
| 资源匹配与租约库（已写好但没人调用） | `crates/scheduler` |
| 远端 CLI 自动安装 | `agent/remote_cli.rs` |
| 组网管理 | `crates/netmesh`、`hub/src/mesh.rs` |

### 2.2 阻碍与缺陷

| 编号 | 问题 | 位置 | 本设计的处理 |
| --- | --- | --- | --- |
| G1 | 换机器或换运行时后只能带一份文字回顾（≤8000 字），工具记录丢失 | `sessions.rs:250-298`，`chat.rs:459-504` | bot 默认在 hub 上思考，机器固定，续接总能成功；换运行时走第 8 节的交接摘要 |
| G2 | 一个工作区同时只能跑一个会话 | `state.rs:94-116` | 每个 bot 有自己的家工作区；工作机器只通过远程手访问，不占准入 |
| G3 | 进程只活一轮 | `run.rs:379-392` | 保持现状，唤醒等于用 `--resume` 新起进程；只有被标记“热”的 bot 例外 |
| G4 | 在远端思考时 skills 送不到 | `skills/library.rs:714-751` | 默认在 hub 思考，不受影响；在远端思考的模式里同步 skills 目录（第 5.6 节） |
| G5 | 排队每个线程只能一条，失败后整个工作区被扣住 | `chat.rs:314-359` | bot 消息走新的收件箱表，不复用 `queued_messages` |
| G6 | 远端 Claude 只能用长期 token 账号 | `accounts.rs:391-410` | 在 hub 上思考时用 hub-host 的 CLI 登录账号，不受影响 |
| G7 | fleet 调用不带调用方身份，用的是 hub 主令牌，授权也不分范围 | `fleet.rs:255,328` | 授权绑定到 bot，把调用方带进工具处理；按 bot 限定工具和范围 |
| G8 | 没有 bot 之间的消息通道；`send_prompt` 没有发件人 | `tools.rs:183` | 新的 `send_message` / `reply` |
| G9 | `get_history` 不分页、不限大小 | `sessions.rs:910-947` | 加分页和摘要模式；`fleet_status` 给精简视图 |
| G10 | `probe_node` 会覆盖掉 `capabilities.$.agents`；体检脚本只支持 Linux；不读节点代理环境 | `api/profiles.rs:38`，`transport/lib.rs:362-410` | 合并写入；补齐 macOS；体检时带上 `node_net_env` |
| G11 | hub 的 SSH 不复用连接；`exec` 没有超时 | `state.rs:129-135`，`transport/lib.rs:479-494` | 统一开启复用；`exec` 带默认超时 |
| G12 | fleet 反向隧道没有健康检查 | `fleet.rs:97-169` | 加看门狗（对照 `proxy.rs:513-535`） |
| G13 | 资源调度库没人调用；`leases` 表在 0014 被删了 | `crates/scheduler`，`0014` | 新建 `claims` 表，接上调度库 |
| G14 | 收件箱只能由写死的调用产生；定时任务没有事件触发；hooks 只触发 2 种事件 | `inbox.rs`，`autopilot.rs`，`sessions.rs:396-414` | 新增规则引擎和事件触发 |
| G15 | hub 只能在本机用：令牌每次重启都重新生成；浏览器只能从本机拿 cookie；桌面端固定内嵌 hub | `auth.rs:36-75,158-175`，`apps/desktop/src/main.rs` | 第 4 节 |
| G16 | 飞书只能往外发：没有事件接收、没有会话映射、没有卡片回调 | `office/mod.rs:427,633-678` | 第 9 节 |
| G17 | 智能体的 `max_concurrent` 没有生效 | `agents.rs:166` | bot 调度里实现 |

## 3. 总体架构

```
 ┌───────────── 客户端 ───────────────────────────────────────────┐
 │ 飞书机器人 [新]           笔记本浏览器/桌面端 [改]     CLI/MCP [有] │
 │ 长连接 · 卡片 · 审批       连远程 hub（SSH 转发或令牌）            │
 └────┬───────────────────────────┬──────────────────────────────┘
      │ lark-cli event consume     │ HTTPS/WS + 令牌
      ▼                            ▼
 ┌──────────────── hub-host：blazar-hub（launchd 常驻）────────────────┐
 │ 飞书网关 [新] ─┐                                                    │
 │                ▼                                                    │
 │ bot 调度器 [新]：收件箱 → 选线程 → 选账号 → 拉起/插话 → 交付          │
 │   ├ bot 档案 [新] bots ← agents [有]                                 │
 │   ├ bot 收件箱 [新] bot_mail                                         │
 │   ├ 运行时路由 [改] accounts.resolve + 优先级 + 额度轮询              │
 │   └ 会话执行 [有] prompt_inner / detached / supervise                │
 │ 事实层 [新] facts            资源认领 [新] claims + scheduler [有]    │
 │ 规则引擎 [新] → 收件箱 [有] → 飞书 / 唤醒总控                         │
 │ 共享 memory [新]            后台任务登记 [新]                        │
 │ fleet MCP [改]：带调用方身份 · 按 bot 限定范围 · 新增工具               │
 └────┬────────────────────────────────────────────────────────────────┘
      │ 远程手（SSH 复用连接）[有]   凭据代理隧道 [有]   组网 [有]
      ▼
 gpu-1 / gpu-2 / robot-1 / web-1 / laptop（可能离线）
```

**bot 在哪思考**（`bots.brain`）：

| 模式 | CLI 跑在哪 | 优点 | 缺点 | 适用 |
| --- | --- | --- | --- | --- |
| `hub`（默认） | hub-host，cwd 是 bot 的家目录 | 续接总能成功；skills、飞书工具、CLI 登录账号都能用；工作机器掉线时 bot 仍在线，能汇报 | 每次工具调用都要过一次 SSH；远程 Bash 单次最长 600 s；看图要先拉回来 | 总控、训练、看板、真机 |
| `node` | 主工作区所在机器 | 本地 I/O 快，可以用原生工具和后台任务 | 续接受限于这台机器；要长期 token 账号或代理；要同步 skills | 大量读图、重 I/O 的仿真调试 |

> 修订说明：v1 推荐“在工作机器上拉起”。读代码后发现 Blazar 默认的 `brain=local` 加远程手已经很成熟，而且避开了 G1、G4、G6，所以 v2 改为默认在 hub 上思考。

## 4. hub 部署与远程访问

### 4.1 hub-host 选择条件

一直在线；能通过组网或公网端口直连所有工作机器；有稳定的模型 API 出口；放得下所有 bot 的 CLI 进程（每个约 0.3～0.5 GB 内存）。容器类机器会被回收，不能用。

### 4.2 部署

- 用 `blazar-hub` 独立程序，launchd 开机自启（Linux 上用 systemd），`BLAZAR_DB` 指向数据目录。
- `BLAZAR_BIND=127.0.0.1:7777`，**第一阶段不对外监听**。
- hub-host 上要准备：每个工作机器的 ssh 配置和授权密钥、claude 与 codex CLI 及登录、lark-cli 及登录、组网。

### 4.3 客户端怎么连

**第一阶段：SSH 本地转发，不改代码。**

笔记本执行 `ssh -N -L 7777:127.0.0.1:7777 hub-host`，浏览器打开 `http://localhost:7777`。对 hub 来说请求来自本机回环，可以直接拿 cookie（`auth.rs:158-161`）。掉线后 autossh 自动重连；页面按会话事件序号补齐（`events_since`）。

**第二阶段：桌面端加“远程 hub”模式。**

- 持久令牌：`auth.rs` 增加 `BLAZAR_HUB_TOKEN_FILE`，存在就读取、不再每次重启都重新生成；支持轮换。
- 浏览器拿 cookie：`POST /api/auth/session` 已经支持 Bearer。桌面端用令牌换 cookie 后再加载页面。
- 桌面端设置：内嵌 hub（现状）或远程 hub（URL + 令牌）。远程模式下不启动内嵌 hub，dock 角标改为订阅远程 WS。
- 如果要在组网 IP 上监听：`BLAZAR_ALLOWED_HOSTS` 加上组网 IP，cookie 加 `Secure` 并上 TLS。在那之前一律只用 SSH 转发。

**第三阶段：全局事件游标。** 低频的状态变化通知（工作区、节点、任务、自动化、账号、收件箱、bot、事实与认领、后台任务、会话改名）先写进 `hub_events`（自增 id）再广播，下发时带 `event_id`；流式条目、排队、界面驱动和 `ResyncRequired` 不入库。表保留 24 小时且最多 5000 条，由规则引擎的周期任务清理。页面在内存里记住最后一个 id，重连时用 `/api/ws?since=<id>` 先订阅再补发，交界处按 id 去重；同样内容的通知补发时只发最后一条。`since` 落在保留窗口之外、或早于 hub 本次启动时写下的标记，就只发 `ResyncRequired`，页面整体重取。对话流不在表里，重连后仍按会话序号重新接上。

### 4.4 “local”的含义

hub 搬到 hub-host 后，代码里所有的 `local` 都指 hub-host，包括 `brain=local`、hub 的 hooks、账号目录、lark-cli。笔记本变成一个普通节点（`role=personal`），可能离线。

### 4.5 迁移

在 hub-host 上新建库；从笔记本导出智能体档案、skills、MCP、自动化、审批规则再导入（新增 `blazar export/import`）；会话历史不迁。账号在 hub-host 上重新登录。

## 5. bot 模型

### 5.1 数据表（新迁移 `0028_bots.sql`）

```sql
CREATE TABLE bots (
    id            TEXT PRIMARY KEY,
    handle        TEXT NOT NULL UNIQUE,      -- bot-0、ops 等，消息寻址用
    display_name  TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'worker',  -- 'lead'（总控）| 'worker'
    agent_id      TEXT NOT NULL REFERENCES agents(id),  -- 职责、模型、权限、MCP、skills
    home_ws_id    TEXT NOT NULL REFERENCES workspaces(id), -- hub 上的家工作区
    brain         TEXT NOT NULL DEFAULT 'hub',     -- 'hub' | 'node'
    brain_ws_id   TEXT REFERENCES workspaces(id),  -- brain='node' 时在哪个工作区思考
    priority      TEXT NOT NULL DEFAULT 'P1',      -- P0 | P1 | P2
    runtimes      TEXT NOT NULL DEFAULT '[]',      -- 首选和备选，如 [{"runtime":"claude","account":"auto"},{"runtime":"codex"}]
    hot_until     TEXT,                            -- 在这个时间之前保持进程常开
    status        TEXT NOT NULL DEFAULT 'idle',    -- idle|running|awaiting|paused|lost
    status_note   TEXT NOT NULL DEFAULT '',
    last_active_at TEXT,
    archived_at   TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;

CREATE TABLE bot_threads (
    id            TEXT PRIMARY KEY,
    bot_id        TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    thread_id     TEXT,                  -- 对应 sessions.thread_id
    kind          TEXT NOT NULL,         -- 'main' | 'task' | 'feishu'
    title         TEXT NOT NULL DEFAULT '',
    task_id       TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    runtime       TEXT,                  -- 线程当前在哪个运行时上
    summary       TEXT NOT NULL DEFAULT '', -- 交接摘要（第 8.4 节）
    closed_at     TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;
```

- **家工作区**：每个 bot 在 hub-host 上有一个目录 `~/blazar-bots/<handle>`，注册成一个工作区。它解决 G2：bot 之间不互相占准入，每个 bot 自己同时只跑一个会话，跟 Claude 的心智一致。
- **线程**：每个 bot 有一个 `main` 线程处理日常消息；任务和飞书话题各开一个线程；线程映射到现有的 `sessions.thread_id`，续接沿用现有机制。

### 5.2 工作机器怎么接入（“手”）

bot 认领的每个工作区都挂一个远程手 MCP，按工作区命名：`hands_gpu1`、`hands_gpu2` 等。

- 现状：`sessions.rs:534-550` 只给一个 `--remote-node --remote-root`。
- 改法：`prompt_inner` 根据 bot 的认领生成多个远程手服务；Claude 的 `--tools` 收紧为 `Task,TodoWrite,WebFetch,WebSearch,Skill`，文件和命令操作一律通过远程手。
- 远程手要增强：
  - 读图：增加 `ReadImage`（返回 base64 图片块），bot 看仿真帧和相机图时用；
  - 长任务：增加 `Spawn` / `JobStatus` / `JobLog`，在远端用 `setsid nohup` 起任务，并登记到后台任务表（第 7.5 节）。不要把 600 s 的 Bash 当后台用；
  - 连接复用：已有（`remote.rs:29-59`），超时沿用。

### 5.3 bot 收件箱（新迁移 `0029_bot_mail.sql`）

```sql
CREATE TABLE bot_mail (
    id           TEXT PRIMARY KEY,
    to_bot       TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    thread_hint  TEXT,                   -- 指定 bot_threads.id；为空就投到 main
    from_kind    TEXT NOT NULL,          -- 'user' | 'bot' | 'feishu' | 'rule' | 'schedule' | 'system'
    from_ref     TEXT NOT NULL DEFAULT '', -- bot id、飞书 open_id、规则 id
    reply_to     TEXT REFERENCES bot_mail(id),
    subject      TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL,
    attachments  TEXT NOT NULL DEFAULT '[]', -- [{kind:image|file, path|blob_id}]
    priority     TEXT NOT NULL DEFAULT 'normal', -- urgent 可以打断正在跑的一轮
    status       TEXT NOT NULL DEFAULT 'queued', -- queued|delivered|done|failed|expired
    session_id   TEXT,                   -- 投进了哪个会话
    delivered_at TEXT,
    done_at      TEXT,
    expires_at   TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE INDEX idx_bot_mail_pending ON bot_mail(to_bot, status, created_at);
```

投递给 bot 的文本统一包一层头，让 bot 知道是谁发的、怎么回：

```
[来自 bot:trainer · 邮件 m_123 · 回复用 reply(m_123, …)]
……正文……
```

来自飞书和规则的消息按“不可信数据”处理，只有 `from_kind=user` 和本人飞书 open_id 发来的消息算用户指令（第 9.5 节）。

### 5.4 调度器（新模块 `hub/src/bots/dispatch.rs`）

由 `bot_mail` 插入、会话结束（`run.rs` 结束时的回调链）和每 5 秒一次的定时扫描驱动：

```
for bot in 有待投递邮件的 bot（按优先级、按最早邮件排序）:
    if bot.status == paused: 跳过
    live = bot 家工作区里正在跑的会话
    if live 且是交互式、没结束输入:
        if 邮件.priority == urgent 或 live 空闲（被后台任务撑着）:
            interject(live, 合并后的邮件文本)       # run.rs:668-739 / send_to_idle
            标记 delivered
        continue                                    # 否则等这一轮结束
    thread = 邮件.thread_hint ?? bot 的 main 线程
    plan   = 路由（第 8 节）选运行时和账号
    if plan.runtime != thread.runtime: 生成交接摘要（第 8.4 节）
    req = PromptRequest{ profile=bot.agent_id, resume=true, resume_session=thread,
                         brain, account, text=合并的邮件文本, hands=认领的工作区 }
    api::prompt(home_ws, req)                       # sessions.rs prompt_inner
    标记 delivered；会话结束时 → done 或 failed
```

- **合并**：同一线程里排着的多封邮件合成一次投递，按时间排序，各自带头部。
- **热 bot**：`hot_until` 没过期时，一轮结束后不结束输入，等下一封邮件直接插话。用在真机评测这类要秒级响应的阶段；要改 `run.rs:379-392`，结束输入前查一下 bot 是不是热的。
- **失败**：会话失败后邮件标成 `failed` 并发收件箱；不像 G5 那样扣住整个工作区。
- **并发**：同一个 bot 同时只跑一个会话（家工作区准入保证）；全局并发按 hub-host 的内存设上限（默认 6）。

### 5.5 bot 的职责说明怎么拼

启动时注入的说明 = 智能体档案里的职责说明 + 自动生成的上下文块：

1. 身份：handle、角色、优先级；
2. 认领的资源清单，以及对应的“手”的名字；
3. 同伴名单：handle、职责一句话、怎么找它；
4. 规则：别人的资源只读，跨界用 `send_message`，越界写要确认，看板文案不写会话名，这些可配置；
5. memory 索引：共享 memory 和 bot 自己 memory 的标题加一句话描述（第 7.3 节）；
6. 当前线程的交接摘要（如果有）。

档案职责说明的 8000 字上限（`agents.rs:108`）只管职责部分，上下文块单独计。

### 5.6 在工作机器上思考的模式

保留现有的 `brain=node` 路径，并补上：skills 同步（G4：把 `library::for_agent` 的产物推到 `~/.blazar/skills/<bot>/`，`--add-dir` 指向那里）；账号要用长期 token 或走代理（G6）；fleet 反向隧道加看门狗（G12）。工作机器离线时，调度器把这个 bot 临时切回在 hub 上思考，带上交接摘要，同时告警。

## 6. fleet MCP 改造

### 6.1 调用方身份与授权范围（修 G7）

- `FleetState::issue` 的授权从 `{session, node}` 扩展为 `{session, node, bot_id, scopes}`。
- `mcp_http`（`fleet.rs:255`）从请求头取出授权，把 `Caller{bot_id, session_id}` 带进 `call`。
- 下游请求不再一律用主令牌：用一个内部令牌，并带上 `X-Blazar-Caller` 头；hub 的处理函数按调用方做权限检查。
- `scopes` 来自 bot 的角色：总控可以对任何 bot 派活、读状态；worker 只能给 bot 发消息、读状态和事实、写自己的事实和认领、操作自己认领的工作区。

### 6.2 新工具

| 工具 | 输入 | 说明 |
| --- | --- | --- |
| `list_bots` | — | handle、职责、状态、认领摘要 |
| `bot_status` | handle | 状态、当前线程、最后指令、最近 N 步、待处理邮件数、后台任务 |
| `fleet_status` | — | 所有 bot 的 `bot_status` 精简版，加节点在线和额度。总控的首选工具（U1） |
| `send_message` | to, body, subject?, thread?, priority?, attachments? | 写一封 `bot_mail`，返回邮件 id |
| `reply` | mail_id, body | 回复，自动投到发件人的同一线程 |
| `wait_reply` | mail_id, timeout_secs | 等对方回复或邮件处理完，代替按工作区等待（G8） |
| `facts_get` / `facts_list` | key / prefix | 读事实层 |
| `facts_put` | key, value, ttl_secs? | 写自己负责的事实，带心跳 |
| `claim` / `release` / `claims` | 第 7.2 节 | 资源认领 |
| `memory_list` / `memory_read` / `memory_write` | 第 7.3 节 | 共享与私有 memory |
| `job_register` / `job_done` | 第 7.5 节 | 登记后台任务 |
| `ask_user` | question, options? | 经飞书问用户，结果作为邮件回来 |

### 6.3 现有工具调整

- `get_history`：加上 `session`、`after`、`limit`（默认 200）、`mode=summary|full` 参数（修 G9）。
- `send_prompt`：保留，用来直接操作工作区；bot 之间协作一律用 `send_message`。
- `list_nodes`：返回机器能力档案（第 7.6 节）。

## 7. hub 层服务

### 7.1 事实层（`0030_facts.sql`）

```sql
CREATE TABLE facts (
    key         TEXT PRIMARY KEY,        -- 如 run/q01、gpu/gpu-1/0、node/robot-1/online
    kind        TEXT NOT NULL,           -- run | gpu | node | dataset | custom
    owner_bot   TEXT REFERENCES bots(id),
    value       TEXT NOT NULL,           -- JSON
    status      TEXT NOT NULL DEFAULT 'ok', -- ok | stale | lost
    ttl_secs    INTEGER,                 -- 心跳间隔；超过 2 倍就标 stale，超过 5 倍标 lost
    updated_at  TEXT NOT NULL,
    created_at  TEXT NOT NULL
) STRICT;
CREATE TABLE fact_history (key TEXT NOT NULL, value TEXT NOT NULL, at TEXT NOT NULL) STRICT;
```

- 写入：bot 用 `facts_put`；外部脚本用 `POST /api/facts`，带一个专门用于上报的令牌，比如训练进程的上报脚本。
- 读取：`GET /api/facts?prefix=`；WS 推 `FactsChanged`；外部看板可以订阅或轮询。
- 只有 owner 能写；别人发现事实不对，要发消息给 owner，不能直接改。这条对应 U2：队列和看板各存一份才导致了残影。

### 7.2 资源认领（`0031_claims.sql`，接上 `crates/scheduler`）

```sql
CREATE TABLE claims (
    id          TEXT PRIMARY KEY,
    bot_id      TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL REFERENCES nodes(id),
    workspace_id TEXT REFERENCES workspaces(id),
    scope       TEXT NOT NULL,           -- gpu:0,1 | dir:/data/queue | repo | port:8766 | device:can1
    mode        TEXT NOT NULL,           -- exclusive | shared | read
    note        TEXT NOT NULL DEFAULT '',
    expires_at  TEXT,
    created_at  TEXT NOT NULL
) STRICT;
```

- **冲突规则**：同一节点上范围有交集时，独占和任何认领都冲突；共用和共用不冲突；只读不冲突。目录按路径前缀判断，GPU 按编号集合判断。
- **执行**：
  1. 认领时检查，冲突就拒绝，并告诉 bot 找谁；
  2. 远程手的 Write/Edit/Bash 在路径落在别的 bot 的独占目录里时，转成审批，推到飞书确认（U6）；
  3. 派活时检查 GPU 需求，用调度库的 `Requirements` / `plan`。
- 认领会写进 bot 的上下文块，也会出现在 `fleet_status` 里。

### 7.3 共享 memory（`0032_memory.sql`）

```sql
CREATE TABLE memories (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,           -- 'shared' | 'bot:<id>'
    name        TEXT NOT NULL,           -- kebab-case slug
    description TEXT NOT NULL,
    type        TEXT NOT NULL,           -- user | feedback | project | reference
    body        TEXT NOT NULL,
    version     INTEGER NOT NULL DEFAULT 1,
    updated_by  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    archived_at TEXT,
    UNIQUE(scope, name)
) STRICT;
```

- **去重**：`memory_write` 写入时如果同一作用域里已有同名条目，或者描述很接近（比如分词后相似度超过阈值），就返回已有条目，要求 bot 改成更新。更新带 `version`，做乐观并发。这条对应 U4：两个会话同时写了一份内容一样的分工。
- **注入**：启动时只放索引，也就是标题加描述；正文靠 `memory_read` 按需取。各运行时都能用。
- **导入**：把现有的 Claude Code memory 目录一次性导入成 `shared`。

### 7.4 规则引擎（`hub/src/bots/rules.rs`）

用 Rust 写死、可配置阈值，每 30 秒跑一次，加上事件触发：

| 规则 | 条件 | 动作 |
| --- | --- | --- |
| `fact_stale` | 事实超过 ttl 没更新 | 标 stale 或 lost；通知 owner；lost 发收件箱 |
| `fact_conflict` | 一些写死的矛盾，比如 run 是 running，但它的节点 GPU 上没有对应进程 | 收件箱，并唤醒总控 |
| `bot_stuck` | bot 在跑但超过 N 分钟没有新事件 | 收件箱；超过 2N 唤醒总控 |
| `approval_waiting` | 审批等待超过 N 分钟 | 飞书再提醒一次 |
| `job_orphan` | 后台任务等待的节点或目标已不存在，或者超过声明时长 | 通知 owner；超时发收件箱（U5） |
| `node_offline` | 节点离线超过 N 分钟 | 挂起所有依赖它的邮件和线程；告警（U9） |
| `egress_down` | 节点模型 API 出口不通 | 告警；在那台机器上思考的 bot 切回 hub |
| `quota_high` | 额度窗口超过阈值 | 按第 8 节降级；告警 |
| `claim_expired` | 认领过期 | 通知 owner，24 h 后自动释放 |

规则结果写 `inbox`（新增 kind：`alert`、`fact`、`job`），按配置转发飞书，严重的再给总控 bot 发一封 `from_kind=rule` 的邮件。

### 7.5 后台任务登记（`0033_jobs.sql`）

```sql
CREATE TABLE jobs (
    id          TEXT PRIMARY KEY,
    bot_id      TEXT NOT NULL REFERENCES bots(id),
    node_id     TEXT NOT NULL REFERENCES nodes(id),
    kind        TEXT NOT NULL,           -- spawn | watch | train | eval | custom
    label       TEXT NOT NULL,
    pid         INTEGER,
    log_path    TEXT,
    waits_for   TEXT NOT NULL DEFAULT '[]', -- 它在等什么：节点、事实 key、文件路径
    deadline_at TEXT,
    status      TEXT NOT NULL DEFAULT 'running', -- running | done | failed | orphan | killed
    notify_bot  INTEGER NOT NULL DEFAULT 1,      -- 结束时给 owner 发邮件
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;
```

远程手的 `Spawn` 会自动登记；hub 每分钟探一次进程是否还活着。进程结束时给 owner 发一封 `from_kind=system` 的邮件，这就是“后台任务结束就唤醒”。它代替了 Claude 进程内的后台任务通知，跨进程、跨运行时都能用。

### 7.6 机器能力档案

- 修 G10：`probe_node` 用 `json_patch` 合并写入；体检脚本补齐 macOS（`vm_stat`、`df -g`、`sysctl vm.loadavg`）；体检时带上节点的代理环境。
- `nodes.capabilities` 统一结构：`hw`（cpu、mem、disk、gpus）、`agents`（已有）、`sims`（从 `sims::scan` 汇总）、`egress`（各 API、走哪个代理）、`mounts`、`devices`（手工填写）、`constraints`（容器、会被回收、TLS 拦截、归属）、`reliability`（近 7 天离线次数和时长，由组网与探测汇总）。
- `/api/state` 和 `list_nodes` 返回精简版能力档案。
- 手工字段在机器页面里编辑；bot 可以用 `node_note` 提议修改，用户在收件箱里确认后生效。

## 8. 多订阅与运行时路由

### 8.1 现状

`accounts::resolve` / `pick` 已经能在同一个 provider 下按额度挑账号，失败后换账号重发（`accounts.rs:369-494,614-756`）。缺的是：跨运行时降级、按优先级分配、主动轮询额度、按账号限并发。

### 8.2 路由

输入：bot 的优先级、`bots.runtimes`（有序列表）、线程当前的运行时、各账号的额度快照。

```
for cand in bot.runtimes:
    acct = accounts::resolve(cand.runtime, cand.account, exclude)
    if acct 不可用: continue
    if 优先级允许在当前额度下使用这个账号（8.3）: return (cand.runtime, acct)
回退：P0 → 仍然用首选，并告警；P1/P2 → 邮件延后，到窗口重置时间再试
```

线程已经在某个运行时上时，优先留在原运行时（续接不丢东西）；只有原运行时不可用时才换。

### 8.3 优先级和额度阈值（可配置，默认值）

| 主力账号状态 | P0 | P1 | P2 |
| --- | --- | --- | --- |
| 两个窗口都 < 70% | 主力 | 主力 | 第二订阅 / 小账号 |
| 5h ≥ 70% | 主力 | 主力 | 第二订阅 / 小账号 |
| 5h ≥ 90% 或 7d ≥ 85% | 主力 | 备选 | 延后或第二订阅 |
| 被限流或模型不可用 | 暂停并请示（真机）/ 备选 | 备选 | 延后 |

- **主动轮询**：新加一个后台循环，每 10 分钟调用一次现有的额度查询（`accounts.rs:1488,1609`）。
- **按账号限并发**：`accounts` 加 `max_concurrent`，路由时检查。

### 8.4 换运行时的交接摘要

现有的 `chat::preface` 只拼了最近的文字。改为：

1. 线程结束或要切换时，用当前运行时（或者一个便宜的模型）生成结构化摘要：目标、已完成、当前状态、下一步、相关事实 key、相关文件、未决问题；存进 `bot_threads.summary`；
2. 新运行时启动时注入摘要、memory 索引和最近 5 轮原文；
3. 记录切换事件；以后切回原运行时时，用原线程续接，再补上中间发生的事情。

### 8.5 交叉复核

- 复核触发条件：bot 档案里配置，比如“推送到共享分支”“删除目录”“修改调度脚本”“对外发布”，用远程手的命令模式匹配，也可以由 bot 主动调用 `request_review`。
- 流程：把产物（diff、命令、目标）作为邮件发给复核 bot，复核 bot 用另一个运行时；结论回到原线程；有异议就暂停，并在飞书请示用户。

## 9. 飞书入口

### 9.1 接收消息

- hub 里起一个常驻的子进程：`lark-cli event consume im.message.receive_v1` 和 `card.action.trigger`，按 NDJSON 读取，带重启和退避。用应用身份。
- **单连接保证**：同一个应用只能有一个消费者。在 kv 里用一个带心跳的锁（`feishu.consumer`），只有拿到锁的 hub 才连；桌面端的内嵌 hub 默认不连。
- 现在 `office/mod.rs:427` 禁止 bot 调 `event`，这条保留；网关是 hub 自己调，不经过 bot 的工具。

### 9.2 会话映射（`0034_feishu.sql`）

```sql
CREATE TABLE feishu_threads (
    chat_id      TEXT NOT NULL,
    root_msg_id  TEXT NOT NULL,          -- 话题根消息；私聊用 chat_id 本身
    bot_id       TEXT NOT NULL REFERENCES bots(id),
    bot_thread   TEXT NOT NULL REFERENCES bot_threads(id),
    card_msg_id  TEXT,                   -- 进度卡片
    created_at   TEXT NOT NULL,
    PRIMARY KEY (chat_id, root_msg_id)
) STRICT;
```

- 私聊机器人：投到总控 bot 的 `main` 线程。
- 群里 @ 机器人开新话题：新建一个总控 `task` 线程，绑定这个话题。
- 消息里写了 `@bot-2` 或者“让 bot-2 …”：直接投给对应的 bot，并抄送总控。
- 话题里的后续回复：按映射投到同一线程。

### 9.3 输出

- 收到消息后 1 秒内回一个“处理中”卡片；之后由会话事件驱动，原地更新这张卡：当前步骤、最近 3 个操作、耗时，最多 2 秒更新一次。
- 一轮结束：卡片定格；最后的回复作为普通消息发出（长回复转成飞书文档链接）。
- 审批：发一张审批卡片（允许、拒绝、本次都允许）；回调走 `decide_approval_with_response`。只认用户本人的 open_id。
- 中断：卡片上的“中断”按钮或者发“停”，走 `run::interrupt`。
- 插话：话题里在 bot 运行时继续发消息，变成 urgent 邮件，直接插话。
- 附件：图片和文件用 `lark-cli` 下载到 hub-host，作为邮件附件；Claude 用图片输入（`user_input_line_with_images`）。

### 9.4 指令

`/状态`（`fleet_status` 摘要）、`/新对话`（关闭当前线程）、`/压缩`、`/中断`、`/切换 <runtime>`、`/热 30m`（让对应 bot 保持常开 30 分钟）、`/暂停 bot-x` 和 `/恢复 bot-x`。

### 9.5 安全

- **身份**：只有配置的用户 open_id 能下指令和审批；其他人的消息只记录不执行，可以配置成回复“无权限”。
- **不可信内容**：群消息、文档内容、网页内容在投给 bot 时都标成数据，不当指令。
- **出站过滤**：发到群里的内容过滤掉令牌模式（`sk-`、`bzh_`、私钥头）、内网 IP 和会话名，可配置；私聊不过滤 IP。
- **高风险操作**：删数据、停训练、推送、对外发布、越过认领的写操作，必须在飞书卡片上确认。

## 10. 界面

- **侧边栏**：最上面新增“bot”分组，每个 bot 一行，显示状态灯和待处理邮件数；原来的“工作”分组保留；“智能体”改名“角色档案”，放到 AI 分组里。
- **bot 页面**：主区是对话，可以切线程；侧栏显示收件箱、认领、后台任务、memory、运行时和账号；顶部有暂停、热模式、切换运行时。
- **总览**：每个 bot 一张卡（U1），加上节点、额度和最近的告警。
- **事实页**：按前缀浏览，显示状态和历史。

## 11. 安全模型补充

- hub-host 上存着所有 SSH 密钥、模型账号、飞书应用密钥、上报令牌。要求：全盘加密或密钥文件加密（和无人值守重启要做取舍）；每台工作机器用单独的密钥；上报令牌只给 `POST /api/facts` 权限。
- 第一阶段只用 SSH 转发访问 hub；对外监听前必须有 TLS 和持久令牌。
- bot 的权限按角色分，worker 不能让别的 bot 执行它自己没有权限做的事；`send_message` 的正文在对方那里标明来源，不当用户指令。这对应 Claude Code 里防“跨会话转手权限”的规则。

## 12. 分阶段计划

| 阶段 | 内容 | 主要改动 | 验收 |
| --- | --- | --- | --- |
| P0 hub 上线 | hub-host 部署、SSH 转发访问、修 G10/G11/G12 | 部署脚本；`profiles.rs`、`transport`、`fleet.rs` | 笔记本合盖 1 小时后再连，正在跑的会话完整无缺（U8） |
| P1 bot 与收件箱 | `bots`、`bot_threads`、`bot_mail`、调度器、多个“手”、调用方身份、`send_message`/`reply`/`fleet_status` | 新模块 `hub/src/bots/`；`sessions.rs`、`fleet.rs`、`tools.rs` | 训练 bot 给看板 bot 发消息、对方休眠时被唤醒并回复（U3）；`fleet_status` 能替代人工汇总（U1） |
| P2 飞书入口 | 事件接收、会话映射、进度卡、审批卡、中断、插话、附件 | `hub/src/feishu/`；`office` | 手机上私聊派活、看进度、审批、中断（U7） |
| P3 事实、认领、memory | `facts`、`claims`、`memories`、上报令牌 | `hub/src/bots/{facts,claims,memory}.rs`；`scheduler` | 停任务后状态 2 个心跳内变 stale（U2）；越界写触发飞书确认（U6）；重复 memory 被拦下（U4） |
| P4 规则与后台任务 | 规则引擎、`jobs`、远程手 `Spawn` | `rules.rs`、`jobs.rs`、`remote.rs` | 等待目标消失的后台任务在 10 分钟内被标出（U5）；节点离线时任务挂起并告警（U9） |
| P5 路由与复核 | 跨运行时降级、优先级、额度轮询、交接摘要、交叉复核 | `accounts.rs`、`chat.rs`、新 `routing.rs` | 模拟 5h 窗口 95%：P2 自动走第二订阅，P0 不受影响（U10） |
| P6 远程模式与界面 | 桌面端远程 hub、持久令牌、全局事件游标、bot 侧边栏 | `auth.rs`、`apps/desktop`、`crates/web` | 桌面端直接连 hub-host 使用 |
| P7 在工作机器上思考 | skills 同步、反向隧道看门狗、离线时切回 hub | `library.rs`、`fleet.rs` | 离线切换后对话能接上 |

每个阶段单独成提交系列；数据库迁移只追加；P0～P2 完成后先用起来，再做后面的。

## 13. 技术验证（在 P0/P1 之前做）

1. hub-host 上的远程手，操作工作机器上 10 万行级别的日志、批量读图，看延迟和稳定性能不能接受。
2. 同一个 bot 线程连续 50 次用 `--resume` 唤醒，上下文是否稳定、自动压缩后能不能接着用。
3. `lark-cli event consume` 长时间运行（24 h）的稳定性、断线后能不能补发、卡片原地更新的频率限制。
4. hub-host 同时跑 6 个 CLI 进程时的内存和响应。
5. 远端思考模式下，对话记录迁到另一台机器、工作目录不同时，`--resume` 能不能接上（只影响 P7）。

## 14. 待定

1. hub-host 用哪台。
2. 飞书里用话题群（一个任务一个话题）还是私聊为主；是否需要团队成员只读可见。
3. bot 默认的运行时与账号分配（第 8.3 节表格的具体取值）。
4. hub-host 的磁盘加密与无人值守重启怎么取舍。
5. 现有的 Claude Code memory 是否全部导入成共享 memory。
6. 越界写的确认范围：只有删除，还是所有写操作。
