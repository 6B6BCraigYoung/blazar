#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/target/release/bundle/macos/Blazar.app"
PROFILE="${BLAZAR_DEV_PROFILE:-dev}"
DEV="$ROOT/target/dev-instance/$PROFILE/Blazar Dev.app"
BIN="$DEV/Contents/MacOS/blazar-desktop"
DATA="$HOME/Library/Application Support/ai.blazar.Blazar/profiles/$PROFILE"
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
  local p
  p=$(pid)
  [ -z "$p" ] && return 0
  kill -TERM "$p"
  for _ in $(seq 1 40); do
    kill -0 "$p" 2>/dev/null || return 0
    sleep 0.25
  done
  kill -KILL "$p" 2>/dev/null || true
}

url() {
  local port
  port=$(cat "$DATA/hub.port" 2>/dev/null || true)
  [ -n "$port" ] && echo "http://127.0.0.1:$port"
}

start() {
  [ -x "$BIN" ] || build
  stop
  open -n -g -a "$DEV" --env "BLAZAR_PROFILE=$PROFILE" --env BLAZAR_DEV_INSTANCE=1 \
    --env "BLAZAR_UI_DRIVER=${BLAZAR_UI_DRIVER:-1}" \
    ${BLAZAR_WINDOW_SIZE:+--env "BLAZAR_WINDOW_SIZE=$BLAZAR_WINDOW_SIZE"}
  for _ in $(seq 1 120); do
    local p u
    p=$(pid)
    u=$(url || true)
    if [ -n "$p" ] && [ -n "$u" ] && curl -fsS -o /dev/null "$u/" 2>/dev/null; then
      [ "${BLAZAR_UI_DRIVER:-1}" = 1 ] && wait_page
      echo "已启动：pid $p，$u"
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
    echo "运行中：pid $p，$(url || echo 无地址)，数据目录 $DATA"
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
  local session="$DATA/hub.session" base token
  [ -n "$(pid)" ] && [ -f "$session" ] || { echo "开发实例没有运行" >&2; return 1; }
  base=$(plutil -extract url raw -o - "$session")
  token=$(plutil -extract token raw -o - "$session")
  curl -sS -X POST "$base/api/dev/ui" -H "Authorization: Bearer $token" \
    -H 'Content-Type: application/json' --data "$1"
  echo
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
  *) echo "用法：dev-instance.sh build|start|stop|restart|status|url|shot [文件名]|ui '<JSON>'" >&2; exit 2 ;;
esac
