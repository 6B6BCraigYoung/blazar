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

fn span_en(secs: f64) -> String {
    let s = secs.max(0.0);
    let (n, unit) = if s < 3600.0 {
        ((s / 60.0).floor().max(1.0), "m")
    } else if s < 86_400.0 {
        ((s / 3600.0).floor(), "h")
    } else {
        ((s / 86_400.0).floor(), "d")
    };
    format!("{n}{unit}")
}

pub fn ago_en(iso: &str) -> String {
    parse_ms(iso).map_or_else(String::new, |t| {
        let d = (js_sys::Date::now() - t) / 1000.0;
        if d < 60.0 {
            "just now".into()
        } else {
            format!("{} ago", span_en(d))
        }
    })
}

pub fn resets_en(iso: Option<&str>) -> String {
    iso.and_then(parse_ms).map_or_else(String::new, |t| {
        let d = (t - js_sys::Date::now()) / 1000.0;
        if d <= 0.0 {
            "reset".into()
        } else {
            format!("resets in {}", span_en(d))
        }
    })
}

pub fn window_label_en(name: &str) -> String {
    match name {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        "blocked" => "Rate limited".into(),
        other => match other.strip_prefix("seven_day_") {
            Some(model) => {
                let mut c = model.chars();
                let head = c.next().map(|f| f.to_uppercase().collect::<String>());
                format!("Weekly {}{}", head.unwrap_or_default(), c.as_str()).replace('_', " ")
            }
            None => other.to_owned(),
        },
    }
}

pub fn window_label(name: &str) -> String {
    match name {
        "five_hour" => "5 小时".into(),
        "seven_day" => "7 天".into(),
        "seven_day_overage_included" => "7 天 · 含超额".into(),
        "seven_day_oauth_apps" => "7 天 · 第三方应用".into(),
        "blocked" => "已限流".into(),
        other => match other.strip_prefix("seven_day_") {
            Some(model) => {
                let mut c = model.chars();
                let head = c.next().map(|f| f.to_uppercase().collect::<String>());
                format!("7 天 · {}{}", head.unwrap_or_default(), c.as_str())
            }
            None => other.to_owned(),
        },
    }
}
