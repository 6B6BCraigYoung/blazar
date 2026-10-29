**简体中文** · [English](#english)

# 更新日志

## v0.1.1 — 2026-10-29

v0.1.0 之后的第一个版本，重点是让多台机器、多个智能体同时干活时更容易看清状态，也更少需要你守着。

### 中控台与收件箱

- 首页改为中控台：按「谁在等你」排列工作区，右侧的「需要你处理」栏汇总待审批、待决定和失败的运行；活动流可以清空，页面不在前台时新消息会弹出提醒。
- 收件箱改为可以用键盘操作的列表，审批可用数字键决定，删除后可在提示里撤销。
- 失败的运行会标明原因（额度、登录、网络、续接点丢失等），卡片上直接给出下一步：重试、重新登录或标记为已处理。
- 新增「助手」对话：不用先建工作区，就能直接和一个带调度工具的助手对话。

### 被拦重试与「待你决定」卡片

- 被安全检查拦下的工具调用会变成卡片，保留原始输入，可以只放行这一次并重试；决定之前卡片一直留在收件箱里，并记录谁在什么时候放行。
- 智能体需要你拍板时会发一张带选项的「待你决定」卡片，点选或回复后对话自动续接。

### 对话面板

- 工作区页面以对话为中心重新布局，会话标签并入工作区标题栏，文件树移进检查器的编辑器页。
- 工具调用按 CLI 的默认样式显示；待办和改动收进输入框上方的托盘；底栏统一为带文字的按钮；早先轮次的编辑折叠为一行，可以一键定位。
- 显示会话实际使用的模型；支持在一轮进行中插话、直接执行 shell 命令，Codex 走 app-server 协议。
- 代码块可以横向滚动并带复制按钮；差异视图带行号，并可从对话直接打开文件和改动。
- 对话里直接显示图片，可以复制图片或复制路径。
- 机器离线时显示离线页，输入框上方给出提示。

### 断网续跑

- 网络恢复后自动续接被打断的会话；续接点丢失时先重试，再判定失败。
- 界面的事件流在重连后从上次看到的位置继续，不会丢更新。
- API 繁忙时的重试会单独显示，错误信息改为直白的说明。

### 账号

- 每个运行时可以管理多个 Claude Code / Codex 账号，新会话按剩余额度分摊到不同账号。
- 启动时和切换前复查各账号的登录状态，显示检查时间和已过期的登录。
- 远端机器上的会话可以使用本机保存的账号：请求经 SSH 隧道交给本机的凭据代理，长期令牌不落到远端。
- 远端缺少 Claude Code 或 Codex 时，自动把与本机同版本、校验过的官方版本装进那台机器的 `~/.blazar`。

### 多机器与常驻 hub

- 常驻 bot：职责、家工作区、收件箱、资源认领、事实层和共享 memory，详见 README。
- 后台任务：`job_spawn`、`job_register`、`list_jobs`、`job_log`、`job_kill`，结束时通知发起方，可声明它在等什么。
- 告警规则：机器离线等事件去重后进入收件箱。
- 配置导入导出：`blazar export` / `blazar import`，或在设置里操作；桌面端可以通过 SSH 连接另一台机器上的 hub，预览端口自动转发。
- 子工作区可以开在其他机器上，工作区之间可以经 hub 复制文件。
- 在工作区所在机器上启动 Isaac Sim、MuJoCo，并把画面推回来看。
- 机器可以从 `~/.ssh/config` 直接添加，不必加入组网。

### 性能

- 长对话只渲染末尾，折叠内容、子智能体记录和思考过程在展开时才加载；历史记录直接读成行，不再逐条重新解析。
- 放弃的请求会被取消，超大的差异在远端机器上就截断；事件表按会话和时间建索引。
- 主滚动区保持在单一图层，切换页面不再越用越慢。

### 下载

- 下载文件或文件夹时弹出另存为面板，显示实时进度，采用流式传输，大文件不再整块读进内存。

### 其他

- 新的命令面板、统一的右键菜单和快捷键、设置页按系统设置的方式排列并即时保存。
- 终端浅色主题随应用切换。
- 依赖升级：rustls、rustls-webpki、webpki-root-certs、pulldown-cmark 和 tokio 升到修复了已公开安全公告的版本。

## v0.1.0 — 2026-09-22

首个公开版本。

---

<a id="english"></a>

[简体中文](#更新日志) · **English**

# Changelog

## v0.1.1 — 2026-10-29

The first release after v0.1.0. It focuses on making it easier to see what is going on when several agents work across several machines, and on needing to babysit them less.

### Console and inbox

- The home page is now a console: workspaces are ordered by who is waiting on you, and a "Needs you" rail collects pending approvals, decisions and failed runs. The activity feed can be cleared, and new messages pop up when the page is out of view.
- The inbox is a keyboard-driven list: decide approvals with number keys and undo removals from the toast.
- A failed run names its cause (quota, login, network, lost resume point, ...) and its card offers the next step: retry, sign in again, or mark as handled.
- New assistant chats: talk to an assistant that has the orchestration tools without creating a workspace first.

### Blocked-call retries and decision cards

- A tool call stopped by the safety check becomes a card that keeps the original input. You can allow it once and retry; the card stays in the inbox until you decide, and the decision is recorded with who and when.
- When an agent needs you to choose, it sends a decision card with options. Pick one or reply and the conversation resumes.

### Conversation pane

- The workspace page is laid out around the conversation; conversation tabs merge into the workspace header and the file tree moves into the inspector's editor tab.
- Tool calls look like the CLI's default view; todos and changes fold into a tray above the prompt; the bottom bar uses labelled buttons; edits from earlier turns fold into one locating row.
- Shows the model the session is actually running; supports mid-turn input and shell commands, and talks to Codex through its app-server.
- Code blocks scroll sideways and have a copy button; diffs show line numbers; files and changes open straight from the conversation.
- Images are shown inline in the conversation, with copy image and copy path.
- An offline page and a composer banner appear when the workspace's machine is down.

### Resume after network loss

- Sessions interrupted by a network drop resume automatically when the network returns; a lost resume point is retried before the turn is marked failed.
- The UI event stream resumes from the last seen event after reconnecting, so no updates are lost.
- API retries are shown as their own entries, and errors are worded plainly.

### Accounts

- Manage several Claude Code and Codex accounts per runtime; new sessions are spread across accounts by remaining quota.
- Logins are rechecked at startup and before switching, with the check age and expired logins shown.
- Sessions on remote machines can use accounts saved on this computer: requests go through an SSH tunnel to the local credential proxy, and long-lived tokens never land on the remote machine.
- Missing Claude Code or Codex CLIs are installed into `~/.blazar` on remote machines, using the verified official build that matches the local version.

### Multiple machines and a resident hub

- Resident bots with a duty, a home workspace, a mailbox, resource claims, facts and shared memory; see the README.
- Background jobs: `job_spawn`, `job_register`, `list_jobs`, `job_log`, `job_kill`. Whoever started a job is notified when it ends, and a job can declare what it waits for.
- Alert rules raise deduplicated inbox items, for example when a machine goes offline.
- Export and import the configuration with `blazar export` / `blazar import` or from settings; the desktop app can connect to a hub on another machine over SSH, with preview ports forwarded automatically.
- Child workspaces can be created on other machines, and files can be copied between workspaces through the hub.
- Launch Isaac Sim and MuJoCo on a workspace's machine and stream the screen back.
- Add machines straight from `~/.ssh/config` without joining the mesh.

### Performance

- Long conversations render only their tail; folds, subagent transcripts and thinking load when opened; history is read straight into rows instead of re-parsing every event.
- Abandoned requests are cancelled and huge diffs are cut on the remote machine; events are indexed by session and time.
- The main scroller stays on one layer, so switching pages no longer slows down over time.

### Downloads

- Downloading a file or folder opens a save-as panel with live progress and streams the data, so large files are no longer read into memory at once.

### Other

- A new command palette, one context menu and keyboard model for every list, and settings laid out like system settings that save on change.
- The terminal gets a light theme that follows the app.
- Dependency updates: rustls, rustls-webpki, webpki-root-certs, pulldown-cmark and tokio move to releases that fix published security advisories.

## v0.1.0 — 2026-09-22

First public release.
