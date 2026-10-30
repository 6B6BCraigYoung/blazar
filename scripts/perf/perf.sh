#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export BLAZAR_DEV_PROFILE="${BLAZAR_DEV_PROFILE:-perf}"
export BLAZAR_DEV_HOME="${BLAZAR_DEV_HOME:-$ROOT/target/perf/home}"
DI="$ROOT/scripts/dev-instance.sh"
PY=(python3 -I "$ROOT/scripts/perf/perf.py")

case "${1:-}" in
  prepare) exec "${PY[@]}" prepare "${@:2}" ;;
  start)
    BLAZAR_WINDOW_SIZE="${BLAZAR_WINDOW_SIZE:-1440x900}" "$DI" start
    exec "${PY[@]}" pids ;;
  stop) exec "$DI" stop ;;
  idle) exec "${PY[@]}" idle "${@:2}" ;;
  ops) exec "${PY[@]}" ops "${@:2}" ;;
  memory) exec "${PY[@]}" memory "${@:2}" ;;
  launch) exec "${PY[@]}" launch "${@:2}" ;;
  compare) exec "${PY[@]}" compare "${@:2}" ;;
  *)
    echo "用法：perf.sh prepare | start | stop | idle [秒数] [--label 名字] | ops [--label 名字] | memory [--cycles N] [--routes a,b] | launch [--reps N] | compare <结果1> <结果2>" >&2
    exit 2 ;;
esac
