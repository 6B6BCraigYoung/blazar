import json
import os
import sys
import threading
import time
import uuid
from pathlib import Path

HOME = Path(os.environ.get("HOME", "/tmp"))
NET_DOWN = HOME / ".fault-net-down"
STEPS_FILE = HOME / ".fake-agent-steps"
STEPS = int(STEPS_FILE.read_text().strip() if STEPS_FILE.exists() else os.environ.get("FAKE_AGENT_STEPS", "40"))
PACE = float(os.environ.get("FAKE_AGENT_PACE", "0.5"))
LOG = HOME / "fake-agent.log"
STATE = HOME / "fake-agent-state.json"

out_lock = threading.Lock()
turns = []
turn_ready = threading.Condition()
interrupted = threading.Event()


def emit(obj):
    with out_lock:
        sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
        sys.stdout.flush()


def note(text):
    with LOG.open("a") as f:
        f.write(f"{time.strftime('%H:%M:%S')} {os.getpid()} {text}\n")


def arg(name):
    if name in sys.argv:
        i = sys.argv.index(name)
        return sys.argv[i + 1] if i + 1 < len(sys.argv) else None
    return None


def text_of(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    return " ".join(b.get("text", "") for b in content or [] if b.get("type") == "text")


def reader():
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        kind = msg.get("type")
        if kind == "user":
            emit({**msg, "isReplay": True})
            with turn_ready:
                turns.append(text_of(msg.get("message", {})))
                turn_ready.notify()
        elif kind == "control_request":
            sub = msg.get("request", {}).get("subtype")
            if sub == "interrupt":
                interrupted.set()
            emit({"type": "control_response",
                  "response": {"subtype": "success", "request_id": msg.get("request_id"), "response": {}}})
    with turn_ready:
        turns.append(None)
        turn_ready.notify()


def wait_for_network(sid):
    attempt = 0
    while NET_DOWN.exists():
        attempt += 1
        emit({"type": "system", "subtype": "api_retry", "attempt": attempt, "max_retries": 10,
              "retry_delay_ms": 2000, "error_status": None, "error": "connection_error", "session_id": sid})
        note(f"network down, retry {attempt}")
        if attempt >= 10:
            return False
        time.sleep(2)
    return True


def progress(sid, value=None):
    data = json.loads(STATE.read_text()) if STATE.exists() else {}
    if value is not None:
        data[sid] = value
        STATE.write_text(json.dumps(data))
    return data.get(sid)


def answer(sid, prompt):
    note(f"turn start: {prompt!r}")
    start = time.time()
    first = 1
    last = progress(sid)
    if prompt.strip().lower().startswith("continue") and last and last["step"] < STEPS:
        prompt, first = last["prompt"], last["step"] + 1
        note(f"continuing {prompt!r} from step {first}")
    for i in range(first, STEPS + 1):
        if interrupted.is_set():
            break
        if not wait_for_network(sid):
            emit({"type": "assistant", "session_id": sid, "error": "unknown",
                  "message": {"role": "assistant", "content": [
                      {"type": "text", "text": "API Error: Connection error."}]}})
            emit({"type": "result", "subtype": "success", "is_error": True, "terminal_reason": "api_error",
                  "result": "API Error: Connection error.", "session_id": sid,
                  "duration_ms": int((time.time() - start) * 1000), "num_turns": 1, "total_cost_usd": 0,
                  "usage": {"input_tokens": 1, "output_tokens": 1}})
            note("gave up after retries")
            return
        emit({"type": "assistant", "session_id": sid, "message": {
            "id": f"msg_{uuid.uuid4().hex[:12]}", "role": "assistant", "model": "claude-demo",
            "content": [{"type": "text", "text": f"Step {i}/{STEPS} for “{prompt}”."}],
            "usage": {"input_tokens": 1, "output_tokens": 8}}})
        progress(sid, {"prompt": prompt, "step": i})
        note(f"step {i}")
        time.sleep(PACE)
    stopped = interrupted.is_set()
    interrupted.clear()
    emit({"type": "result", "subtype": "success", "is_error": False,
          "result": "Interrupted." if stopped else f"Done: {prompt}", "session_id": sid,
          "duration_ms": int((time.time() - start) * 1000), "num_turns": 1, "total_cost_usd": 0,
          "usage": {"input_tokens": 1, "output_tokens": 1}})
    note("turn end" + (" (interrupted)" if stopped else ""))


def main():
    if "--version" in sys.argv:
        print("2.1.0 (Claude Code)")
        return
    if sys.argv[1:3] == ["auth", "status"]:
        print(json.dumps({"loggedIn": True, "authMethod": "claude.ai", "email": "demo@example.invalid",
                          "subscriptionType": "pro"}))
        return
    sid = arg("--resume") or arg("--session-id") or str(uuid.uuid4())
    note(f"start {' '.join(a for a in sys.argv[1:] if not a.startswith('{'))[:200]}")
    emit({"type": "system", "subtype": "init", "session_id": sid, "model": "claude-demo",
          "cwd": os.getcwd(), "tools": ["Read", "Edit", "Bash"], "permissionMode": "default"})
    if "-p" in sys.argv and "--input-format" not in sys.argv:
        answer(sid, sys.argv[-1])
        return
    threading.Thread(target=reader, daemon=True).start()
    while True:
        with turn_ready:
            while not turns:
                turn_ready.wait()
            prompt = turns.pop(0)
        if prompt is None:
            note("stdin closed")
            return
        answer(sid, prompt)


main()
