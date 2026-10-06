use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post, put};

use super::{
    AppState, accounts, agents, analytics, api, asset, auth, autopilot, catalog, chat, checkpoint,
    git, inbox, library, mesh, office, remote_cli, rules, scripts, skillhub, snippets, tasks,
    titles, v2_redirect, web,
};

pub fn build_router(st: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(web))
        .route(
            "/v2",
            get(|| async { axum::response::Redirect::permanent("/") }),
        )
        .route(
            "/v2/",
            get(|| async { axum::response::Redirect::permanent("/") }),
        )
        .route("/v2/{*path}", get(v2_redirect))
        .route("/vendor/{*path}", get(asset))
        .route("/img/{*path}", get(asset))
        .route("/api/state", get(api::get_state))
        .route("/api/agents", get(api::list_agents))
        .route("/api/runtimes", get(api::runtimes))
        .route("/api/runtimes/{id}/models", get(api::runtime_models))
        .route("/api/runtimes/claude/catalog", get(catalog::claude_catalog))
        .route(
            "/api/agent-profiles",
            get(agents::list).post(agents::create),
        )
        .route(
            "/api/agent-profiles/{id}",
            get(agents::detail)
                .put(agents::update)
                .delete(agents::delete),
        )
        .route("/api/agent-profiles/{id}/archive", post(agents::archive))
        .route("/api/agent-profiles/{id}/restore", post(agents::restore))
        .route(
            "/api/agent-profiles/{id}/cancel-runs",
            post(agents::cancel_runs),
        )
        .route(
            "/api/agent-profiles/{id}/duplicate",
            post(agents::duplicate),
        )
        .route("/api/nodes/{name}/probe", post(api::probe_node))
        .route("/api/nodes/{name}/agents", get(api::node_agents))
        .route("/api/nodes/{name}/browse", get(api::browse_node))
        .route(
            "/api/agent-configs",
            get(api::list_agent_configs).post(api::upsert_agent_config),
        )
        .route("/api/agent-configs/{id}", delete(api::delete_agent_config))
        .route("/api/hooks", get(api::list_hooks).post(api::create_hook))
        .route("/api/hooks/{id}", delete(api::delete_hook))
        .route("/api/mesh/refresh", post(api::refresh_mesh))
        .route(
            "/api/ssh-hosts",
            get(api::list_ssh_hosts).post(api::add_ssh_hosts),
        )
        .route("/api/ssh-hosts/{name}", delete(api::remove_ssh_node))
        .route("/api/mesh/local", get(mesh::local))
        .route("/api/mesh/external-rpc", put(mesh::set_external_rpc))
        .route("/api/mesh/topology", get(mesh::topology))
        .route("/api/mesh/join/preview", post(mesh::preview))
        .route("/api/mesh/join", post(mesh::join).delete(mesh::dismiss))
        .route("/api/mesh/leave", post(mesh::leave))
        .route(
            "/api/mesh/config",
            get(mesh::config_get).put(mesh::config_apply),
        )
        .route("/api/mesh/config/preview", post(mesh::config_preview))
        .route(
            "/api/mesh/issuer",
            get(mesh::issuer)
                .put(mesh::set_issuer)
                .delete(mesh::clear_issuer),
        )
        .route(
            "/api/mesh/invites",
            get(mesh::list_invites).post(mesh::issue),
        )
        .route("/api/mesh/invites/{id}", delete(mesh::revoke))
        .route("/api/nodes/health", get(api::node_health))
        .route("/api/nodes/roles", put(api::set_node_roles))
        .route("/api/workspaces", post(api::create_workspace))
        .route("/api/workspaces/isolated", post(api::create_isolated))
        .route("/api/workspaces/{id}/push", post(api::push_workspace))
        .route("/api/nodes/{name}/branches", get(api::list_branches))
        .route("/api/nodes/{name}/leftovers", get(api::node_leftovers))
        .route("/api/approvals", get(api::list_approvals))
        .route("/api/approvals/{id}", post(api::decide_approval))
        .route("/api/sessions/{id}/input", post(api::session_input))
        .route("/api/sessions/{id}/control", post(api::session_control))
        .route("/api/workspaces/{id}/checkpoints", get(checkpoint::list))
        .route(
            "/api/workspaces/{id}/sessions",
            get(api::workspace_sessions),
        )
        .route("/api/sessions/{id}/title", put(titles::rename))
        .route("/api/tasks", get(tasks::list).post(tasks::create))
        .route(
            "/api/tasks/{id}",
            get(tasks::detail).put(tasks::update).delete(tasks::delete),
        )
        .route("/api/tasks/{id}/start", post(tasks::start))
        .route("/api/tasks/{id}/comments", post(tasks::comment))
        .route("/api/checkpoints/{id}/restore", post(checkpoint::restore))
        .route("/api/nodes/{name}/sweep", post(api::sweep_node))
        .route("/api/workspaces/{id}", delete(api::delete_workspace))
        .route("/api/workspaces/{id}/pause", post(api::pause_workspace))
        .route("/api/workspaces/{id}/resume", post(api::resume_workspace))
        .route("/api/workspaces/{id}/commit", post(api::commit_workspace))
        .route("/api/workspaces/{id}/destroy", post(api::destroy_workspace))
        .route("/api/workspaces/{id}/tree", get(api::workspace_tree))
        .route("/api/workspaces/{id}/ls", get(api::workspace_ls))
        .route("/api/workspaces/{id}/raw", get(api::workspace_raw))
        .route(
            "/api/workspaces/{id}/download",
            get(api::workspace_download),
        )
        .route(
            "/api/workspaces/{id}/file",
            get(api::workspace_file).put(api::workspace_file_write),
        )
        .route("/api/workspaces/{id}/diff", get(api::workspace_diff))
        .route(
            "/api/autopilots",
            get(autopilot::list).post(autopilot::create),
        )
        .route("/api/autopilots/preview", post(autopilot::preview))
        .route(
            "/api/autopilots/{id}",
            get(autopilot::detail)
                .put(autopilot::update)
                .delete(autopilot::delete),
        )
        .route("/api/autopilots/{id}/run", post(autopilot::run_now))
        .route(
            "/api/autopilots/{id}/webhook",
            post(autopilot::rotate_webhook).delete(autopilot::disable_webhook),
        )
        .route("/api/webhooks/{token}", post(autopilot::hook))
        .route(
            "/api/workspaces/{id}/scripts",
            get(scripts::get).put(scripts::put),
        )
        .route(
            "/api/workspaces/{id}/scripts/{kind}/run",
            post(scripts::run),
        )
        .route("/api/workspaces/{id}/script-runs", get(scripts::runs))
        .route("/api/workspaces/{id}/dev", get(scripts::dev_get))
        .route("/api/workspaces/{id}/dev/start", post(scripts::dev_start))
        .route("/api/workspaces/{id}/dev/stop", post(scripts::dev_stop))
        .route(
            "/api/skills",
            get(library::skills).post(library::create_skill),
        )
        .route(
            "/api/skills/import",
            get(library::import_scan).post(library::import),
        )
        .route("/api/skills/market/providers", get(skillhub::providers))
        .route("/api/skills/market/key", put(skillhub::set_key))
        .route("/api/skills/market/search", get(skillhub::search))
        .route("/api/skills/market/repo", get(skillhub::repo))
        .route("/api/skills/market/preview", post(skillhub::preview))
        .route("/api/skills/market/install", post(skillhub::install))
        .route(
            "/api/skills/{id}",
            get(library::skill).delete(library::delete_skill),
        )
        .route(
            "/api/skills/{id}/files",
            put(library::put_file).delete(library::delete_file),
        )
        .route(
            "/api/mcp-servers",
            get(library::mcp_list).post(library::mcp_create),
        )
        .route(
            "/api/mcp-servers/{id}",
            put(library::mcp_update).delete(library::mcp_delete),
        )
        .route(
            "/api/agent-profiles/{id}/capabilities",
            get(library::agent_caps).put(library::set_agent_caps),
        )
        .route(
            "/api/settings",
            get(office::get_prefs).put(office::put_prefs),
        )
        .route("/api/about", get(office::about))
        .route("/api/office/lark", get(office::lark_status))
        .route("/api/office/lark/settings", put(office::put_lark_settings))
        .route("/api/office/lark/run", post(office::lark_run))
        .route("/api/office/lark/calls", get(office::lark_calls))
        .route("/api/office/github", get(office::github_status))
        .route("/api/office/obsidian", get(office::obsidian::status))
        .route(
            "/api/office/obsidian/settings",
            put(office::obsidian::put_settings),
        )
        .route("/api/office/obsidian/note", post(office::obsidian::note))
        .route(
            "/api/tasks/{id}/obsidian-note",
            post(office::obsidian::task_note),
        )
        .route("/api/office/{app}/connect", post(office::connect::start))
        .route("/api/office/{app}/job", get(office::connect::job))
        .route(
            "/api/office/{app}/job/cancel",
            post(office::connect::cancel),
        )
        .route("/api/office/lark/test-notify", post(office::test_notify))
        .route("/api/tasks/{id}/lark-doc", post(office::task_doc))
        .route("/api/inbox", get(inbox::list).post(inbox::mark))
        .route("/api/inbox/count", get(inbox::count))
        .route("/api/inbox/seen/{thread}", post(inbox::seen_thread))
        .route("/api/approval-rules", get(rules::list).post(rules::create))
        .route(
            "/api/approval-rules/{id}",
            put(rules::toggle).delete(rules::delete),
        )
        .route("/api/snippets", get(snippets::list).post(snippets::create))
        .route(
            "/api/snippets/{id}",
            put(snippets::update).delete(snippets::delete),
        )
        .route("/api/workspaces/{id}/queue", get(chat::list).put(chat::put))
        .route("/api/workspaces/{id}/queue/{qid}", delete(chat::remove))
        .route(
            "/api/workspaces/{id}/queue/{qid}/send",
            post(chat::send_now),
        )
        .route("/api/workspaces/{id}/retry", post(chat::retry))
        .route("/api/workspaces/{id}/wait", get(chat::wait))
        .route("/api/analytics", get(analytics::overview))
        .route("/api/workspaces/{id}/git", get(git::status))
        .route("/api/workspaces/{id}/git/branches", get(git::branches))
        .route("/api/workspaces/{id}/git/describe", post(git::describe))
        .route("/api/workspaces/{id}/git/{op}", post(git::op))
        .route("/api/workspaces/{id}/pr", get(git::pr))
        .route("/api/workspaces/{id}/history", get(api::workspace_history))
        .route("/api/workspaces/{id}/prompt", post(api::prompt))
        .route("/api/workspaces/{id}/context", get(api::get_context))
        .route("/api/workspaces/{id}/context/{kind}", put(api::put_context))
        .route("/api/sessions/{id}/events", get(api::session_events))
        .route("/api/sessions/{id}/interrupt", post(api::interrupt))
        .route("/api/usage", get(api::usage))
        .route("/api/accounts", get(accounts::list).post(accounts::create))
        .route("/api/accounts/mode", put(accounts::set_mode))
        .route(
            "/api/accounts/global",
            get(accounts::global_get).put(accounts::global_put),
        )
        .route("/api/accounts/check", post(accounts::check_all))
        .route(
            "/api/accounts/{id}",
            put(accounts::update).delete(accounts::remove),
        )
        .route("/api/accounts/{id}/check", post(accounts::check_one))
        .route("/api/accounts/{id}/token", put(accounts::set_token))
        .route("/api/accounts/{id}/quota", post(accounts::quota))
        .route(
            "/api/accounts/{id}/models/{model}",
            delete(accounts::clear_model_block),
        )
        .route("/api/accounts/{id}/login/ws", get(accounts::login_ws))
        .route(
            "/api/nodes/{name}/login/{runtime}/ws",
            get(accounts::node_login_ws),
        )
        .route("/api/nodes/{name}/cli-versions", get(remote_cli::versions))
        .route(
            "/api/nodes/{name}/update/{runtime}",
            post(remote_cli::update),
        )
        .route("/api/workspaces/{id}/detail", get(api::workspace_detail))
        .route("/api/search", get(api::search))
        .route("/api/workspaces/{id}/terminal/ws", get(api::terminal_ws))
        .route("/api/ws", get(api::ws_handler))
        .nest(
            "/ws",
            Router::new()
                .route("/api/ws", get(api::ws_handler))
                .route("/api/workspaces/{id}/terminal/ws", get(api::terminal_ws))
                .route("/api/accounts/{id}/login/ws", get(accounts::login_ws))
                .route(
                    "/api/nodes/{name}/login/{runtime}/ws",
                    get(accounts::node_login_ws),
                ),
        )
        .fallback(web)
        .layer(axum::middleware::from_fn_with_state(
            st.clone(),
            auth::guard,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(st)
}
