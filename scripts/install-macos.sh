#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NEW="${1:-$ROOT/target/bundle.noindex/Blazar.app}"
APP=/Applications/Blazar.app
DATA="$HOME/Library/Application Support/ai.blazar.Blazar"
PROFILE_DIR="$DATA/profiles/${BLAZAR_PROFILE:-default}"
DB="$PROFILE_DIR/blazar.sqlite"
ROLLBACK="$DATA/backups/rollback.noindex"
LOG="$ROOT/target/install.log"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

if [ -z "${BLAZAR_INSTALL_DETACHED:-}" ]; then
  [ -x "$NEW/Contents/MacOS/blazar-desktop" ] || { echo "no app bundle at $NEW"; exit 1; }
  mkdir -p "$(dirname "$LOG")"
  BLAZAR_INSTALL_DETACHED=1 nohup perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' \
    /bin/bash "$0" "$NEW" > "$LOG" 2>&1 < /dev/null &
  echo "installing in the background; progress in $LOG"
  exit 0
fi

stamp() { date +%Y%m%d-%H%M%S; }
version_of() { plutil -extract CFBundleVersion raw -o - "$1/Contents/Info.plist" 2>/dev/null || echo unknown; }
to_trash() {
  local dest
  dest="$HOME/.Trash/$(basename "$1" .app)-$(stamp).app"
  "$LSREGISTER" -u "$1" 2>/dev/null || true
  mv "$1" "$dest"
  echo "moved to the Trash: $dest"
}

codesign --verify --deep --strict "$NEW"

if [ -f "$DB" ]; then
  remote=$(sqlite3 -readonly "$DB" \
    "SELECT COUNT(*) FROM sessions WHERE status = 'running' AND COALESCE(node, 'local') <> 'local';")
  if [ "$remote" != "0" ]; then
    echo "an agent is running on a remote machine ($remote); install when it is idle"
    exit 2
  fi
  backup="blazar.sqlite.bak-$(stamp)"
  sqlite3 "$DB" ".backup '$PROFILE_DIR/$backup'"
  echo "database backed up: $backup"
fi

old=$(pgrep -f "^$APP/Contents/MacOS/blazar-desktop$" | head -1 || true)

mkdir -p "$ROLLBACK"
for previous in "$ROLLBACK"/*.app; do
  [ -e "$previous" ] && to_trash "$previous"
done
if [ -d "$APP" ]; then
  kept="$ROLLBACK/Blazar-$(version_of "$APP").app"
  mv "$APP" "$kept"
  "$LSREGISTER" -u "$kept" 2>/dev/null || true
  echo "rollback copy: $kept"
fi

ditto "$NEW" "$APP"
codesign --verify --deep --strict "$APP"
"$LSREGISTER" -f "$APP" 2>/dev/null || true
echo "installed build $(version_of "$APP")"
case "$NEW" in
  "$ROOT/target/"*)
    "$LSREGISTER" -u "$NEW" 2>/dev/null || true
    rm -rf "$NEW"
    echo "removed the staged copy"
    ;;
esac

if [ -n "$old" ]; then
  kill -TERM "$old"
  for _ in $(seq 1 40); do
    kill -0 "$old" 2>/dev/null || break
    sleep 0.5
  done
  if kill -0 "$old" 2>/dev/null; then
    echo "the old app ($old) did not quit; quit it and open $APP yourself"
    exit 3
  fi
fi

new=""
for attempt in 1 2 3 4 5; do
  env -i HOME="$HOME" USER="$USER" LOGNAME="$USER" SHELL="$SHELL" TMPDIR="$TMPDIR" \
    SSH_AUTH_SOCK="$(launchctl getenv SSH_AUTH_SOCK)" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    open -a "$APP" || echo "open failed (attempt $attempt)"
  for _ in $(seq 1 20); do
    new=$(pgrep -f "^$APP/Contents/MacOS/blazar-desktop$" | head -1 || true)
    if [ -n "$new" ] && [ "$new" != "$old" ]; then
      break 2
    fi
    sleep 0.5
  done
  new=""
  sleep 2
done
if [ -z "$new" ]; then
  echo "Blazar did not come back up after 5 tries; open $APP yourself"
  exit 4
fi
echo "running: pid $new, started $(ps -o lstart= -p "$new")"
