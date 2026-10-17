use axum::Json;
use axum::extract::{Path, State};
use blazar_transport::ExecSpec;
use blazar_transport::detached::{DetachedRun, RunMode};
use serde::Serialize;
use serde_json::{Value, json};

use crate::api::{ApiError, ApiResult, Shared};

pub const HEADLESS_CONF: &str = "/etc/X11/blazar-headless.conf";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Display {
    pub number: u32,
    pub owner: String,
    pub mine: bool,
    pub size: Option<(u32, u32)>,
    pub managed: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Survey {
    pub displays: Vec<Display>,
    pub os: String,
    pub xorg: bool,
    pub nvidia_driver: bool,
    pub wrapper_anybody: bool,
    pub headless_conf: bool,
    pub ffmpeg: Option<String>,
    pub nvenc: bool,
}

impl Survey {
    pub fn virtual_ready(&self) -> bool {
        self.os == "Linux"
            && self.xorg
            && self.nvidia_driver
            && self.wrapper_anybody
            && self.headless_conf
    }

    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.os != "Linux" {
            m.push("只支持 Linux");
            return m;
        }
        if !self.xorg {
            m.push("没装 Xorg");
        }
        if !self.nvidia_driver {
            m.push("没有 NVIDIA 的 X 驱动模块");
        }
        if !self.wrapper_anybody {
            m.push("Xwrapper 不允许普通用户起 X");
        }
        if !self.headless_conf {
            m.push("没有 Blazar 的无头显示配置");
        }
        m
    }

    pub fn to_json(&self) -> Value {
        json!({
            "displays": self.displays.iter().map(|d| json!({
                "number": d.number, "owner": d.owner, "mine": d.mine, "managed": d.managed, "locked": d.locked,
                "size": d.size.map(|(w, h)| format!("{w}x{h}")),
            })).collect::<Vec<_>>(),
            "os": self.os,
            "virtual_ready": self.virtual_ready(),
            "virtual_missing": self.missing(),
            "ffmpeg": self.ffmpeg,
            "nvenc": self.nvenc,
        })
    }
}

const SURVEY_SH: &str = r#"
me=$(id -un); uid=$(id -u)
for s in /tmp/.X11-unix/X*; do
  [ -e "$s" ] || continue
  n=${s##*X}; o=$(stat -c %U "$s" 2>/dev/null || stat -f %Su "$s" 2>/dev/null)
  size=""
  if [ "$o" = "$me" ]; then
    size=$(DISPLAY=:$n xrandr --current 2>/dev/null | sed -n 's/.*current \([0-9]*\) x \([0-9]*\).*/\1x\2/p' | head -1)
    [ -z "$size" ] && [ -f "$HOME/.blazar/xauth-$n" ] && size=$(DISPLAY=:$n XAUTHORITY="$HOME/.blazar/xauth-$n" xrandr --current 2>/dev/null | sed -n 's/.*current \([0-9]*\) x \([0-9]*\).*/\1x\2/p' | head -1)
    [ -z "$size" ] && [ -f "/run/user/$uid/gdm/Xauthority" ] && size=$(DISPLAY=:$n XAUTHORITY="/run/user/$uid/gdm/Xauthority" xrandr --current 2>/dev/null | sed -n 's/.*current \([0-9]*\) x \([0-9]*\).*/\1x\2/p' | head -1)
  fi
  managed=no; [ -f "$HOME/.blazar/sims/xorg-$n/pid" ] && managed=yes
  locked=no
  if [ "$o" = "$me" ] && [ "$managed" = no ] && command -v loginctl >/dev/null 2>&1; then
    for sid in $(loginctl list-sessions --no-legend 2>/dev/null | awk -v u="$me" '$3==u{print $1}'); do
      case "$(loginctl show-session "$sid" -p Display -p LockedHint --value 2>/dev/null | tr '\n' ' ')" in ":$n "*yes*) locked=yes;; esac
    done
  fi
  printf 'DISPLAY\t%s\t%s\t%s\t%s\t%s\n' "$n" "$o" "$size" "$managed" "$locked"
done
printf 'OS\t%s\n' "$(uname -s)"
printf 'XORG\t%s\n' "$(command -v Xorg 2>/dev/null || ls /usr/lib/xorg/Xorg 2>/dev/null || echo -)"
printf 'NVDRV\t%s\n' "$(ls /usr/lib/x86_64-linux-gnu/nvidia/xorg/nvidia_drv.so /usr/lib/xorg/modules/drivers/nvidia_drv.so /usr/lib/aarch64-linux-gnu/nvidia/xorg/nvidia_drv.so 2>/dev/null | head -1)"
printf 'XWRAP\t%s\n' "$(sed -n 's/^allowed_users=//p' /etc/X11/Xwrapper.config 2>/dev/null | tail -1)"
printf 'CONF\t%s\n' "$([ -f /etc/X11/blazar-headless.conf ] && echo yes || echo no)"
printf 'FFMPEG\t%s\t%s\n' "$(command -v ffmpeg 2>/dev/null || echo -)" "$(ffmpeg -hide_banner -loglevel error -nostdin -f lavfi -i color=c=black:s=256x256:r=30 -frames:v 1 -c:v h264_nvenc -f null - >/dev/null 2>&1 && echo 1 || echo 0)"
"#;

pub fn parse_survey(raw: &str) -> Survey {
    let mut s = Survey::default();
    for line in raw.lines() {
        let p: Vec<&str> = line.split('\t').collect();
        match p.as_slice() {
            ["DISPLAY", n, owner, size, managed, locked] => {
                let Ok(number) = n.parse::<u32>() else {
                    continue;
                };
                let size = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                s.displays.push(Display {
                    number,
                    owner: (*owner).to_owned(),
                    mine: size.is_some(),
                    size,
                    managed: *managed == "yes",
                    locked: *locked == "yes",
                });
            }
            ["OS", os] => s.os = (*os).to_owned(),
            ["XORG", x] => s.xorg = x.starts_with('/'),
            ["NVDRV", x] => s.nvidia_driver = x.starts_with('/'),
            ["XWRAP", x] => s.wrapper_anybody = x.trim() == "anybody",
            ["CONF", x] => s.headless_conf = *x == "yes",
            ["FFMPEG", path, nvenc] => {
                s.ffmpeg = path.starts_with('/').then(|| (*path).to_owned());
                s.nvenc = nvenc.trim().parse::<u32>().unwrap_or(0) > 0;
            }
            _ => {}
        }
    }
    s
}

pub async fn survey(st: &Shared, node: &str) -> Result<Survey, String> {
    let script = format!("{}\n{SURVEY_SH}", blazar_transport::PATH_PRELUDE);
    let out = st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
        .map_err(|e| format!("查看 {node} 的显示失败：{e}"))?;
    Ok(parse_survey(&out.stdout))
}

pub struct Chosen {
    pub number: u32,
    pub size: Option<(u32, u32)>,
    pub xauthority: Option<String>,
}

fn xauth_path(home: &str, n: u32) -> String {
    format!("{home}/.blazar/xauth-{n}")
}

pub async fn choose(st: &Shared, node: &str, want: &str) -> Result<Chosen, String> {
    let s = survey(st, node).await?;
    let home = super::home_of(&st.transport(node)).await?;
    let pick = |d: &Display| Chosen {
        number: d.number,
        size: d.size,
        xauthority: d.managed.then(|| xauth_path(&home, d.number)),
    };
    let want = want.trim();
    if let Some(n) = want.strip_prefix(':').and_then(|n| n.parse::<u32>().ok()) {
        return s
            .displays
            .iter()
            .find(|d| d.number == n && d.mine)
            .map(pick)
            .ok_or_else(|| format!("{node} 上没有你能用的显示 :{n}"));
    }
    if want != "virtual"
        && let Some(d) = s
            .displays
            .iter()
            .find(|d| d.mine && !d.managed && !d.locked)
    {
        return Ok(pick(d));
    }
    if let Some(d) = s.displays.iter().find(|d| d.mine && d.managed) {
        return Ok(pick(d));
    }
    if want != "virtual"
        && !s.virtual_ready()
        && let Some(d) = s.displays.iter().find(|d| d.mine && !d.managed)
    {
        return Ok(pick(d));
    }
    if !s.virtual_ready() {
        return Err(format!(
            "{node} 上没有你能用的桌面，也还没准备好虚拟显示（{}）。到软件页点「准备脚本（root）」拿到脚本，在那台机器上用 root 跑一次",
            s.missing().join("、")
        ));
    }
    let n = start_virtual_inner(st, node, &home, &s).await?;
    Ok(Chosen {
        number: n,
        size: Some((1920, 1080)),
        xauthority: Some(xauth_path(&home, n)),
    })
}

async fn start_virtual_inner(
    st: &Shared,
    node: &str,
    home: &str,
    s: &Survey,
) -> Result<u32, String> {
    let taken: Vec<u32> = s.displays.iter().map(|d| d.number).collect();
    let n = (9..30)
        .find(|n| !taken.contains(n))
        .ok_or("显示编号都被占了")?;
    let auth = xauth_path(home, n);
    let prep = format!(
        r#"umask 077; mkdir -p "$HOME/.blazar/sims"
if command -v xauth >/dev/null 2>&1 && command -v mcookie >/dev/null 2>&1; then
  rm -f {auth}; xauth -f {auth} add :{n} . "$(mcookie)" 2>/dev/null && echo AUTH
fi"#,
        auth = blazar_transport::shell_quote(&auth),
    );
    let prepared = st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(prep))
        .await
        .map_err(|e| e.to_string())?;
    let with_auth = prepared.stdout.contains("AUTH");
    let mut spec = ExecSpec::new("Xorg").args([
        format!(":{n}"),
        "-config".into(),
        HEADLESS_CONF.into(),
        "-configdir".into(),
        "/dev/null".into(),
        "-noreset".into(),
        "-nolisten".into(),
        "tcp".into(),
        "-sharevts".into(),
        "-novtswitch".into(),
        "+extension".into(),
        "GLX".into(),
        "+extension".into(),
        "RANDR".into(),
        "+extension".into(),
        "RENDER".into(),
    ]);
    if with_auth {
        spec = spec.arg("-auth").arg(&auth);
    }
    let root = std::path::PathBuf::from(format!("{home}/.blazar/sims"));
    DetachedRun::launch_in(
        st.transport(node),
        &root,
        &format!("xorg-{n}"),
        &spec,
        None,
        RunMode::Null,
    )
    .await
    .map_err(|e| format!("起虚拟显示失败：{e}"))?;
    let wait = format!(
        r#"for i in $(seq 1 40); do
  if [ -e /tmp/.X11-unix/X{n} ] && DISPLAY=:{n} XAUTHORITY={auth} xrandr --current >/dev/null 2>&1; then echo UP; exit 0; fi
  [ -e "$HOME/.blazar/sims/xorg-{n}/exit.code" ] && break
  sleep 0.25
done
tail -n 5 "$HOME/.blazar/sims/xorg-{n}/err.txt" 2>/dev/null"#,
        auth = blazar_transport::shell_quote(&auth),
    );
    let out = st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(wait))
        .await
        .map_err(|e| e.to_string())?;
    if out.stdout.contains("UP") {
        Ok(n)
    } else {
        Err(format!(
            "虚拟显示 :{n} 没起来：{}",
            out.stdout.trim().lines().last().unwrap_or("没有日志")
        ))
    }
}

pub async fn start_virtual(
    State(st): State<Shared>,
    Path(node): Path<String>,
) -> ApiResult<Json<Value>> {
    let s = survey(&st, &node)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    if let Some(d) = s.displays.iter().find(|d| d.mine && d.managed) {
        return Ok(Json(
            json!({ "display": format!(":{}", d.number), "already": true }),
        ));
    }
    if !s.virtual_ready() {
        return Err(ApiError::bad_request(format!(
            "{node} 还没准备好虚拟显示：{}。先点「准备脚本（root）」拿到脚本，在那台机器上用 root 跑一次",
            s.missing().join("、")
        )));
    }
    let home = super::home_of(&st.transport(&node))
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    let n = start_virtual_inner(&st, &node, &home, &s)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    Ok(Json(
        json!({ "display": format!(":{n}"), "already": false }),
    ))
}

pub fn setup_script() -> String {
    format!(
        r#"#!/bin/bash
# Blazar：给无头的 NVIDIA 机器准备一个虚拟显示（只需用 root 跑一次）
set -e
[ "$(id -u)" = 0 ] || {{ echo "请用 sudo 运行"; exit 1; }}
MAJ=$(nvidia-smi --query-gpu=driver_version --format=csv,noheader 2>/dev/null | head -1 | cut -d. -f1)
[ -n "$MAJ" ] || {{ echo "没有找到 NVIDIA 驱动（nvidia-smi 不可用）"; exit 1; }}
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq xserver-xorg-core xserver-xorg-legacy xauth x11-xserver-utils
if ! ls /usr/lib/x86_64-linux-gnu/nvidia/xorg/nvidia_drv.so /usr/lib/xorg/modules/drivers/nvidia_drv.so >/dev/null 2>&1; then
  apt-get install -y -qq "xserver-xorg-video-nvidia-$MAJ" || echo "装不上 xserver-xorg-video-nvidia-$MAJ：如果驱动是 .run 装的，X 模块应已自带"
fi
cat > {conf} <<'EOF'
Section "ServerLayout"
    Identifier "Blazar"
    Screen 0 "BlazarScreen" 0 0
EndSection
Section "Device"
    Identifier "BlazarGPU"
    Driver "nvidia"
    Option "AllowEmptyInitialConfiguration" "True"
    Option "UseDisplayDevice" "None"
EndSection
Section "Screen"
    Identifier "BlazarScreen"
    Device "BlazarGPU"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Virtual 1920 1080
    EndSubSection
EndSection
EOF
cat > /etc/X11/Xwrapper.config <<'EOF'
allowed_users=anybody
needs_root_rights=yes
EOF
echo "完成：之后 Blazar 可以在这台机器上自动起虚拟显示"
"#,
        conf = HEADLESS_CONF
    )
}

pub async fn setup_script_route() -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "script": setup_script() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_marks_only_readable_displays_as_mine() {
        let raw = "DISPLAY\t1\tcraig\t1920x1080\tno\tyes\nDISPLAY\t2\tghr\t\tno\tno\nDISPLAY\t9\tcraig\t1920x1080\tyes\tno\n\
                   OS\tLinux\nXORG\t/usr/bin/Xorg\nNVDRV\t/usr/lib/xorg/modules/drivers/nvidia_drv.so\nXWRAP\tconsole\nCONF\tno\nFFMPEG\t/usr/bin/ffmpeg\t3\n";
        let s = parse_survey(raw);
        assert_eq!(s.displays.len(), 3);
        assert!(s.displays[0].mine && !s.displays[0].managed && s.displays[0].locked);
        assert!(!s.displays[1].mine);
        assert!(s.displays[2].managed);
        assert!(s.xorg && s.nvidia_driver && !s.wrapper_anybody && !s.headless_conf);
        assert!(!s.virtual_ready());
        assert_eq!(
            s.missing(),
            vec!["Xwrapper 不允许普通用户起 X", "没有 Blazar 的无头显示配置"]
        );
        assert_eq!(s.ffmpeg.as_deref(), Some("/usr/bin/ffmpeg"));
        assert!(s.nvenc);
    }

    #[test]
    fn survey_script_runs_on_a_machine_without_x() {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(SURVEY_SH)
            .env("HOME", tempfile::tempdir().unwrap().path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(out.status.success());
        let s = parse_survey(&String::from_utf8_lossy(&out.stdout));
        assert!(!s.os.is_empty());
    }

    #[test]
    fn setup_script_touches_only_blazar_config_and_the_wrapper() {
        let s = setup_script();
        assert!(s.contains(HEADLESS_CONF));
        assert!(s.contains("allowed_users=anybody"));
        assert!(
            !s.contains("/etc/X11/xorg.conf "),
            "不能动机器原有的 xorg.conf"
        );
        assert!(s.contains("AllowEmptyInitialConfiguration"));
    }
}
