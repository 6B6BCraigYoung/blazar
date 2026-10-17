use std::time::Duration;

use serde_json::Value;

use crate::api::Shared;
use crate::remote_cli::{self, Platform};

const RELEASES: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

pub struct Build {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

pub fn max_major_for(driver: Option<u32>) -> u32 {
    match driver {
        Some(d) if d >= 610 => u32::MAX,
        Some(d) if d >= 570 => 8,
        Some(_) => 7,
        None => u32::MAX,
    }
}

pub fn pick_asset(release: &Value, arch: &str, driver: Option<u32>) -> Option<Build> {
    let cap = max_major_for(driver);
    let oldest = pick_asset_capped(release, arch, u32::MAX, false)?;
    Some(pick_asset_capped(release, arch, cap, true).unwrap_or(oldest))
}

fn pick_asset_capped(release: &Value, arch: &str, cap: u32, newest: bool) -> Option<Build> {
    let suffix = match arch {
        "x64" => "linux64",
        "arm64" => "linuxarm64",
        _ => return None,
    };
    let mut best: Option<((u32, u32), Build)> = None;
    for a in release["assets"].as_array()? {
        let name = a["name"].as_str()?;
        let Some(rest) = name.strip_prefix("ffmpeg-n") else {
            continue;
        };
        let Some((ver, tail)) = rest.split_once("-latest-") else {
            continue;
        };
        if tail != format!("{suffix}-gpl-{ver}.tar.xz") {
            continue;
        }
        let mut it = ver.split('.').map(|x| x.parse::<u32>().ok());
        let (Some(Some(major)), Some(Some(minor))) = (it.next(), it.next()) else {
            continue;
        };
        if major > cap {
            continue;
        }
        let Some(sha) = a["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .and_then(remote_cli::sha256_digest)
        else {
            continue;
        };
        let Some(url) = a["browser_download_url"].as_str() else {
            continue;
        };
        if url
            .parse::<reqwest::Url>()
            .ok()
            .is_none_or(|u| u.scheme() != "https" || u.host_str() != Some("github.com"))
        {
            continue;
        }
        let better = |v: &(u32, u32)| {
            if newest {
                (major, minor) > *v
            } else {
                (major, minor) < *v
            }
        };
        if best.as_ref().is_none_or(|(v, _)| better(v)) {
            best = Some((
                (major, minor),
                Build {
                    name: name.to_owned(),
                    url: url.to_owned(),
                    sha256: sha.to_owned(),
                },
            ));
        }
    }
    best.map(|(_, b)| b)
}

async fn latest(arch: &str, driver: Option<u32>) -> Result<Build, String> {
    let release: Value = remote_cli::http()?
        .get(RELEASES)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "blazar")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("查不到 ffmpeg 静态构建的发布信息：{e}"))?
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or("ffmpeg 发布信息格式不对")?;
    pick_asset(&release, arch, driver).ok_or_else(|| format!("没有 {arch} 的 ffmpeg 静态构建"))
}

fn install_script(name: &str, sha256: &str, source: &str) -> String {
    format!(
        r#"set -e
umask 022
V={ver}
R="$HOME/.blazar/agents/ffmpeg/$V"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
mkdir -p "$R" "$HOME/{bin}"
{source}
S=$( (sha256sum "$T/pkg.tar.xz" 2>/dev/null || shasum -a 256 "$T/pkg.tar.xz") | cut -d' ' -f1)
[ "$S" = {sum} ] || {{ echo "校验失败" >&2; exit 1; }}
tar -xJf "$T/pkg.tar.xz" -C "$T" --wildcards '*/bin/ffmpeg' '*/bin/ffprobe'
cp -f "$T"/*/bin/ffmpeg "$T"/*/bin/ffprobe "$R/"
chmod +x "$R/ffmpeg" "$R/ffprobe"
ln -sfn "$R/ffmpeg" "$HOME/{bin}/ffmpeg"
ln -sfn "$R/ffprobe" "$HOME/{bin}/ffprobe"
"#,
        ver = blazar_transport::shell_quote(name.trim_end_matches(".tar.xz")),
        bin = remote_cli::MANAGED_BIN,
        sum = blazar_transport::shell_quote(sha256),
    )
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Probe {
    pub path: Option<String>,
    pub nvenc: bool,
    pub managed: bool,
    pub driver: Option<u32>,
}

const PROBE_SH: &str = r#"
F=$(command -v ffmpeg 2>/dev/null || echo -)
NV=0; [ "$F" != - ] && ffmpeg -hide_banner -loglevel error -nostdin -f lavfi -i color=c=black:s=256x256:r=30 -frames:v 1 -c:v h264_nvenc -f null - >/dev/null 2>&1 && NV=1
M=0; case "$(readlink -f "$F" 2>/dev/null)" in "$HOME/.blazar/"*) M=1;; esac
D=$(nvidia-smi --query-gpu=driver_version --format=csv,noheader 2>/dev/null | head -1 | cut -d. -f1)
printf 'PROBE\t%s\t%s\t%s\t%s\n' "$F" "$NV" "$M" "$D"
"#;

pub fn parse_probe(raw: &str) -> Probe {
    raw.lines()
        .find_map(|l| {
            let p: Vec<&str> = l.split('\t').collect();
            match p.as_slice() {
                ["PROBE", f, nv, m, d] => Some(Probe {
                    path: f.starts_with('/').then(|| (*f).to_owned()),
                    nvenc: *nv == "1",
                    managed: *m == "1",
                    driver: d.trim().parse().ok(),
                }),
                _ => None,
            }
        })
        .unwrap_or_default()
}

async fn probe(st: &Shared, node: &str) -> Probe {
    remote_cli::run(
        st,
        node,
        &format!("{}\n{PROBE_SH}", blazar_transport::PATH_PRELUDE),
        60,
    )
    .await
    .map(|o| parse_probe(&o))
    .unwrap_or_default()
}

fn good_enough(p: &Probe) -> bool {
    p.path.is_some() && (p.nvenc || p.driver.is_none() || !p.managed)
}

pub async fn ensure(st: &Shared, node: &str) -> Result<String, String> {
    let first = probe(st, node).await;
    if good_enough(&first) {
        return Ok("已有 ffmpeg".into());
    }
    let p: Platform = remote_cli::platform(st, node).await?;
    if p.os != "linux" {
        return Err(format!("{node} 不是 Linux，暂不支持自动装 ffmpeg"));
    }
    let build = latest(p.arch, first.driver).await?;
    let fetch = format!(
        "curl -fsSL --connect-timeout 15 --max-time 600 --speed-limit 50000 --speed-time 30 --retry 1 -o \"$T/pkg.tar.xz\" -- {u} 2>/dev/null || wget -q -T 30 -t 1 -O \"$T/pkg.tar.xz\" -- {u}",
        u = blazar_transport::shell_quote(&build.url)
    );
    let script = format!(
        "{}{}",
        remote_cli::exports(st, node).await,
        install_script(&build.name, &build.sha256, &fetch)
    );
    let _ = remote_cli::run(st, node, &script, 660).await;
    if good_enough(&probe(st, node).await) {
        return Ok(format!(
            "已把 ffmpeg（{}）装到 {node} 的 ~/.blazar",
            build.name
        ));
    }
    let local = remote_cli::cache_dir(st).join("ffmpeg").join(&build.name);
    remote_cli::cached_download(&local, &build.url, &build.sha256).await?;
    let remote_tmp = format!("/tmp/blazar-{}", build.name);
    remote_cli::scp_up(&local, &remote_cli::ssh_target(st, node), &remote_tmp)
        .await
        .map_err(|e| format!("没能把 ffmpeg 传到 {node}：{e}"))?;
    let local_src = format!(
        "mv {} \"$T/pkg.tar.xz\"",
        blazar_transport::shell_quote(&remote_tmp)
    );
    let out = remote_cli::run(
        st,
        node,
        &install_script(&build.name, &build.sha256, &local_src),
        300,
    )
    .await;
    if good_enough(&probe(st, node).await) {
        Ok(format!(
            "{node} 下载不了，已由本机下载后传过去，ffmpeg（{}）装在 ~/.blazar",
            build.name
        ))
    } else {
        Err(format!(
            "没能在 {node} 上装好 ffmpeg：{}",
            out.unwrap_or_else(|e| e)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn asset(name: &str, digest: Option<&str>, url: &str) -> Value {
        json!({ "name": name, "digest": digest, "browser_download_url": url })
    }

    #[test]
    fn picks_the_newest_stable_gpl_build_with_a_digest() {
        let d = format!("sha256:{}", "a".repeat(64));
        let r = json!({ "assets": [
            asset("ffmpeg-master-latest-linux64-gpl.tar.xz", Some(&d), "https://github.com/x/m.tar.xz"),
            asset("ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz", Some(&d), "https://github.com/x/8.tar.xz"),
            asset("ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz", Some(&d), "https://github.com/x/9.tar.xz"),
            asset("ffmpeg-n9.1-latest-linux64-gpl-9.1.tar.xz", None, "https://github.com/x/91.tar.xz"),
            asset("ffmpeg-n9.2-latest-linux64-gpl-9.2.tar.xz", Some(&d), "http://github.com/x/92.tar.xz"),
            asset("ffmpeg-n9.0-latest-linux64-gpl-shared-9.0.tar.xz", Some(&d), "https://github.com/x/s.tar.xz"),
            asset("ffmpeg-n9.0-latest-linuxarm64-gpl-9.0.tar.xz", Some(&d), "https://github.com/x/a.tar.xz"),
        ]});
        let b = pick_asset(&r, "x64", None).unwrap();
        assert_eq!(b.name, "ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz");
        assert_eq!(b.url, "https://github.com/x/9.tar.xz");
        assert_eq!(
            pick_asset(&r, "arm64", Some(620)).unwrap().url,
            "https://github.com/x/a.tar.xz"
        );
        assert!(pick_asset(&r, "riscv", None).is_none());
        assert_eq!(
            pick_asset(&r, "x64", Some(595)).unwrap().name,
            "ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz",
            "595 的驱动只能用 8.x 的 NVENC"
        );
        assert_eq!(
            pick_asset(&r, "x64", Some(550)).unwrap().name,
            "ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz",
            "没有更旧的构建时退到最旧的那条线"
        );
    }

    #[test]
    fn probe_decides_when_a_reinstall_is_needed() {
        let p = parse_probe("PROBE\t/home/u/.blazar/bin/ffmpeg\t0\t1\t595\n");
        assert_eq!(p.driver, Some(595));
        assert!(p.managed && !p.nvenc);
        assert!(!good_enough(&p), "Blazar 装的 ffmpeg 用不了 NVENC 就该换");
        assert!(
            good_enough(&parse_probe("PROBE\t/usr/bin/ffmpeg\t0\t0\t595\n")),
            "系统自带的不动"
        );
        assert!(
            good_enough(&parse_probe("PROBE\t/home/u/.blazar/bin/ffmpeg\t0\t1\t\n")),
            "没有 NVIDIA 驱动时 CPU 编码就够"
        );
        assert!(!good_enough(&parse_probe("PROBE\t-\t0\t0\t595\n")));
        assert_eq!(parse_probe("garbage"), Probe::default());
    }

    #[test]
    fn install_script_verifies_before_linking() {
        let s = install_script(
            "ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz",
            &"b".repeat(64),
            ":",
        );
        let verify = s.find("校验失败").unwrap();
        let link = s.find("ln -sfn").unwrap();
        assert!(verify < link);
        assert!(s.contains("V='ffmpeg-n9.0-latest-linux64-gpl-9.0'"));
        assert!(s.contains("agents/ffmpeg/$V"));
    }
}
