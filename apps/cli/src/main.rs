use std::path::PathBuf;

use anyhow::{Context, Result};
use blazar_core_types::{ActivityState, EntryKind, NodeId, Outcome, SessionId, WorkspaceId};
use blazar_db::Db;
use blazar_runtime::claude::ClaudeCliRuntime;
use blazar_runtime::{AgentRuntime, SessionSpec};
use chrono::Utc;
use clap::{Parser, Subcommand};
use futures::StreamExt;

mod hub;
mod join;
mod probe;

#[derive(Parser)]
#[command(name = "blazar", version, about = "分布式 AI Agent 工作区编排")]
struct Cli {
    #[arg(long, env = "BLAZAR_DB", default_value = "blazar.sqlite")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run {
        #[arg(long, default_value = "local")]
        host: String,

        #[arg(long, default_value = ".")]
        cwd: PathBuf,

        prompt: String,

        #[arg(long = "tool")]
        tools: Vec<String>,

        #[arg(long = "no-tool")]
        deny_tools: Vec<String>,

        #[arg(long = "env")]
        envs: Vec<String>,
    },

    Replay {
        session_id: String,
        #[arg(long, default_value_t = 0)]
        after_seq: u64,
    },

    Mesh {
        #[arg(long, default_value = "local")]
        via: String,

        #[arg(long)]
        container: Option<String>,
    },

    Join {
        file: PathBuf,

        #[arg(long, env = "BLAZAR_ENGINE_DIR")]
        engine: Option<PathBuf>,

        #[arg(long, short)]
        yes: bool,
    },

    Leave,

    MeshStatus,

    Health {
        host: String,
    },

    Tree {
        host: String,
        root: String,
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },

    Agents {
        host: String,
    },

    Worktree {
        host: String,
        repo: String,
        #[arg(default_value = "create")]
        action: String,
        #[arg(default_value = "demo")]
        name: String,
    },

    Search {
        host: String,
        root: String,
        query: String,
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },

    Prompt {
        workspace: String,
        text: String,
        #[arg(long, env = "BLAZAR_HUB", default_value = "http://127.0.0.1:61528")]
        hub: String,

        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,

        #[arg(long)]
        new: bool,

        #[arg(long)]
        wait: bool,

        #[arg(long, default_value = "idle")]
        until: String,

        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    Wait {
        workspace: String,
        #[arg(long, env = "BLAZAR_HUB", default_value = "http://127.0.0.1:61528")]
        hub: String,
        #[arg(long, default_value = "idle")]
        until: String,
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    Task {
        action: String,
        arg: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, env = "BLAZAR_HUB", default_value = "http://127.0.0.1:61528")]
        hub: String,
    },

    Autopilot {
        action: String,
        arg: Option<String>,
        #[arg(long, env = "BLAZAR_HUB", default_value = "http://127.0.0.1:61528")]
        hub: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazar=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Command::Join { file, engine, yes } => {
            return join::join(file, engine.as_deref(), *yes).await;
        }
        Command::Leave => return join::leave().await,

        Command::Prompt {
            workspace,
            text,
            hub,
            agent,
            model,
            permission_mode,
            new,
            wait,
            until,
            timeout,
        } => {
            let code = hub::prompt(
                hub,
                hub::PromptOpts {
                    workspace,
                    text,
                    agent: agent.as_deref(),
                    model: model.as_deref(),
                    permission_mode: permission_mode.as_deref(),
                    new_chat: *new,
                    wait: *wait,
                    until,
                    timeout: *timeout,
                },
            )
            .await?;
            std::process::exit(code);
        }
        Command::Wait {
            workspace,
            hub,
            until,
            timeout,
        } => {
            std::process::exit(hub::wait(hub, workspace, until, *timeout).await?);
        }
        Command::Task {
            action,
            arg,
            workspace,
            hub,
        } => {
            std::process::exit(
                hub::tasks(hub, action, arg.as_deref(), workspace.as_deref()).await?,
            );
        }
        Command::Autopilot { action, arg, hub } => {
            std::process::exit(hub::autopilots(hub, action, arg.as_deref()).await?);
        }
        Command::MeshStatus => return join::status().await,
        _ => {}
    }
    let db = Db::open(&cli.db)
        .await
        .with_context(|| format!("打开数据库 {} 失败", cli.db.display()))?;

    match cli.command {
        Command::Run {
            host,
            cwd,
            prompt,
            tools,
            deny_tools,
            envs,
        } => run(&db, host, cwd, prompt, tools, deny_tools, envs).await,
        Command::Replay {
            session_id,
            after_seq,
        } => replay(&db, &session_id, after_seq).await,
        Command::Mesh { via, container } => probe::mesh(&via, container.as_deref()).await,
        Command::Join { .. }
        | Command::Leave
        | Command::MeshStatus
        | Command::Prompt { .. }
        | Command::Wait { .. }
        | Command::Task { .. }
        | Command::Autopilot { .. } => unreachable!("已在上面处理"),
        Command::Health { host } => probe::health(&host).await,
        Command::Tree { host, root, limit } => probe::tree(&host, &root, limit).await,
        Command::Search {
            host,
            root,
            query,
            limit,
        } => probe::search(&host, &root, &query, limit).await,
        Command::Agents { host } => probe::agents(&host).await,
        Command::Worktree {
            host,
            repo,
            action,
            name,
        } => probe::worktree(&host, &repo, &action, &name).await,
    }
}

async fn run(
    db: &Db,
    host: String,
    cwd: PathBuf,
    prompt: String,
    tools: Vec<String>,
    deny_tools: Vec<String>,
    envs: Vec<String>,
) -> Result<()> {
    let cwd = if host == "local" {
        cwd.canonicalize().context("解析工作目录失败")?
    } else {
        cwd
    };
    let (workspace_id, session_id) = ensure_local_session(db, &cwd).await?;
    println!("workspace {workspace_id}\nsession   {session_id}\n");

    let mut spec = SessionSpec::new(&cwd, prompt);
    spec.allowed_tools = tools;
    spec.disallowed_tools = deny_tools;
    for kv in &envs {
        let (k, v) = kv.split_once('=').unwrap_or((kv.as_str(), ""));
        spec.env.insert(k.to_owned(), v.to_owned());
    }

    let runtime = ClaudeCliRuntime::new(probe::transport_for(&host));
    let mut handle = runtime.start(spec).await.context("启动 claude 失败")?;

    while let Some(entry) = handle.events.next().await {
        db.append_event(session_id, workspace_id, &entry)
            .await
            .context("写入事件失败")?;
        println!("{}", render(&entry.seq, &entry.kind));
    }

    let activity = db.workspace_activity(workspace_id).await?;
    println!("\n工作区状态: {}", badge(activity));
    Ok(())
}

async fn replay(db: &Db, session_id: &str, after_seq: u64) -> Result<()> {
    let id = SessionId(session_id.parse().context("session id 不是合法 UUID")?);
    let entries = db.events_since(id, after_seq).await?;
    println!("自 seq={after_seq} 起共 {} 条事件\n", entries.len());
    for entry in &entries {
        println!("{}", render(&entry.seq, &entry.kind));
    }
    Ok(())
}

async fn ensure_local_session(db: &Db, cwd: &std::path::Path) -> Result<(WorkspaceId, SessionId)> {
    let now = Utc::now().to_rfc3339();
    let pool = db.pool();

    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES (?1, 'local', 'local', 'online', ?2)
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(NodeId::new().to_string())
    .bind(&now)
    .execute(pool)
    .await?;

    let node_id: String = sqlx::query_scalar("SELECT id FROM nodes WHERE name = 'local'")
        .fetch_one(pool)
        .await?;

    let path = cwd.display().to_string();
    let name = cwd
        .file_name()
        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());

    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (node_id, path) DO NOTHING",
    )
    .bind(WorkspaceId::new().to_string())
    .bind(&node_id)
    .bind(&name)
    .bind(&path)
    .bind(&now)
    .execute(pool)
    .await?;

    let workspace_id: String =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE node_id = ?1 AND path = ?2")
            .bind(&node_id)
            .bind(&path)
            .fetch_one(pool)
            .await?;

    let session_id = SessionId::new();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, created_at)
         VALUES (?1, ?2, 'claude_cli', ?3)",
    )
    .bind(session_id.to_string())
    .bind(&workspace_id)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok((
        WorkspaceId(workspace_id.parse().context("workspace id 非法")?),
        session_id,
    ))
}

fn render(seq: &u64, kind: &EntryKind) -> String {
    let body = match kind {
        EntryKind::SessionStarted {
            provider_session_id,
            model,
            ..
        } => format!(
            "会话建立  provider_session={provider_session_id} model={}",
            model.as_deref().unwrap_or("-")
        ),
        EntryKind::UserMessage { text } => format!("用户  {}", truncate(text)),
        EntryKind::AssistantMessage { text } => format!("助手  {}", truncate(text)),
        EntryKind::Thinking { text } => format!("思考  {}", truncate(text)),
        EntryKind::ToolUse { name, id, .. } => format!("工具  {name} ({})", id.0),
        EntryKind::ToolResult { id, ok, .. } => {
            format!("结果  {} {}", id.0, if *ok { "ok" } else { "失败" })
        }
        EntryKind::Approval { .. } => "审批  等待人工裁决".to_owned(),
        EntryKind::ApprovalResolved { decision, .. } => format!(
            "裁决  {}",
            match decision {
                blazar_core_types::ApprovalDecision::Allow => "允许".to_owned(),
                blazar_core_types::ApprovalDecision::AutoAllowed { rule } => {
                    format!("自动批准（{rule}）")
                }
                blazar_core_types::ApprovalDecision::Deny { message } => format!("拒绝：{message}"),
                blazar_core_types::ApprovalDecision::Cancelled => "agent 撤回了询问".to_owned(),
                blazar_core_types::ApprovalDecision::Aborted => "会话结束，作废".to_owned(),
            }
        ),
        EntryKind::InputConsumed { text } => format!("送达  {}", truncate(text)),
        EntryKind::TokenUsage(u) => format!(
            "用量  in={} out={} cache_read={}{}",
            u.input,
            u.output,
            u.cache_read,
            u.cost_usd
                .map(|c| format!(" cost=${c:.4}"))
                .unwrap_or_default()
        ),
        EntryKind::RateLimit(rl) => {
            let windows: Vec<_> = rl
                .windows
                .iter()
                .map(|w| format!("{}={:.0}%", w.name, w.utilization * 100.0))
                .collect();
            format!("额度  {}", windows.join(" "))
        }
        EntryKind::Error { message } => format!("错误  {message}"),
        EntryKind::Finished(Outcome::Success { denied, .. }) if !denied.is_empty() => {
            format!(
                "被拦  {} 次操作被权限拒绝、没有执行：{}",
                denied.len(),
                denied.join("；")
            )
        }
        EntryKind::Finished(Outcome::Success { text, .. }) => {
            format!("完成  {}", truncate(text.as_deref().unwrap_or("")))
        }
        EntryKind::BackgroundTask {
            status,
            description,
            task_id,
            ..
        } => format!(
            "后台  {} {}",
            description.as_deref().unwrap_or(task_id),
            match status.as_str() {
                "killed" | "stopped" | "failed" =>
                    format!("—— 随本轮结束被终止（{status}），没有跑完"),
                s => s.to_owned(),
            }
        ),
        EntryKind::Finished(Outcome::Failed { message }) => format!("失败  {message}"),
        EntryKind::Finished(Outcome::Interrupted) => "中断".to_owned(),
    };
    format!("[{seq:>3}] {body}")
}

fn badge(state: ActivityState) -> &'static str {
    match state {
        ActivityState::AwaitingApproval => "🔴 等待审批",
        ActivityState::Errored => "🟠 出错",
        ActivityState::Completed => "🟢 已完成",
        ActivityState::Running => "🔵 运行中",
        ActivityState::Idle => "⚪ 空闲",
    }
}

fn truncate(s: &str) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= 100 {
        return flat;
    }
    flat.chars().take(100).collect::<String>() + "…"
}
