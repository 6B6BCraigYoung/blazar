import json
import sys
import threading
import time
from pathlib import Path

CONF = json.loads((Path(__file__).resolve().parent / "fake-claude.json").read_text())


def drain():
    for _ in sys.stdin:
        pass


def main():
    if "--version" in sys.argv:
        print("2.1.0 (Claude Code)")
        return
    threading.Thread(target=drain, daemon=True).start()
    lines = Path(CONF["record"]).read_text().splitlines()
    rate = float(CONF.get("rate", 10))
    limit = int(CONF.get("lines", 300))
    sent = 0
    for line in lines:
        if sent >= limit:
            break
        try:
            kind = json.loads(line).get("type")
        except ValueError:
            continue
        if kind in ("control_request", "control_response") or (kind == "result" and not CONF.get("keep_results")):
            continue
        sys.stdout.write(line + "\n")
        sys.stdout.flush()
        sent += 1
        time.sleep(1 / rate)
    result = {"type": "result", "subtype": "success", "is_error": False, "result": "done",
              "duration_ms": int(sent / rate * 1000), "num_turns": 1, "total_cost_usd": 0,
              "usage": {"input_tokens": 1, "output_tokens": 1}}
    sys.stdout.write(json.dumps(result) + "\n")
    sys.stdout.flush()


main()
