pub fn describe(source: Option<&str>, port: Option<u32>, ports: &[u16], running: bool) -> String {
    match (source, port) {
        (Some("manual"), Some(p)) => format!("端口 {p}：脚本设置里手动指定"),
        (Some("log"), Some(p)) => format!("端口 {p}：从 dev server 日志识别"),
        (Some("listen"), Some(p)) => {
            let others: Vec<String> = ports
                .iter()
                .map(|x| u32::from(*x))
                .filter(|x| *x != p)
                .map(|x| x.to_string())
                .collect();
            if others.is_empty() {
                format!("端口 {p}：从监听端口识别")
            } else {
                format!(
                    "端口 {p}：从监听端口识别，还在监听 {}，可在上方地址栏改",
                    others.join("、")
                )
            }
        }
        _ if running => "还没识别出端口：日志里没有地址，启动几秒后会查进程监听的端口".to_owned(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_line_names_where_the_port_came_from() {
        assert_eq!(
            describe(Some("manual"), Some(4321), &[], true),
            "端口 4321：脚本设置里手动指定"
        );
        assert_eq!(
            describe(Some("log"), Some(5173), &[], true),
            "端口 5173：从 dev server 日志识别"
        );
        assert_eq!(
            describe(Some("listen"), Some(8000), &[8000], true),
            "端口 8000：从监听端口识别"
        );
        assert_eq!(
            describe(Some("listen"), Some(8000), &[8000, 8001, 35729], true),
            "端口 8000：从监听端口识别，还在监听 8001、35729，可在上方地址栏改"
        );
    }

    #[test]
    fn without_a_port_the_line_only_speaks_while_the_server_runs() {
        assert!(describe(None, None, &[], true).starts_with("还没识别出端口"));
        assert_eq!(describe(None, None, &[], false), "");
        assert_eq!(describe(Some("log"), None, &[], false), "");
    }
}
