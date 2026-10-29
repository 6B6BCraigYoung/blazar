#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/target/release/bundle/macos/Blazar.app"
PROFILE="${BLAZAR_DEV_PROFILE:-dev}"
DEV="$ROOT/target/dev-instance/$PROFILE/Blazar Dev.app"
BIN="$DEV/Contents/MacOS/blazar-desktop"
DATA="${BLAZAR_DEV_HOME:-$HOME}/Library/Application Support/ai.blazar.Blazar/profiles/$PROFILE"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

pid() {
  pgrep -f "^$BIN" | head -1 || true
}

build() {
  BLAZAR_BUNDLES=app "$ROOT/scripts/package-macos.sh"
  "$LSREGISTER" -u "$SRC" 2>/dev/null || true
  rm -rf "$DEV"
  mkdir -p "$(dirname "$DEV")"
  ditto "$SRC" "$DEV"
  local plist="$DEV/Contents/Info.plist"
  plutil -replace CFBundleIdentifier -string ai.blazar.desktop.dev "$plist"
  plutil -replace CFBundleName -string "Blazar Dev" "$plist"
  plutil -replace CFBundleDisplayName -string "Blazar Dev" "$plist"
  plutil -remove CFBundleDocumentTypes "$plist" 2>/dev/null || true
  "$ROOT/scripts/local-signing.sh" sign "$DEV"
}

stop() {
  local ps p
  ps=$(pgrep -f "^$BIN" || true)
  [ -z "$ps" ] && return 0
  kill -TERM $ps 2>/dev/null || true
  for _ in $(seq 1 40); do
    [ -z "$(pgrep -f "^$BIN" || true)" ] && return 0
    sleep 0.25
  done
  for p in $(pgrep -f "^$BIN" || true); do kill -KILL "$p" 2>/dev/null || true; done
}

url() {
  local port
  port=$(cat "$DATA/hub.port" 2>/dev/null || true)
  [ -n "$port" ] && echo "http://127.0.0.1:$port"
}

start() {
  [ -x "$BIN" ] || build
  stop
  env -u ANTHROPIC_BASE_URL -u ANTHROPIC_AUTH_TOKEN -u ANTHROPIC_API_KEY \
    open -n -g -a "$DEV" --env "BLAZAR_PROFILE=$PROFILE" --env BLAZAR_DEV_INSTANCE=1 \
    --env "BLAZAR_UI_DRIVER=${BLAZAR_UI_DRIVER:-1}" \
    --env "BLAZAR_MESH_DISCOVERY=${BLAZAR_MESH_DISCOVERY:-0}" \
    ${BLAZAR_DEV_HOME:+--env "HOME=$BLAZAR_DEV_HOME"} \
    ${RUST_LOG:+--env "RUST_LOG=$RUST_LOG"} \
    ${BLAZAR_WINDOW_SIZE:+--env "BLAZAR_WINDOW_SIZE=$BLAZAR_WINDOW_SIZE"} \
    ${BLAZAR_DEV_DOWNLOAD_DIR:+--env "BLAZAR_DEV_DOWNLOAD_DIR=$BLAZAR_DEV_DOWNLOAD_DIR"} \
    ${BLAZAR_DEV_PICK_FOLDER:+--env "BLAZAR_DEV_PICK_FOLDER=$BLAZAR_DEV_PICK_FOLDER"} \
    --args -ApplePersistenceIgnoreState YES
  for _ in $(seq 1 120); do
    local p u
    p=$(pid)
    u=$(url || true)
    if [ -n "$p" ] && [ -n "$u" ] && curl -fsS -o /dev/null "$u/" 2>/dev/null; then
      [ "${BLAZAR_UI_DRIVER:-1}" = 1 ] && wait_page
      [ "${BLAZAR_DEV_LINK_ACCOUNT:-0}" = 1 ] && link_account
      echo "已启动：pid ${p}，$u"
      return 0
    fi
    sleep 0.5
  done
  echo "开发实例没有在 60 秒内就绪，日志：$DATA/logs/desktop.log" >&2
  return 1
}

status() {
  local p
  p=$(pid)
  if [ -n "$p" ]; then
    echo "运行中：pid ${p}，$(url || echo 无地址)，数据目录 $DATA"
  else
    echo "没有运行"
    return 1
  fi
}

shot() {
  local name="${1:-}" p id reply path
  p=$(pid)
  [ -z "$p" ] && { echo "开发实例没有运行" >&2; return 1; }
  reply=$(ui "$(jq -cn --arg name "$name" '{action: "shot"} + (if $name == "" then {} else {name: $name} end)')" 2>/dev/null || true)
  path=$(jq -r '.path // empty' <<<"$reply" 2>/dev/null || true)
  if [ -n "$path" ]; then
    echo "$path"
    return 0
  fi
  echo "界面驱动没有截到图，改用 screencapture：$(jq -r '.error // empty' <<<"$reply" 2>/dev/null || echo "$reply")" >&2
  local out="$ROOT/target/dev-instance/$PROFILE/${name:-shot.png}"
  id=$(swift - "$p" <<'EOF'
import CoreGraphics
let pid = Int(CommandLine.arguments[1]) ?? 0
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for w in list where (w["kCGWindowOwnerPID"] as? Int) == pid && (w["kCGWindowLayer"] as? Int) == 0 {
  print(w["kCGWindowNumber"]!)
  break
}
EOF
)
  [ -z "$id" ] && { echo "开发实例的窗口不在屏幕上" >&2; return 1; }
  screencapture -x -o -l"$id" "$out"
  echo "$out"
}

wait_page() {
  for _ in $(seq 1 40); do
    case "$(ui '{"action":"init","timeout_ms":1000}' 2>/dev/null)" in
      *'"ok":true'*) return 0 ;;
    esac
    sleep 0.25
  done
  echo "页面还没有连上界面驱动" >&2
}

ui() {
  [ -n "$(pid)" ] || { echo "开发实例没有运行" >&2; return 1; }
  hub_api "$DATA/hub.session" POST /api/dev/ui "$1"
  echo
}

hub_api() {
  local session="$1" base token
  [ -f "$session" ] || return 1
  base=$(plutil -extract url raw -o - "$session")
  token=$(plutil -extract token raw -o - "$session")
  curl -sS -m 10 -X "$2" "$base$3" -H "Authorization: Bearer $token" \
    -H 'Content-Type: application/json' ${4:+--data "$4"}
}

host_session() {
  local root="$HOME/Library/Application Support/ai.blazar.Blazar" f
  for f in "$root/hub/hub.session" "$root/profiles/default/hub.session"; do
    [ "$f" = "$DATA/hub.session" ] && continue
    [ -f "$f" ] || continue
    hub_api "$f" GET /api/about >/dev/null 2>&1 && { echo "$f"; return 0; }
  done
  return 1
}

link_account() {
  local host src dir label mine id
  host=$(host_session) || { echo "所在机器上没有在运行的正式 hub，Claude 账号保持原样" >&2; return 0; }
  src=$(hub_api "$host" GET /api/accounts | jq -r '
    .accounts
    | map(select(.provider == "claude" and .kind == "token" and (.disabled | not) and .config_dir != null and .linked == null))
    | (map(select(.status == "ok")) + .)[0]
    | if . then [.config_dir, .label] | @tsv else empty end' || true)
  [ -n "$src" ] || { echo "所在 hub 没有可用的 token 账号，Claude 账号保持原样" >&2; return 0; }
  IFS=$'\t' read -r dir label <<<"$src"
  mine=$(hub_api "$DATA/hub.session" GET /api/accounts || true)
  id=$(jq -r --arg f "$dir/.blazar-oauth-token" '.accounts[]? | select(.linked == $f) | .id' <<<"$mine" | head -1)
  if [ -z "$id" ]; then
    id=$(hub_api "$DATA/hub.session" POST /api/accounts \
      "$(jq -cn --arg l "${label}（引用）" --arg f "$dir/.blazar-oauth-token" '{provider: "claude", label: $l, token_file: $f}')" \
      | jq -r '.id // empty' || true)
  fi
  [ -n "$id" ] || { echo "建不了引用账号，Claude 账号保持原样" >&2; return 0; }
  hub_api "$DATA/hub.session" PUT /api/accounts/mode "$(jq -cn --arg id "$id" '{provider: "claude", mode: $id}')" >/dev/null
  echo "Claude 账号：引用所在 hub 的「${label}」"
}

case "${1:-status}" in
  build) build ;;
  start) start ;;
  stop) stop ;;
  restart) stop; start ;;
  status) status ;;
  url) url ;;
  shot) shot "${2:-}" ;;
  ui) ui "${2:?用法：dev-instance.sh ui '<JSON 命令>'}" ;;
  link) link_account ;;
  *)
    echo "用法：dev-instance.sh build|start|stop|restart|status|url|shot [文件名]|ui '<JSON>'|link" >&2
    echo "环境变量：BLAZAR_DEV_LINK_ACCOUNT=1 让 start 自动引用所在 hub 的 token 账号（默认 0 不引用；link 随时可手动引用）" >&2
    exit 2 ;;
esac
