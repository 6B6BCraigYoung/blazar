pub fn svg(d: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{d}</svg>"#
    )
}

pub const INBOX: &str = r#"<path d="M3 13l2.5-7.5A2 2 0 0 1 7.4 4h9.2a2 2 0 0 1 1.9 1.5L21 13v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 13h5l1.5 2.5h5L16 13h5"/>"#;
pub const FOLDER: &str =
    r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#;
pub const TASKS: &str = r#"<rect x="3" y="4" width="5" height="16" rx="1.5"/><rect x="9.5" y="4" width="5" height="10" rx="1.5"/><rect x="16" y="4" width="5" height="13" rx="1.5"/>"#;
pub const SPARKLES: &str = r#"<path d="M10 6l1.56 5.44L17 13l-5.44 1.56L10 20l-1.56-5.44L3 13l5.44-1.56z"/><path d="M18.5 2.5l.7 2.3 2.3.7-2.3.7-.7 2.3-.7-2.3-2.3-.7 2.3-.7z"/><path d="M18 15.5l.5 2 2 .5-2 .5-.5 2-.5-2-2-.5 2-.5z"/>"#;
pub const ID_CARD: &str = r#"<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="8.5" cy="11" r="2"/><path d="M5.5 16c.6-1.5 1.7-2.2 3-2.2s2.4.7 3 2.2M14 9.5h4.5M14 12.5h4.5M14 15.5h3"/>"#;
pub const WAND: &str = r#"<path d="M3.5 20.5 13.5 10.5"/><path d="M17 2.5l1.1 3.4 3.4 1.1-3.4 1.1L17 11.5l-1.1-3.4L12.5 7l3.4-1.1z"/><path d="M6 3.5v3M4.5 5h3M20 13.5v3M18.5 15h3"/>"#;
pub const APPS: &str = r#"<rect x="3" y="3" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/><path d="M17.25 14v6.5M14 17.25h6.5"/>"#;
pub const NODES: &str = r#"<circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M10.9 6.9 6.1 16M13.1 6.9l4.8 9.1M7.2 18h9.6"/>"#;
pub const GEAR: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/>"#;
pub const FILE: &str = r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#;
pub const BRANCH: &str = r#"<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M18 9c0 5-6 4-11 8.5"/>"#;
pub const SEARCH: &str = r#"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"#;
pub const BOT: &str = r#"<rect x="5" y="8" width="14" height="11" rx="3"/><path d="M12 8V5M8 5h8M9.5 13h.01M14.5 13h.01M9 16.5h6"/>"#;
pub const CLOCK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#;
pub const TERMINAL: &str =
    r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M12.5 15H17"/>"#;
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
pub const PALETTE: &str = r#"<path d="M12 3a9 9 0 1 0 0 18c1.4 0 2-.9 2-1.8 0-.9-.7-1.3-.7-2.3 0-1 .8-1.9 2.2-1.9H17a4 4 0 0 0 4-4c0-4.4-4-8-9-8z"/><path d="M7.5 11.5h.01M10.5 7.5h.01M15 7.5h.01"/>"#;
pub const BELL: &str =
    r#"<path d="M6 16v-5a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20a2 2 0 0 0 4 0"/>"#;
pub const KEYBOARD: &str = r#"<rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 10h.01M11 10h.01M15 10h.01M7.5 14h9"/>"#;
pub const LINES: &str = r#"<path d="M4 6h16M4 12h10M4 18h7"/>"#;
pub const INFO: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>"#;
pub const ARROW: &str = r#"<path d="M5 12h14M13 6l6 6-6 6"/>"#;
pub const PENCIL: &str = r#"<path d="M4 20h4L18.5 9.5a2 2 0 0 0 0-2.8l-1.2-1.2a2 2 0 0 0-2.8 0L4 16z"/><path d="M13.5 6.5l4 4"/>"#;
pub const COPY: &str =
    r#"<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>"#;
pub const TRASH: &str =
    r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/><path d="M10 11v6M14 11v6"/>"#;
pub const EYE: &str = r#"<path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12z"/><circle cx="12" cy="12" r="2.5"/>"#;
pub const PLUS: &str = r#"<path d="M12 5v14M5 12h14"/>"#;
pub const TICK: &str = r#"<path d="m5 12.5 4.5 4.5L19 7"/>"#;
