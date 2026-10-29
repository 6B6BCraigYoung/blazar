use blazar_transport::ByteStream;

use super::*;

/// tar 退出码 1 只表示有文件在打包时变了或读不了（GNU 的 "file changed as we read it"、
/// BSD 的单个文件权限不足），包本身是完整的；2 以上才是真失败。
const TAR_WARNED: i32 = 1;

pub struct Download {
    /// 单个文件的字节数；打包文件夹事先不知道大小，为 None。
    pub size: Option<u64>,
    pub stream: ByteStream,
    pub folder: bool,
}

impl Download {
    /// stdout 读完后判断这次传输是否完整：文件要正好读够 size 字节且 head 正常退出，
    /// 打包要 tar 正常退出（只有警告也算）。
    pub async fn finished(&mut self, sent: u64) -> std::result::Result<(), String> {
        let code = self.stream.exit_code().await;
        let ok = match (self.size, code) {
            (Some(size), Some(0)) => sent == size,
            (None, Some(0 | TAR_WARNED)) => true,
            _ => false,
        };
        if ok {
            return Ok(());
        }
        let stderr = self.stream.stderr_summary();
        Err(match (code, self.size) {
            (Some(EXIT_SSH), _) => format!("和节点的连接断了：{stderr}"),
            (Some(EXIT_PATH_ESCAPE), _) => "路径越界".to_owned(),
            (Some(0), Some(size)) => format!("文件在下载途中变了：应有 {size} 字节，只读到 {sent}"),
            (None, _) => "节点上的读取进程被中断".to_owned(),
            (Some(c), _) if stderr.is_empty() => format!("节点上的读取进程失败，退出码 {c}"),
            (Some(c), _) => format!("节点上的读取进程失败（退出码 {c}）：{stderr}"),
        })
    }
}

fn refused(code: i32, stderr: String) -> VfsError {
    blazar_transport::TransportError::Command { code, stderr }.into()
}

impl Vfs {
    /// 先在节点上确认目标并拿到大小，再起一个只往 stdout 写原始字节的进程。
    /// 不转 base64、不落临时文件，hub 边读边转发；丢弃返回的流就会杀掉节点上的进程。
    pub async fn open_download(&self, rel: &str, folder: bool, max: u64) -> Result<Download> {
        let rel = if folder && (rel.is_empty() || rel == ".") {
            ".".to_owned()
        } else {
            self.safe_rel(rel)?
        };
        let guard = guarded_path(&self.root, &rel);
        let probe = if folder {
            format!("{guard}\n[ -d \"$F\" ] || exit 4\nprintf ok")
        } else {
            format!(
                r#"{guard}
[ -f "$F" ] || exit 4
SZ=$(stat -c %s -- "$F" 2>/dev/null || stat -f %z -- "$F" 2>/dev/null || wc -c < "$F" | tr -d ' ')
[ "$SZ" -le {max} ] || exit 8
printf '%s' "$SZ""#
            )
        };
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(probe))
            .await?;
        match out.code {
            4 if folder => return Err(refused(4, format!("{rel} 不是一个文件夹"))),
            4 => return Err(refused(4, format!("{rel} 不是一个文件"))),
            8 => return Err(refused(8, format!("{rel} 太大了，超过 {} MB", max >> 20))),
            _ => check(&out, &rel)?,
        }
        let size =
            if folder {
                None
            } else {
                Some(out.stdout.trim().parse::<u64>().map_err(|_| {
                    refused(1, format!("读不出 {rel} 的大小：{}", out.stdout.trim()))
                })?)
            };
        let script = match size {
            // BSD 的 head 不认 -c 0，空文件只确认它还在。
            Some(0) => format!("{guard}\n[ -f \"$F\" ] || exit 4"),
            Some(size) => {
                format!("{guard}\n[ -f \"$F\" ] || exit 4\nexec head -c {size} -- \"$F\"")
            }
            None => format!(
                r#"{guard}
[ -d "$F" ] || exit 4
cd -- "$F" || exit 3
set -o pipefail
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z -co --exclude-standard -- . | while IFS= read -r -d '' f; do
    if [ -e "$f" ] || [ -L "$f" ]; then printf '%s\0' "$f"; fi
  done | tar --null -czf - -T -
else
  tar -czf - .
fi"#
            ),
        };
        let stream = self
            .transport
            .spawn_bytes(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        Ok(Download {
            size,
            stream,
            folder,
        })
    }
}
