use std::process::Stdio;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::Shared;
use crate::library::{MAX_FILE, MAX_FILES, front_matter_description, valid_rel};

const ALLOWED_HOSTS: &[&str] = &[
    "skills.sh",
    "skillsmp.com",
    "clawhub.ai",
    "api.github.com",
    "raw.githubusercontent.com",
];
const MAX_TOTAL: usize = 2 * 1024 * 1024;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
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

fn enc_path(p: &str) -> String {
    p.split('/').map(enc).collect::<Vec<_>>().join("/")
}

struct Fetched {
    status: u16,
    body: Vec<u8>,
}

async fn fetch(
    url: &str,
    accept: &str,
    auth: Option<String>,
    max: usize,
) -> Result<Fetched, String> {
    let host = url
        .strip_prefix("https://")
        .and_then(|r| r.split(['/', '?']).next())
        .unwrap_or_default();
    if !ALLOWED_HOSTS.contains(&host) {
        return Err(format!("不在白名单里的主机：{host}"));
    }
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-sS",
        "-L",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--max-time",
        "25",
        "--max-filesize",
        &max.to_string(),
        "-H",
        &format!("Accept: {accept}"),
        "-H",
        "User-Agent: blazar-skills",
        "-w",
        "\n%{http_code}",
    ]);
    if auth.is_some() {
        cmd.args(["-H", "@-"]);
    }
    cmd.arg(url)
        .stdin(if auth.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("起不了 curl：{e}"))?;
    if let (Some(line), Some(mut stdin)) = (auth, child.stdin.take()) {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "请求超时".to_owned())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() && out.stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "连不上 {host}：{}",
            err.trim().chars().take(200).collect::<String>()
        ));
    }

    let mut body = out.stdout;
    let cut = body.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let status = String::from_utf8_lossy(&body[cut..])
        .trim()
        .parse()
        .unwrap_or(0);
    body.truncate(cut);
    Ok(Fetched { status, body })
}

async fn fetch_json(url: &str, auth: Option<String>) -> Result<Value, String> {
    let f = fetch(url, "application/json", auth, 4 * 1024 * 1024).await?;
    let v: Value = serde_json::from_slice(&f.body).unwrap_or(Value::Null);
    if !(200..300).contains(&f.status) {
        let why = v["message"]
            .as_str()
            .or_else(|| v["error"]["message"].as_str())
            .or_else(|| v["error"].as_str())
            .unwrap_or_default();
        return Err(match f.status {
            403 | 429 => format!("对方限流了（{}）：{why}", f.status),
            404 => "没找到（404）".to_owned(),
            s => format!("对方返回 {s}：{why}"),
        });
    }
    Ok(v)
}

async fn setting(st: &Shared, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM kv_settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .filter(|v: &String| !v.is_empty())
}

pub async fn providers(State(st): State<Shared>) -> Response {
    let has_key = setting(&st, "skillsmp_api_key").await.is_some();
    Json(json!([
        { "id": "skills_sh", "label": "skills.sh", "home": "https://skills.sh", "metric": "安装",
          "note": "Vercel 的开放技能目录，按真实安装量排；技能都在 GitHub 仓库里。" },
        { "id": "skillsmp", "label": "SkillsMP", "home": "https://skillsmp.com", "metric": "Star",
          "note": "收录最多的聚合站。匿名每天 50 次搜索，填 API key 后 500 次。", "needs_key": false, "has_key": has_key },
        { "id": "clawhub", "label": "ClawHub", "home": "https://clawhub.ai", "metric": "下载",
          "note": "OpenClaw 的技能仓库，带版本号和安全扫描结果。" },
        { "id": "github", "label": "GitHub 仓库", "home": "https://github.com", "metric": "",
          "note": "任何放着 SKILL.md 的仓库，包括 Claude Code 插件市场的仓库。",
          "featured": ["anthropics/skills", "openai/skills", "vercel-labs/agent-skills", "anthropics/knowledge-work-plugins", "github/awesome-copilot"] },
    ]))
    .into_response()
}

#[derive(Deserialize)]
pub struct KeyBody {
    pub provider: String,

    pub key: String,
}

pub async fn set_key(State(st): State<Shared>, Json(b): Json<KeyBody>) -> Response {
    if b.provider != "skillsmp" {
        return fail(StatusCode::BAD_REQUEST, "这个平台不需要 API key");
    }
    let key = b.key.trim();
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) || key.len() > 200 {
        return fail(StatusCode::BAD_REQUEST, "API key 里不该有空白");
    }
    let r = if key.is_empty() {
        sqlx::query("DELETE FROM kv_settings WHERE key = 'skillsmp_api_key'")
            .execute(st.db.pool())
            .await
    } else {
        sqlx::query(
            "INSERT INTO kv_settings (key, value, updated_at) VALUES ('skillsmp_api_key', ?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await
    };
    match r {
        Ok(_) => Json(json!({ "ok": true, "has_key": !key.is_empty() })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhSource {
    pub repo: String,
    pub path: String,
    pub git_ref: String,
}

fn valid_repo_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn parse_github(src: &str) -> Result<GhSource, String> {
    let s = src.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
        .unwrap_or(s);
    let s = s.trim_end_matches(".git");
    let (s, at_ref) = match s.rsplit_once('@') {
        Some((a, r)) if !r.contains('/') && !r.is_empty() => (a, Some(r)),
        _ => (s, None),
    };
    let mut parts = s.split('/');
    let (owner, repo) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    if !valid_repo_part(owner) || !valid_repo_part(repo) {
        return Err("写成 owner/repo，或者贴 GitHub 的地址".into());
    }
    let rest: Vec<&str> = parts.collect();
    let (git_ref, path) = match rest.as_slice() {
        ["tree" | "blob", r, p @ ..] => ((*r).to_owned(), p.join("/")),
        p => (at_ref.unwrap_or("HEAD").to_owned(), p.join("/")),
    };
    let path = path
        .trim_end_matches("/SKILL.md")
        .trim_end_matches("SKILL.md")
        .trim_matches('/')
        .to_owned();
    if !path.is_empty() && !valid_rel(&path) {
        return Err("仓库里的路径不对".into());
    }
    if git_ref.is_empty()
        || git_ref.len() > 120
        || git_ref.starts_with('-')
        || !git_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        return Err("分支 / 提交写得不对".into());
    }
    Ok(GhSource {
        repo: format!("{owner}/{repo}"),
        path,
        git_ref,
    })
}

pub fn skills_in_tree(tree: &Value, under: &str) -> Vec<String> {
    let mut out: Vec<String> = tree["tree"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["type"] == "blob")
        .filter_map(|t| t["path"].as_str())
        .filter_map(|p| {
            p.strip_suffix("SKILL.md")
                .map(|d| d.trim_end_matches('/').to_owned())
        })
        .filter(|d| under.is_empty() || d == under || d.starts_with(&format!("{under}/")))
        .filter(|d| !d.split('/').any(|s| s == "node_modules" || s == "vendor"))
        .collect();
    out.sort();
    out
}

async fn github_tree(src: &GhSource) -> Result<Value, String> {
    let path = format!(
        "repos/{}/git/trees/{}?recursive=1",
        src.repo,
        enc_path(&src.git_ref)
    );
    match fetch_json(&format!("https://api.github.com/{path}"), None).await {
        Ok(v) => Ok(v),

        Err(e) if e.contains("限流") => {
            let out = tokio::process::Command::new("gh")
                .args(["api", &path])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|_| format!("{e}。装上并登录 GitHub CLI（gh）可以不受这个限制"))?;
            serde_json::from_slice(&out.stdout)
                .map_err(|_| format!("{e}。装上并登录 GitHub CLI（gh）可以不受这个限制"))
        }
        Err(e) => Err(e),
    }
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub provider: String,
    #[serde(default)]
    pub q: String,
}

fn map_skills_sh(v: &Value) -> Vec<Value> {
    v["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let repo = s["source"].as_str()?;
            let skill = s["skillId"].as_str().or_else(|| s["name"].as_str())?;
            parse_github(repo).ok()?;
            Some(json!({
                "provider": "skills_sh", "name": s["name"].as_str().unwrap_or(skill), "description": "",
                "by": repo, "count": s["installs"], "url": format!("https://skills.sh/{}", s["id"].as_str().unwrap_or_default()),
                "install": { "kind": "github", "repo": repo, "skill": skill },
            }))
        })
        .collect()
}

fn map_skillsmp(v: &Value) -> Vec<Value> {
    v["data"]["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let gh = parse_github(s["githubUrl"].as_str()?).ok()?;
            Some(json!({
                "provider": "skillsmp", "name": s["name"], "description": s["description"],
                "by": s["author"], "count": s["stars"], "url": s["skillUrl"],
                "install": { "kind": "github", "repo": gh.repo, "path": gh.path, "ref": gh.git_ref },
            }))
        })
        .collect()
}

fn map_clawhub(v: &Value) -> Vec<Value> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let (owner, slug) = s["install"]["reference"].as_str()?.split_once('/')?;
            if !valid_repo_part(owner) || !valid_repo_part(slug) {
                return None;
            }
            let sk = &s["native"]["skill"];
            Some(json!({
                "provider": "clawhub", "name": slug, "title": s["displayName"],
                "description": sk["summary"].as_str().or_else(|| s["summary"].as_str()).unwrap_or_default(),
                "by": owner, "count": s["downloads"],
                "url": format!("https://clawhub.ai{}", s["canonicalUrl"].as_str().unwrap_or_default()),
                "suspicious": sk["isSuspicious"].as_bool().unwrap_or(false),
                "install": { "kind": "clawhub", "owner": owner, "slug": slug },
            }))
        })
        .collect()
}

pub async fn search(State(st): State<Shared>, Query(q): Query<SearchQuery>) -> Response {
    let term = q.q.trim();
    if term.chars().count() < 2 || term.len() > 120 {
        return Json(json!({ "items": [], "hint": "至少输两个字" })).into_response();
    }
    let t = enc(term);
    let res = match q.provider.as_str() {
        "skills_sh" => fetch_json(
            &format!("https://skills.sh/api/search?q={t}&limit=30"),
            None,
        )
        .await
        .map(|v| map_skills_sh(&v)),
        "skillsmp" => {
            let auth = setting(&st, "skillsmp_api_key")
                .await
                .map(|k| format!("Authorization: Bearer {k}"));
            fetch_json(
                &format!("https://skillsmp.com/api/v1/skills/search?q={t}&limit=30&sortBy=stars"),
                auth,
            )
            .await
            .map(|v| map_skillsmp(&v))
        }
        "clawhub" => fetch_json(
            &format!("https://clawhub.ai/api/v1/search?q={t}&limit=30"),
            None,
        )
        .await
        .map(|v| map_clawhub(&v)),
        _ => Err("不认识的平台".to_owned()),
    };
    match res {
        Ok(mut items) => {
            mark_installed(&st, &mut items).await;
            Json(json!({ "items": items })).into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

async fn mark_installed(st: &Shared, items: &mut [Value]) {
    let have: Vec<String> = sqlx::query_scalar("SELECT LOWER(name) FROM skills")
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    for it in items {
        let n = slugify(it["name"].as_str().unwrap_or_default());
        it["installed"] = json!(have.contains(&n));
    }
}

#[derive(Deserialize)]
pub struct RepoQuery {
    pub source: String,
}

pub async fn repo(State(st): State<Shared>, Query(q): Query<RepoQuery>) -> Response {
    let src = match parse_github(&q.source) {
        Ok(s) => s,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let tree = match github_tree(&src).await {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let dirs = skills_in_tree(&tree, &src.path);
    let mut items: Vec<Value> = dirs
        .iter()
        .take(300)
        .map(|d| {
            let name = if d.is_empty() {
                src.repo.rsplit('/').next().unwrap_or("skill")
            } else {
                d.rsplit('/').next().unwrap_or(d)
            };
            json!({
                "provider": "github", "name": name, "description": "", "by": src.repo, "path": d,
                "url": format!("https://github.com/{}/tree/{}/{}", src.repo, src.git_ref, d),
                "install": { "kind": "github", "repo": src.repo, "path": d, "ref": src.git_ref },
            })
        })
        .collect();
    mark_installed(&st, &mut items).await;
    Json(json!({
        "items": items, "repo": src.repo, "ref": src.git_ref,
        "truncated": tree["truncated"].as_bool().unwrap_or(false) || dirs.len() > 300,
    }))
    .into_response()
}

#[derive(Deserialize, Clone)]
pub struct Install {
    pub kind: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub slug: String,
}

fn slugify(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    s.chars().take(64).collect()
}

fn looks_executable(path: &str) -> bool {
    let p = path.to_lowercase();
    [
        ".sh", ".bash", ".zsh", ".py", ".js", ".mjs", ".ts", ".rb", ".pl", ".ps1", ".bat", ".cmd",
        ".exe",
    ]
    .iter()
    .any(|e| p.ends_with(e))
}

struct Plan {
    name: String,

    files: Vec<(String, u64, String)>,
    skipped: Vec<String>,
    origin: Value,
    version: String,
    warnings: Vec<String>,
}

async fn plan_github(i: &Install) -> Result<Plan, String> {
    let mut src = parse_github(&i.repo)?;
    if let Some(r) = i.git_ref.as_deref().filter(|r| !r.is_empty()) {
        src = parse_github(&format!("{}@{r}", src.repo))?;
    }
    let tree = github_tree(&src).await?;
    let dirs = skills_in_tree(&tree, "");

    let dir = match (i.path.as_deref(), i.skill.as_deref()) {
        (Some(p), _) if dirs.iter().any(|d| d == p.trim_matches('/')) => {
            p.trim_matches('/').to_owned()
        }
        (_, Some(sk)) => dirs
            .iter()
            .find(|d| d.rsplit('/').next() == Some(sk))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "在 {} 里没找到叫 {sk} 的技能目录（仓库可能改过结构）",
                    src.repo
                )
            })?,
        _ => return Err("这个目录里没有 SKILL.md".into()),
    };

    let sha = fetch(
        &format!(
            "https://api.github.com/repos/{}/commits/{}",
            src.repo,
            enc_path(&src.git_ref)
        ),
        "application/vnd.github.sha",
        None,
        200,
    )
    .await
    .ok()
    .filter(|f| f.status == 200)
    .map(|f| String::from_utf8_lossy(&f.body).trim().to_owned())
    .filter(|s| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()));
    let pin = sha.clone().unwrap_or_else(|| src.git_ref.clone());
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for t in tree["tree"].as_array().into_iter().flatten() {
        let (Some(p), true) = (t["path"].as_str(), t["type"] == "blob") else {
            continue;
        };
        let Some(rel) = p.strip_prefix(&prefix) else {
            continue;
        };

        if dirs
            .iter()
            .any(|d| d != &dir && d.starts_with(&prefix) && p.starts_with(&format!("{d}/")))
        {
            continue;
        }
        let size = t["size"].as_u64().unwrap_or(0);
        if !valid_rel(rel) || size as usize > MAX_FILE || files.len() >= MAX_FILES {
            skipped.push(rel.to_owned());
            continue;
        }
        files.push((
            rel.to_owned(),
            size,
            format!(
                "https://raw.githubusercontent.com/{}/{}/{}",
                src.repo,
                enc_path(&pin),
                enc_path(p)
            ),
        ));
    }
    let name = slugify(if dir.is_empty() {
        src.repo.rsplit('/').next().unwrap_or("skill")
    } else {
        dir.rsplit('/').next().unwrap_or(&dir)
    });
    let mut warnings = Vec::new();
    if sha.is_none() {
        warnings.push("没拿到提交号，版本只能记成分支名（以后无法确认装的是哪一版）".to_owned());
    }
    if tree["truncated"].as_bool().unwrap_or(false) {
        warnings.push("仓库太大，GitHub 只给了部分文件清单，可能有文件没列出来".to_owned());
    }
    Ok(Plan {
        name,
        files,
        skipped,
        origin: json!({ "provider": "github", "repo": src.repo, "path": dir, "ref": src.git_ref,
                        "url": format!("https://github.com/{}/tree/{pin}/{dir}", src.repo) }),
        version: pin,
        warnings,
    })
}

async fn plan_clawhub(i: &Install) -> Result<Plan, String> {
    if !valid_repo_part(&i.owner) || !valid_repo_part(&i.slug) {
        return Err("ClawHub 的技能标识不对".into());
    }
    let (slug, owner) = (enc(&i.slug), enc(&i.owner));
    let detail = fetch_json(
        &format!("https://clawhub.ai/api/v1/skills/{slug}?ownerHandle={owner}"),
        None,
    )
    .await?;
    let version = detail["latestVersion"]["version"]
        .as_str()
        .ok_or("ClawHub 没给出版本号")?
        .to_owned();
    let ver = fetch_json(
        &format!(
            "https://clawhub.ai/api/v1/skills/{slug}/versions/{}?ownerHandle={owner}",
            enc(&version)
        ),
        None,
    )
    .await?;
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for f in ver["version"]["files"].as_array().into_iter().flatten() {
        let Some(p) = f["path"].as_str() else {
            continue;
        };
        let size = f["size"].as_u64().unwrap_or(0);
        if !valid_rel(p) || size as usize > MAX_FILE || files.len() >= MAX_FILES {
            skipped.push(p.to_owned());
            continue;
        }
        files.push((
            p.to_owned(),
            size,
            format!("https://clawhub.ai/api/v1/skills/{slug}/file?path={}&ownerHandle={owner}&version={}", enc(p), enc(&version)),
        ));
    }
    let mut warnings = Vec::new();
    let sec = &ver["version"]["security"];
    if !sec.is_null() {
        let verdict = sec["status"]
            .as_str()
            .or_else(|| sec["verdict"].as_str())
            .unwrap_or_default();
        if !verdict.is_empty() && !matches!(verdict, "clean" | "passed" | "safe" | "ok") {
            warnings.push(format!("ClawHub 的安全扫描结果：{verdict}"));
        }
    }
    Ok(Plan {
        name: slugify(&i.slug),
        files,
        skipped,
        origin: json!({ "provider": "clawhub", "owner": i.owner, "slug": i.slug,
                        "url": format!("https://clawhub.ai/{}/skills/{}", i.owner, i.slug) }),
        version,
        warnings,
    })
}

async fn plan(i: &Install) -> Result<Plan, String> {
    let p = match i.kind.as_str() {
        "github" => plan_github(i).await?,
        "clawhub" => plan_clawhub(i).await?,
        _ => return Err("不认识的安装方式".into()),
    };
    if p.name.is_empty() || !crate::library::valid_name(&p.name) {
        return Err("这个技能的名字没法当目录名".into());
    }
    if !p.files.iter().any(|(f, ..)| f == "SKILL.md") {
        return Err("里面没有 SKILL.md，不是一个技能".into());
    }
    if p.files.iter().map(|(_, s, _)| *s as usize).sum::<usize>() > MAX_TOTAL {
        return Err("这个技能的文件加起来超过 2MB，不在这里装".into());
    }
    Ok(p)
}

async fn text_of(url: &str) -> Result<Option<String>, String> {
    let f = fetch(url, "*/*", None, MAX_FILE).await?;
    if f.status != 200 {
        return Err(format!("取文件失败（{}）", f.status));
    }

    Ok(String::from_utf8(f.body).ok().filter(|t| !t.contains('\0')))
}

pub async fn preview(State(st): State<Shared>, Json(i): Json<Install>) -> Response {
    let p = match plan(&i).await {
        Ok(p) => p,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let md_url = p
        .files
        .iter()
        .find(|(f, ..)| f == "SKILL.md")
        .map(|(.., u)| u.clone())
        .unwrap_or_default();
    let skill_md = match text_of(&md_url).await {
        Ok(Some(t)) => t,
        Ok(None) => return fail(StatusCode::BAD_GATEWAY, "SKILL.md 不是文本"),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE name = ?1")
        .bind(&p.name)
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0);
    let mut warnings = p.warnings.clone();
    let scripts: Vec<&str> = p
        .files
        .iter()
        .map(|(f, ..)| f.as_str())
        .filter(|f| looks_executable(f))
        .collect();
    if !scripts.is_empty() {
        warnings.insert(
            0,
            format!(
                "带了 {} 个脚本（{}）：agent 用这个技能时可能会执行它们，装之前看一眼",
                scripts.len(),
                scripts
                    .iter()
                    .take(4)
                    .copied()
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        );
    }
    if !p.skipped.is_empty() {
        warnings.push(format!(
            "{} 个文件不会装进来（二进制、超过 256KB 或路径不合规）",
            p.skipped.len()
        ));
    }
    Json(json!({
        "name": p.name, "description": front_matter_description(&skill_md), "skill_md": skill_md,
        "files": p.files.iter().map(|(f, s, _)| json!({ "path": f, "size": s, "script": looks_executable(f) })).collect::<Vec<_>>(),
        "origin": p.origin, "version": p.version, "warnings": warnings, "exists": exists > 0,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct InstallBody {
    pub install: Install,

    #[serde(default)]
    pub conflict: String,
}

pub async fn install(State(st): State<Shared>, Json(b): Json<InstallBody>) -> Response {
    let p = match plan(&b.install).await {
        Ok(p) => p,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let mut files = Vec::new();
    let mut skipped = p.skipped.clone();
    for (rel, _, url) in &p.files {
        match text_of(url).await {
            Ok(Some(t)) => files.push((rel.clone(), t)),
            Ok(None) => skipped.push(rel.clone()),
            Err(e) if rel == "SKILL.md" => return fail(StatusCode::BAD_GATEWAY, e),
            Err(_) => skipped.push(rel.clone()),
        }
    }
    let Some(md) = files
        .iter()
        .find(|(f, _)| f == "SKILL.md")
        .map(|(_, c)| c.clone())
    else {
        return fail(StatusCode::BAD_GATEWAY, "SKILL.md 没取下来");
    };
    let mut name = p.name.clone();
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM skills WHERE name = ?1")
        .bind(&name)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();

    let mut keep_agents: Vec<String> = Vec::new();
    if let Some(old) = &existing {
        match b.conflict.as_str() {
            "overwrite" => {
                keep_agents =
                    sqlx::query_scalar("SELECT agent_id FROM agent_skills WHERE skill_id = ?1")
                        .bind(old)
                        .fetch_all(st.db.pool())
                        .await
                        .unwrap_or_default();
                let _ = sqlx::query("DELETE FROM skills WHERE id = ?1")
                    .bind(old)
                    .execute(st.db.pool())
                    .await;
            }
            "rename" => {
                let mut n = 2;
                loop {
                    let cand = format!("{}-{n}", p.name);
                    let taken: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE name = ?1")
                            .bind(&cand)
                            .fetch_one(st.db.pool())
                            .await
                            .unwrap_or(0);
                    if taken == 0 {
                        name = cand;
                        break;
                    }
                    n += 1;
                }
            }
            _ => {
                return fail(
                    StatusCode::CONFLICT,
                    format!("库里已经有一个叫 {name} 的技能了"),
                );
            }
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let r = sqlx::query(
        "INSERT INTO skills (id, name, description, source, origin, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
    )
    .bind(&id)
    .bind(&name)
    .bind(front_matter_description(&md))
    .bind(p.origin["url"].as_str())
    .bind(p.origin.to_string())
    .bind(&p.version)
    .bind(&now)
    .execute(st.db.pool())
    .await;
    if let Err(e) = r {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    for (path, content) in &files {
        let _ =
            sqlx::query("INSERT INTO skill_files (skill_id, path, content) VALUES (?1, ?2, ?3)")
                .bind(&id)
                .bind(path)
                .bind(content)
                .execute(st.db.pool())
                .await;
    }
    for a in keep_agents {
        let _ =
            sqlx::query("INSERT OR IGNORE INTO agent_skills (agent_id, skill_id) VALUES (?1, ?2)")
                .bind(a)
                .bind(&id)
                .execute(st.db.pool())
                .await;
    }
    Json(json!({ "id": id, "name": name, "files": files.len(), "skipped": skipped, "version": p.version })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_sources_in_all_their_spellings() {
        let g = |s: &str| parse_github(s).unwrap();
        assert_eq!(
            g("anthropics/skills"),
            GhSource {
                repo: "anthropics/skills".into(),
                path: String::new(),
                git_ref: "HEAD".into()
            }
        );
        assert_eq!(
            g("https://github.com/anthropics/skills/").repo,
            "anthropics/skills"
        );
    }

    #[test]
    fn github_sources_with_paths_and_refs() {
        let g = |s: &str| parse_github(s).unwrap();
        let t = g("https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf");
        assert_eq!(
            (t.repo.as_str(), t.git_ref.as_str(), t.path.as_str()),
            ("openclaw/openclaw", "main", "skills/nano-pdf")
        );
        let b = g("https://github.com/a/b/blob/v1.2/skills/x/SKILL.md");
        assert_eq!((b.git_ref.as_str(), b.path.as_str()), ("v1.2", "skills/x"));
        assert_eq!(g("a/b/skills/x").path, "skills/x");
        assert_eq!(g("a/b@dev").git_ref, "dev");
        assert_eq!(g("a/b.git").repo, "a/b");
        for bad in [
            "",
            "justone",
            "a/b/../../etc",
            "a/..",
            "a b/c",
            "a/b@--upload-pack=x",
            "https://evil.com/a/b",
        ] {
            assert!(parse_github(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn skills_are_found_in_a_repo_tree() {
        let tree = json!({ "tree": [
            { "path": "README.md", "type": "blob" },
            { "path": "skills/pdf/SKILL.md", "type": "blob" },
            { "path": "skills/pdf/scripts/fill.py", "type": "blob" },
            { "path": "skills/docx/SKILL.md", "type": "blob" },
            { "path": "plugins/x/skills/deep/SKILL.md", "type": "blob" },
            { "path": "node_modules/dep/skills/evil/SKILL.md", "type": "blob" },
            { "path": "skills/pdf", "type": "tree" },
        ]});
        assert_eq!(
            skills_in_tree(&tree, ""),
            vec!["plugins/x/skills/deep", "skills/docx", "skills/pdf"]
        );
        assert_eq!(
            skills_in_tree(&tree, "skills"),
            vec!["skills/docx", "skills/pdf"]
        );
        assert_eq!(
            skills_in_tree(
                &json!({ "tree": [{ "path": "SKILL.md", "type": "blob" }] }),
                ""
            ),
            vec![""]
        );
    }

    #[test]
    fn provider_results_map_to_one_shape() {
        let sh = json!({ "skills": [
            { "id": "anthropics/skills/pdf", "skillId": "pdf", "name": "pdf", "installs": 198_460, "source": "anthropics/skills" },
            { "id": "x", "skillId": "y", "name": "y", "installs": 1, "source": "not a repo" },
        ]});
        let m = map_skills_sh(&sh);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0]["install"],
            json!({ "kind": "github", "repo": "anthropics/skills", "skill": "pdf" })
        );

        let mp = json!({ "data": { "skills": [{ "name": "nano-pdf", "author": "openclaw", "description": "d", "stars": 9,
            "githubUrl": "https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf", "skillUrl": "https://skillsmp.com/x" }] } });
        assert_eq!(
            map_skillsmp(&mp)[0]["install"],
            json!({ "kind": "github", "repo": "openclaw/openclaw", "path": "skills/nano-pdf", "ref": "main" })
        );

        let ch = json!({ "results": [
            { "displayName": "Pdf", "downloads": 5, "canonicalUrl": "/awspace/skills/pdf", "install": { "reference": "awspace/pdf" },
              "native": { "skill": { "summary": "s", "isSuspicious": true } } },
            { "displayName": "Bad", "install": { "reference": "../x/y" } },
        ]});
        let c = map_clawhub(&ch);
        assert_eq!(c.len(), 1);
        assert_eq!(
            c[0]["install"],
            json!({ "kind": "clawhub", "owner": "awspace", "slug": "pdf" })
        );
        assert_eq!(c[0]["suspicious"], true);
    }

    #[test]
    fn names_become_directory_safe_and_urls_stay_on_the_allow_list() {
        assert_eq!(slugify("React PDF!"), "react-pdf");
        assert_eq!(slugify("  --a__b--  "), "a__b");
        assert_eq!(slugify("在建工程"), "");
        assert_eq!(enc("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(enc_path("skills/my skill"), "skills/my%20skill");
        assert!(looks_executable("scripts/run.SH") && !looks_executable("reference.md"));
    }

    #[tokio::test]
    async fn fetch_refuses_hosts_off_the_allow_list() {
        for bad in [
            "https://evil.example/x",
            "http://skills.sh/api",
            "https://skills.sh.evil.com/",
            "file:///etc/passwd",
        ] {
            assert!(fetch(bad, "*/*", None, 10).await.is_err(), "{bad}");
        }
    }
}
