#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/hub-service.sh install|uninstall|status|logs

Keeps blazar-hub running on this machine as a user service, so bots keep
working after the laptop that started them is closed.

  install     write the service definition, start it now, start it on login
  uninstall   stop the service and remove the definition
  status      show whether the service is running
  logs        tail the hub log

environment:
  BLAZAR_HUB_BIN   path to blazar-hub (default: the one on PATH)
  BLAZAR_DB        database file (default: the platform data directory)
  BLAZAR_BIND      listen address (default: 127.0.0.1:7777; keep it loopback and
                   reach it with `ssh -N -L 7777:127.0.0.1:7777 hub-host`)
  HTTP_PROXY, HTTPS_PROXY, ALL_PROXY, NO_PROXY
                   copied into the service when set, so the hub and the agent
                   CLIs it starts reach the model APIs the same way your shell does
EOF
}

cmd=${1:-}
[ -n "$cmd" ] || { usage; exit 2; }

bin=${BLAZAR_HUB_BIN:-$(command -v blazar-hub || true)}
bind=${BLAZAR_BIND:-127.0.0.1:7777}
case "$(uname -s)" in
  Darwin)
    data="${HOME}/Library/Application Support/ai.blazar.Blazar/hub"
    plist="${HOME}/Library/LaunchAgents/ai.blazar.hub.plist"
    ;;
  *)
    data="${XDG_DATA_HOME:-$HOME/.local/share}/blazar/hub"
    unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/blazar-hub.service"
    ;;
esac
db=${BLAZAR_DB:-$data/blazar.sqlite}
log="$data/logs/hub.log"

need_bin() {
  [ -n "$bin" ] && [ -x "$bin" ] || { echo "blazar-hub not found; set BLAZAR_HUB_BIN" >&2; exit 1; }
  mkdir -p "$(dirname "$db")" "$(dirname "$log")"
  chmod 700 "$(dirname "$db")"
}

proxy_vars() {
  for v in HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY; do
    val=${!v:-}
    [ -n "$val" ] && printf '%s\t%s\n' "$v" "$val"
  done
  return 0
}

plist_env() {
  proxy_vars | while IFS=$'\t' read -r k v; do
    printf '    <key>%s</key><string>%s</string>\n' "$k" "$v"
  done
}

unit_env() {
  proxy_vars | while IFS=$'\t' read -r k v; do
    printf 'Environment=%s=%s\n' "$k" "$v"
  done
}

case "$(uname -s)" in
  Darwin)
    case "$cmd" in
      install)
        need_bin
        mkdir -p "$(dirname "$plist")"
        cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>ai.blazar.hub</string>
  <key>ProgramArguments</key>
  <array><string>$bin</string></array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>BLAZAR_DB</key><string>$db</string>
    <key>BLAZAR_BIND</key><string>$bind</string>
    <key>PATH</key><string>$HOME/.blazar/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
$(plist_env)  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>5</integer>
  <key>StandardOutPath</key><string>$log</string>
  <key>StandardErrorPath</key><string>$log</string>
</dict>
</plist>
EOF
        launchctl bootout "gui/$(id -u)" "$plist" 2>/dev/null || true
        launchctl bootstrap "gui/$(id -u)" "$plist"
        echo "installed: $plist"
        echo "hub listens on $bind, database $db, log $log"
        ;;
      uninstall)
        launchctl bootout "gui/$(id -u)" "$plist" 2>/dev/null || true
        rm -f "$plist"
        echo "removed"
        ;;
      status)
        launchctl print "gui/$(id -u)/ai.blazar.hub" 2>/dev/null | grep -E 'state|pid' || echo "not installed"
        ;;
      logs) tail -n 100 -f "$log" ;;
      *) usage; exit 2 ;;
    esac
    ;;
  *)
    case "$cmd" in
      install)
        need_bin
        mkdir -p "$(dirname "$unit")"
        cat > "$unit" <<EOF
[Unit]
Description=Blazar hub
After=network-online.target

[Service]
ExecStart=$bin
Environment=BLAZAR_DB=$db
Environment=BLAZAR_BIND=$bind
Environment=PATH=$HOME/.blazar/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
$(unit_env)Restart=always
RestartSec=5
StandardOutput=append:$log
StandardError=append:$log

[Install]
WantedBy=default.target
EOF
        systemctl --user daemon-reload
        systemctl --user enable --now blazar-hub.service
        echo "installed: $unit"
        echo "hub listens on $bind, database $db, log $log"
        if command -v loginctl >/dev/null 2>&1 && ! loginctl show-user "$USER" 2>/dev/null | grep -q 'Linger=yes'; then
          echo "run 'sudo loginctl enable-linger $USER' so the service survives logout"
        fi
        ;;
      uninstall)
        systemctl --user disable --now blazar-hub.service 2>/dev/null || true
        rm -f "$unit"
        systemctl --user daemon-reload
        echo "removed"
        ;;
      status) systemctl --user status blazar-hub.service --no-pager || true ;;
      logs) tail -n 100 -f "$log" ;;
      *) usage; exit 2 ;;
    esac
    ;;
esac
