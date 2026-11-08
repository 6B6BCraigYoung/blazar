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
    #[serde(default = "not_turned_off")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
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

fn not_turned_off() -> bool {
    true
}

#[must_use]
pub fn shown_by_default() -> bool {
    !cfg!(target_os = "macos")
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            enabled: true,
            chosen: false,
            trying: None,
            stuck_secs: None,
            skipped: None,
        }
    }
}

impl Prefs {
    #[must_use]
    pub fn on(&self) -> bool {
        self.enabled && (self.chosen || shown_by_default())
    }

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
        if self.on() { Launch::Show } else { Launch::Off }
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

    pub fn hung_later(&mut self, now: u64, secs: u64) {
        self.trying.get_or_insert(now);
        self.stuck_secs = Some(self.stuck_secs.map_or(secs, |s| s.max(secs)));
    }

    pub fn hung_now(&mut self, at: u64, secs: u64) {
        let at = self.trying.take().unwrap_or(at);
        let longest = self.stuck_secs.take().map_or(secs, |s| s.max(secs));
        self.enabled = false;
        self.skipped = Some(Skipped {
            at,
            stuck_secs: (longest > 0).then_some(longest),
            noticed: false,
        });
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
        self.chosen = true;
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
            tracing::info!("菜单栏图标没开（设置里可以打开），启动时不建");
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

pub fn hung_later(secs: u64) {
    let at = now();
    with(|p| p.hung_later(at, secs));
}

pub fn stuck(secs: u64) {
    with(|p| p.stuck(secs));
}

pub fn hung_now(secs: u64) {
    let at = now();
    with(|p| p.hung_now(at, secs));
}

pub fn on_exit() {
    if !crate::dev_instance() {
        with(Prefs::exiting);
    }
}

fn view(app: &AppHandle, prefs: &Prefs) -> Value {
    json!({
        "available": true,
        "enabled": prefs.on(),
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

    fn chosen_on() -> Prefs {
        let mut p = Prefs::default();
        p.set_enabled(true);
        p
    }

    #[test]
    fn a_missing_or_broken_file_follows_the_platform_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(FILE), "{not json").unwrap();
        assert_eq!(load(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(FILE), "{}").unwrap();
        assert_eq!(load(dir.path()).on(), shown_by_default());
        assert_eq!(at_launch(Some(dir.path())), shown_by_default());
        assert!(at_launch(None));
    }

    #[test]
    fn on_macos_the_icon_stays_off_until_it_is_turned_on_by_hand() {
        assert_eq!(shown_by_default(), !cfg!(target_os = "macos"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), r#"{"enabled": true}"#).unwrap();
        assert_eq!(
            at_launch(Some(dir.path())),
            shown_by_default(),
            "旧版留下的 enabled 不算手动打开过"
        );
        update(dir.path(), |p| p.set_enabled(true));
        assert!(at_launch(Some(dir.path())), "在设置里打开过就照办");
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(dir.path().join(FILE)).unwrap())
                .unwrap(),
            json!({ "enabled": true, "chosen": true })
        );
        update(dir.path(), |p| p.hung_now(1, 5));
        assert!(
            !at_launch(Some(dir.path())),
            "手动打开过、后来卡住了，照样关掉"
        );
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
        assert!(p.on());
        assert!(p.skipped.is_none());
        assert!(p.notice().is_none());
        assert_eq!(p.launch(), Launch::Show);
    }

    #[test]
    fn a_clean_quit_right_after_launch_is_not_a_stall_but_a_quit_during_one_is() {
        let mut quick = chosen_on();
        quick.begin(1);
        quick.exiting();
        assert_eq!(quick.launch(), Launch::Show);

        let mut stalled = chosen_on();
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
    fn an_icon_that_hangs_long_after_it_was_built_is_skipped_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        update(dir.path(), |p| p.begin(1));
        update(dir.path(), Prefs::settled);
        update(dir.path(), |p| p.hung_later(1_700_000_000, 6));
        update(dir.path(), |p| p.hung_later(1_700_000_030, 66));
        update(dir.path(), |p| p.hung_later(1_700_000_060, 3));
        update(dir.path(), Prefs::exiting);
        assert!(!at_launch(Some(dir.path())), "运行中卡在图标上，下次也跳过");
        let notice = load(dir.path()).notice().cloned().unwrap();
        assert_eq!(notice.at, 1_700_000_000, "记第一次卡住的时间");
        assert_eq!(notice.stuck_secs, Some(66), "记卡得最久的那次");
    }

    #[test]
    fn an_icon_that_hangs_while_running_is_turned_off_at_once_and_stays_off() {
        let dir = tempfile::tempdir().unwrap();
        update(dir.path(), |p| p.begin(1));
        update(dir.path(), Prefs::settled);
        update(dir.path(), |p| p.hung_later(1_700_000_000, 6));
        update(dir.path(), |p| p.hung_now(1_700_000_009, 9));
        let after = load(dir.path());
        assert!(!after.enabled, "恢复后立刻关掉，并记住关闭");
        assert_eq!(after.trying, None);
        assert_eq!(after.stuck_secs, None);
        let notice = after.notice().cloned().unwrap();
        assert_eq!(notice.at, 1_700_000_000, "记第一次发现卡住的时间");
        assert_eq!(notice.stuck_secs, Some(9));
        assert!(
            !at_launch(Some(dir.path())),
            "下次启动也不建，直到在设置里打开"
        );
        assert!(
            load(dir.path()).notice().is_some(),
            "启动不会把还没看过的说明吞掉"
        );
        update(dir.path(), Prefs::noticed);
        assert!(load(dir.path()).notice().is_none(), "原因只提示一次");
    }

    #[test]
    fn a_stall_while_building_is_turned_off_with_the_time_it_took() {
        let mut p = Prefs::default();
        p.begin(7);
        p.stuck(12);
        p.hung_now(20, 0);
        assert!(!p.enabled);
        assert_eq!(
            p.notice().map(|s| (s.at, s.stuck_secs)),
            Some((7, Some(12)))
        );
        assert_eq!(p.launch(), Launch::Off);
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
        update(dir.path(), Prefs::settled);
        assert!(!dir.path().join(FILE).exists(), "没改动就不写文件");
        update(dir.path(), |p| p.set_enabled(false));
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({ "enabled": false, "chosen": true })
        );
    }
}
