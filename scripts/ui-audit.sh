#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DI="$ROOT/scripts/dev-instance.sh"
case "${1:-}" in
  seed) exec python3 -I "$ROOT/scripts/ui_audit.py" seed ;;
  clean) exec python3 -I "$ROOT/scripts/ui_audit.py" clean ;;
  shots) exec python3 -I "$ROOT/scripts/ui_audit.py" shots "${2:?用法：ui-audit.sh shots <目录> empty|data [--rehearse]}" "${3:-empty}" "${@:4}" ;;
  index) exec python3 -I "$ROOT/scripts/ui_audit.py" index "${2:?用法：ui-audit.sh index <目录>}" ;;
esac
read -r -a ROUTES <<<"${BLAZAR_AUDIT_ROUTES:-/ /inbox /tasks /autopilots /runtimes /agents /skills /apps /nodes /usage /settings}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

"$DI" ui '{"action":"init"}' >/dev/null
"$DI" ui '{"action":"errors"}' >/dev/null
printf '页面\t标题\t元素\t溢出\t错误\n'
for route in "${ROUTES[@]}"; do
  "$DI" ui "{\"action\":\"navigate\",\"path\":\"$route\"}" >/dev/null
  "$DI" ui '{"action":"wait","ms":300}' >/dev/null
  "$DI" ui '{"action":"snapshot"}' >"$TMP/snapshot.json"
  "$DI" ui '{"action":"layout"}' >"$TMP/layout.json"
  "$DI" ui '{"action":"errors"}' >"$TMP/errors.json"
  python3 -I - "$route" "$TMP" <<'EOF'
import json, sys
route, tmp = sys.argv[1], sys.argv[2]
load = lambda name: json.load(open(f"{tmp}/{name}.json"))
snap, layout, errors = load("snapshot"), load("layout"), load("errors")
for part in (snap, layout, errors):
    if "error" in part:
        print(f"{route}\t驱动出错：{part['error']}")
        sys.exit(0)
title = (snap.get("headings") or ["—"])[0]
over = layout.get("overflowing", [])
errs = errors.get("errors", [])
print(f"{route}\t{title}\t{len(snap.get('items', []))}\t{len(over)}\t{len(errs)}")
for o in over:
    print(f"  溢出\t{o.get('name', '')}\t{o.get('rect')}")
for e in errs:
    print(f"  错误\t{e[:200]}")
EOF
done
