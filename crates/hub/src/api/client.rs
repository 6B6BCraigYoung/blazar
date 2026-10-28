use std::io::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::*;

const MAX_REPORT_CHARS: usize = 4000;
const MAX_MESSAGE_CHARS: usize = 2000;
const LINES_PER_MINUTE: u32 = 120;
const ROTATE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Deserialize)]
pub struct ClientEvent {
    kind: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    trail: Vec<String>,
}

struct Budget {
    since: Instant,
    used: u32,
    dropped: u32,
}

static BUDGET: Mutex<Option<Budget>> = Mutex::new(None);

pub async fn report_panic(State(st): State<Shared>, body: String) -> StatusCode {
    let text: String = body.chars().take(MAX_REPORT_CHARS).collect();
    tracing::error!(target: "blazar::web", "前端崩溃：{text}");
    write(
        &logs_dir(&st),
        &[ClientEvent {
            kind: "panic".into(),
            message: text,
            path: String::new(),
            trail: Vec::new(),
        }],
    );
    StatusCode::NO_CONTENT
}

pub async fn report_client(
    State(st): State<Shared>,
    Json(events): Json<Vec<ClientEvent>>,
) -> StatusCode {
    if events.iter().any(|e| e.kind == "panic") {
        tracing::error!(target: "blazar::web", "前端崩溃，详情见 logs/web.log");
    }
    write(&logs_dir(&st), &events);
    StatusCode::NO_CONTENT
}

fn logs_dir(st: &AppState) -> PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, FsPath::to_path_buf)
        .join("logs")
}

fn one_line(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn write(dir: &FsPath, events: &[ClientEvent]) {
    let mut lines = Vec::new();
    {
        let mut budget = BUDGET
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let b = budget.get_or_insert(Budget {
            since: Instant::now(),
            used: 0,
            dropped: 0,
        });
        if b.since.elapsed() > Duration::from_secs(60) {
            if b.dropped > 0 {
                lines.push(format!(
                    "{} dropped - {} events over the limit",
                    Utc::now().to_rfc3339(),
                    b.dropped
                ));
            }
            *b = Budget {
                since: Instant::now(),
                used: 0,
                dropped: 0,
            };
        }
        for e in events {
            if b.used >= LINES_PER_MINUTE {
                b.dropped += 1;
                continue;
            }
            b.used += 1;
            let trail = e
                .trail
                .iter()
                .rev()
                .take(12)
                .rev()
                .map(|p| one_line(p, 200))
                .collect::<Vec<_>>()
                .join(" > ");
            lines.push(format!(
                "{} {} {} {}{}",
                Utc::now().to_rfc3339(),
                one_line(&e.kind, 20),
                if e.path.is_empty() {
                    "-".into()
                } else {
                    one_line(&e.path, 200)
                },
                one_line(&e.message, MAX_MESSAGE_CHARS),
                if trail.is_empty() {
                    String::new()
                } else {
                    format!(" | trail: {trail}")
                },
            ));
        }
    }
    if lines.is_empty() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join("web.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
        let _ = std::fs::rename(&path, dir.join("web.log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all((lines.join("\n") + "\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_line_per_event_with_the_trail() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &[ClientEvent {
                kind: "console".into(),
                message: "boom\nsecond line".into(),
                path: "/nodes".into(),
                trail: vec!["/".into(), "/nodes".into()],
            }],
        );
        let text = std::fs::read_to_string(dir.path().join("web.log")).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("console /nodes boom second line | trail: / > /nodes"));
    }
}
