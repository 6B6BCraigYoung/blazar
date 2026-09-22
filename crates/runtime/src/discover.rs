use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

use crate::spec::{AgentSpec, BUILTIN};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredAgent {
    pub id: String,
    pub label: String,

    pub path: Option<String>,
    pub version: Option<String>,

    pub authed: Option<bool>,

    pub auth_hint: Option<String>,
    pub structured_output: bool,
    pub supports_resume: bool,
}

impl DiscoveredAgent {
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.path.is_some() && self.authed != Some(false)
    }
}

pub async fn discover(
    transport: &Arc<dyn NodeTransport>,
) -> Result<Vec<DiscoveredAgent>, blazar_transport::TransportError> {
    let script = build_script(BUILTIN);
    let out = transport
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await?;
    Ok(parse(&out.stdout, BUILTIN))
}

fn build_script(specs: &[AgentSpec]) -> String {
    let names: Vec<&str> = specs.iter().map(|s| s.program).collect();
    format!(
        r#"
# 有些 CLI 只在登录 shell 的 PATH 里；先把常见安装前缀补进来，
# 比 fork 一个登录 shell 便宜，且对非交互会话同样有效。
for d in "$HOME/.local/bin" "$HOME/.npm-global/bin" "$HOME/bin" \
         "$HOME/.bun/bin" "$HOME/.deno/bin" "$HOME/.cargo/bin" \
         /opt/homebrew/bin /usr/local/bin; do
  [ -d "$d" ] && case ":$PATH:" in *":$d:"*) ;; *) PATH="$d:$PATH";; esac
done
# nvm / fnm 的多版本目录：取最新一个
for base in "$HOME/.nvm/versions/node" "$HOME/.local/share/fnm/node-versions"; do
  [ -d "$base" ] || continue
  latest=$(ls -1 "$base" 2>/dev/null | sort -V | tail -1)
  [ -n "$latest" ] && [ -d "$base/$latest/bin" ] && PATH="$base/$latest/bin:$PATH"
  [ -n "$latest" ] && [ -d "$base/$latest/installation/bin" ] && PATH="$base/$latest/installation/bin:$PATH"
done
export PATH

# macOS 默认没有 timeout(1)。缺了它就退化成直接执行 ——
# 宁可偶尔慢，也不要因为命令不存在而拿不到任何版本号。
T=""
command -v timeout >/dev/null 2>&1 && T="timeout 8"
command -v gtimeout >/dev/null 2>&1 && T="gtimeout 8"

# macOS：有些 CLI 只存在于 app bundle 里，PATH 里永远找不到
BUNDLES="/Applications/ChatGPT.app/Contents/Resources
/Applications/Codex.app/Contents/Resources
$HOME/Applications/ChatGPT.app/Contents/Resources"

resolve() {{
  p=$(command -v "$1" 2>/dev/null) && {{ echo "$p"; return 0; }}
  echo "$BUNDLES" | while read -r b; do
    [ -x "$b/$1" ] && {{ echo "$b/$1"; return 0; }}
  done | head -1
  # DeepSeek 的 dsh 装在 ~/.dsh 下、不一定有启动器：直接用 node 跑它的入口文件
  if [ "$1" = dsh ] && [ -f "$HOME/.dsh/profiles/node_modules/@deepseek-ai/dsh/lib/bin.js" ] && command -v node >/dev/null 2>&1; then
    echo "$HOME/.dsh/profiles/node_modules/@deepseek-ai/dsh/lib/bin.js"
  fi
}}

for name in {names}; do
  p=$(resolve "$name")
  if [ -n "$p" ]; then
    echo "PATH|$name|$p"
    # 版本号取第一行第一个像版本的 token；超时兜底防止 CLI 卡住整次发现
    case "$p" in
      *.js) v=$($T node "$p" --version 2>/dev/null | head -1 | tr -d '\r') ;;
      *)    v=$($T "$p" --version 2>/dev/null | head -1 | tr -d '\r') ;;
    esac
    [ -n "$v" ] && echo "VER|$name|$v"
  fi
done

# 登录态：各家凭据落点不同，只做存在性检查，绝不读内容
[ -f "$HOME/.claude/.credentials.json" ] && echo "AUTH|claude|1|credentials.json"
# macOS 把 claude 凭据放在钥匙串里。只问「这一项在不在」—— 不带 -w，不读出任何内容
if [ ! -f "$HOME/.claude/.credentials.json" ] && command -v security >/dev/null 2>&1 \
   && security find-generic-password -s 'Claude Code-credentials' >/dev/null 2>&1; then
  echo "AUTH|claude|1|macOS 钥匙串"
fi
[ -d "$HOME/.claude" ] && [ ! -f "$HOME/.claude/.credentials.json" ] && echo "AUTH|claude|?|有 ~/.claude 但无凭据文件（macOS 可能在 Keychain）"
[ -f "$HOME/.codex/auth.json" ] && echo "AUTH|codex|1|auth.json"
[ -n "$OPENAI_API_KEY" ] && echo "AUTH|codex|1|OPENAI_API_KEY"
[ -n "$ANTHROPIC_API_KEY" ] && echo "AUTH|claude|1|ANTHROPIC_API_KEY"
[ -f "$HOME/.config/github-copilot/apps.json" ] && echo "AUTH|copilot|1|apps.json"
[ -d "$HOME/.local/share/opencode" ] && echo "AUTH|opencode|1|opencode 数据目录"
[ -f "$HOME/.gemini/oauth_creds.json" ] && echo "AUTH|gemini|1|oauth_creds.json"
[ -n "$GEMINI_API_KEY" ] && echo "AUTH|gemini|1|GEMINI_API_KEY"
[ -d "$HOME/.qwen" ] && echo "AUTH|qwen|1|~/.qwen"
# grok 登出后 auth.json 仍在，光看文件判断不了；只有 API key 能确定
[ -n "$XAI_API_KEY" ] && echo "AUTH|grok|1|XAI_API_KEY"
[ -f "$HOME/.grok/auth.json" ] && echo "AUTH|grok|?|有 auth.json，是否仍有效要运行 grok 才知道"
[ -f "$HOME/.deepcode/settings.json" ] && echo "AUTH|deepcode|?|有 settings.json（API key 写在里面，不读取）"
[ -n "$DEEPSEEK_API_KEY" ] && echo "AUTH|dsh|1|DEEPSEEK_API_KEY"
[ -f "$HOME/.dsh/.credentials.yaml" ] && echo "AUTH|dsh|?|有 .credentials.yaml，是否配了 DeepSeek 的 key 要运行 dsh 才知道"
"#,
        names = names.join(" "),
    )
}

fn parse(raw: &str, specs: &[AgentSpec]) -> Vec<DiscoveredAgent> {
    use std::collections::HashMap;
    let mut paths: HashMap<&str, String> = HashMap::new();
    let mut versions: HashMap<&str, String> = HashMap::new();
    let mut auth: HashMap<&str, (Option<bool>, String)> = HashMap::new();

    let by_program: HashMap<&str, &str> = specs.iter().map(|s| (s.program, s.id)).collect();

    for line in raw.lines() {
        let parts: Vec<&str> = line.trim().split('|').collect();
        match parts.as_slice() {
            ["PATH", name, p] => {
                if let Some(id) = by_program.get(name) {
                    paths.insert(id, (*p).to_owned());
                }
            }
            ["VER", name, v] => {
                if let Some(id) = by_program.get(name) {
                    versions.insert(id, clean_version(v));
                }
            }
            ["AUTH", id, flag, hint] => {
                if let Some(spec) = specs.iter().find(|s| s.id == *id) {
                    let state = match *flag {
                        "1" => Some(true),
                        "0" => Some(false),
                        _ => None,
                    };

                    let entry = auth.entry(spec.id).or_insert((state, (*hint).to_owned()));
                    if entry.0.is_none() && state.is_some() {
                        *entry = (state, (*hint).to_owned());
                    }
                }
            }
            _ => {}
        }
    }

    specs
        .iter()
        .map(|s| {
            let a = auth.get(s.id);
            DiscoveredAgent {
                id: s.id.to_owned(),
                label: s.label.to_owned(),
                path: paths.get(s.id).cloned(),
                version: versions.get(s.id).cloned(),

                authed: paths.get(s.id).and(a.map(|x| x.0)).flatten(),
                auth_hint: paths.get(s.id).and(a.map(|x| x.1.clone())),
                structured_output: !matches!(s.output, crate::spec::OutputFormat::PlainLines),
                supports_resume: s.resume.is_supported(),
            }
        })
        .collect()
}

fn clean_version(raw: &str) -> String {
    raw.split_whitespace()
        .find(|t| {
            let t = t.trim_start_matches('v');
            !t.is_empty() && t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.')
        })
        .unwrap_or(raw.trim())
        .trim_start_matches('v')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_extracted_from_varied_formats() {
        assert_eq!(clean_version("2.1.274 (Claude Code)"), "2.1.274");
        assert_eq!(clean_version("codex-cli 0.154.0"), "0.154.0");
        assert_eq!(clean_version("v1.2.3"), "1.2.3");
        assert_eq!(clean_version("1.0"), "1.0");

        assert_eq!(clean_version("unknown build"), "unknown build");
    }

    #[test]
    fn parses_paths_versions_and_auth() {
        let raw = "PATH|claude|/Users/c/.local/bin/claude\n\
                   VER|claude|2.1.274 (Claude Code)\n\
                   AUTH|claude|1|credentials.json\n\
                   PATH|codex|/opt/homebrew/bin/codex\n\
                   VER|codex|codex-cli 0.154.0\n";
        let got = parse(raw, BUILTIN);
        let claude = got.iter().find(|a| a.id == "claude").unwrap();
        assert_eq!(claude.path.as_deref(), Some("/Users/c/.local/bin/claude"));
        assert_eq!(claude.version.as_deref(), Some("2.1.274"));
        assert_eq!(claude.authed, Some(true));
        assert!(claude.is_usable());

        let codex = got.iter().find(|a| a.id == "codex").unwrap();
        assert_eq!(codex.authed, None);
        assert!(codex.is_usable(), "无法判断登录态时不应判为不可用");
    }

    #[test]
    fn missing_agent_has_no_auth_noise() {
        let raw = "AUTH|gemini|1|oauth_creds.json\n";
        let got = parse(raw, BUILTIN);
        let g = got.iter().find(|a| a.id == "gemini").unwrap();
        assert!(g.path.is_none());
        assert_eq!(g.authed, None);
        assert!(!g.is_usable());
    }

    #[test]
    fn uncertain_auth_does_not_override_certain() {
        let raw = "PATH|claude|/x/claude\n\
                   AUTH|claude|1|credentials.json\n\
                   AUTH|claude|?|有 ~/.claude 但无凭据文件\n";
        let got = parse(raw, BUILTIN);
        assert_eq!(
            got.iter().find(|a| a.id == "claude").unwrap().authed,
            Some(true)
        );
    }

    #[test]
    fn every_builtin_appears_even_when_absent() {
        let got = parse("", BUILTIN);
        assert_eq!(got.len(), BUILTIN.len());
        assert!(got.iter().all(|a| a.path.is_none()));
    }

    #[test]
    fn script_mentions_login_shell_prefixes_and_bundles() {
        let s = build_script(BUILTIN);
        assert!(s.contains(".nvm/versions/node"), "nvm 多版本目录必须兜底");
        assert!(s.contains("ChatGPT.app"), "macOS app bundle 必须兜底");
        assert!(s.contains("$HOME/.local/bin"), "原生安装器前缀必须兜底");

        assert!(
            s.contains("command -v timeout"),
            "必须探测 timeout 是否存在"
        );
        assert!(s.contains("gtimeout"), "macOS 上常见的替代品也要兜底");
    }
}
