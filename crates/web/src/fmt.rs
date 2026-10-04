fn parse_ms(iso: &str) -> Option<f64> {
    let ms = js_sys::Date::parse(iso);
    (!ms.is_nan()).then_some(ms)
}

fn span(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 60.0 {
        "不到 1 分钟".into()
    } else if s < 3600.0 {
        format!("{} 分钟", (s / 60.0).floor())
    } else if s < 86_400.0 {
        format!("{} 小时", (s / 3600.0).floor())
    } else {
        format!("{} 天", (s / 86_400.0).floor())
    }
}

pub fn ago(iso: &str) -> String {
    parse_ms(iso).map_or_else(String::new, |t| {
        let d = (js_sys::Date::now() - t) / 1000.0;
        if d < 60.0 {
            "刚刚".into()
        } else {
            format!("{}前", span(d))
        }
    })
}

pub fn resets(iso: Option<&str>) -> String {
    iso.and_then(parse_ms).map_or_else(String::new, |t| {
        let d = (t - js_sys::Date::now()) / 1000.0;
        if d <= 0.0 {
            "已重置".into()
        } else {
            format!("{}后重置", span(d))
        }
    })
}

pub fn window_label(name: &str) -> &str {
    match name {
        "five_hour" => "5 小时",
        "seven_day" => "7 天",
        "seven_day_opus" => "7 天 · Opus",
        "seven_day_sonnet" => "7 天 · Sonnet",
        "seven_day_overage_included" => "7 天 · 含超额",
        "blocked" => "已限流",
        other => other,
    }
}
