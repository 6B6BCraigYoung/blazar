use axum::extract::{Query, State, WebSocketUpgrade, ws};
use axum::response::Response;
use blazar_transport::ExecSpec;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::api::Shared;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub node: String,
    pub display: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_kbps")]
    pub kbps: u32,
    #[serde(default)]
    pub height: u32,
}

fn default_fps() -> u32 {
    30
}

fn default_kbps() -> u32 {
    12000
}

pub struct Plan {
    pub display: u32,
    pub fps: u32,
    pub kbps: u32,
    pub height: u32,
}

impl Plan {
    pub fn from_query(q: &StreamQuery) -> Result<Self, String> {
        let display = q
            .display
            .trim()
            .trim_start_matches(':')
            .parse::<u32>()
            .map_err(|_| "显示编号不对".to_owned())?;
        Ok(Self {
            display,
            fps: q.fps.clamp(5, 60),
            kbps: q.kbps.clamp(1000, 60000),
            height: match q.height {
                0 => 0,
                h => h.clamp(360, 2160),
            },
        })
    }

    pub fn script(&self) -> String {
        let Self {
            display,
            fps,
            kbps,
            height,
        } = self;
        let scale = if *height > 0 {
            format!("-vf scale=-2:{height}")
        } else {
            String::new()
        };
        format!(
            r#"{prelude}
export DISPLAY=:{display}
if [ -z "$XAUTHORITY" ]; then
  if [ -f "$HOME/.blazar/xauth-{display}" ]; then export XAUTHORITY="$HOME/.blazar/xauth-{display}"
  elif ! xrandr --current >/dev/null 2>&1 && [ -f "/run/user/$(id -u)/gdm/Xauthority" ]; then export XAUTHORITY="/run/user/$(id -u)/gdm/Xauthority"; fi
fi
SIZE=$(xrandr --current 2>/dev/null | sed -n 's/.*current \([0-9]*\) x \([0-9]*\).*/\1x\2/p' | head -1)
[ -n "$SIZE" ] || {{ echo "显示 :{display} 打不开" >&2; exit 3; }}
if ffmpeg -hide_banner -loglevel error -nostdin -f lavfi -i color=c=black:s=256x256:r=30 -frames:v 1 -c:v h264_nvenc -f null - >/dev/null 2>&1; then
  ENC="-c:v h264_nvenc -preset p1 -tune ll -rc cbr -b:v {kbps}k -maxrate {kbps}k -bufsize {buf}k -g {gop} -bf 0 -profile:v high -forced-idr 1 -zerolatency 1"
else
  ENC="-c:v libx264 -preset ultrafast -tune zerolatency -b:v {kbps}k -maxrate {kbps}k -bufsize {buf}k -g {gop} -bf 0 -x264-params repeat-headers=1"
fi
exec ffmpeg -hide_banner -loglevel error -nostdin -f x11grab -framerate {fps} -video_size "$SIZE" -draw_mouse 1 -i :{display} {scale} $ENC -pix_fmt yuv420p -f h264 -"#,
            prelude = blazar_transport::PATH_PRELUDE,
            buf = kbps / 2,
            gop = fps * 2,
        )
    }
}

pub async fn ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Query(q): Query<StreamQuery>,
) -> Response {
    ws.on_upgrade(move |socket| run(st, q, socket))
}

type Sink = futures::stream::SplitSink<ws::WebSocket, ws::Message>;

async fn fail(mut tx: Sink, msg: String) {
    let _ = tx
        .send(ws::Message::Text(
            serde_json::json!({ "error": msg }).to_string().into(),
        ))
        .await;
    let _ = tx.close().await;
}

async fn run(st: Shared, q: StreamQuery, socket: ws::WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let plan = match Plan::from_query(&q) {
        Ok(p) => p,
        Err(e) => return fail(tx, e).await,
    };
    if q.node == "local" {
        return fail(tx, "本机暂不支持推流".into()).await;
    }
    if let Err(e) = super::ffmpeg::ensure(&st, &q.node).await {
        return fail(tx, e).await;
    }
    let stream = st
        .transport(&q.node)
        .spawn_bytes(ExecSpec::new("bash").arg("-lc").arg(plan.script()))
        .await;
    let mut stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(tx, format!("起不了推流：{e}")).await,
    };
    let _ = tx
        .send(ws::Message::Text(
            serde_json::json!({ "ready": true, "fps": plan.fps, "kbps": plan.kbps })
                .to_string()
                .into(),
        ))
        .await;
    loop {
        tokio::select! {
            chunk = stream.stdout.next() => match chunk {
                Some(bytes) => {
                    if tx.send(ws::Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                None => {
                    let why = stream.stderr_summary();
                    let _ = tx.send(ws::Message::Text(serde_json::json!({ "ended": true, "stderr": why }).to_string().into())).await;
                    break;
                }
            },
            msg = rx.next() => match msg {
                Some(Ok(ws::Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    let _ = tx.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_clamps_and_builds_a_low_latency_pipeline() {
        let q = StreamQuery {
            node: "n".into(),
            display: ":9".into(),
            fps: 240,
            kbps: 1,
            height: 1080,
        };
        let p = Plan::from_query(&q).unwrap();
        assert_eq!((p.display, p.fps, p.kbps, p.height), (9, 60, 1000, 1080));
        let s = p.script();
        assert!(s.contains("-f x11grab -framerate 60"));
        assert!(s.contains("-i :9"));
        assert!(s.contains("scale=-2:1080"));
        assert!(s.contains("h264_nvenc"));
        assert!(s.contains("libx264"));
        assert!(s.contains("-f h264 -"));
        assert!(
            Plan::from_query(&StreamQuery {
                display: "x".into(),
                ..q
            })
            .is_err()
        );
    }
}
