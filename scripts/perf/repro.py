import json
import sqlite3
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
HOME = ROOT / "target/perf/home"
DATA = HOME / "Library/Application Support/ai.blazar.Blazar/profiles/perf"


def biggest_local_workspace():
    c = sqlite3.connect(f"file:{DATA / 'blazar.sqlite'}?mode=ro", uri=True)
    row = c.execute(
        "SELECT w.id FROM workspaces w JOIN nodes d ON d.id = w.node_id"
        " WHERE w.kind = 'project' AND d.transport = 'local'"
        " ORDER BY (SELECT COUNT(*) FROM sessions s WHERE s.workspace_id = w.id) DESC LIMIT 1"
    ).fetchone()
    c.close()
    return row[0]


WS = biggest_local_workspace()


def session():
    s = json.loads((DATA / "hub.session").read_text())
    return s["url"], s["token"]


def call(method, path, body=None, timeout=70):
    url, token = session()
    req = urllib.request.Request(
        f"{url}{path}",
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        return {"http_error": e.code, "body": e.read().decode()[:300]}


def ui(cmd):
    return call("POST", "/api/dev/ui", cmd)


def db():
    return sqlite3.connect(f"file:{DATA / 'blazar.sqlite'}?mode=ro", uri=True)


def wait_idle(limit=120):
    end = time.time() + limit
    while time.time() < end:
        if all(w["activity"] != "running" for w in call("GET", "/api/workspaces") if w["id"] == WS):
            return True
        time.sleep(1)
    return False


def start_slow():
    wait_idle()
    r = call("POST", f"/api/workspaces/{WS}/prompt", {"text": "slow turn", "agent": "claude", "permission_mode": "auto"})
    time.sleep(3)
    return r["session_id"], r["thread_id"]


def ref_of(label):
    snap = ui({"action": "snapshot"})
    for it in snap.get("items", []):
        name = it.get("name") or it.get("label") or ""
        if name == label or name.startswith(label):
            return it["ref"]
    return None


def page_text():
    return ui({"action": "text", "selector": "body"})["texts"][0]["text"]


def composer_send(text):
    ref = ref_of("Message")
    r = ui({"action": "type", "ref": ref, "value": text, "submit": True, "settle_ms": 600})
    return r


def user_messages(thread):
    c = db()
    rows = c.execute(
        "SELECT s.id, s.status, e.payload FROM sessions s JOIN events e ON e.session_id = s.id"
        " WHERE COALESCE(s.thread_id, s.id) = ? AND e.payload LIKE '{\"type\":\"user_message\"%' ORDER BY s.created_at, e.seq",
        (thread,),
    ).fetchall()
    c.close()
    return [(sid[:13], st, json.loads(p).get("text", "")[:40]) for sid, st, p in rows]


def r20():
    c = sqlite3.connect(DATA / "blazar.sqlite")
    c.execute("UPDATE accounts SET status = 'ok' WHERE id LIKE 'perf-%'")
    c.commit()
    c.close()
    ui({"action": "navigate", "path": "/nodes", "settle_ms": 500})
    ui({"action": "navigate", "path": f"/w/{WS}", "settle_ms": 2000})
    ui({"action": "click", "ref": ref_of("Agent and runtime"), "settle_ms": 600})
    snap = ui({"action": "snapshot"})["items"]
    r = next(i["ref"] for i in snap if (i.get("name") or "").startswith("perf-codex"))
    ui({"action": "click", "ref": r, "settle_ms": 800})
    ui({"action": "click", "ref": ref_of("Agent and runtime"), "settle_ms": 600})
    snap = ui({"action": "snapshot"})["items"]
    r = next(i["ref"] for i in snap if (i.get("name") or "") == "Claude Code")
    ui({"action": "click", "ref": r, "settle_ms": 800})
    composer_send("hello from r20")
    time.sleep(3)
    text = page_text()
    wait_idle()
    return {"failure_lines": [l for l in text.splitlines() if "ccount" in l or "ailed" in l][:4],
            "sent": user_messages_all("hello from r20")}


def r2():
    sid, thread = start_slow()
    ui({"action": "navigate", "path": f"/w/{WS}?thread={thread}", "settle_ms": 1500})
    composer_send("/new")
    composer_send("hello fresh r2")
    time.sleep(3)
    queue = call("GET", f"/api/workspaces/{WS}/queue")
    msgs = user_messages(thread)
    wait_idle(150)
    time.sleep(3)
    after = call("GET", "/api/workspaces")
    return {"old_thread_user_messages": msgs, "queue": [q.get("thread_id") for q in queue] if isinstance(queue, list) else queue,
            "fresh_thread": [s for s in user_messages_all("hello fresh r2")]}


def user_messages_all(text):
    c = db()
    rows = c.execute(
        "SELECT COALESCE(s.thread_id, s.id), s.status FROM sessions s JOIN events e ON e.session_id = s.id"
        " WHERE e.payload LIKE ? ORDER BY s.created_at",
        (f'%"text":"{text}"%',),
    ).fetchall()
    c.close()
    return [(t[:13], st) for t, st in rows]


def r1():
    sid, thread = start_slow()
    ui({"action": "navigate", "path": f"/w/{WS}?thread={thread}", "settle_ms": 1500})
    call("POST", f"/api/sessions/{sid}/interrupt", {})
    composer_send("after esc r1")
    time.sleep(1)
    queue_now = call("GET", f"/api/workspaces/{WS}/queue")
    wait_idle(90)
    time.sleep(4)
    wait_idle(90)
    time.sleep(2)
    queue_after = call("GET", f"/api/workspaces/{WS}/queue")
    return {"thread_sessions": user_messages(thread),
            "queue_right_after": [(q.get("thread_id", "")[:13], q.get("held")) for q in queue_now] if isinstance(queue_now, list) else queue_now,
            "queue_after": [(q.get("thread_id", "")[:13], q.get("held")) for q in queue_after] if isinstance(queue_after, list) else queue_after}


def r3():
    wait_idle()
    c = db()
    row = c.execute(
        "SELECT s.id, COALESCE(s.thread_id, s.id) FROM sessions s WHERE s.workspace_id = ? AND s.status = 'done'"
        " AND s.rewound_at IS NULL"
        " AND EXISTS (SELECT 1 FROM sessions n WHERE COALESCE(n.thread_id, n.id) = COALESCE(s.thread_id, s.id) AND n.created_at > s.created_at)"
        " ORDER BY s.created_at DESC LIMIT 1",
        (WS,),
    ).fetchone()
    c.close()
    sid, thread = row
    r = call("POST", f"/api/workspaces/{WS}/retry", {"session_id": sid, "options": {"agent": "no-such-agent"}})
    time.sleep(1)
    c = db()
    rewound = c.execute("SELECT COUNT(*) FROM sessions WHERE COALESCE(thread_id, id) = ? AND rewound_at IS NOT NULL", (thread,)).fetchone()[0]
    c.close()
    return {"retry_response": r, "rewound_sessions_in_thread": rewound}


def r12():
    ui({"action": "navigate", "path": "/", "settle_ms": 2500})
    before = ref_of("Message")
    ui({"action": "click", "text": "新对话", "partial": True, "settle_ms": 2500})
    probe = ui({"action": "type", "ref": before, "value": "still mounted?", "settle_ms": 100})
    ui({"action": "type", "selector": "textarea", "value": "", "settle_ms": 100})
    return {"same_composer_after_new": "error" not in probe, "probe": probe}


if __name__ == "__main__":
    out = {}
    for name in sys.argv[1:]:
        out[name] = globals()[name]()
        print(name, json.dumps(out[name], ensure_ascii=False), flush=True)
