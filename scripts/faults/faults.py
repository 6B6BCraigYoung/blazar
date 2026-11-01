import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
ROOT = REPO / "target/faults"
HOME = ROOT / "home"
PROFILE = "faults"
DATA = HOME / "Library/Application Support/ai.blazar.Blazar/profiles" / PROFILE
APP = REPO / "target/dev-instance" / PROFILE / "Blazar Dev.app"
BIN = APP / "Contents/MacOS/blazar-desktop"
DI = REPO / "scripts/dev-instance.sh"
REMOTE = "demo-box"
GIT_ENV = dict(os.environ, GIT_AUTHOR_NAME="Demo User", GIT_AUTHOR_EMAIL="demo@example.invalid",
               GIT_COMMITTER_NAME="Demo User", GIT_COMMITTER_EMAIL="demo@example.invalid")


def env():
    return dict(os.environ, BLAZAR_DEV_PROFILE=PROFILE, BLAZAR_DEV_HOME=str(HOME),
                BLAZAR_DEV_PATH=f"{ROOT}/bin:/usr/bin:/bin:/usr/sbin:/sbin")


def install(src, dst):
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.unlink(missing_ok=True)
    dst.write_text(f"#!{sys.executable}\n" + src.read_text())
    dst.chmod(0o755)


def repo(path, files):
    if (path / ".git").exists():
        return
    path.mkdir(parents=True, exist_ok=True)
    for name, text in files.items():
        (path / name).parent.mkdir(parents=True, exist_ok=True)
        (path / name).write_text(text)
    for cmd in (["init", "-q", "-b", "main"], ["add", "."], ["commit", "-qm", "Initial commit"]):
        subprocess.run(["git", "-C", str(path), *cmd], check=True, env=GIT_ENV)


def prepare(_args):
    if pid():
        die("开发实例还在运行，先 faults.py stop")
    src = REPO / "target/dev-instance/dev/Blazar Dev.app"
    if not src.exists():
        die("先用 scripts/dev-instance.sh build 构建开发实例")
    shutil.rmtree(APP.parent, ignore_errors=True)
    APP.parent.mkdir(parents=True)
    subprocess.run(["ditto", str(src), str(APP)], check=True)
    shutil.rmtree(DATA, ignore_errors=True)
    agent = REPO / "scripts/faults/fake-agent.py"
    install(agent, HOME / ".local/bin/claude")
    install(agent, ROOT / "remote" / REMOTE / ".local/bin/claude")
    install(REPO / "scripts/faults/fake-ssh.py", ROOT / "bin/ssh")
    (ROOT / "offline").mkdir(parents=True, exist_ok=True)
    ssh = HOME / ".ssh"
    ssh.mkdir(mode=0o700, parents=True, exist_ok=True)
    (ssh / "config").write_text(f"Host {REMOTE}\n  HostName {REMOTE}\n  User demo\n")
    readme = "# {0}\n\nA small demo project for fault-injection tests.\n"
    repo(HOME / "projects/demo-app", {
        "README.md": readme.format("demo-app"),
        "src/app.py": "def greet(name):\n    return f\"Hello, {name}!\"\n",
    })
    repo(ROOT / "remote" / REMOTE / "projects/demo-service", {
        "README.md": readme.format("demo-service"),
        "service.py": "def health():\n    return \"ok\"\n",
    })
    print(f"已准备：{ROOT}")


def die(msg):
    print(msg, file=sys.stderr)
    sys.exit(1)


def pid():
    out = subprocess.run(["pgrep", "-f", f"^{BIN}"], capture_output=True, text=True).stdout.split()
    return int(out[0]) if out else None


def session():
    s = json.loads((DATA / "hub.session").read_text())
    return s["url"], s["token"]


def api(method, path, body=None, timeout=15):
    base, token = session()
    req = urllib.request.Request(base + path, method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        raw = r.read()
    return json.loads(raw) if raw else None


def ui(command, timeout=70):
    return api("POST", "/api/dev/ui", command, timeout=timeout)


def start(_args):
    subprocess.run([str(DI), "start"], check=True, env=env())


def stop(_args):
    subprocess.run([str(DI), "stop"], check=True, env=env())


def workspaces(_args):
    have = {(w.get("node"), w.get("path")) for w in api("GET", "/api/workspaces")}
    for node, path, name in (
        ("local", str(HOME / "projects/demo-app"), "demo-app"),
        (REMOTE, str(ROOT / "remote" / REMOTE / "projects/demo-service"), "demo-service"),
    ):
        if (node, path) not in have:
            print(api("POST", "/api/workspaces", {"node": node, "path": path, "name": name}))
    accounts = api("GET", "/api/accounts")["accounts"]
    demo = next((a for a in accounts if a.get("label") == "Demo token"), None)
    if not demo:
        demo = api("POST", "/api/accounts", {"provider": "claude", "label": "Demo token",
                                             "token": "-".join(["sk", "ant", "oat01", "demo" * 10])})
    api("PUT", "/api/accounts/mode", {"provider": "claude", "mode": demo["id"]})
    for w in api("GET", "/api/workspaces"):
        print(w["id"], w.get("node"), w.get("name"))


LOCAL_WS = "demo-app"
REMOTE_WS = "demo-service"
STEPS = 40
SHOTS = REPO / "target/faults/shots"


def ws_id(name):
    for w in api("GET", "/api/workspaces"):
        if w.get("name") == name:
            return w["id"]
    die(f"没有工作区 {name}，先 faults.py workspaces")


def step(cmd, timeout=70):
    r = ui(cmd, timeout)
    if "error" in r:
        raise RuntimeError(f"{r['error']}（{json.dumps(cmd, ensure_ascii=False)}）")
    return r


def wait_ui(text, ms=30000):
    return ui({"action": "wait", "text": text, "partial": True, "timeout_ms": ms}, timeout=ms / 1000 + 10).get("found")


def wait_ready(limit=90):
    end = time.time() + limit
    while time.time() < end:
        try:
            if ui({"action": "init", "timeout_ms": 1000}, timeout=5).get("ok"):
                return True
        except Exception:
            pass
        time.sleep(0.5)
    return False


def history_texts(wid):
    out = []
    for e in api("GET", f"/api/workspaces/{wid}/history", timeout=30):
        k = e.get("kind", {})
        out.append((k.get("type"), k.get("text") or ""))
    return out


def shot(name):
    SHOTS.mkdir(parents=True, exist_ok=True)
    try:
        r = ui({"action": "shot", "name": name})
        if r.get("path"):
            shutil.copy(r["path"], SHOTS / name)
    except Exception as e:
        print(f"截图失败：{e}")


def descendants(root):
    rows = subprocess.run(["ps", "-eo", "pid=,ppid="], capture_output=True, text=True).stdout.split("\n")
    kids, frontier = set(), {root}
    pairs = [tuple(map(int, r.split())) for r in rows if r.strip()]
    while frontier:
        nxt = {p for p, pp in pairs if pp in frontier} - kids
        kids |= nxt
        frontier = nxt
    return kids


def agents():
    out = subprocess.run(["pgrep", "-f", f"{ROOT}.*/.local/bin/claude|{HOME}/.local/bin/claude"],
                         capture_output=True, text=True).stdout.split()
    return {int(p) for p in out}


def inject(fault, hold):
    if fault == "net":
        (HOME / ".fault-net-down").touch()
        (ROOT / "offline/all").touch()
        time.sleep(hold)
    elif fault == "node":
        (ROOT / "offline" / REMOTE).touch()
        time.sleep(hold)
    elif fault == "sleep":
        app = pid()
        frozen = {app} | descendants(app) | agents()
        for p in frozen:
            subprocess.run(["kill", "-STOP", str(p)], capture_output=True)
        time.sleep(hold)
        for p in frozen:
            subprocess.run(["kill", "-CONT", str(p)], capture_output=True)
    elif fault == "hub":
        subprocess.run(["kill", "-KILL", str(pid())], capture_output=True)
        time.sleep(2)


def recover(fault):
    if fault == "net":
        (HOME / ".fault-net-down").unlink(missing_ok=True)
        (ROOT / "offline/all").unlink(missing_ok=True)
    elif fault == "node":
        (ROOT / "offline" / REMOTE).unlink(missing_ok=True)
    elif fault == "hub":
        start([])
    wait_ready()


HOLD = {"net": 25, "node": 25, "sleep": 60, "hub": 0}


def open_ws(wid):
    if ui({"action": "wait", "text": "离开这个页面？", "timeout_ms": 300}).get("found"):
        ui({"action": "click", "text": "离开"})
        time.sleep(0.5)
    r = ui({"action": "navigate", "path": f"/w/{wid}"})
    if not r.get("ok", True):
        ui({"action": "click", "text": "离开"})
        time.sleep(0.5)
        ui({"action": "navigate", "path": f"/w/{wid}"})
    step({"action": "wait", "selector": ".cc-box .prompt", "timeout_ms": 20000})


def chat(fault, tag, typed=False):
    wid = ws_id(REMOTE_WS if fault == "node" else LOCAL_WS)
    open_ws(wid)
    task, follow = f"Task {tag}", f"Follow-up {tag}"
    step({"action": "type", "selector": ".cc-box .prompt", "value": task})
    step({"action": "key", "selector": ".cc-box .prompt", "key": "Enter", "code": "Enter"})
    if not wait_ui(f"Step 5/{STEPS} for “{task}”", 60000):
        raise RuntimeError("第 5 步没有出现，基线就不通")
    shot(f"{tag}-before.png")
    if fault == "hub":
        step({"action": "type", "selector": ".cc-box .prompt", "value": follow})
        time.sleep(0.5)
        inject(fault, 0)
        recover(fault)
        open_ws(wid)
        shot(f"{tag}-after-restart.png")
        step({"action": "key", "selector": ".cc-box .prompt", "key": "Enter", "code": "Enter"})
    elif typed:
        import threading
        t = threading.Thread(target=inject, args=(fault, HOLD[fault]))
        t.start()
        time.sleep(3)
        step({"action": "type", "selector": ".cc-box .prompt", "value": follow})
        step({"action": "key", "selector": ".cc-box .prompt", "key": "Enter", "code": "Enter"})
        shot(f"{tag}-during.png")
        t.join()
        recover(fault)
    else:
        import threading
        t = threading.Thread(target=inject, args=(fault, HOLD[fault]))
        t.start()
        if fault != "sleep":
            time.sleep(8)
            shot(f"{tag}-during.png")
        t.join()
        recover(fault)
        wait_ui(f"Done: {task}", 150000)
        step({"action": "type", "selector": ".cc-box .prompt", "value": follow})
        step({"action": "key", "selector": ".cc-box .prompt", "key": "Enter", "code": "Enter"})
    done = wait_ui(f"Done: {follow}", 150000)
    shot(f"{tag}-after.png")
    texts = history_texts(wid)
    counts = {i: sum(t.count(f"Step {i}/{STEPS} for “{task}”.") for _, t in texts if _ != "user_message")
              for i in range(1, STEPS + 1)}
    missing = [i for i, c in counts.items() if c == 0]
    doubled = [i for i, c in counts.items() if c > 1]
    sent = sum(1 for k, t in texts if k == "user_message" and t == follow)
    page = step({"action": "text", "selector": ".cc-log"})["texts"]
    shown = page[0]["text"] if page else ""
    shown_doubled = [i for i in range(1, STEPS + 1) if shown.count(f"Step {i}/{STEPS} for “{task}”.") > 1]
    errors = [t for k, t in texts if k in ("error", "run_failed", "failure") or "Error" in t][:5]
    return {"task_done": any(f"Done: {task}" in t for _, t in texts), "missing_steps": missing,
            "doubled_steps": doubled, "follow_up_sent": sent, "follow_up_answered": bool(done),
            "page_doubled": shown_doubled, "errors": errors}


def edit(fault, tag):
    wid = ws_id(REMOTE_WS if fault == "node" else LOCAL_WS)
    open_ws(wid)
    fname = "README.md"
    draft, line = f"Draft {tag}", f"Unsaved edit {tag}. "
    step({"action": "type", "selector": ".cc-box .prompt", "value": draft})
    if not ui({"action": "wait", "selector": ".editor-host textarea", "timeout_ms": 1500}).get("found"):
        step({"action": "click", "text": "文件"})
    step({"action": "click", "text": fname, "selector": ".ed-files button"})
    time.sleep(1.5)
    step({"action": "type", "selector": ".editor-host textarea", "value": line})
    time.sleep(2.5)
    shot(f"{tag}-before.png")
    before = step({"action": "text", "selector": ".cc-box .prompt"})["texts"][0].get("value")
    if fault == "hub":
        inject(fault, 0)
        recover(fault)
        open_ws(wid)
    else:
        inject(fault, HOLD[fault])
        recover(fault)
    time.sleep(2)
    shot(f"{tag}-after.png")
    editor = step({"action": "text", "selector": ".insp-body"})["texts"]
    in_editor = bool(editor) and line.strip() in editor[0]["text"]
    offered = bool(ui({"action": "wait", "selector": ".ed-draft", "timeout_ms": 1500}).get("found"))
    box = step({"action": "text", "selector": ".cc-box .prompt"})["texts"]
    draft_kept = bool(box) and box[0].get("value", "") == draft
    return {"composer_before": before, "composer_after": box[0].get("value") if box else None,
            "edit_in_editor": in_editor, "restore_offered": offered, "composer_draft_seen": draft_kept}


def run(args):
    fault, state = args[0], args[1]
    tag = f"{fault}-{state}-{time.strftime('%H%M%S')}"
    result = {"fault": fault, "state": state, "tag": tag}
    try:
        result.update(edit(fault, tag) if state == "edit" else chat(fault, tag, typed=state == "typed"))
    except Exception as e:
        result["error"] = str(e)
    out = REPO / "target/faults/results.jsonl"
    with out.open("a") as f:
        f.write(json.dumps(result, ensure_ascii=False) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=1))


def main():
    cmd, args = (sys.argv[1] if len(sys.argv) > 1 else ""), sys.argv[2:]
    if cmd == "ui":
        print(json.dumps(ui(json.loads(args[0])), ensure_ascii=False))
    elif cmd == "api":
        body = json.loads(args[2]) if len(args) > 2 else None
        print(json.dumps(api(args[0], args[1], body), ensure_ascii=False, indent=1))
    elif cmd in ("prepare", "start", "stop", "workspaces", "run"):
        globals()[cmd](args)
    elif cmd == "pid":
        print(pid() or "")
    else:
        die("用法：faults.py prepare | start | stop | workspaces | pid | ui '<JSON>' | api METHOD PATH [JSON]")


main()
