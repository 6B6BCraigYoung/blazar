pub fn notice(stuck_secs: Option<u64>) -> String {
    match stuck_secs.filter(|s| *s > 0) {
        Some(secs) => format!(
            "上次运行时菜单栏没有响应，Blazar 卡住了{}，所以这次没有显示菜单栏图标。可以随时在这里重新打开。",
            span(secs)
        ),
        None => "上次启动时菜单栏图标没建完，Blazar 没能正常打开，所以这次没有显示菜单栏图标。可以随时在这里重新打开。".to_owned(),
    }
}

fn span(secs: u64) -> String {
    if secs < 60 {
        format!(" {secs} 秒")
    } else if secs < 3600 {
        format!("约 {} 分钟", (secs + 30) / 60)
    } else {
        format!("约 {} 小时", (secs + 1800) / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_the_menu_bar_froze_the_app() {
        assert!(notice(Some(8)).contains("Blazar 卡住了 8 秒，"));
        assert!(notice(Some(1440)).contains("卡住了约 24 分钟，"));
        assert!(notice(Some(89)).contains("卡住了约 1 分钟，"));
        assert!(notice(Some(5400)).contains("卡住了约 2 小时，"));
        assert!(notice(Some(1440)).ends_with("可以随时在这里重新打开。"));
    }

    #[test]
    fn without_a_duration_it_says_the_icon_never_finished() {
        for n in [None, Some(0)] {
            let text = notice(n);
            assert!(text.contains("没建完"), "{text}");
            assert!(!text.contains("卡住了"), "{text}");
        }
    }
}
