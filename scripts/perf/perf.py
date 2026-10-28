import json
import os
import re
import sqlite3
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
PROFILE = os.environ.get("BLAZAR_DEV_PROFILE", "perf")
APP_DATA = "Library/Application Support/ai.blazar.Blazar/profiles"
HOME = Path(os.environ.get("BLAZAR_DEV_HOME") or ROOT / "target/perf/home")
DATA = HOME / APP_DATA / PROFILE
SOURCE = Path.home() / APP_DATA / os.environ.get("BLAZAR_PERF_SOURCE", "default")
RECORD = ROOT / "target/perf/stream.jsonl"
BIN = ROOT / "target/dev-instance" / PROFILE / "Blazar Dev.app/Contents/MacOS/blazar-desktop"
OUT = ROOT / "target/perf"
PIDS = OUT / f"{PROFILE}.pids.json"
WEBKIT = ("com.apple.WebKit.WebContent", "com.apple.WebKit.GPU", "com.apple.WebKit.Networking")

SANITIZE = [
    "UPDATE sessions SET status = 'interrupted', run_dir = NULL WHERE status = 'running'",
    "UPDATE workspaces SET activity = 'idle' WHERE activity IN ('running', 'awaiting_approval')",
    "UPDATE jobs SET status = 'done' WHERE status = 'running'",
    "UPDATE accounts SET disabled = 1",
    "UPDATE bots SET archived_at = COALESCE(archived_at, datetime('now'))",
    "UPDATE kv_settings SET value = '\"\"' WHERE key LIKE 'accounts.mode.%'",
    "DELETE FROM kv_settings WHERE key LIKE 'accounts.global%'",
    "DELETE FROM fleet_grants",
    "DELETE FROM queued_messages",
    "DELETE FROM mesh_invites",
    "DELETE FROM kv_settings WHERE key LIKE 'office.lark%' OR key LIKE 'proxy.tunnel_port.%'"
    " OR key LIKE 'fleet.tunnel_port.%' OR key = 'sims.launches'",
]


def die(msg):
    print(msg, file=sys.stderr)
    sys.exit(1)


def desktop_pid():
    out = subprocess.run(["pgrep", "-f", f"^{BIN}"], capture_output=True, text=True).stdout.split()
    return int(out[0]) if out else None


def prepare(args):
    if desktop_pid():
        die("开发实例还在运行，先 perf.sh stop")
    src = SOURCE / "blazar.sqlite"
    if not src.exists():
        die(f"找不到 {src}")
    DATA.mkdir(parents=True, exist_ok=True)
    os.chmod(DATA, 0o700)
    dst = DATA / "blazar.sqlite"
    for suffix in ("", "-wal", "-shm"):
        Path(f"{dst}{suffix}").unlink(missing_ok=True)
    reader = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    writer = sqlite3.connect(dst)
    reader.backup(writer)
    reader.close()
    for sql in SANITIZE:
        writer.execute(sql)
    relocate_local_workspaces(writer)
    writer.commit()
    writer.execute("VACUUM")
    writer.close()
    os.chmod(dst, 0o600)
    install_fake_claude(flag(args, "--record"))
    print(f"已从 {SOURCE.name} 复制并清理：{dst}（{dst.stat().st_size // (1 << 20)} MB）")


def relocate_local_workspaces(db):
    rows = db.execute(
        "SELECT w.id FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE n.transport = 'local'"
    ).fetchall()
    for (wid,) in rows:
        path = HOME / "ws" / wid
        if not (path / ".git").exists():
            path.mkdir(parents=True, exist_ok=True)
            (path / "README.md").write_text("perf\n")
            (path / ".gitignore").write_text("")
            env = dict(os.environ, GIT_AUTHOR_NAME="perf", GIT_AUTHOR_EMAIL="perf@example.invalid",
                       GIT_COMMITTER_NAME="perf", GIT_COMMITTER_EMAIL="perf@example.invalid")
            for cmd in (["init", "-q", "-b", "main"], ["add", "."], ["commit", "-qm", "init"]):
                subprocess.run(["git", "-C", str(path), *cmd], check=True, env=env)
        db.execute("UPDATE workspaces SET path = ?, repo_root = NULL WHERE id = ?", (str(path), wid))


def install_fake_claude(record):
    src = Path(record) if record else newest_record()
    if src and src.exists() and src != RECORD:
        RECORD.write_bytes(src.read_bytes())
        os.chmod(RECORD, 0o600)
    if not RECORD.exists():
        die("没有可回放的会话输出：用 --record 指定一份 out.jsonl")
    bin_dir = HOME / ".local/bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    fake = bin_dir / "claude"
    fake.unlink(missing_ok=True)
    fake.write_text("#!/usr/bin/env python3\n" + (ROOT / "scripts/perf/fake-claude.py").read_text())
    fake.chmod(0o755)
    (bin_dir / "fake-claude.json").write_text(json.dumps({"record": str(RECORD), "rate": 10, "lines": 300}))


def newest_record():
    runs = Path.home() / ".blazar/runs"
    files = [p for p in runs.glob("*/out.jsonl") if p.stat().st_size > 100_000]
    return max(files, key=lambda p: p.stat().st_size) if files else None


def ps_rows():
    out = subprocess.run(["ps", "-axo", "pid=,ppid=,etime=,comm="], capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            rows.append((int(parts[0]), int(parts[1]), etime_secs(parts[2]), parts[3]))
    return rows


def etime_secs(s):
    days = 0
    if "-" in s:
        d, s = s.split("-", 1)
        days = int(d)
    nums = [int(x) for x in s.split(":")]
    while len(nums) < 3:
        nums.insert(0, 0)
    return days * 86400 + nums[0] * 3600 + nums[1] * 60 + nums[2]


def find_pids():
    pid = desktop_pid()
    if not pid:
        die("开发实例没有运行，先 perf.sh start")
    rows = ps_rows()
    age = next(r[2] for r in rows if r[0] == pid)
    helpers = {}
    for p, ppid, et, comm in rows:
        name = Path(comm).name
        if ppid == 1 and name in WEBKIT and 0 <= age - et <= 20:
            helpers[name.rsplit(".", 1)[-1].lower()] = p
    return {"desktop": pid, **helpers}


def pids(_args):
    found = find_pids()
    OUT.mkdir(parents=True, exist_ok=True)
    PIDS.write_text(json.dumps(found))
    print(" ".join(f"{k}={v}" for k, v in found.items()))


def session():
    raw = (DATA / "hub.session").read_text()
    s = json.loads(raw)
    return s["url"], s["token"]


def ui(command, timeout=70):
    url, token = session()
    req = urllib.request.Request(
        f"{url}/api/dev/ui",
        data=json.dumps(command).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


def api(path, body=None):
    url, token = session()
    req = urllib.request.Request(
        f"{url}{path}",
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        method="GET" if body is None else "POST",
    )
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.loads(r.read() or b"{}")


def cpu_secs(s):
    parts = s.split(":")
    try:
        nums = [float(x) for x in parts]
    except ValueError:
        return None
    total = 0.0
    for n in nums:
        total = total * 60 + n
    return total


def mem_mb(s):
    m = re.match(r"([\d.]+)([BKMG])", s)
    if not m:
        return None
    return float(m.group(1)) * {"B": 1 / (1 << 20), "K": 1 / 1024, "M": 1, "G": 1024}[m.group(2)]


def footprint_mb(pid):
    out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    m = re.search(r"Footprint:\s*([\d.]+)\s*([KMG]B)", out)
    if not m:
        return None
    return round(float(m.group(1)) * {"KB": 1 / 1024, "MB": 1, "GB": 1024}[m.group(2)], 1)


def descendants(rows, root):
    kids, out = {}, set()
    for p, ppid, _, _ in rows:
        kids.setdefault(ppid, []).append(p)
    stack = [root]
    while stack:
        for k in kids.get(stack.pop(), []):
            if k not in out:
                out.add(k)
                stack.append(k)
    return out


def watch_children(root, stop, seen):
    while not stop.is_set():
        rows = ps_rows()
        for p in descendants(rows, root):
            seen.add(p)
        stop.wait(1)


def idle(args):
    secs = int(args[0]) if args and args[0].isdigit() else 300
    label = flag(args, "--label") or "idle"
    route = flag(args, "--route") or "/"
    interval = 5
    found = find_pids()
    ui({"action": "navigate", "path": route})
    warm = int(flag(args, "--warm") or 20)
    time.sleep(warm)
    mem0 = {k: footprint_mb(v) for k, v in found.items()}
    stop, seen = threading.Event(), set()
    watcher = threading.Thread(target=watch_children, args=(found["desktop"], stop, seen), daemon=True)
    watcher.start()
    n = secs // interval + 1
    proc = subprocess.Popen(
        ["top", "-l", str(n), "-s", str(interval), "-stats", "pid,ppid,time,idlew,power,mem,command"],
        stdout=subprocess.PIPE, text=True,
    )
    samples = []
    current = None
    for line in proc.stdout:
        if line.startswith("PID"):
            current = {}
            samples.append((time.time(), current))
            continue
        if current is None:
            continue
        parts = line.split(None, 6)
        if len(parts) < 7 or not parts[0].isdigit():
            continue
        current[int(parts[0])] = {
            "ppid": int(parts[1]), "time": cpu_secs(parts[2]), "idlew": int(re.sub(r"\D", "", parts[3]) or 0),
            "power": float(parts[4]) if re.match(r"^[\d.]+$", parts[4]) else 0.0, "mem": mem_mb(parts[5]),
        }
    proc.wait()
    stop.set()
    watcher.join()
    mem1 = {k: footprint_mb(v) for k, v in found.items()}
    spawned = len(seen)
    groups = {k: [v] for k, v in found.items()}
    first_t, first = samples[1] if len(samples) > 1 else samples[0]
    last_t, last = samples[-1]
    elapsed = last_t - first_t
    kids_ever = set()
    for _, s in samples:
        kids_ever |= descendants([(p, d["ppid"], 0, "") for p, d in s.items()], found["desktop"])
    groups["children"] = sorted(kids_ever)
    result = {"label": label, "route": route, "seconds": round(elapsed), "spawned_children": spawned,
              "at": datetime.now(timezone.utc).isoformat(timespec="seconds"), "groups": {}}
    for name, members in groups.items():
        cpu = wake = 0.0
        power = []
        for p in members:
            a, b = first.get(p), last.get(p)
            if a and b and a["time"] is not None and b["time"] is not None:
                cpu += b["time"] - a["time"]
                wake += max(0, b["idlew"] - a["idlew"])
            elif b and b["time"] is not None:
                cpu += b["time"]
        for _, s in samples[1:]:
            power.append(sum(s[p]["power"] for p in members if p in s))
        g = {
            "cpu_pct": round(cpu / elapsed * 100, 2) if elapsed else None,
            "wakeups_per_s": round(wake / elapsed, 1) if elapsed else None,
            "power": round(statistics.mean(power), 2) if power else None,
        }
        if name in mem0:
            g["footprint_mb"] = [mem0[name], mem1[name]]
        result["groups"][name] = g
    save(result)
    print_idle(result)


def flag(args, name):
    if name in args:
        i = args.index(name)
        if i + 1 < len(args):
            return args[i + 1]
    return None


def save(result):
    OUT.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now().strftime("%m%d-%H%M%S")
    kind = "ops" if "ops" in result else "idle"
    path = OUT / f"{kind}-{result['label']}-{stamp}.json"
    path.write_text(json.dumps(result, ensure_ascii=False, indent=1))
    print(path)


def print_idle(r):
    print(f"空闲 {r['seconds']}s @ {r['route']}，期间新起子进程 {r['spawned_children']} 个")
    print(f"{'进程':<12}{'CPU%':>8}{'唤醒/s':>10}{'能耗':>8}{'内存MB(前→后)':>20}")
    for name, g in r["groups"].items():
        fp = g.get("footprint_mb")
        mem = f"{fp[0]}→{fp[1]}" if fp else ""
        print(f"{name:<12}{g['cpu_pct']:>8}{g['wakeups_per_s']:>10}{g['power']:>8}{mem:>20}")


def biggest_workspace(local_only=True):
    db = sqlite3.connect(f"file:{DATA / 'blazar.sqlite'}?mode=ro", uri=True)
    rows = db.execute(
        "SELECT w.id, w.name, (SELECT COUNT(*) FROM events e JOIN sessions s ON s.id = e.session_id"
        " WHERE s.workspace_id = w.id) AS n FROM workspaces w JOIN nodes d ON d.id = w.node_id"
        " WHERE w.kind = 'project' AND (? = 0 OR d.transport = 'local') ORDER BY n DESC LIMIT 2",
        (1 if local_only else 0,),
    ).fetchall()
    db.close()
    return rows


def measure(cmd, until=None, quiet=400, timeout=15000):
    m = {"action": "measure", "then": cmd, "quiet_ms": quiet, "timeout_ms": timeout}
    if until:
        m["until"] = until
    r = ui(m)
    if "error" in r:
        die(f"驱动出错：{r['error']}（{json.dumps(cmd, ensure_ascii=False)}）")
    return r


def median_of(runs, key):
    vals = [r[key] for r in runs if isinstance(r.get(key), (int, float))]
    return round(statistics.median(vals), 1) if vals else None


def ops(args):
    label = flag(args, "--label") or "ops"
    reps = int(flag(args, "--reps") or 3)
    only = set((flag(args, "--only") or "").split(",")) - {""}
    want = lambda name: not only or name in only
    ws = biggest_workspace()
    if len(ws) < 2:
        die("副本里本机工作区不到两个，没法测切换")
    (a, a_name, a_n), (b, b_name, b_n) = ws
    ui({"action": "init"})
    ui({"action": "errors"})
    results = {}

    def record(name, runs):
        results[name] = {
            "settle_ms": median_of(runs, "settle_ms"),
            "ready_ms": median_of(runs, "ready_ms"),
            "frames": median_of(runs, "frames"),
            "dropped": median_of(runs, "dropped"),
            "long_frames": median_of(runs, "long_frames"),
            "max_gap": median_of(runs, "max_gap"),
            "mutations": median_of(runs, "mutations"),
        }

    if want("open_workspace"):
        runs = []
        for _ in range(reps):
            ui({"action": "navigate", "path": "/", "settle_ms": 1500})
            runs.append(measure({"action": "navigate", "path": f"/w/{a}"}, until=".chat-body"))
        record("open_workspace", runs)

    if want("switch_workspace"):
        runs = []
        for i in range(reps * 2):
            target = b if i % 2 == 0 else a
            runs.append(measure({"action": "navigate", "path": f"/w/{target}"}, until=".chat-body"))
        record("switch_workspace", runs)

    if want("scroll_conversation"):
        ui({"action": "navigate", "path": f"/w/{a}", "settle_ms": 2000})
        scroller = scroll_selector()
        runs = []
        for _ in range(reps):
            up = ui({"action": "sweep", "selector": scroller, "to": "start", "ms": 2500})
            down = ui({"action": "sweep", "selector": scroller, "to": "end", "ms": 2500})
            for r in (up, down):
                if "error" in r:
                    die(f"驱动出错：{r['error']}")
            runs.extend([up, down])
        record("scroll_conversation", runs)

    if want("open_console"):
        runs = []
        for _ in range(reps):
            ui({"action": "navigate", "path": f"/w/{a}", "settle_ms": 500})
            runs.append(measure({"action": "navigate", "path": "/"}, until="main"))
        record("open_console", runs)

    if want("console_idle_10s"):
        record("console_idle_10s", [ui({"action": "frames", "ms": 10000})])

    for name, route in (("stream_workspace", f"/w/{a}"), ("stream_console", "/")):
        if want(name):
            runs = [stream(a, route) for _ in range(max(1, reps // 2))]
            results[name] = {k: median_of(runs, k) for k in runs[0]}

    errors = ui({"action": "errors"}).get("errors", [])
    result = {"label": label, "workspaces": [[a_name, a_n], [b_name, b_n]], "ops": results, "errors": errors[:10],
              "at": datetime.now(timezone.utc).isoformat(timespec="seconds")}
    save(result)
    print_ops(result)


def cpu_times(found):
    out = subprocess.run(["ps", "-o", "pid=,time=", "-p", ",".join(str(p) for p in found.values())],
                         capture_output=True, text=True).stdout
    times = {int(p): cpu_secs(t) for p, t in (line.split() for line in out.splitlines())}
    return {k: times.get(v) for k, v in found.items()}


def wait_idle(ws, limit=90):
    end = time.time() + limit
    while time.time() < end:
        if all(w.get("activity") != "running" for w in api("/api/workspaces") if w.get("id") == ws):
            return
        time.sleep(1)


def stream(ws, route, secs=20):
    found = find_pids()
    wait_idle(ws)
    ui({"action": "navigate", "path": route, "settle_ms": 1500})
    api(f"/api/workspaces/{ws}/prompt", {"text": "perf"})
    time.sleep(3)
    c0, t0 = cpu_times(found), time.time()
    r = ui({"action": "frames", "ms": secs * 1000})
    c1, t1 = cpu_times(found), time.time()
    wait_idle(ws)
    out = {k: r.get(k) for k in ("frames", "dropped", "long_frames", "max_gap", "mutations")}
    for k in ("desktop", "webcontent", "gpu"):
        if c0.get(k) is not None and c1.get(k) is not None:
            out[f"cpu_{k}"] = round((c1[k] - c0[k]) / (t1 - t0) * 100, 1)
    return out


def scroll_selector():
    sel = ".chat-body .cc-log"
    r = ui({"action": "style", "selector": sel, "props": ["overflow-y"], "rects": True})
    if not r.get("matches"):
        die(f"找不到对话的滚动区 {sel}：{r}")
    return sel


def print_ops(r):
    print(f"工作区：{r['workspaces'][0][0]}（{r['workspaces'][0][1]} 条事件）、{r['workspaces'][1][0]}（{r['workspaces'][1][1]} 条）")
    keys = ("ready_ms", "settle_ms", "frames", "dropped", "long_frames", "max_gap", "mutations",
            "cpu_webcontent", "cpu_desktop", "cpu_gpu")
    widths = (8, 8, 6, 6, 6, 8, 9, 9, 9, 8)
    print(f"{'操作':<22}{'就绪ms':>8}{'稳定ms':>8}{'帧':>6}{'掉帧':>6}{'长帧':>6}{'最长ms':>8}{'DOM变更':>9}"
          f"{'网页CPU%':>9}{'本体CPU%':>9}{'GPU%':>8}")
    for name, o in r["ops"].items():
        cols = [o.get(k) for k in keys]
        print(f"{name:<22}" + "".join(f"{'' if v is None else v:>{w}}" for v, w in zip(cols, widths)))
    if r["errors"]:
        print("页面报错：", *r["errors"], sep="\n  ")


def compare(args):
    if len(args) != 2:
        die("用法：perf.sh compare <结果1.json> <结果2.json>")
    a, b = (json.loads(Path(x).read_text()) for x in args)
    rows = []
    if "groups" in a:
        for name in a["groups"]:
            for k in ("cpu_pct", "wakeups_per_s", "power"):
                rows.append((f"{name}.{k}", a["groups"][name].get(k), b["groups"].get(name, {}).get(k)))
            fa, fb = a["groups"][name].get("footprint_mb"), b["groups"].get(name, {}).get("footprint_mb")
            if fa and fb and None not in fa + fb:
                rows.append((f"{name}.mem_growth_mb", round(fa[1] - fa[0], 1), round(fb[1] - fb[0], 1)))
        rows.append(("spawned_children", a["spawned_children"], b["spawned_children"]))
    else:
        for name in a["ops"]:
            for k in ("settle_ms", "dropped", "long_frames", "max_gap", "mutations", "cpu_webcontent", "cpu_desktop", "cpu_gpu"):
                rows.append((f"{name}.{k}", a["ops"][name].get(k), b["ops"].get(name, {}).get(k)))
    print(f"{'指标':<36}{a['label']:>12}{b['label']:>12}{'变化':>10}")
    for name, x, y in rows:
        delta = f"{(y - x) / x * 100:+.0f}%" if isinstance(x, (int, float)) and isinstance(y, (int, float)) and x else ""
        print(f"{name:<36}{'' if x is None else x:>12}{'' if y is None else y:>12}{delta:>10}")


COMMANDS = {"prepare": prepare, "pids": pids, "idle": idle, "ops": ops, "compare": compare}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        die("用法：perf.py prepare|pids|idle|ops|compare")
    COMMANDS[sys.argv[1]](sys.argv[2:])
