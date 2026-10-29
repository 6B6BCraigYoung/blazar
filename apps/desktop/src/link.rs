//! 连远程 hub 时，窗口和页面怎么知道连接断了、又回来了。
//!
//! 断了不换页面：页面原地保持，只告诉它显示一条“正在重新连接 gpu-1…”横条；
//! 回来了也不跳转，页面自己的实时连接会重新订阅、重新拉取。
//! 只有第一次启动、一次都没连上过时，才显示随包的连接页。

use crate::hub_mode::Target;

/// 页面上显示的机器名：SSH 用别名，直连地址只取主机名。
pub fn machine_name(target: &Target) -> String {
    match target {
        Target::Ssh { alias, .. } => alias.clone(),
        Target::Direct(url) => url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| url.clone()),
    }
}

/// 交给页面执行的一小段脚本。页面注册了 `window.__blazarHubLink` 就交给它；
/// 连回来时如果页面不是 Blazar（比如断线期间刷新成了出错页），就重新载入一次。
pub fn script(up: bool, machine: &str) -> String {
    let name = serde_json::to_string(machine).unwrap_or_else(|_| "\"\"".into());
    if up {
        format!(
            "if (window.__blazarHubLink) {{ window.__blazarHubLink(true, {name}); }} else {{ location.reload(); }}"
        )
    } else {
        format!("window.__blazarHubLink && window.__blazarHubLink(false, {name});")
    }
}

/// 连不上之后怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterLoss {
    /// 把窗口换成随包的连接页。只在一次都没连上过时这么做。
    pub show_bundled: bool,
    /// 下一次重试前等多久（秒）。连上过的话很快再试，横条不用挂太久。
    pub retry_secs: u64,
}

pub const fn after_loss(ever_connected: bool) -> AfterLoss {
    if ever_connected {
        AfterLoss {
            show_bundled: false,
            retry_secs: 3,
        }
    } else {
        AfterLoss {
            show_bundled: true,
            retry_secs: 30,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_name_is_the_alias_or_the_host() {
        let ssh = Target::Ssh {
            alias: "gpu-1".into(),
            port: 8766,
        };
        assert_eq!(machine_name(&ssh), "gpu-1");
        let direct = Target::Direct("http://10.0.0.5:8766".into());
        assert_eq!(machine_name(&direct), "10.0.0.5");
        let odd = Target::Direct("not a url".into());
        assert_eq!(machine_name(&odd), "not a url");
    }

    #[test]
    fn scripts_quote_the_machine_name_and_never_navigate_on_loss() {
        let down = script(false, "gpu-\"1\"");
        assert!(
            down.contains(r#"__blazarHubLink(false, "gpu-\"1\"")"#),
            "{down}"
        );
        assert!(!down.contains("location"), "{down}");
        let up = script(true, "gpu-1");
        assert!(up.contains(r#"__blazarHubLink(true, "gpu-1")"#), "{up}");
        assert!(up.contains("location.reload()"), "{up}");
    }

    #[test]
    fn only_a_hub_never_reached_gets_the_bundled_page() {
        assert!(after_loss(false).show_bundled);
        assert!(!after_loss(true).show_bundled);
        assert!(after_loss(true).retry_secs < after_loss(false).retry_secs);
    }
}
