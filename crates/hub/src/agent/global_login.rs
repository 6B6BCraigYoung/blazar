use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use super::accounts::{Account, TOKEN_FILE};

const SETUP_TOKEN_SCOPES: [&str; 1] = ["user:inference"];
const SHARED_KEYS: [&str; 5] = [
    "mcpOAuth",
    "mcpOAuthClientConfig",
    "mcpXaaIdp",
    "mcpXaaIdpConfig",
    "pluginSecrets",
];

#[derive(Debug, Clone)]
pub struct Places {
    pub home: PathBuf,
    pub keychain_service: String,
    pub keychain_account: String,
    pub use_keychain: bool,
}

impl Places {
    pub fn from_env() -> Self {
        let home = std::env::var_os("BLAZAR_GLOBAL_HOME")
            .or_else(|| std::env::var_os("HOME"))
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let use_keychain =
            cfg!(target_os = "macos") && std::env::var("BLAZAR_GLOBAL_NO_KEYCHAIN").is_err();
        Self {
            home,
            keychain_service: std::env::var("BLAZAR_GLOBAL_KEYCHAIN_SERVICE")
                .unwrap_or_else(|_| "Claude Code-credentials".into()),
            keychain_account: std::env::var("USER").unwrap_or_else(|_| "claude-code-user".into()),
            use_keychain,
        }
    }

    fn claude_home(&self) -> PathBuf {
        self.home.join(".claude")
    }

    fn claude_json(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    fn codex_auth(&self) -> PathBuf {
        self.home.join(".codex").join("auth.json")
    }
}

fn security(args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("/usr/bin/security")
        .args(args)
        .output()
}

fn keychain_get(service: &str, account: &str) -> Option<String> {
    let out = security(&["find-generic-password", "-a", account, "-s", service, "-w"]).ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_owned()
    })
}

fn keychain_set(service: &str, account: &str, secret: &str) -> Result<(), String> {
    use std::io::Write;
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let hex: String = secret.bytes().map(|b| format!("{b:02x}")).collect();
    let cmd = format!(
        "add-generic-password -U -a {} -s {} -X {hex}\n",
        quote(account),
        quote(service)
    );
    let mut child = std::process::Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("起不了 security：{e}"))?;
    child
        .stdin
        .take()
        .ok_or("security 没有标准输入")?
        .write_all(cmd.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() && out.stderr.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "写钥匙串失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn hashed_service(config_dir: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(config_dir.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("Claude Code-credentials-{hex}")
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}：{e}", dir.display()))?;
    }
    #[cfg(windows)]
    let staging = path.with_file_name(format!(".blazar-private-{}", uuid::Uuid::now_v7()));
    #[cfg(windows)]
    blazar_core_types::private_storage::protect_directory(&staging)
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    let tmp = staging.join("credential.tmp");
    #[cfg(not(windows))]
    let tmp = path.with_extension(format!("blazar-{}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = blazar_core_types::private_storage::open(&tmp, &opts)
            .map_err(|e| format!("写不了 {}：{e}", tmp.display()))?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| format!("换不了 {}：{e}", path.display()))
    })();
    #[cfg(windows)]
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn read_live_claude(p: &Places) -> Option<String> {
    if p.use_keychain {
        keychain_get(&p.keychain_service, &p.keychain_account)
    } else {
        std::fs::read_to_string(p.claude_home().join(".credentials.json")).ok()
    }
}

fn write_live_claude(p: &Places, cred: &str) -> Result<(), String> {
    if p.use_keychain {
        keychain_set(&p.keychain_service, &p.keychain_account, cred)
    } else {
        write_private(&p.claude_home().join(".credentials.json"), cred)
    }
}

fn read_oauth_account(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| v["oauthAccount"].clone())
        .unwrap_or(Value::Null)
}

fn set_oauth_account(path: &Path, account: &Value) -> Result<(), String> {
    let mut doc: Map<String, Value> = match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t)
            .map_err(|e| format!("{} 不是合法的 JSON：{e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
    };
    if account.is_null() {
        doc.remove("oauthAccount");
    } else {
        doc.insert("oauthAccount".into(), account.clone());
    }
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    write_private(path, &text)
}

struct DirLock(PathBuf);

impl DirLock {
    fn take(path: PathBuf, stale: Duration) -> Result<Self, String> {
        let deadline = SystemTime::now() + Duration::from_secs(9);
        loop {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let old = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > stale);
                    if old {
                        let _ = std::fs::remove_dir(&path);
                        continue;
                    }
                    if SystemTime::now() > deadline {
                        return Err(format!("Claude Code 正占着 {}，稍后再试", path.display()));
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(e) => return Err(format!("加不了锁 {}：{e}", path.display())),
            }
        }
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn claude_locks(p: &Places) -> Result<Vec<DirLock>, String> {
    let cred = Duration::from_secs(60);
    Ok(vec![
        DirLock::take(p.claude_home().join(".oauth_refresh.lock"), cred)?,
        DirLock::take(p.home.join(".claude.lock"), cred)?,
        DirLock::take(p.home.join(".claude.json.lock"), Duration::from_secs(10))?,
    ])
}

pub fn compose(target: &str, live: Option<&str>) -> String {
    let Ok(Value::Object(mut t)) = serde_json::from_str::<Value>(target) else {
        return target.to_owned();
    };
    let live: Map<String, Value> = live
        .and_then(|l| serde_json::from_str::<Value>(l).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for k in SHARED_KEYS {
        t.remove(k);
        if let Some(v) = live.get(k) {
            t.insert(k.into(), v.clone());
        }
    }
    Value::Object(t).to_string()
}

pub fn token_credential(token: &str) -> String {
    json!({ "claudeAiOauth": { "accessToken": token, "scopes": SETUP_TOKEN_SCOPES } }).to_string()
}

pub fn token_oauth_account(a: &Account) -> Value {
    let email = a
        .email
        .clone()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| {
            let name: String = a
                .label
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            format!(
                "{}@token.local",
                if name.trim_matches('-').is_empty() {
                    a.id.as_str()
                } else {
                    name.as_str()
                }
            )
        });
    json!({
        "emailAddress": email,
        "displayName": a.label,
        "accountUuid": "",
        "organizationUuid": null,
        "organizationName": null,
    })
}

fn backup_file(dir: &Path, provider: &str) -> PathBuf {
    dir.join(format!("{provider}-default.json"))
}

fn claude_target(
    p: &Places,
    backups: &Path,
    a: Option<&Account>,
) -> Result<(String, Value), String> {
    match a {
        None => {
            let b: Value = std::fs::read_to_string(backup_file(backups, "claude"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .ok_or("没有原来的默认登录的备份，没法切回去")?;
            Ok((
                b["credential"].as_str().unwrap_or_default().to_owned(),
                b["oauthAccount"].clone(),
            ))
        }
        Some(a) if a.kind == "token" => {
            let dir = a.config_dir.as_deref().ok_or("这个账号没有目录")?;
            let tok = std::fs::read_to_string(Path::new(dir).join(TOKEN_FILE))
                .map_err(|e| format!("读不了「{}」的 token：{e}", a.label))?;
            Ok((token_credential(tok.trim()), token_oauth_account(a)))
        }
        Some(a) => {
            let dir = a.config_dir.as_deref().ok_or("这个账号没有目录")?;
            let cred = if p.use_keychain {
                keychain_get(&hashed_service(dir), &p.keychain_account)
            } else {
                std::fs::read_to_string(Path::new(dir).join(".credentials.json")).ok()
            }
            .ok_or_else(|| format!("「{}」还没有登录", a.label))?;
            Ok((
                cred,
                read_oauth_account(&Path::new(dir).join(".claude.json")),
            ))
        }
    }
}

fn claude_save_back(
    p: &Places,
    backups: &Path,
    prev: Option<&Account>,
    live: Option<&str>,
) -> Result<(), String> {
    match prev {
        None => {
            let f = backup_file(backups, "claude");
            let Some(live) = live else { return Ok(()) };
            let doc =
                json!({ "credential": live, "oauthAccount": read_oauth_account(&p.claude_json()) });
            write_private(&f, &doc.to_string())
        }
        Some(a) if a.kind == "login" => {
            let (Some(dir), Some(live)) = (a.config_dir.as_deref(), live) else {
                return Ok(());
            };
            if p.use_keychain {
                keychain_set(&hashed_service(dir), &p.keychain_account, live)
            } else {
                write_private(&Path::new(dir).join(".credentials.json"), live)
            }
        }
        Some(_) => Ok(()),
    }
}

pub fn switch_claude(
    p: &Places,
    backups: &Path,
    prev: Option<&Account>,
    target: Option<&Account>,
) -> Result<(), String> {
    let _locks = claude_locks(p)?;
    let live = read_live_claude(p);
    if prev.is_none() && target.is_none() {
        return Ok(());
    }
    claude_save_back(p, backups, prev, live.as_deref())?;
    let (cred, oauth) = claude_target(p, backups, target)?;
    write_live_claude(p, &compose(&cred, live.as_deref()))?;
    set_oauth_account(&p.claude_json(), &oauth)
}

pub fn switch_codex(
    p: &Places,
    backups: &Path,
    prev: Option<&Account>,
    target: Option<&Account>,
) -> Result<(), String> {
    if prev.is_none() && target.is_none() {
        return Ok(());
    }
    let live_path = p.codex_auth();
    let live = std::fs::read_to_string(&live_path).ok();
    match (prev, &live) {
        (None, Some(l)) => write_private(&backup_file(backups, "codex"), l)?,
        (Some(a), Some(l)) => {
            if let Some(dir) = a.config_dir.as_deref() {
                write_private(&Path::new(dir).join("auth.json"), l)?;
            }
        }
        _ => {}
    }
    let next = match target {
        None => std::fs::read_to_string(backup_file(backups, "codex"))
            .map_err(|_| "没有原来的 Codex 登录的备份，没法切回去".to_owned())?,
        Some(a) => {
            let dir = a.config_dir.as_deref().ok_or("这个账号没有目录")?;
            std::fs::read_to_string(Path::new(dir).join("auth.json"))
                .map_err(|_| format!("「{}」还没有登录", a.label))?
        }
    };
    write_private(&live_path, &next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(home: &Path) -> Places {
        Places {
            home: home.to_owned(),
            keychain_service: "test".into(),
            keychain_account: "t".into(),
            use_keychain: false,
        }
    }

    fn account(id: &str, kind: &str, dir: &Path) -> Account {
        Account {
            id: id.into(),
            provider: "claude".into(),
            label: id.into(),
            config_dir: Some(dir.display().to_string()),
            email: Some(format!("{id}@example.com")),
            plan: None,
            status: "ok".into(),
            disabled: false,
            checked_at: None,
            created_at: String::new(),
            builtin: false,
            kind: kind.into(),
        }
    }

    #[test]
    fn hashed_service_matches_claude_code() {
        assert_eq!(hashed_service("/tmp/x"), "Claude Code-credentials-2e56aa36");
    }

    #[test]
    fn refreshed_default_claude_login_replaces_its_previous_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backups = tmp.path().join("backups");
        let p = places(&home);
        let live_path = home.join(".claude/.credentials.json");
        std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        std::fs::write(
            &live_path,
            r#"{"claudeAiOauth":{"accessToken":"default-old"}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"old@example.com"}}"#,
        )
        .unwrap();
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(TOKEN_FILE), "fixture-other-token").unwrap();
        let other = account("other", "token", &other);
        switch_claude(&p, &backups, None, Some(&other)).unwrap();
        switch_claude(&p, &backups, Some(&other), None).unwrap();
        std::fs::write(
            &live_path,
            r#"{"claudeAiOauth":{"accessToken":"default-refreshed"}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"new@example.com"},"projects":{"fixture":{}}}"#,
        )
        .unwrap();
        switch_claude(&p, &backups, None, Some(&other)).unwrap();
        switch_claude(&p, &backups, Some(&other), None).unwrap();
        let current: Value =
            serde_json::from_str(&std::fs::read_to_string(&live_path).unwrap()).unwrap();
        assert_eq!(current["claudeAiOauth"]["accessToken"], "default-refreshed");
        let metadata: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["oauthAccount"]["emailAddress"], "new@example.com");
        assert!(metadata["projects"]["fixture"].is_object());
    }

    #[test]
    fn refreshed_default_codex_login_replaces_its_previous_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backups = tmp.path().join("backups");
        let p = places(&home);
        let live_path = home.join(".codex/auth.json");
        std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        std::fs::write(&live_path, "default-old").unwrap();
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("auth.json"), "fixture-other-login").unwrap();
        let mut other = account("other", "login", &other);
        other.provider = "codex".into();
        switch_codex(&p, &backups, None, Some(&other)).unwrap();
        switch_codex(&p, &backups, Some(&other), None).unwrap();
        std::fs::write(&live_path, "default-refreshed").unwrap();
        switch_codex(&p, &backups, None, Some(&other)).unwrap();
        switch_codex(&p, &backups, Some(&other), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&live_path).unwrap(),
            "default-refreshed"
        );
    }

    #[test]
    fn switching_to_a_token_and_back_restores_the_original_login() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backups = tmp.path().join("backups");
        let p = places(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let original =
            r#"{"claudeAiOauth":{"accessToken":"orig","refreshToken":"r"},"mcpOAuth":{"x":1}}"#;
        std::fs::write(home.join(".claude/.credentials.json"), original).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"projects":{"/a":{}},"oauthAccount":{"emailAddress":"me@x.com"}}"#,
        )
        .unwrap();

        let dir = tmp.path().join("acc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TOKEN_FILE), "sk-ant-oat01-abc\n").unwrap();
        let tok = account("max", "token", &dir);

        switch_claude(&p, &backups, None, Some(&tok)).unwrap();
        let live: Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(live["claudeAiOauth"]["accessToken"], "sk-ant-oat01-abc");
        assert_eq!(live["claudeAiOauth"]["scopes"], json!(["user:inference"]));
        assert_eq!(live["mcpOAuth"], json!({"x": 1}), "机器共享的 MCP 登录保留");
        let cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(cfg["oauthAccount"]["emailAddress"], "max@example.com");
        assert_eq!(cfg["projects"], json!({"/a": {}}), "其它字段原样保留");
        assert!(!home.join(".claude.lock").exists(), "锁要放掉");

        switch_claude(&p, &backups, Some(&tok), None).unwrap();
        let back: Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(back["claudeAiOauth"]["accessToken"], "orig");
        let cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(cfg["oauthAccount"]["emailAddress"], "me@x.com");
    }

    #[test]
    fn a_browser_login_gets_its_refreshed_credential_back_when_switched_away() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backups = tmp.path().join("backups");
        let p = places(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"orig"}}"#,
        )
        .unwrap();
        let dir = tmp.path().join("work");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"w1"}}"#,
        )
        .unwrap();
        let work = account("work", "login", &dir);

        switch_claude(&p, &backups, None, Some(&work)).unwrap();
        std::fs::write(
            home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"w2"}}"#,
        )
        .unwrap();
        switch_claude(&p, &backups, Some(&work), None).unwrap();
        assert!(
            std::fs::read_to_string(dir.join(".credentials.json"))
                .unwrap()
                .contains("w2")
        );
        assert!(
            std::fs::read_to_string(home.join(".claude/.credentials.json"))
                .unwrap()
                .contains("orig")
        );
    }

    #[test]
    fn codex_auth_is_swapped_and_restored() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backups = tmp.path().join("backups");
        let p = places(&home);
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/auth.json"), "orig").unwrap();
        let dir = tmp.path().join("cx");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("auth.json"), "second").unwrap();
        let mut a = account("cx", "login", &dir);
        a.provider = "codex".into();
        switch_codex(&p, &backups, None, Some(&a)).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".codex/auth.json")).unwrap(),
            "second"
        );
        std::fs::write(home.join(".codex/auth.json"), "second-refreshed").unwrap();
        switch_codex(&p, &backups, Some(&a), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".codex/auth.json")).unwrap(),
            "orig"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("auth.json")).unwrap(),
            "second-refreshed"
        );
    }
}
