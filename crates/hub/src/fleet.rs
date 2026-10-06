use std::collections::BTreeMap;

use blazar_runtime::McpServerSpec;

use crate::api::Shared;

pub async fn enabled(st: &Shared) -> bool {
    crate::office::prefs(st).await["fleet"]["enabled"]
        .as_bool()
        .unwrap_or(false)
}

pub async fn mcp_spec(st: &Shared) -> Option<McpServerSpec> {
    if !enabled(st).await {
        return None;
    }
    Some(McpServerSpec {
        name: blazar_mcp::fleet::SERVER_NAME.into(),
        command: std::env::current_exe().ok()?.display().to_string(),
        args: vec![
            blazar_mcp::fleet::SUBCOMMAND.into(),
            "--hub".into(),
            crate::office::HUB_URL.get()?.clone(),
        ],
        env: BTreeMap::from([(
            "BLAZAR_HUB_SESSION".into(),
            st.auth.get()?.path.display().to_string(),
        )]),
        ..Default::default()
    })
}
