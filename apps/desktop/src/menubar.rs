use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::backend::Hub;

pub const FILE: &str = "menubar.json";

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default = "shown_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trying: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck_secs: Option<u64>,
    #[serde(default)]
    pub noticed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Show,
    Off,
    Skip { stuck_secs: Option<u64> },
}

fn shown_by_default() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            enabled: true,
            trying: None,
            stuck_secs: None,
            skipped: None,
        }
    }
}

impl Prefs {
    pub fn launch(&mut self) -> Launch {
        if let Some(at) = self.trying.take() {
            let stuck_secs = self.stuck_secs.take();
            self.enabled = false;
            self.skipped = Some(Skipped {
                at,
                stuck_secs,
                noticed: false,
            });
            return Launch::Skip { stuck_secs };
        }
        if self.enabled {
            Launch::Show
        } else {
            Launch::Off
        }
    }

    pub fn begin(&mut self, now: u64) {
        self.trying = Some(now);
        self.stuck_secs = None;
    }

    pub fn stuck(&mut self, secs: u64) {
        if self.trying.is_some() {
            self.stuck_secs = Some(secs);
        }
    }

    pub fn settled(&mut self) {
        self.trying = None;
        self.stuck_secs = None;
    }

    pub fn exiting(&mut self) {
        if self.stuck_secs.is_none() {
            self.trying = None;
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.settled();
        if on {
            self.skipped = None;
        }
    }

    pub fn noticed(&mut self) {
        if let Some(s) = &mut self.skipped {
            s.noticed = true;
        }
    }

    pub fn notice(&self) -> Option<&Skipped> {
        self.skipped.as_ref().filter(|s| !s.noticed)
    }
}

pub fn load(dir: &Path) -> Prefs {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store(dir: &Path, prefs: &Prefs) {
    let Ok(text) = serde_json::to_string_pretty(prefs) else {
        return;
    };
    let tmp = dir.join(format!("{FILE}.tmp"));
    let saved = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, dir.join(FILE)));
    if let Err(e) = saved {
        tracing::warn!("菜单栏图标的设置没存上：{e}");
    }
}

pub fn update<T>(dir: &Path, change: impl FnOnce(&mut Prefs) -> T) -> T {
    let _held = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut prefs = load(dir);
    let before = prefs.clone();
    let out = change(&mut prefs);
    if prefs != before {
        store(dir, &prefs);
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn at_launch(dir: Option<&Path>) -> bool {
    let Some(dir) = dir else {
        return true;
    };
    match update(dir, Prefs::launch) {
        Launch::Show => true,
        Launch::Off => {
            tracing::info!("菜单栏图标在设置里关掉了，启动时不建");
            false
        }
        Launch::Skip {
            stuck_secs: Some(secs),
        } => {
            tracing::warn!("上次建菜单栏图标时主线程卡了 {secs} 秒，这次启动不建菜单栏图标");
            false
        }
        Launch::Skip { stuck_secs: None } => {
            tracing::warn!("上次建菜单栏图标没完成程序就退出了，这次启动不建菜单栏图标");
            false
        }
    }
}

fn with(change: impl FnOnce(&mut Prefs)) {
    if let Ok(dir) = crate::profile::data_dir() {
        update(&dir, change);
    }
}

pub fn trying() {
    with(|p| p.begin(now()));
}

pub fn settled() {
    with(Prefs::settled);
}

pub fn stuck(secs: u64) {
    with(|p| p.stuck(secs));
}

pub fn on_exit() {
    if !crate::dev_instance() {
        with(Prefs::exiting);
    }
}

fn view(app: &AppHandle, prefs: &Prefs) -> Value {
    json!({
        "available": true,
        "enabled": prefs.enabled,
        "shown": app.tray_by_id(crate::menu::TRAY_ID).is_some(),
        "notice": prefs.notice().map(|s| json!({ "at": s.at, "stuck_secs": s.stuck_secs })),
    })
}

fn dir_or_err() -> Result<PathBuf, String> {
    crate::profile::data_dir().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn menubar(app: AppHandle) -> Result<Value, String> {
    if crate::dev_instance() {
        return Ok(json!({ "available": false }));
    }
    Ok(view(&app, &load(&dir_or_err()?)))
}

#[tauri::command]
pub fn set_menubar(
    app: AppHandle,
    hub: State<'_, Arc<Hub>>,
    enabled: bool,
) -> Result<Value, String> {
    if crate::dev_instance() {
        return Err("开发实例不放菜单栏图标".into());
    }
    let dir = dir_or_err()?;
    update(&dir, |p| p.set_enabled(enabled));
    if enabled {
        if let Err(e) = crate::menu::show_tray(&app) {
            update(&dir, |p| p.set_enabled(false));
            tracing::warn!("菜单栏图标没建起来：{e}");
            return Err(format!("菜单栏图标没建起来：{e}"));
        }
        hub.accounts_changed();
        tracing::info!("菜单栏图标已打开");
    } else {
        crate::menu::hide_tray(&app);
        tracing::info!("菜单栏图标已关掉，启动时不再建");
    }
    Ok(view(&app, &load(&dir)))
}

#[tauri::command]
pub fn menubar_noticed() -> Result<(), String> {
    update(&dir_or_err()?, Prefs::noticed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_broken_file_means_the_icon_is_shown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(FILE), "{not json").unwrap();
        assert_eq!(load(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(FILE), "{}").unwrap();
        assert!(load(dir.path()).enabled);
        assert!(at_launch(Some(dir.path())));
        assert!(at_launch(None));
    }

    #[test]
    fn a_launch_that_never_got_past_the_icon_skips_it_next_time_and_says_why_once() {
        let dir = tempfile::tempdir().unwrap();
        update(dir.path(), |p| p.begin(1_700_000_000));
        update(dir.path(), |p| p.stuck(1440));
        assert!(!at_launch(Some(dir.path())), "上次卡在图标上，这次跳过");
        let after = load(dir.path());
        assert!(!after.enabled);
        assert_eq!(after.trying, None);
        let notice = after.notice().unwrap();
        assert_eq!(notice.at, 1_700_000_000);
        assert_eq!(notice.stuck_secs, Some(1440));
        update(dir.path(), Prefs::noticed);
        assert!(load(dir.path()).notice().is_none(), "原因只提示一次");
        assert!(
            !at_launch(Some(dir.path())),
            "跳过之后保持关闭，直到在设置里打开"
        );
    }

    #[test]
    fn a_crash_while_building_the_icon_also_skips_it_without_a_duration() {
        let mut p = Prefs::default();
        p.begin(5);
        assert_eq!(p.launch(), Launch::Skip { stuck_secs: None });
        assert_eq!(p.notice().map(|s| s.stuck_secs), Some(None));
    }

    #[test]
    fn turning_it_off_keeps_launch_away_from_the_icon_and_turning_it_on_clears_the_notice() {
        let dir = tempfile::tempdir().unwrap();
        update(dir.path(), |p| p.set_enabled(false));
        assert!(!at_launch(Some(dir.path())));
        assert!(load(dir.path()).notice().is_none(), "自己关掉的不提示");
        let mut p = Prefs::default();
        p.begin(1);
        p.stuck(9);
        p.launch();
        assert!(p.notice().is_some());
        p.set_enabled(true);
        assert!(p.enabled);
        assert!(p.skipped.is_none());
        assert!(p.notice().is_none());
        assert_eq!(p.launch(), Launch::Show);
    }

    #[test]
    fn a_clean_quit_right_after_launch_is_not_a_stall_but_a_quit_during_one_is() {
        let mut quick = Prefs::default();
        quick.begin(1);
        quick.exiting();
        assert_eq!(quick.launch(), Launch::Show);

        let mut stalled = Prefs::default();
        stalled.begin(1);
        stalled.stuck(12);
        stalled.exiting();
        assert_eq!(
            stalled.launch(),
            Launch::Skip {
                stuck_secs: Some(12)
            }
        );
    }

    #[test]
    fn stuck_time_only_counts_while_the_icon_is_being_built() {
        let mut p = Prefs::default();
        p.stuck(30);
        assert_eq!(p.stuck_secs, None);
        p.begin(1);
        p.stuck(30);
        p.settled();
        assert_eq!(p, Prefs::default());
    }

    #[test]
    fn the_file_round_trips_and_stays_small() {
        let dir = tempfile::tempdir().unwrap();
        update(dir.path(), |p| p.set_enabled(true));
        assert!(!dir.path().join(FILE).exists(), "没改动就不写文件");
        update(dir.path(), |p| p.set_enabled(false));
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({ "enabled": false })
        );
    }
}
