pub fn svg(d: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{d}</svg>"#
    )
}

pub const INBOX: &str = r#"<path d="M3 13l2.5-7.5A2 2 0 0 1 7.4 4h9.2a2 2 0 0 1 1.9 1.5L21 13v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 13h5l1.5 2.5h5L16 13h5"/>"#;
pub const FOLDER: &str =
    r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#;
pub const TASKS: &str = r#"<rect x="3" y="4" width="5" height="16" rx="1.5"/><rect x="9.5" y="4" width="5" height="10" rx="1.5"/><rect x="16" y="4" width="5" height="13" rx="1.5"/>"#;
pub const AGENT: &str = r#"<rect x="4" y="7" width="16" height="12" rx="2"/><path d="M12 7V4M9 12h.01M15 12h.01M9 16h6"/>"#;
pub const APPS: &str = r#"<rect x="3" y="3" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/><path d="M17.25 14v6.5M14 17.25h6.5"/>"#;
pub const NODES: &str = r#"<circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M10.9 6.9 6.1 16M13.1 6.9l4.8 9.1M7.2 18h9.6"/>"#;
pub const GEAR: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/>"#;
pub const PLUS: &str = r#"<path d="M12 5v14M5 12h14"/>"#;
pub const FILE: &str = r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#;
pub const BRANCH: &str = r#"<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M18 9c0 5-6 4-11 8.5"/>"#;
pub const SEARCH: &str = r#"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"#;
pub const BOT: &str = r#"<rect x="5" y="8" width="14" height="11" rx="3"/><path d="M12 8V5M8 5h8M9.5 13h.01M14.5 13h.01M9 16.5h6"/>"#;
pub const BELL: &str = r#"<path d="M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>"#;
pub const PLAY: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M10 8.5v7l5.5-3.5z"/>"#;
pub const CLOCK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#;
pub const TERMINAL: &str =
    r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M12.5 15H17"/>"#;
pub const SPARK: &str = r#"<path d="M12 3v4M12 17v4M3 12h4M17 12h4M5.6 5.6l2.8 2.8M15.6 15.6l2.8 2.8M5.6 18.4l2.8-2.8M15.6 8.4l2.8-2.8"/>"#;
pub const CHART: &str = r#"<path d="M4 20V4M4 20h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/>"#;
pub const CHAT: &str = r#"<path d="M5 5h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-7l-4 3.5V17H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z"/>"#;
pub const MAIL: &str =
    r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="m3.5 6.5 8.5 6.5 8.5-6.5"/>"#;
pub const LOCK: &str =
    r#"<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>"#;
pub const CHECK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="m8 12.5 2.8 2.8L16.5 9.5"/>"#;
pub const FILTER: &str = r#"<path d="M4 5h16l-6.2 7.4V19l-3.6-1.8v-4.8z"/>"#;
pub const WINDOW: &str = r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 8.5h18M6.5 6.2h.01M9 6.2h.01"/>"#;
pub const ACTIVITY: &str = r#"<path d="M3 12h4l2.5-6 5 12 2.5-6h4"/>"#;
