use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::AppHandle;

const TICK: Duration = Duration::from_millis(500);
const BEAT: Duration = Duration::from_secs(1);
pub const STUCK: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_secs(2);
const REPORT_EVERY: Duration = Duration::from_secs(30);
const KEEP_SAMPLES: usize = 4;
const RELAUNCH_AFTER: Duration = Duration::from_secs(60);
const TRAY_FRAMES: [&str; 3] = ["NSSceneStatusItem", "NSStatusItem", "NSStatusBar"];

static TRAY_HANG: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Window,
    Menu,
    Tray,
    AccountMenu,
}

impl Stage {
    const ALL: [Self; 5] = [
        Self::Idle,
        Self::Window,
        Self::Menu,
        Self::Tray,
        Self::AccountMenu,
    ];

    fn code(self) -> u8 {
        Self::ALL
            .iter()
            .position(|s| *s == self)
            .and_then(|i| u8::try_from(i).ok())
            .unwrap_or(0)
    }

    fn of(code: u8) -> Self {
        Self::ALL
            .get(usize::from(code))
            .copied()
            .unwrap_or(Self::Idle)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "不在已标记的阶段",
            Self::Window => "建主窗口",
            Self::Menu => "建菜单",
            Self::Tray => "建菜单栏图标",
            Self::AccountMenu => "刷新账号菜单",
        }
    }
}

static STAGE: AtomicU8 = AtomicU8::new(0);

pub fn stage() -> Stage {
    Stage::of(STAGE.load(Ordering::SeqCst))
}

pub struct Marked {
    set: Stage,
    prev: Stage,
}

pub fn mark(stage: Stage) -> Marked {
    let prev = self::stage();
    if prev == Stage::Tray {
        return Marked {
            set: Stage::Tray,
            prev: Stage::Tray,
        };
    }
    STAGE.store(stage.code(), Ordering::SeqCst);
    Marked { set: stage, prev }
}

impl Drop for Marked {
    fn drop(&mut self) {
        let _ = STAGE.compare_exchange(
            self.set.code(),
            self.prev.code(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Fine,
    Stuck { waited: Duration, stage: Stage },
    StillStuck { waited: Duration, stage: Stage },
    Recovered { stuck: Duration, stage: Stage },
}

#[derive(Debug, Default)]
pub struct Monitor {
    sent: u64,
    posted_at: Option<Instant>,
    stall: Option<(Instant, Stage)>,
    reported_at: Option<Instant>,
}

impl Monitor {
    pub fn observe(&mut self, answered: u64, now: Instant, stage: Stage) -> Seen {
        let Some(posted) = self.posted_at else {
            return Seen::Fine;
        };
        if answered >= self.sent {
            return match self.stall.take() {
                Some((since, stage)) => Seen::Recovered {
                    stuck: now.saturating_duration_since(since),
                    stage,
                },
                None => Seen::Fine,
            };
        }
        let waited = now.saturating_duration_since(posted);
        match self.stall {
            None if waited >= STUCK => {
                self.stall = Some((posted, stage));
                self.reported_at = Some(now);
                Seen::Stuck { waited, stage }
            }
            Some((_, stage))
                if self
                    .reported_at
                    .is_some_and(|t| now.saturating_duration_since(t) >= REPORT_EVERY) =>
            {
                self.reported_at = Some(now);
                Seen::StillStuck { waited, stage }
            }
            _ => Seen::Fine,
        }
    }

    pub fn answered(&self, answered: u64) -> bool {
        self.posted_at.is_some() && answered >= self.sent
    }

    pub fn due(&self, answered: u64, now: Instant) -> bool {
        answered >= self.sent
            && self
                .posted_at
                .is_none_or(|p| now.saturating_duration_since(p) >= BEAT)
    }

    pub fn post(&mut self, now: Instant) -> u64 {
        self.sent += 1;
        self.posted_at = Some(now);
        self.sent
    }

    pub fn posted_at(&self) -> Option<Instant> {
        self.posted_at
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TrayWatch {
    active: bool,
    built: Option<Instant>,
    stalled: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Settle {
    Wait,
    Healthy,
    Stalled,
}

impl TrayWatch {
    pub fn settle(&self, answered_post: Option<Instant>) -> Settle {
        if !self.active {
            return Settle::Wait;
        }
        let (Some(built), Some(posted)) = (self.built, answered_post) else {
            return Settle::Wait;
        };
        if posted < built + SETTLE {
            Settle::Wait
        } else if self.stalled {
            Settle::Stalled
        } else {
            Settle::Healthy
        }
    }
}

static TRAY: Mutex<TrayWatch> = Mutex::new(TrayWatch {
    active: false,
    built: None,
    stalled: false,
});

fn tray() -> MutexGuard<'static, TrayWatch> {
    TRAY.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn tray_begins() {
    *tray() = TrayWatch {
        active: true,
        built: None,
        stalled: false,
    };
    STAGE.store(Stage::Tray.code(), Ordering::SeqCst);
}

pub fn tray_built() {
    tray().built = Some(Instant::now());
}

pub fn tray_dropped() {
    *tray() = TrayWatch::default();
    release_tray_stage();
}

fn release_tray_stage() {
    let _ = STAGE.compare_exchange(
        Stage::Tray.code(),
        Stage::Idle.code(),
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
}

pub fn start(app: AppHandle, dir: Option<PathBuf>) {
    let spawned = std::thread::Builder::new()
        .name("blazar-main-watchdog".into())
        .spawn(move || watch(&app, dir.as_deref()));
    if let Err(e) = spawned {
        tracing::warn!("主线程看门狗没起来：{e}");
    }
}

fn watch(app: &AppHandle, dir: Option<&Path>) {
    let answered = Arc::new(AtomicU64::new(0));
    let mut monitor = Monitor::default();
    loop {
        let now = Instant::now();
        let got = answered.load(Ordering::SeqCst);
        match monitor.observe(got, now, stage()) {
            Seen::Fine => {}
            Seen::Stuck { waited, stage } => {
                tracing::warn!(
                    stage = stage.label(),
                    "主线程 {} 秒没响应，卡在：{}",
                    waited.as_secs(),
                    stage.label()
                );
                TRAY_HANG.store(false, Ordering::SeqCst);
                if stage == Stage::Tray {
                    tray().stalled = true;
                    crate::menubar::stuck(waited.as_secs());
                }
                sample(dir, waited);
            }
            Seen::StillStuck { waited, stage } => {
                tracing::warn!(
                    stage = stage.label(),
                    "主线程仍然没响应，已经 {} 秒，卡在：{}",
                    waited.as_secs(),
                    stage.label()
                );
                let by_tray = TRAY_HANG.load(Ordering::SeqCst);
                if stage == Stage::Tray {
                    crate::menubar::stuck(waited.as_secs());
                } else if by_tray {
                    crate::menubar::hung_later(waited.as_secs());
                }
                if (stage == Stage::Tray || by_tray)
                    && waited >= RELAUNCH_AFTER
                    && !crate::dev_instance()
                {
                    relaunch(waited);
                }
            }
            Seen::Recovered { stuck, stage } => {
                tracing::warn!(
                    stage = stage.label(),
                    "主线程恢复响应，卡了 {} 秒，卡在：{}",
                    stuck.as_secs(),
                    stage.label()
                );
                if stage == Stage::Tray {
                    crate::menubar::stuck(stuck.as_secs());
                } else if TRAY_HANG.swap(false, Ordering::SeqCst) {
                    crate::menubar::hung_later(stuck.as_secs());
                }
            }
        }
        if monitor.answered(got) {
            let verdict = tray().settle(monitor.posted_at());
            match verdict {
                Settle::Wait => {}
                Settle::Healthy => {
                    tray_dropped();
                    crate::menubar::settled();
                }
                Settle::Stalled => {
                    tray_dropped();
                    tracing::warn!("这次建菜单栏图标时主线程卡住过，下次启动不建菜单栏图标");
                }
            }
        }
        if monitor.due(got, now) {
            let seq = monitor.post(now);
            let flag = answered.clone();
            if app
                .run_on_main_thread(move || flag.store(seq, Ordering::SeqCst))
                .is_err()
            {
                return;
            }
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(unix)]
fn relaunch(waited: Duration) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    tracing::warn!(
        "主线程卡在菜单栏图标上 {} 秒，不会自己恢复，重开 Blazar（这次不建菜单栏图标）",
        waited.as_secs()
    );
    let me = std::process::id().to_string();
    let spawned = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec \"$2\"",
            "blazar-relaunch",
        ])
        .arg(&me)
        .arg(exe)
        .env_remove(crate::hub_mode::URL_ENV)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", me.as_str()])
                .status();
            std::process::exit(0);
        }
        Err(e) => tracing::warn!("重开失败，只能手动退出再打开：{e}"),
    }
}

#[cfg(not(unix))]
fn relaunch(waited: Duration) {
    let _ = waited;
}

fn thread_header(line: &str) -> bool {
    let rest = line.trim_start_matches(' ');
    line.len() - rest.len() <= 4 && rest.starts_with(|c: char| c.is_ascii_digit())
}

fn leading_count(line: &str) -> Option<u64> {
    let rest = line.trim_start_matches([' ', '+', '!', ':', '|']);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn tray_in_main_stack(text: &str) -> bool {
    let mut lines = text
        .lines()
        .skip_while(|l| !l.contains("com.apple.main-thread"));
    let Some(total) = lines.next().and_then(leading_count).filter(|t| *t > 0) else {
        return false;
    };
    let held = lines
        .take_while(|l| !l.trim().is_empty() && !thread_header(l))
        .filter(|l| TRAY_FRAMES.iter().any(|f| l.contains(f)))
        .filter_map(leading_count)
        .max()
        .unwrap_or(0);
    held * 5 >= total * 4
}

#[cfg(target_os = "macos")]
fn sample(dir: Option<&Path>, waited: Duration) {
    let Some(logs) = dir.map(|d| d.join("logs")) else {
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = logs.join(format!("hang-{stamp}.txt"));
    let pid = std::process::id().to_string();
    let spawned = std::thread::Builder::new()
        .name("blazar-hang-sample".into())
        .spawn(move || {
            prune_samples(&logs, KEEP_SAMPLES - 1);
            let ran = std::process::Command::new("/usr/bin/sample")
                .args([pid.as_str(), "3", "-file"])
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match ran {
                Ok(s) if s.success() => {
                    tracing::warn!("主线程卡住时的调用栈存到了 {}", path.display());
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    if stage() != Stage::Tray && tray_in_main_stack(&text) {
                        TRAY_HANG.store(true, Ordering::SeqCst);
                        crate::menubar::hung_later(waited.as_secs() + 3);
                        tracing::warn!("主线程卡在菜单栏图标上，下次启动不建菜单栏图标");
                    }
                }
                Ok(s) => tracing::warn!("没取到主线程卡住时的调用栈：sample {s}"),
                Err(e) => tracing::warn!("没取到主线程卡住时的调用栈：{e}"),
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("没取到主线程卡住时的调用栈：{e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn sample(dir: Option<&Path>, waited: Duration) {
    let _ = (dir, waited);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn prune_samples(logs: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return;
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("hang-") && n.ends_with(".txt"))
        })
        .collect();
    found.sort();
    let extra = found.len().saturating_sub(keep);
    for old in found.into_iter().take(extra) {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_stall_is_reported_once_with_its_stage_and_its_length_when_it_ends() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        assert_eq!(m.observe(0, t0, Stage::Idle), Seen::Fine);
        assert!(m.due(0, t0));
        let seq = m.post(t0);
        assert!(!m.answered(0));
        assert_eq!(m.observe(0, t0 + secs(2), Stage::Tray), Seen::Fine);
        assert_eq!(
            m.observe(0, t0 + secs(3), Stage::Tray),
            Seen::Stuck {
                waited: secs(3),
                stage: Stage::Tray
            }
        );
        assert_eq!(m.observe(0, t0 + secs(6), Stage::Idle), Seen::Fine);
        assert!(!m.due(0, t0 + secs(20)), "不在没回音时再投递心跳");
        assert_eq!(
            m.observe(0, t0 + secs(33), Stage::Idle),
            Seen::StillStuck {
                waited: secs(33),
                stage: Stage::Tray
            }
        );
        assert_eq!(m.observe(0, t0 + secs(38), Stage::Idle), Seen::Fine);
        assert_eq!(
            m.observe(seq, t0 + secs(1440), Stage::Idle),
            Seen::Recovered {
                stuck: secs(1440),
                stage: Stage::Tray
            }
        );
        assert_eq!(m.observe(seq, t0 + secs(1441), Stage::Idle), Seen::Fine);
        assert!(m.answered(seq));
        assert!(m.due(seq, t0 + secs(1442)));
    }

    #[test]
    fn a_quick_answer_is_never_a_stall_and_beats_are_spaced() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        let first = m.post(t0);
        assert_eq!(
            m.observe(first, t0 + Duration::from_millis(30), Stage::Window),
            Seen::Fine
        );
        assert!(!m.due(first, t0 + Duration::from_millis(500)));
        assert!(m.due(first, t0 + secs(1)));
        let second = m.post(t0 + secs(1));
        assert_eq!(second, first + 1);
        assert_eq!(m.observe(first, t0 + secs(3), Stage::Idle), Seen::Fine);
        assert_eq!(
            m.observe(first, t0 + secs(4), Stage::Idle),
            Seen::Stuck {
                waited: secs(3),
                stage: Stage::Idle
            }
        );
    }

    #[test]
    fn the_tray_is_trusted_only_after_the_main_thread_answers_past_the_settle_window() {
        let t0 = Instant::now();
        let fresh = TrayWatch {
            active: true,
            built: None,
            stalled: false,
        };
        assert_eq!(fresh.settle(Some(t0)), Settle::Wait, "还没建好");
        let built = TrayWatch {
            built: Some(t0),
            ..fresh
        };
        assert_eq!(built.settle(None), Settle::Wait);
        assert_eq!(built.settle(Some(t0 + secs(1))), Settle::Wait);
        assert_eq!(built.settle(Some(t0 + secs(2))), Settle::Healthy);
        let stalled = TrayWatch {
            stalled: true,
            ..built
        };
        assert_eq!(stalled.settle(Some(t0 + secs(30))), Settle::Stalled);
        assert_eq!(TrayWatch::default().settle(Some(t0)), Settle::Wait);
    }

    #[test]
    fn stages_nest_and_the_tray_keeps_its_claim_until_released() {
        assert_eq!(Stage::of(Stage::AccountMenu.code()), Stage::AccountMenu);
        assert_eq!(Stage::of(200), Stage::Idle);
        STAGE.store(Stage::Idle.code(), Ordering::SeqCst);
        {
            let _window = mark(Stage::Window);
            assert_eq!(stage(), Stage::Window);
            {
                let _menu = mark(Stage::Menu);
                assert_eq!(stage(), Stage::Menu);
            }
            assert_eq!(stage(), Stage::Window);
        }
        assert_eq!(stage(), Stage::Idle);
        STAGE.store(Stage::Tray.code(), Ordering::SeqCst);
        {
            let _accounts = mark(Stage::AccountMenu);
            assert_eq!(
                stage(),
                Stage::Tray,
                "图标还在等主线程回话时，卡住都算图标的"
            );
        }
        assert_eq!(stage(), Stage::Tray);
        release_tray_stage();
        assert_eq!(stage(), Stage::Idle);
    }

    #[test]
    fn only_a_main_thread_stuck_in_the_status_item_counts_as_the_tray() {
        let tray = "Call graph:\n    2077 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 2077 start  (in dyld)\n    +   2077 -[NSSceneStatusItem _requestScene]  (in AppKit)\n    +     2077 kevent_id\n    2077 Thread_2: tokio-rt-worker\n    + 2077 thread_start\n";
        assert!(tray_in_main_stack(tray));
        let elsewhere = "Call graph:\n    10 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 10 start  (in dyld)\n    +   10 sqlite3_step\n    10 Thread_2: worker\n    +   10 -[NSStatusItem button]  (in AppKit)\n";
        assert!(!tray_in_main_stack(elsewhere), "别的线程碰到图标不算");
        assert!(!tray_in_main_stack("no threads here"));
    }

    const HUNG: &str = "Call graph:
    2077 Thread_18862341   DispatchQueue_1: com.apple.main-thread  (serial)
    + 2077 start  (in dyld) + 6652  [0x187854d7c]
    +   2077 -[NSApplication run]  (in AppKit) + 396  [0x19178a920]
    +     2077 __CFRunLoopDoTimer  (in CoreFoundation) + 1084  [0x18bb075a4]
    +       2077 __34-[NSSceneStatusItem _requestScene]_block_invoke.17  (in AppKit) + 40  [0x191dd942c]
    +         2077 -[NSSceneStatusItem _requestScene]  (in AppKit) + 576  [0x191dd8d18]
    +           2077 _dispatch_async_and_wait_f  (in libdispatch.dylib) + 104  [0x18c8aefec]
    2077 Thread_18862348: tokio-rt-worker
    + 2077 start_wqthread  (in libsystem_pthread.dylib) + 8  [0x1878e0b9c]
";

    const HEALTHY: &str = "Call graph:
    667 Thread_18862341   DispatchQueue_1: com.apple.main-thread  (serial)
    + 667 start  (in dyld) + 6652  [0x187854d7c]
    +   667 -[NSApplication run]  (in AppKit) + 396  [0x19178a920]
    +   ! 666 _DPSNextEvent  (in AppKit) + 576  [0x19179759c]
    +   ! : 645 mach_msg2_trap  (in libsystem_kernel.dylib) + 8  [0x1878e3c20]
    +   ! 1 __34-[NSSceneStatusItem _requestScene]_block_invoke.17  (in AppKit) + 40  [0x191dd942c]
    +   !   1 -[NSSceneStatusItem _requestScene]  (in AppKit) + 576  [0x191dd8d18]
    667 Thread_18862348: tokio-rt-worker
    + 667 start_wqthread  (in libsystem_pthread.dylib) + 8  [0x1878e0b9c]
";

    #[test]
    fn a_main_thread_parked_in_the_status_item_is_blamed_on_the_icon() {
        assert!(tray_in_main_stack(HUNG));
    }

    #[test]
    fn a_passing_status_item_call_is_not_blamed_for_some_other_stall() {
        assert!(
            !tray_in_main_stack(HEALTHY),
            "健康时主线程也会偶尔落一两个样本在这里"
        );
    }

    #[test]
    fn sample_counts_are_read_through_the_tree_glyphs() {
        assert_eq!(
            leading_count("    2077 Thread_18862341   DispatchQueue_1: com.apple.main-thread"),
            Some(2077)
        );
        assert_eq!(leading_count("    +   ! : 645 mach_msg2_trap"), Some(645));
        assert_eq!(leading_count("    + : | 945 __CFRunLoopRun"), Some(945));
        assert_eq!(leading_count("Call graph:"), None);
        assert!(thread_header("    667 Thread_1: worker"));
        assert!(!thread_header("    + 667 start"));
    }

    #[test]
    fn old_hang_samples_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        for stamp in [100, 200, 300, 400, 500] {
            std::fs::write(dir.path().join(format!("hang-{stamp}.txt")), "x").unwrap();
        }
        std::fs::write(dir.path().join("desktop.log"), "x").unwrap();
        prune_samples(dir.path(), 3);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "desktop.log",
                "hang-300.txt",
                "hang-400.txt",
                "hang-500.txt"
            ]
        );
    }
}
