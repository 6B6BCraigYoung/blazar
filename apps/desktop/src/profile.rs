use std::path::PathBuf;

use anyhow::{Context, Result};
use blazar_hub::HubConfig;

#[must_use]
pub fn profile_name() -> String {
    sanitize_profile(std::env::var("BLAZAR_PROFILE").ok().as_deref())
}

fn sanitize_profile(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| {
            s.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or("default")
        .to_owned()
}

pub fn data_dir() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("ai", "blazar", "Blazar")
        .context("无法确定平台数据目录")?
        .data_dir()
        .to_path_buf();
    let dir = base.join("profiles").join(profile_name());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建数据目录 {} 失败", dir.display()))?;
    Ok(dir)
}

pub fn hub_config() -> Result<HubConfig> {
    Ok(HubConfig {
        db_path: data_dir()?.join("blazar.sqlite"),

        bind: ([127, 0, 0, 1], 0).into(),
        mesh_via: std::env::var("BLAZAR_MESH_VIA").unwrap_or_else(|_| "local".into()),
        mesh_container: std::env::var("BLAZAR_MESH_CONTAINER").ok(),

        engine_dir: std::env::var_os("BLAZAR_ENGINE_DIR").map(Into::into),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_rejects_path_traversal() {
        assert_eq!(sanitize_profile(Some("../../etc")), "default");
        assert_eq!(sanitize_profile(Some("a/b")), "default");
        assert_eq!(sanitize_profile(Some("~/x")), "default");
        assert_eq!(sanitize_profile(Some("")), "default");
        assert_eq!(sanitize_profile(Some("   ")), "default");
        assert_eq!(sanitize_profile(None), "default");
    }

    #[test]
    fn valid_profile_names_pass_through() {
        assert_eq!(sanitize_profile(Some("lab-1")), "lab-1");
        assert_eq!(sanitize_profile(Some("prod_2")), "prod_2");
        assert_eq!(sanitize_profile(Some(" trimmed ")), "trimmed");
    }
}
