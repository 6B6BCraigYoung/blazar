import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROFILE = os.environ.get("BLAZAR_DEV_PROFILE", "dev")
DATA = Path.home() / "Library/Application Support/ai.blazar.Blazar/profiles" / PROFILE
DB = DATA / "blazar.sqlite"
DEMO = Path("/tmp/blazar-ui-demo")
PREFIX = "0d000000-0000-7000-8000-"
NODE = "gpu-a.invalid"
JOB_LABELS = ("演示：等待数据集同步", "演示：长时间评测")


def did(kind, n):
    return f"{PREFIX}{kind:06x}{n:06x}"


LEGACY = ["comet-cli", "pulsar-bot", "meteor-ui", "nova-infra", "lyra-sdk", "vega-tests", "halo-docs"]
FAILED = [
    ("comet-sync", "network", "断网", "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
     "把 docs 目录同步到对象存储，结束后把链接发我", "同步文档到对象存储"),
    ("orbit-site", "busy", "服务繁忙", "API Error: 529 overloaded",
     "把首页的英雄区文案改成三句话，突出离线可用", "改首页文案"),
    ("quasar-eval", "node", "机器连不上", "agent 进程已经不在了（机器重启、被整组杀掉或目录被删），可以续接上一段对话重来",
     "在 v2 评测集上跑一遍 baseline，把分数写进 eval.md", "跑 v2 评测"),
    ("atlas-worker", "quota", "额度用完", "You've hit your usage limit · resets 8am (Asia/Shanghai)",
     "给任务队列加上重试和指数退避", "队列重试与退避"),
    ("lyra-build", "other", "其他原因", "agent 异常退出（退出码 1）。stderr: Error: ENOSPC: no space left on device, write",
     "把三步打包脚本合成一条命令", "一条命令打包"),
]
FAILED_LINE = {
    "network": "网络断了，没做完，可以继续",
    "busy": "服务繁忙，没做完，可以继续",
    "node": "机器连不上，没做完，先把机器连上",
    "quota": "额度用完了，没做完，可以换个账号",
    "other": "磁盘满了，打包脚本没跑完，清掉空间后可以重试",
}
ASK = {"question": "正式站的首页文案要不要一起改？", "options": ["一起改", "先不动正式站"]}
WS = {name: did(1, i) for i, name in enumerate(
    ["atlas-api", "nebula-web", "quasar-ml", "orbit-docs", "quasar-train", "orbit-site",
     "comet-sync", "quasar-eval", "atlas-worker", "lyra-build", *LEGACY], 1)}
ACCOUNT = did(25, 1)
AGENTS = {name: did(2, i) for i, name in enumerate(["backend", "reviewer", "writer"], 1)}
BOTS = {name: did(3, i) for i, name in enumerate(["demo-trainer", "demo-reviewer"], 1)}
PROJECTS = {name: did(4, i) for i, name in enumerate(["demo-orbit", "demo-quasar", "demo-legacy"], 1)}
AUTOPILOTS = [did(5, i) for i in range(1, 4)]
SKILLS = [did(6, i) for i in range(1, 3)]
NODE_ID = did(7, 1)
THREAD = did(10, 1)
APPROVAL = did(9, 1)
QUESTION = did(9, 2)
CHAT_APPROVAL = did(9, 3)
ASSISTANT = did(26, 1)
CHATS = {name: did(27, i) for i, name in enumerate(["status", "handoff", "gpu", "denied", "hard", "hard-retry", "ask"], 1)}
REFUSED = ("Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Remote Shell Writes]. "
           "If you have other tasks that don't depend on this action, continue working on those. IMPORTANT: You *may* attempt to "
           "accomplish this action using other tools that might naturally be used to accomplish this goal, e.g. using head instead of cat. "
           "But you *should not* attempt to work around this denial in malicious ways, e.g. do not use your ability to run tests to execute "
           "non-test actions. If you believe this capability is essential to complete the user's request, first try a safer method. "
           "Get as much of the rest of the task done as you can, then STOP and explain to the user what you were trying to do and why you "
           "need this permission. Let the user decide how to proceed.")
REFUSED_CMD = "ssh edge.invalid 'systemctl restart demo-agent && systemctl status demo-agent --no-pager | head -5'"


def iso(t):
    return t.isoformat()


def ago(**kw):
    return datetime.now(timezone.utc) - timedelta(**kw)


def next_sunday_eight():
    local = datetime.now().astimezone()
    days = (6 - local.weekday()) % 7 or 7
    at = (local + timedelta(days=days)).replace(hour=8, minute=0, second=0, microsecond=0)
    return at.astimezone(timezone.utc)


def git(repo, *args):
    env = dict(os.environ, GIT_AUTHOR_NAME="Demo User", GIT_AUTHOR_EMAIL="demo@example.invalid",
               GIT_COMMITTER_NAME="Demo User", GIT_COMMITTER_EMAIL="demo@example.invalid")
    subprocess.run(["git", "-C", str(repo), *args], check=True, env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def write(repo, rel, text):
    p = repo / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text)


USERS_PY = '''from fastapi import APIRouter, Query

from atlas.db import Session
from atlas.models import User

router = APIRouter(prefix="/users", tags=["users"])


@router.get("")
def list_users(db: Session):
    rows = db.query(User).order_by(User.id).all()
    return {"items": [u.to_dict() for u in rows]}


@router.get("/{user_id}")
def get_user(user_id: int, db: Session):
    return db.get(User, user_id).to_dict()
'''

USERS_PY_NEW = '''from fastapi import APIRouter, HTTPException, Query

from atlas.db import Session
from atlas.models import User

router = APIRouter(prefix="/users", tags=["users"])

MAX_LIMIT = 100


@router.get("")
def list_users(
    db: Session,
    limit: int = Query(20, ge=1, le=MAX_LIMIT),
    offset: int = Query(0, ge=0),
):
    total = db.query(User).count()
    rows = db.query(User).order_by(User.id).offset(offset).limit(limit).all()
    return {"items": [u.to_dict() for u in rows], "total": total, "limit": limit, "offset": offset}


@router.get("/{user_id}")
def get_user(user_id: int, db: Session):
    user = db.get(User, user_id)
    if user is None:
        raise HTTPException(status_code=404, detail="user not found")
    return user.to_dict()
'''

README = '''# atlas-api

Demo HTTP service used by the Blazar UI audit. Nothing here is real.

## Endpoints

| Method | Path | Notes |
| --- | --- | --- |
| GET | /users | list users |
| GET | /users/{id} | one user |
| GET | /health | liveness probe |
'''

README_NEW = README.replace(
    "| GET | /users | list users |",
    "| GET | /users | list users, `limit` 1-100 (default 20) and `offset` |",
) + '''
Invalid paging parameters return `400` with a JSON error body.
'''

APP_PY_NEW = '''from fastapi import FastAPI
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse

from atlas.routes import users

app = FastAPI(title="atlas-api")
app.include_router(users.router)


@app.exception_handler(RequestValidationError)
def bad_request(_, exc):
    return JSONResponse(status_code=400, content={"error": "invalid_params", "detail": exc.errors()})


@app.get("/health")
def health():
    return {"ok": True}
'''

THEME_TS_NEW = '''export const themes = ["light", "dark"] as const;

export const dark = {
  button: "#3b82f6",
  buttonText: "#f8fafc",
  buttonHover: "#2563eb",
};
'''

CONTRAST_TS = '''import { dark } from "./theme";

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

console.assert(contrast(dark.button, dark.buttonText) >= 4.5);
'''

TEST_PAGINATION = '''import pytest

from atlas.testing import client, seed_users


@pytest.fixture
def api():
    with client() as c:
        seed_users(c, 45)
        yield c


def test_default_page_has_twenty_items(api):
    body = api.get("/users").json()
    assert len(body["items"]) == 20
    assert body["total"] == 45


def test_offset_moves_the_window(api):
    first = api.get("/users", params={"limit": 10}).json()["items"]
    second = api.get("/users", params={"limit": 10, "offset": 10}).json()["items"]
    assert first[-1]["id"] < second[0]["id"]


@pytest.mark.parametrize("params", [{"limit": 0}, {"limit": 101}, {"offset": -1}, {"limit": "x"}])
def test_bad_paging_is_a_client_error(api, params):
    assert api.get("/users", params=params).status_code == 400
'''

INDEX_HTML = '''<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><title>atlas-api 演示首页</title>
<style>
body { font-family: -apple-system, sans-serif; margin: 40px; color: #222; }
table { border-collapse: collapse; } td, th { border: 1px solid #ccc; padding: 6px 12px; }
.badge { background: #2a7; color: #fff; border-radius: 4px; padding: 2px 6px; font-size: 12px; }
</style></head>
<body>
<h1>atlas-api <span class="badge">demo</span></h1>
<p>这是界面审计用的演示页面，数据都是假的。</p>
<table>
<tr><th>id</th><th>name</th><th>email</th></tr>
<tr><td>1</td><td>Ada Example</td><td>ada@example.invalid</td></tr>
<tr><td>2</td><td>Bob Sample</td><td>bob@example.invalid</td></tr>
<tr><td>3</td><td>Cy Placeholder</td><td>cy@example.invalid</td></tr>
</table>
</body></html>
'''


def make_repos():
    DEMO.mkdir(parents=True, exist_ok=True)
    atlas = DEMO / "atlas-api"
    if not (atlas / ".git").exists():
        atlas.mkdir(parents=True, exist_ok=True)
        git(atlas, "init", "-q", "-b", "main")
        write(atlas, "README.md", README)
        write(atlas, "pyproject.toml", '[project]\nname = "atlas-api"\nversion = "0.3.0"\n'
              'dependencies = ["fastapi", "sqlalchemy"]\n')
        write(atlas, "src/atlas/__init__.py", "")
        write(atlas, "src/atlas/app.py", "from fastapi import FastAPI\n\nfrom atlas.routes import users\n\n"
              "app = FastAPI(title=\"atlas-api\")\napp.include_router(users.router)\n\n\n"
              "@app.get(\"/health\")\ndef health():\n    return {\"ok\": True}\n")
        write(atlas, "src/atlas/routes/__init__.py", "")
        write(atlas, "src/atlas/routes/users.py", USERS_PY)
        write(atlas, "public/index.html", INDEX_HTML)
        git(atlas, "add", "-A")
        git(atlas, "commit", "-q", "-m", "feat: scaffold atlas api")
        write(atlas, "tests/test_users.py", "from atlas.testing import client\n\n\n"
              "def test_lists_users():\n    with client() as c:\n        assert c.get(\"/users\").status_code == 200\n")
        git(atlas, "add", "-A")
        git(atlas, "commit", "-q", "-m", "test: cover the users list")
        write(atlas, "src/atlas/routes/users.py", USERS_PY_NEW)
        write(atlas, "README.md", README_NEW)
        write(atlas, "tests/test_pagination.py", TEST_PAGINATION)
    nebula = DEMO / "nebula-web"
    if not (nebula / ".git").exists():
        nebula.mkdir(parents=True, exist_ok=True)
        git(nebula, "init", "-q", "-b", "main")
        write(nebula, "package.json", '{\n  "name": "nebula-web",\n  "private": true,\n  "scripts": {"dev": "vite"}\n}\n')
        write(nebula, "src/App.tsx", "export function App() {\n  return <h1>Nebula</h1>;\n}\n")
        git(nebula, "add", "-A")
        git(nebula, "commit", "-q", "-m", "feat: initial app shell")
        git(nebula, "checkout", "-q", "-b", "feature/dark-mode")
        write(nebula, "src/theme.ts", "export const themes = [\"light\", \"dark\"] as const;\n")
        git(nebula, "add", "-A")
        git(nebula, "commit", "-q", "-m", "feat: add theme tokens")
    quasar = DEMO / "quasar-ml"
    if not (quasar / ".git").exists():
        quasar.mkdir(parents=True, exist_ok=True)
        git(quasar, "init", "-q", "-b", "main")
        write(quasar, "train.py", "import yaml\n\n\ndef main(cfg):\n    print(\"training\", cfg[\"run\"])\n")
        write(quasar, "configs/base.yaml", "run: demo-base\nlr: 0.0003\nbatch_size: 64\nsteps: 20000\n")
        git(quasar, "add", "-A")
        git(quasar, "commit", "-q", "-m", "feat: baseline training script")
        write(quasar, "configs/base.yaml", "run: demo-base\nlr: 0.0002\nbatch_size: 128\nsteps: 40000\nwarmup: 500\n")
    docs = DEMO / "orbit-docs"
    if not (docs / ".git").exists():
        docs.mkdir(parents=True, exist_ok=True)
        git(docs, "init", "-q", "-b", "main")
        write(docs, "docs/index.md", "# Orbit 文档\n\n演示用的文档仓库。\n")
        write(docs, "docs/deploy.md", "# 部署\n\n1. 构建\n2. 上传\n3. 切流量\n")
        git(docs, "add", "-A")
        git(docs, "commit", "-q", "-m", "docs: first draft")
    site = DEMO / "orbit-site"
    if not (site / ".git").exists():
        site.mkdir(parents=True, exist_ok=True)
        git(site, "init", "-q", "-b", "main")
        write(site, "index.html", "<!doctype html>\n<title>Orbit</title>\n<h1>Orbit 官网</h1>\n")
        write(site, "styles.css", "body { font-family: sans-serif; margin: 40px; }\n")
        git(site, "add", "-A")
        git(site, "commit", "-q", "-m", "feat: landing page")
    small = {
        "comet-sync": ("sync.sh", "#!/usr/bin/env bash\nset -e\naws s3 sync docs/ s3://demo-bucket/docs/\n"),
        "atlas-worker": ("worker.py", "import queue\n\n\ndef run(q: queue.Queue):\n    while job := q.get():\n        job()\n"),
        "lyra-build": ("build.sh", "#!/usr/bin/env bash\nset -e\nnpm run clean\nnpm run compile\nnpm run bundle\n"),
    }
    for name, (rel, text) in small.items():
        repo = DEMO / name
        if (repo / ".git").exists():
            continue
        repo.mkdir(parents=True, exist_ok=True)
        git(repo, "init", "-q", "-b", "main")
        write(repo, "README.md", f"# {name}\n\n演示仓库，内容是假的。\n")
        write(repo, rel, text)
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "feat: scaffold")


class Clock:
    def __init__(self, start):
        self.t = start

    def tick(self, secs):
        self.t += timedelta(seconds=secs)
        return iso(self.t)


def ev(kind, **kw):
    return dict({"type": kind}, **kw)


def cat_n(text):
    return "\n".join(f"{i:>6}\t{line}" for i, line in enumerate(text.splitlines(), 1))


def edited(path, text, lo, hi):
    lines = text.splitlines()[lo - 1:hi]
    body = "\n".join(f"{i:>6}\t{line}" for i, line in enumerate(lines, lo))
    return f"The file {path} has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n{body}"


def long_thread(cwd):
    p = f"{cwd}/src/atlas/routes/users.py"
    turns = []
    t1 = [
        ev("user_message", text="看一下 atlas-api 的用户列表接口，给 /users 加上分页（limit/offset），默认每页 20 条。顺便补测试。"),
        ev("thinking", text="先读路由文件，确认现在 list_users 是怎么查的，再看测试目录里有没有现成的夹具可以复用。分页参数要有上限，避免一次拉全表。"),
        ev("tool", name="TodoWrite", input={"todos": [
            {"content": "读 users 路由和现有测试", "status": "in_progress", "activeForm": "读 users 路由和现有测试"},
            {"content": "给 list_users 加 limit/offset", "status": "pending", "activeForm": "给 list_users 加 limit/offset"},
            {"content": "补分页测试并跑通", "status": "pending", "activeForm": "补分页测试并跑通"}]},
            result="Todos have been modified successfully"),
        ev("tool", name="Read", input={"file_path": p}, result=cat_n(USERS_PY)),
        ev("tool", name="Grep", input={"pattern": "def client|def seed_users", "path": f"{cwd}/src"},
           result=f"{cwd}/src/atlas/testing.py:12:def client():\n{cwd}/src/atlas/testing.py:27:def seed_users(c, n):"),
        ev("assistant_message", text="现在 `list_users` 直接 `.all()` 拉全表。我会加 `limit`（1–100，默认 20）和 `offset`（≥0），并在响应里带上 `total`。"),
        ev("tool", name="Edit", input={"file_path": p,
            "old_string": '@router.get("")\ndef list_users(db: Session):\n    rows = db.query(User).order_by(User.id).all()\n    return {"items": [u.to_dict() for u in rows]}',
            "new_string": '@router.get("")\ndef list_users(\n    db: Session,\n    limit: int = Query(20, ge=1, le=MAX_LIMIT),\n    offset: int = Query(0, ge=0),\n):\n    total = db.query(User).count()\n    rows = db.query(User).order_by(User.id).offset(offset).limit(limit).all()\n    return {"items": [u.to_dict() for u in rows], "total": total, "limit": limit, "offset": offset}'},
            result=edited(p, USERS_PY_NEW, 7, 23)),
        ev("tool", name="Bash", input={"command": "python -m pytest -q tests/test_users.py", "description": "跑用户接口测试"},
           result=".                                                                        [100%]\n1 passed in 0.41s"),
        ev("tool", name="TodoWrite", input={"todos": [
            {"content": "读 users 路由和现有测试", "status": "completed", "activeForm": "读 users 路由和现有测试"},
            {"content": "给 list_users 加 limit/offset", "status": "completed", "activeForm": "给 list_users 加 limit/offset"},
            {"content": "补分页测试并跑通", "status": "in_progress", "activeForm": "补分页测试并跑通"}]},
            result="Todos have been modified successfully"),
        ev("finish", text="分页已经加上了：\n\n| 参数 | 默认 | 范围 |\n| --- | --- | --- |\n| `limit` | 20 | 1–100 |\n| `offset` | 0 | ≥ 0 |\n\n响应里多了 `total`、`limit`、`offset` 三个字段，旧测试仍然通过。分页本身的测试我下一轮补。"),
    ]
    t2 = [
        ev("user_message", text="分页参数非法时现在返回什么？我希望 400 而不是 422。另外帮我把 README 里的接口表更新一下。"),
        ev("thinking", text="FastAPI 默认对 Query 校验失败返回 422。要改成 400 需要注册 RequestValidationError 的处理器。先让子智能体查一下项目里有没有已有的异常处理约定，避免重复造轮子。"),
        ev("task", description="查项目里的异常处理约定", prompt="在 src/atlas 下找有没有注册过 exception_handler，或者统一的错误响应格式，列出文件和行号。", children=[
            ev("tool", name="Grep", input={"pattern": "exception_handler|HTTPException", "path": f"{cwd}/src"},
               result=f"{cwd}/src/atlas/routes/users.py:1:from fastapi import APIRouter, HTTPException, Query\n{cwd}/src/atlas/routes/users.py:27:        raise HTTPException(status_code=404, detail=\"user not found\")"),
            ev("tool", name="Read", input={"file_path": f"{cwd}/src/atlas/app.py"},
               result=cat_n("from fastapi import FastAPI\n\nfrom atlas.routes import users\n\napp = FastAPI(title=\"atlas-api\")\napp.include_router(users.router)\n")),
            ev("assistant_message", text="项目里没有全局的 exception_handler，只有 users.py 里一处 HTTPException(404)。"),
        ], result="没有全局异常处理器；错误响应目前只有 users.py:27 的 404，格式是 FastAPI 默认的 {\"detail\": ...}。"),
        ev("tool", name="Write", input={"file_path": f"{cwd}/tests/test_pagination.py", "content": TEST_PAGINATION},
           result=f"File created successfully at: {cwd}/tests/test_pagination.py"),
        ev("tool", name="Bash", input={"command": "python -m pytest -q tests/test_pagination.py", "description": "跑分页测试"},
           result="..FFFF                                                                   [100%]\n=================================== FAILURES ===================================\n____________ test_bad_paging_is_a_client_error[params0] ____________\nE       assert 422 == 400\n4 failed, 2 passed in 0.62s", ok=False),
        ev("tool", name="Edit", input={"file_path": f"{cwd}/src/atlas/app.py",
            "old_string": 'app = FastAPI(title="atlas-api")\napp.include_router(users.router)',
            "new_string": 'app = FastAPI(title="atlas-api")\napp.include_router(users.router)\n\n\n@app.exception_handler(RequestValidationError)\ndef bad_request(_, exc):\n    return JSONResponse(status_code=400, content={"error": "invalid_params", "detail": exc.errors()})'},
            result=edited(f"{cwd}/src/atlas/app.py", APP_PY_NEW, 1, 14)),
        ev("tool", name="Bash", input={"command": "python -m pytest -q tests/test_pagination.py", "description": "再跑分页测试"},
           result="......                                                                   [100%]\n6 passed in 0.58s"),
        ev("tool", name="Edit", input={"file_path": f"{cwd}/README.md",
            "old_string": "| GET | /users | list users |",
            "new_string": "| GET | /users | list users, `limit` 1-100 (default 20) and `offset` |"},
            result=edited(f"{cwd}/README.md", README_NEW, 5, 12)),
        ev("finish", text="改好了，非法分页参数现在返回 400：\n\n```python\nfrom fastapi.exceptions import RequestValidationError\nfrom fastapi.responses import JSONResponse\n\n\n@app.exception_handler(RequestValidationError)\ndef bad_request(_, exc):\n    return JSONResponse(\n        status_code=400,\n        content={\"error\": \"invalid_params\", \"detail\": exc.errors()},\n    )\n```\n\n测试覆盖了这几种输入：\n\n| 输入 | 之前 | 现在 |\n| --- | --- | --- |\n| `limit=0` | 422 | 400 |\n| `limit=101` | 422 | 400 |\n| `offset=-1` | 422 | 400 |\n| `limit=x` | 422 | 400 |\n\nREADME 的接口表也更新了。注意这个处理器是全局的，其他接口的参数校验失败也会变成 400。"),
    ]
    pytest_out = "\n".join(
        [f"tests/test_{m}.py::test_case_{i:02d} PASSED{' ' * 30}[{min(100, (i + 1) * 3):>3}%]"
         for i, m in enumerate(["users", "pagination", "health", "auth", "orders"] * 7)]
    ) + "\n\n============================== 35 passed in 2.84s =============================="
    t3 = [
        ev("user_message", text="跑一下全部测试和 lint"),
        ev("tool", name="Bash", input={"command": "ruff check src tests", "description": "lint"}, result="All checks passed!"),
        ev("tool", name="Bash", input={"command": "python -m pytest -v", "description": "跑全部测试"}, result=pytest_out),
        ev("finish", text="全部通过：\n\n| 检查 | 结果 |\n| --- | --- |\n| ruff | 无问题 |\n| pytest | 35 passed（2.84s） |"),
    ]
    t4 = [
        ev("user_message", text="还有两件事：\n\n1. `get_user` 找不到用户时现在是 404，但响应体格式和新的 400 不一致，统一成 `{\"error\": ..., \"detail\": ...}`；\n2. 写一个简单的脚本 `scripts/seed_dev.py`，往本地开发库里灌 50 个假用户，名字用 example.invalid 的邮箱。\n\n不要动生产配置。"),
        ev("thinking", text="第一件事：给 HTTPException 也注册一个处理器，输出同样的结构。第二件事：脚本只依赖 atlas.db 的本地会话，邮箱用 user{i}@example.invalid。两件都不碰 settings 里的生产配置。"),
        ev("tool", name="Read", input={"file_path": f"{cwd}/src/atlas/db.py"},
           result=cat_n("from sqlalchemy import create_engine\nfrom sqlalchemy.orm import sessionmaker\n\nengine = create_engine(\"sqlite:///dev.db\")\nSessionLocal = sessionmaker(bind=engine)\n")),
        ev("tool", name="Write", input={"file_path": f"{cwd}/scripts/seed_dev.py", "content": "from atlas.db import SessionLocal\nfrom atlas.models import User\n\n\ndef main(n=50):\n    with SessionLocal() as db:\n        for i in range(n):\n            db.add(User(name=f\"Demo {i:02d}\", email=f\"user{i:02d}@example.invalid\"))\n        db.commit()\n\n\nif __name__ == \"__main__\":\n    main()\n"},
           result=f"File created successfully at: {cwd}/scripts/seed_dev.py"),
        ev("tool", name="Bash", input={"command": "python scripts/seed_dev.py && sqlite3 dev.db 'select count(*) from users'", "description": "灌假用户并计数"}, result="50"),
        ev("finish", text="两件都做完了：\n\n- 404 和 400 现在用同一种响应体 `{\"error\": ..., \"detail\": ...}`；\n- `scripts/seed_dev.py` 往本地 `dev.db` 写了 50 个假用户，邮箱都是 `@example.invalid`。\n\n生产配置没有改动。"),
    ]
    t5 = [
        ev("user_message", text="提交吧，提交信息用英文"),
        ev("tool", name="Bash", input={"command": "git add -A && git commit -m 'feat(users): paginate the users list and return 400 on bad params'", "description": "提交"},
           result="[main 3f9c2ab] feat(users): paginate the users list and return 400 on bad params\n 5 files changed, 61 insertions(+), 4 deletions(-)\n create mode 100644 scripts/seed_dev.py\n create mode 100644 tests/test_pagination.py"),
        ev("finish", text="已提交：`3f9c2ab feat(users): paginate the users list and return 400 on bad params`。没有推送。"),
    ]
    return [t1, t2, t3, t4, t5]


def expand(sid, clock, items, cwd, tools, parent=None):
    out = []
    for it in items:
        kind = it["type"]
        if kind == "tool" or kind == "task":
            tools[0] += 1
            tid = f"toolu_demo_{tools[0]:04d}"
            name = it.get("name", "Task")
            inp = it.get("input") or {"description": it["description"], "prompt": it["prompt"], "subagent_type": "general-purpose"}
            out.append((clock.tick(2), {"type": "tool_use", "id": tid, "name": name, "input": inp}, parent))
            if kind == "task":
                out += expand(sid, clock, it["children"], cwd, tools, tid)
            out.append((clock.tick(3), {"type": "tool_result", "id": tid, "ok": it.get("ok", True), "content": it["result"]}, parent))
        elif kind == "finish":
            out.append((clock.tick(4), {"type": "assistant_message", "text": it["text"]}, None))
        else:
            out.append((clock.tick(3), it, parent))
    return out


def usage(inp, out, cost):
    return {"type": "token_usage", "input": inp, "output": out, "cache_read": inp * 3, "cache_creation": inp // 5, "cost_usd": cost}


def add_session(db, sid, ws, when, runtime, title, thread, status, events, agent=None, account=None, summary=None, ask=None):
    db.execute("INSERT INTO sessions (id, workspace_id, runtime_kind, provider_session_id, status, created_at, title, thread_id, agent_profile, account_id,"
               " summary, ask) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
               (sid, ws, runtime, f"demo-{sid[-8:]}", status, iso(when), title, thread, agent, account, summary,
                json.dumps(ask, ensure_ascii=False) if ask else None))
    for seq, (ts, payload, parent) in enumerate(events, 1):
        db.execute("INSERT INTO events (session_id, seq, ts, payload, parent_tool_id) VALUES (?,?,?,?,?)",
                   (sid, seq, ts, json.dumps(payload, ensure_ascii=False, separators=(",", ":")), parent))


def short_session(db, sid, ws, when, runtime, title, ask, reply, status, cost, cwd, agent=None):
    c = Clock(when)
    model = "claude-opus-5-5" if runtime == "claude_cli" else "gpt-5.5-codex"
    evs = [
        (c.tick(0), {"type": "session_started", "provider_session_id": f"demo-{sid[-8:]}", "model": model, "cwd": cwd, "permission_mode": "default"}, None),
        (c.tick(1), {"type": "user_message", "text": ask}, None),
        (c.tick(20), {"type": "assistant_message", "text": reply}, None),
        (c.tick(1), usage(int(cost * 9000), int(cost * 1500), cost), None),
    ]
    if status == "failed":
        evs.append((c.tick(1), {"type": "finished", "status": "failed", "message": "API Error: 529 overloaded"}, None))
    elif status == "interrupted":
        evs.append((c.tick(1), {"type": "finished", "status": "interrupted"}, None))
    else:
        evs.append((c.tick(1), {"type": "finished", "status": "success", "text": reply}, None))
    summary = {"failed": FAILED_LINE["busy"], "interrupted": "被中断了，没做完"}.get(status, reply.rstrip("。"))
    add_session(db, sid, ws, when, runtime, title, sid, status, evs, agent, summary=summary)


def tables(db):
    return {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}


def clean(db):
    like = PREFIX + "%"
    present = tables(db)
    for table, col in [("approvals", "id"), ("denial_decisions", "session_id"), ("events", "session_id"), ("sessions", "id"),
                       ("task_comments", "task_id"),
                       ("rate_limit_snapshots", "account_id"), ("accounts", "id"),
                       ("tasks", "id"), ("autopilot_runs", "autopilot_id"), ("autopilots", "id"), ("claims", "id"),
                       ("bot_mail", "to_bot"), ("bot_threads", "bot_id"), ("bots", "id"), ("agents", "id"),
                       ("skill_files", "skill_id"), ("skills", "id"), ("memories", "id"), ("inbox", "id"),
                       ("jobs", "id"), ("snippets", "id"), ("hooks", "id"), ("approval_rules", "id"),
                       ("workspace_scripts", "workspace_id"), ("workspaces", "id"), ("projects", "id")]:
        if table in present:
            db.execute(f"DELETE FROM {table} WHERE {col} LIKE ?", (like,))

    def wipe(table, where, args=()):
        if table in present:
            db.execute(f"DELETE FROM {table} WHERE {where}", args)

    wipe("jobs", "label IN (?, ?)", JOB_LABELS)
    wipe("facts", "key LIKE 'demo/%'")
    wipe("fact_history", "key LIKE 'demo/%'")
    for ref in (like, f"rule:%:{like}", f"rule:%:{NODE}%", "rule:%:demo%"):
        wipe("inbox", "ref_id LIKE ?", (ref,))
    wipe("inbox", "title LIKE '%演示：%' OR title LIKE '%demo-%' OR title LIKE '%demo/%' OR title LIKE ?", (f"%{NODE}%",))
    wipe("rule_alerts", "subject = ? OR subject LIKE 'demo%' OR subject LIKE ?"
         " OR (inbox_id IS NOT NULL AND inbox_id NOT IN (SELECT id FROM inbox))", (NODE, like))
    wipe("nodes", "id = ? OR name = ?", (NODE_ID, NODE))


def stopped():
    if subprocess.run(["pgrep", "-f", "Blazar Dev.app/Contents/MacOS/blazar-desktop"], stdout=subprocess.DEVNULL).returncode == 0:
        sys.exit("开发实例还在运行，先 scripts/dev-instance.sh stop")


def unseed():
    stopped()
    db = sqlite3.connect(DB)
    db.execute("PRAGMA foreign_keys = ON")
    with db:
        clean(db)
    db.close()
    print(f"演示数据已从 {DB} 清掉")


def seed():
    stopped()
    make_repos()
    db = sqlite3.connect(DB)
    db.execute("PRAGMA foreign_keys = ON")
    with db:
        clean(db)
        seed_rows(db)
    db.close()
    print(f"演示数据已写入 {DB}")
    print(f"临时仓库：{DEMO}")


def fleet_listing():
    rows = [("atlas-api", "completed", "local", "demo-orbit"), ("nebula-web", "running", "local", "demo-orbit"),
            ("quasar-ml", "awaiting_approval", "local", "demo-quasar")]
    return json.dumps([{"id": WS[n], "name": n, "node": node, "project": p, "activity": a, "path": str(DEMO / n)}
                       for n, a, node, p in rows], ensure_ascii=False, indent=1)


def seed_chats(db, local):
    path = str(DATA / "assistant")
    Path(path).mkdir(parents=True, exist_ok=True)
    row = db.execute("SELECT id FROM workspaces WHERE kind = 'assistant'").fetchone()
    ws = row[0] if row else ASSISTANT
    if not row:
        db.execute("INSERT INTO workspaces (id, node_id, name, path, kind, activity, status, created_at) VALUES (?,?,?,?,?,?,?,?)",
                   (ws, local, "对话", path, "assistant", "idle", "active", iso(ago(days=9))))
    db.execute("UPDATE workspaces SET activity = 'awaiting_approval', last_active_at = ? WHERE id = ?", (iso(ago(minutes=2)), ws))
    model = {"type": "session_started", "model": "claude-opus-5-5", "cwd": path, "permission_mode": "default",
             "mcp_servers": [{"name": "blazar-fleet", "status": "connected"}]}
    summary = ("三个都看了：\n\n| 工作区 | 状态 | 在做什么 |\n| --- | --- | --- |\n| atlas-api | 已完成 | 分页改动已提交，没有推送 |\n"
               "| nebula-web | 运行中 | 正在调暗色模式的按钮对比度 |\n| quasar-ml | 等你批准 | 要跑 200 步冒烟训练，等你点允许 |\n\n只有 quasar-ml 在等你。")
    c = Clock(ago(hours=1))
    status = [
        (c.tick(0), dict(model, provider_session_id="demo-chat-status"), None),
        (c.tick(1), {"type": "user_message", "text": "看看 atlas-api、nebula-web、quasar-ml 现在都在干什么，有没有在等我的"}, None),
        (c.tick(3), {"type": "tool_use", "id": "toolu_demo_chat1", "name": "mcp__blazar-fleet__list_workspaces", "input": {}}, None),
        (c.tick(2), {"type": "tool_result", "id": "toolu_demo_chat1", "ok": True, "content": fleet_listing()}, None),
        (c.tick(6), {"type": "assistant_message", "text": summary}, None),
        (c.tick(1), usage(6200, 480, 0.09), None),
        (c.tick(1), {"type": "finished", "status": "success", "text": summary}, None),
    ]
    add_session(db, CHATS["status"], ws, ago(hours=1), "claude", "看三个工作区的状态", CHATS["status"], "done", status,
                summary="三个工作区里只有 quasar-ml 在等你批准")
    req = {"tool_name": "mcp__blazar-fleet__send_prompt", "tool_use_id": "toolu_demo_chat2",
           "input": {"workspace_id": WS["nebula-web"], "resume": True, "wait_secs": 15,
                     "text": "暗色模式收尾后，把主题切换按钮放到顶栏右侧，顺手在 README 里补一张截图。"}}
    c = Clock(ago(minutes=4))
    handoff = [
        (c.tick(0), dict(model, provider_session_id="demo-chat-handoff"), None),
        (c.tick(1), {"type": "user_message", "text": "让 nebula-web 接着把暗色模式做完，然后把主题切换按钮放到顶栏右侧"}, None),
        (c.tick(3), {"type": "thinking", "text": "先看 nebula-web 这一轮做到哪了，再决定怎么派。"}, None),
        (c.tick(2), {"type": "tool_use", "id": "toolu_demo_chat2a", "name": "mcp__blazar-fleet__get_history",
                     "input": {"workspace_id": WS["nebula-web"], "limit": 5, "mode": "summary"}}, None),
        (c.tick(2), {"type": "tool_result", "id": "toolu_demo_chat2a", "ok": True,
                     "content": "[user_message] 暗色模式下按钮对比度不够，调一下 src/theme.ts 里的颜色\n[tool_use] Read src/theme.ts\n（这一轮还在进行）"}, None),
        (c.tick(3), {"type": "assistant_message", "text": "nebula-web 正在调 src/theme.ts 的对比度，快收尾了。我准备给它派下一步：做完暗色模式后把切换按钮放到顶栏右侧，并在 README 里补截图。这会改 nebula-web 的文件，要你点允许。"}, None),
        (c.tick(2), {"type": "tool_use", "id": "toolu_demo_chat2", "name": "mcp__blazar-fleet__send_prompt", "input": req["input"]}, None),
        (c.tick(1), {"type": "approval", "id": CHAT_APPROVAL, "request": req}, None),
    ]
    add_session(db, CHATS["handoff"], ws, ago(minutes=4), "claude", "让 nebula-web 接着做暗色模式", CHATS["handoff"], "done", handoff)
    db.execute("INSERT INTO approvals (id, session_id, request, created_at, provider_request_id, workspace_id) VALUES (?,?,?,?,?,?)",
               (CHAT_APPROVAL, CHATS["handoff"], json.dumps(req, ensure_ascii=False), iso(ago(minutes=2)), "demo-chat-appr", ws))
    c = Clock(ago(hours=5))
    gpu = [
        (c.tick(0), dict(model, provider_session_id="demo-chat-gpu"), None),
        (c.tick(1), {"type": "user_message", "text": "汇总一下所有机器现在的 GPU 占用"}, None),
        (c.tick(3), {"type": "tool_use", "id": "toolu_demo_chat3", "name": "mcp__blazar-fleet__probe_node", "input": {"node": NODE}}, None),
        (c.tick(4), {"type": "tool_result", "id": "toolu_demo_chat3", "ok": False, "content": f"机器 {NODE} 不在线，上次在线 5 小时前"}, None),
        (c.tick(2), {"type": "finished", "status": "failed", "message": "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"}, None),
    ]
    add_session(db, CHATS["gpu"], ws, ago(hours=5), "claude", "汇总所有机器的 GPU 占用", CHATS["gpu"], "failed", gpu,
                summary=FAILED_LINE["network"])
    seed_denied_chats(db, ws, dict(model, permission_mode="auto"))
    seed_asking_chat(db, ws, model)
    return ws


def seed_asking_chat(db, ws, model):
    said = ("首页的三句话文案已经改好，提交在 orbit-site 的 main 分支上，没有推送。\n\n"
            "正式站的首页文案要不要一起改？改的话我顺手把两边同步成一样的。")
    c = Clock(ago(minutes=5))
    evs = [
        (c.tick(0), dict(model, provider_session_id="demo-chat-ask"), None),
        (c.tick(1), {"type": "user_message", "text": "orbit-site 的首页文案改完了吗？正式站是不是也要改"}, None),
        (c.tick(3), {"type": "tool_use", "id": "toolu_demo_ask1", "name": "mcp__blazar-fleet__get_history",
                     "input": {"workspace_id": WS["orbit-site"], "limit": 3, "mode": "summary"}}, None),
        (c.tick(2), {"type": "tool_result", "id": "toolu_demo_ask1", "ok": True,
                     "content": "[user_message] 把首页的英雄区文案改成三句话\n[assistant_message] 改好了，已提交，没有推送"}, None),
        (c.tick(4), {"type": "assistant_message", "text": said}, None),
        (c.tick(1), usage(3800, 210, 0.04), None),
        (c.tick(1), {"type": "finished", "status": "success", "text": said}, None),
    ]
    add_session(db, CHATS["ask"], ws, ago(minutes=5), "claude", "首页文案与正式站", CHATS["ask"], "done", evs,
                summary="首页文案改好了，问正式站要不要一起改", ask=ASK)


def denied_turn(c, model, tool_id, ask, retry=False):
    denied = f"Bash: {REFUSED_CMD}"
    denials = [{"tool_name": "Bash", "tool_use_id": tool_id, "input": {"command": REFUSED_CMD, "description": "重启 edge.invalid 上的 demo-agent"}}]
    said = ("这条命令第二次也被安全检查拦下了。这类远程写操作属于安全边界，我批不了，请你在终端里自己运行上面那条命令。" if retry else
            "重启 demo-agent 要通过 ssh 在 edge.invalid 上执行 systemctl restart，这条命令被安全检查拦下了（Remote Shell Writes），我没有执行。")
    return [
        (c.tick(0), dict(model, provider_session_id=f"demo-{tool_id}"), None),
        (c.tick(1), {"type": "user_message", "text": ask}, None),
        (c.tick(3), {"type": "thinking", "text": "demo-agent 跑在 edge.invalid 上，重启要走 ssh。先重启再看状态。"}, None),
        (c.tick(2), {"type": "tool_use", "id": tool_id, "name": "Bash", "input": denials[0]["input"]}, None),
        (c.tick(1), {"type": "tool_result", "id": tool_id, "ok": False, "content": REFUSED}, None),
        (c.tick(4), {"type": "assistant_message", "text": said}, None),
        (c.tick(1), usage(4100, 260, 0.05), None),
        (c.tick(1), {"type": "finished", "status": "success", "text": said, "denied": [denied], "denials": denials}, None),
    ]


def seed_denied_chats(db, ws, model):
    ask = "edge.invalid 上的 demo-agent 掉线了，帮我把它重启一下"
    c = Clock(ago(minutes=9))
    add_session(db, CHATS["denied"], ws, ago(minutes=9), "claude", "重启 edge.invalid 上的 demo-agent", CHATS["denied"], "done",
                denied_turn(c, model, "toolu_demo_deny1", ask), summary="重启命令被安全检查拦下，没有执行")
    c = Clock(ago(minutes=30))
    first = denied_turn(c, model, "toolu_demo_deny2", ask)
    first.append((c.tick(40), {"type": "denial_decided", "tool_use_id": "toolu_demo_deny2", "allow": True, "by": "demo", "what": REFUSED_CMD}, None))
    add_session(db, CHATS["hard"], ws, ago(minutes=30), "claude", "重启 edge.invalid 上的 demo-agent（第二次）", CHATS["hard"], "done", first,
                summary="重启命令被安全检查拦下，没有执行")
    c = Clock(ago(minutes=28))
    again = "我同意执行刚才被拦下的这条命令，请原样重试一次，不要改动：\n```\n" + REFUSED_CMD + "\n```"
    add_session(db, CHATS["hard-retry"], ws, ago(minutes=28), "claude", "重启 edge.invalid 上的 demo-agent（第二次）", CHATS["hard"], "done",
                denied_turn(c, model, "toolu_demo_deny3", again, retry=True), summary="第二次也被拦下，要你自己在终端里重启")
    db.execute("INSERT INTO denial_decisions (id, session_id, workspace_id, tool_use_id, tool_name, what, reason, allow, decided_by, decided_at)"
               " VALUES (?,?,?,?,?,?,?,?,?,?)",
               (did(28, 1), CHATS["hard"], ws, "toolu_demo_deny2", "Bash", REFUSED_CMD, "Remote Shell Writes", 1, "demo", iso(ago(minutes=29))))


def seed_rows(db):
    now = datetime.now(timezone.utc)
    local = db.execute("SELECT id FROM nodes WHERE name = 'local'").fetchone()[0]
    db.execute("INSERT INTO nodes (id, name, transport, endpoint, network, os, arch, labels, capabilities, load, status, last_seen_at, created_at)"
               " VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
               (NODE_ID, NODE, "ssh", "192.0.2.41", "easytier", "linux", "x86_64", '{"gpus":"8x demo"}', "{}", "{}",
                "offline", iso(ago(hours=5)), iso(ago(days=12))))
    for name, pid in PROJECTS.items():
        db.execute("INSERT INTO projects (id, name, created_at) VALUES (?,?,?)", (pid, name, iso(ago(days=20))))
    wss = [
        ("atlas-api", local, str(DEMO / "atlas-api"), PROJECTS["demo-orbit"], "completed", ago(minutes=12), (61, 4, 3)),
        ("nebula-web", local, str(DEMO / "nebula-web"), PROJECTS["demo-orbit"], "running", ago(minutes=1), (0, 0, 0)),
        ("quasar-ml", local, str(DEMO / "quasar-ml"), PROJECTS["demo-quasar"], "awaiting_approval", ago(minutes=4), (3, 2, 1)),
        ("orbit-docs", local, str(DEMO / "orbit-docs"), None, "running", ago(minutes=2), (0, 0, 0)),
        ("quasar-train", NODE_ID, "/home/demo/quasar-train", PROJECTS["demo-quasar"], "idle", ago(days=2), None),
        ("orbit-site", local, str(DEMO / "orbit-site"), PROJECTS["demo-orbit"], "errored", ago(hours=3), (0, 0, 0)),
        ("comet-sync", local, str(DEMO / "comet-sync"), PROJECTS["demo-orbit"], "errored", ago(hours=1), (0, 0, 0)),
        ("quasar-eval", NODE_ID, "/home/demo/quasar-eval", PROJECTS["demo-quasar"], "errored", ago(hours=5), None),
        ("atlas-worker", local, str(DEMO / "atlas-worker"), PROJECTS["demo-orbit"], "errored", ago(minutes=40), (0, 0, 0)),
        ("lyra-build", local, str(DEMO / "lyra-build"), None, "errored", ago(hours=2), (0, 0, 0)),
    ] + [(name, NODE_ID, f"/home/demo/{name}", PROJECTS["demo-legacy"], "completed" if i % 3 == 0 else "idle",
          ago(days=3 + i, hours=i * 5), None) for i, name in enumerate(LEGACY)]
    for name, node, path, proj, act, last, diff in wss:
        a, r, f = diff or (None, None, None)
        db.execute("INSERT INTO workspaces (id, project_id, node_id, name, path, activity, status, last_active_at, created_at, repo_root,"
                   " diff_added, diff_removed, diff_files, diff_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (WS[name], proj, node, name, path, act, "active", iso(last), iso(ago(days=9)), path if node == local else None,
                    a, r, f, iso(last) if diff else None))
    db.execute("INSERT INTO workspace_scripts (workspace_id, dev, updated_at, preview_port) VALUES (?,?,?,?)",
               (WS["atlas-api"], "python3 -u -m http.server 18731 --bind localhost --directory public", iso(ago(days=3)), 18731))
    assistant = seed_chats(db, local)

    agents = [
        ("backend", "后端助手", "claude", "写和改 Python 服务端代码，先读再改，改完跑测试。", "claude-opus-5-5", "#3b82f6", "🛠"),
        ("reviewer", "代码评审", "claude", "只读代码，按严重程度列出问题，不直接改。", "claude-sonnet-5-5", "#a855f7", "🔍"),
        ("writer", "文档写手", "codex", "维护 README 和 docs 目录，语气简洁。", None, "#10b981", "📝"),
    ]
    for key, name, rt, desc, model, color, avatar in agents:
        db.execute("INSERT INTO agents (id, name, runtime, description, instructions, model, color, avatar, starters, created_at, updated_at)"
                   " VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                   (AGENTS[key], name, rt, desc, desc + "\n\n这是演示用的角色档案。", model, color, avatar,
                    json.dumps(["看看最近的改动", "跑一遍测试"], ensure_ascii=False), iso(ago(days=15)), iso(ago(days=2))))
    for i, (name, desc, body) in enumerate([
        ("demo-release-notes", "根据 git log 写发布说明", "# 发布说明\n\n读最近一次 tag 以来的提交，按 feat/fix 分组写成中文发布说明。"),
        ("demo-api-style", "接口风格约定", "# 接口风格\n\n- 错误统一返回 {error, detail}\n- 分页参数用 limit/offset"),
    ]):
        db.execute("INSERT INTO skills (id, name, description, source, created_at, updated_at) VALUES (?,?,?,?,?,?)",
                   (SKILLS[i], name, desc, "local", iso(ago(days=6)), iso(ago(days=1))))
        db.execute("INSERT INTO skill_files (skill_id, path, content) VALUES (?,?,?)",
                   (SKILLS[i], "SKILL.md", f"---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n"))
    db.execute("INSERT INTO agent_skills (agent_id, skill_id) VALUES (?,?)", (AGENTS["backend"], SKILLS[1]))
    db.execute("INSERT INTO agent_skills (agent_id, skill_id) VALUES (?,?)", (AGENTS["writer"], SKILLS[0]))

    cwd = str(DEMO / "atlas-api")
    clock = Clock(ago(minutes=55))
    tools = [0]
    model = {"type": "session_started", "provider_session_id": "demo-long", "model": "claude-opus-5-5", "cwd": cwd,
             "permission_mode": "default", "mcp_servers": [{"name": "blazar-fleet", "status": "connected"}]}
    costs = [(48211, 2310, 0.62), (91240, 5120, 1.37), (30110, 880, 0.31), (64200, 3900, 0.95), (12040, 420, 0.12)]
    lines = ["给 /users 加了 limit/offset 分页，旧测试通过", "非法分页参数改成返回 400，README 已更新", "ruff 和 35 个测试全部通过",
             "统一了错误响应体，加了灌假用户的脚本", "提交了分页改动，没有推送"]
    for n, turn in enumerate(long_thread(cwd)):
        sid = did(10, n + 1)
        start = clock.t
        evs = [(clock.tick(1), dict(model, provider_session_id=f"demo-long-{n}"), None)]
        evs += expand(sid, clock, turn, cwd, tools)
        last = next(e for e in reversed(turn) if e["type"] == "finish")
        evs.append((clock.tick(1), usage(*costs[n][:2], costs[n][2]), None))
        evs.append((clock.tick(1), {"type": "finished", "status": "success", "text": last["text"]}, None))
        add_session(db, sid, WS["atlas-api"], start, "claude_cli", "给 /users 加分页", THREAD, "done", evs, AGENTS["backend"], summary=lines[n])
        clock.tick(90)

    asks = [
        ("atlas-api", "claude_cli", "检查依赖更新", "帮我看看 pyproject 里有没有该升级的依赖", "fastapi 和 sqlalchemy 都有小版本更新，没有破坏性变更。", "backend"),
        ("nebula-web", "claude_cli", "暗色模式", "给 nebula-web 加暗色模式的主题变量", "主题变量放在 src/theme.ts，已经加上 light/dark 两套。", "backend"),
        ("quasar-ml", "codex_app_server", "调学习率", "把 base.yaml 的学习率调小一点，batch 翻倍", "lr 改成 0.0002，batch_size 改成 128，加了 500 步 warmup。", None),
        ("orbit-docs", "codex_app_server", "部署文档", "把部署步骤写细一点", "部署文档补了构建、上传、切流量三步的命令。", "writer"),
        ("atlas-api", "claude_cli", "评审分页改动", "评审一下最近的分页改动", "主要问题：全局 400 处理器会影响其他接口，建议在说明里写清楚。", "reviewer"),
    ]
    statuses = ["done", "done", "done", "failed", "done", "interrupted", "done"]
    k = 0
    for day in range(1, 8):
        for j in range(1 + (day * 7) % 4):
            ws, rt, title, ask, reply, agent = asks[k % len(asks)]
            status = statuses[k % len(statuses)]
            when = now - timedelta(days=day, hours=j * 3 + 1)
            cost = round(0.18 + ((k * 37) % 23) / 10, 2)
            short_session(db, did(11, k + 1), WS[ws], when, rt, title, ask, reply, status, cost,
                          str(DEMO / ws), AGENTS.get(agent) if agent else None)
            k += 1

    c = Clock(ago(minutes=3))
    nebula = [
        (c.tick(0), {"type": "session_started", "provider_session_id": "demo-live", "model": "claude-opus-5-5", "cwd": str(DEMO / "nebula-web"), "permission_mode": "default"}, None),
        (c.tick(1), {"type": "user_message", "text": "暗色模式下按钮对比度不够，调一下 src/theme.ts 里的颜色"}, None),
        (c.tick(4), {"type": "thinking", "text": "先读主题文件，再对照 WCAG AA 的 4.5:1 对比度要求挑颜色。"}, None),
        (c.tick(3), {"type": "tool_use", "id": "toolu_demo_live1", "name": "Read", "input": {"file_path": str(DEMO / "nebula-web/src/theme.ts")}}, None),
        (c.tick(1), {"type": "tool_result", "id": "toolu_demo_live1", "ok": True, "content": cat_n("export const themes = [\"light\", \"dark\"] as const;")}, None),
        (c.tick(6), {"type": "tool_use", "id": "toolu_demo_live2", "name": "Edit", "input": {"file_path": str(DEMO / "nebula-web/src/theme.ts"),
                     "old_string": "export const themes = [\"light\", \"dark\"] as const;", "new_string": THEME_TS_NEW.rstrip("\n")}}, None),
        (c.tick(1), {"type": "tool_result", "id": "toolu_demo_live2", "ok": True, "content": edited(str(DEMO / "nebula-web/src/theme.ts"), THEME_TS_NEW, 1, 7)}, None),
        (c.tick(5), {"type": "tool_use", "id": "toolu_demo_live3", "name": "Write", "input": {"file_path": str(DEMO / "nebula-web/src/contrast.ts"), "content": CONTRAST_TS}}, None),
        (c.tick(1), {"type": "tool_result", "id": "toolu_demo_live3", "ok": True, "content": f"File created successfully at: {DEMO / 'nebula-web/src/contrast.ts'}"}, None),
        (c.tick(2), {"type": "progress", "message_id": "msg_demo_live", "output_tokens": 412}, None),
    ]
    add_session(db, did(12, 1), WS["nebula-web"], ago(minutes=3), "claude_cli", "暗色模式按钮对比度", did(12, 1), "done", nebula, AGENTS["backend"])

    c = Clock(ago(minutes=2))
    retry = {"type": "api_retry", "max_retries": 10, "status": 529, "error": "server_error"}
    orbit = [
        (c.tick(0), {"type": "session_started", "provider_session_id": "demo-retry", "model": "claude-opus-5-5", "cwd": str(DEMO / "orbit-docs"), "permission_mode": "default"}, None),
        (c.tick(1), {"type": "user_message", "text": "给部署文档补一节回滚步骤"}, None),
        (c.tick(3), dict(retry, attempt=1, delay_ms=1000), None),
        (c.tick(2), dict(retry, attempt=2, delay_ms=4000), None),
    ]
    add_session(db, did(12, 3), WS["orbit-docs"], ago(minutes=2), "claude_cli", "补充回滚步骤", did(12, 3), "done", orbit, AGENTS["writer"])

    c = Clock(ago(hours=3, minutes=2))
    site = [
        (c.tick(0), {"type": "session_started", "provider_session_id": "demo-failed", "model": "claude-opus-5-5", "cwd": str(DEMO / "orbit-site"), "permission_mode": "default"}, None),
        (c.tick(1), {"type": "user_message", "text": "把首页的英雄区文案改成三句话，突出离线可用"}, None),
    ]
    site += [(c.tick(2 + n), dict(retry, attempt=n, delay_ms=min(1000 * 2 ** (n - 1), 32000)), None) for n in range(1, 11)]
    site.append((c.tick(5), {"type": "finished", "status": "failed", "message": "API Error: 529 overloaded"}, None))
    add_session(db, did(12, 4), WS["orbit-site"], ago(hours=3, minutes=2), "claude_cli", "改首页文案", did(12, 4), "failed", site, AGENTS["writer"],
                summary=FAILED_LINE["busy"])

    db.execute("INSERT INTO accounts (id, provider, display_name, auth_mode, status, created_at) VALUES (?,?,?,?,?,?)",
               (ACCOUNT, "claude", "演示账号", "cli", "active", iso(ago(days=30))))
    db.execute("INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at) VALUES (?,?,?,?,?)",
               (ACCOUNT, "five_hour", 1.0, iso(next_sunday_eight()), iso(ago(minutes=40))))
    for n, (name, cause, _, message, ask, title) in enumerate(FAILED, 5):
        if name == "orbit-site":
            continue
        when = next(last for ws, _, _, _, _, last, _ in wss if ws == name)
        c = Clock(when - timedelta(minutes=2))
        cwd = f"/home/demo/{name}" if cause == "node" else str(DEMO / name)
        evs = [
            (c.tick(0), {"type": "session_started", "provider_session_id": f"demo-{cause}", "model": "claude-opus-5-5", "cwd": cwd, "permission_mode": "default"}, None),
            (c.tick(1), {"type": "user_message", "text": ask}, None),
            (c.tick(90), {"type": "finished", "status": "failed", "message": message}, None),
        ]
        add_session(db, did(12, n), WS[name], when - timedelta(minutes=2), "claude", title, did(12, n), "failed", evs,
                    AGENTS["backend"], ACCOUNT if cause == "quota" else None, summary=FAILED_LINE[cause])

    c = Clock(ago(minutes=6))
    req = {"tool_name": "Bash", "input": {"command": "python train.py --config configs/base.yaml --steps 200", "description": "冒烟训练 200 步"}, "tool_use_id": "toolu_demo_appr"}
    quasar = [
        (c.tick(0), {"type": "session_started", "provider_session_id": "demo-appr", "model": "claude-opus-5-5", "cwd": str(DEMO / "quasar-ml"), "permission_mode": "default"}, None),
        (c.tick(1), {"type": "user_message", "text": "改完配置后跑 200 步冒烟训练，看 loss 有没有下降"}, None),
        (c.tick(5), {"type": "tool_use", "id": "toolu_demo_appr", "name": "Bash", "input": req["input"]}, None),
        (c.tick(1), {"type": "approval", "id": APPROVAL, "request": req}, None),
    ]
    add_session(db, did(12, 2), WS["quasar-ml"], ago(minutes=6), "claude_cli", "冒烟训练", did(12, 2), "done", quasar, AGENTS["backend"])
    db.execute("INSERT INTO approvals (id, session_id, request, created_at, provider_request_id, workspace_id) VALUES (?,?,?,?,?,?)",
               (APPROVAL, did(12, 2), json.dumps(req, ensure_ascii=False), iso(ago(minutes=4)), "demo-appr-1", WS["quasar-ml"]))
    ask = {"tool_name": "AskUserQuestion", "tool_use_id": "toolu_demo_ask", "input": {"questions": [
        {"header": "节点离线", "question": "lr=3e-4 那组的节点离线了，要不要换到本机重跑？", "multiSelect": False,
         "options": [{"label": "换到本机重跑", "description": "本机 GPU 空闲，大约 40 分钟"},
                     {"label": "等节点恢复", "description": "保留进度，恢复后继续"}]}]}}
    db.execute("INSERT INTO approvals (id, session_id, request, created_at, provider_request_id, workspace_id) VALUES (?,?,?,?,?,?)",
               (QUESTION, did(12, 2), json.dumps(ask, ensure_ascii=False), iso(ago(minutes=3)), "demo-ask-1", WS["quasar-ml"]))

    tasks = [
        ("给 /users 加分页", "limit/offset，默认 20，上限 100；非法参数返回 400。", "done", "high", ["api"], "atlas-api", "backend"),
        ("评审分页改动", "重点看全局异常处理器的影响面。", "in_review", "medium", ["review"], "atlas-api", "reviewer"),
        ("暗色模式按钮对比度", "对比度至少 4.5:1。", "in_progress", "medium", ["ui"], "nebula-web", "backend"),
        ("学习率扫描 3 组", "lr ∈ {1e-4, 2e-4, 3e-4}，各跑 2 万步。", "in_progress", "high", ["train"], "quasar-ml", None),
        ("部署文档补充回滚步骤", "", "todo", "low", ["docs"], "orbit-docs", "writer"),
        ("升级 fastapi 小版本", "确认没有破坏性变更后再升。", "todo", "none", [], None, None),
        ("整理旧的实验分支", "", "backlog", "low", ["cleanup"], None, None),
        ("评测集换成 v2", "等数据组给出 v2 的下载地址。", "backlog", "medium", ["eval"], None, None),
        ("迁移到新的日志格式", "方案被否了。", "cancelled", "none", [], None, None),
    ]
    base = db.execute("SELECT COALESCE(MAX(number), 0) FROM tasks").fetchone()[0]
    for i, (title, desc, status, prio, labels, ws, agent) in enumerate(tasks, 1):
        tid = did(13, i)
        created = ago(days=10 - i)
        started = iso(ago(days=3)) if status in ("in_progress", "in_review", "done") else None
        done = iso(ago(hours=2)) if status in ("done", "cancelled") else None
        thread = THREAD if i == 1 else (did(12, 1) if i == 3 else None)
        db.execute("INSERT INTO tasks (id, number, title, description, status, priority, labels, workspace_id, agent_profile, runtime, thread_id,"
                   " position, created_at, updated_at, started_at, completed_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (tid, base + i, title, desc, status, prio, json.dumps(labels), WS.get(ws), AGENTS.get(agent),
                    "claude" if agent else None, thread, float(i), iso(created), iso(ago(hours=i)), started, done))
    db.execute("INSERT INTO task_comments (id, task_id, author, body, created_at) VALUES (?,?,?,?,?)",
               (did(14, 1), did(13, 2), "user", "全局处理器的影响写进 PR 描述里。", iso(ago(hours=1))))
    db.execute("INSERT INTO task_comments (id, task_id, author, body, note, created_at) VALUES (?,?,?,?,?,?)",
               (did(14, 2), did(13, 1), "agent", "已提交 3f9c2ab，35 个测试全部通过。", 1, iso(ago(minutes=20))))

    autos = [
        ("每日依赖巡检", "检查 pyproject 和 package.json 里可以升级的依赖，列出变更日志要点。", "atlas-api", "reviewer", "0 9 * * 1-5", None, 0, None),
        ("夜间评测汇总", "汇总昨晚所有评测任务的指标，写进 orbit-docs/docs/eval.md。", "orbit-docs", "writer", "30 2 * * *", "连续失败 3 次，已自动暂停", 3, None),
        ("PR 合并后更新文档", "收到合并通知后，把变更同步到 docs。", "orbit-docs", "writer", None, None, 0, "demo-webhook-not-a-real-token"),
    ]
    for i, (name, ins, ws, agent, cron, reason, streak, hook) in enumerate(autos):
        db.execute("INSERT INTO autopilots (id, name, instructions, workspace_id, agent_profile, runtime, mode, status, paused_reason, cron, timezone,"
                   " next_run_at, webhook_token, fail_streak, last_run_at, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (AUTOPILOTS[i], name, ins, WS[ws], AGENTS[agent], "claude", "task", "paused", reason, cron, "Asia/Shanghai",
                    None, hook, streak, iso(ago(days=1, hours=i)), iso(ago(days=14)), iso(ago(days=1))))
    for i, (ap, status, why) in enumerate([(0, "done", None), (0, "done", None), (1, "failed", "工作区不在线"), (1, "failed", "工作区不在线"),
                                          (1, "failed", "工作区不在线"), (2, "skipped", "上一次还没跑完")]):
        db.execute("INSERT INTO autopilot_runs (id, autopilot_id, source, status, reason, triggered_at, completed_at) VALUES (?,?,?,?,?,?,?)",
                   (did(15, i + 1), AUTOPILOTS[ap], "webhook" if ap == 2 else "schedule", status, why,
                    iso(ago(days=i + 1)), iso(ago(days=i + 1) + timedelta(minutes=4))))

    bots = [
        ("demo-trainer", "训练值守", "lead", "backend", "quasar-ml", "P0", "演示：已暂停，不会被唤醒"),
        ("demo-reviewer", "评审员", "worker", "reviewer", "atlas-api", "P2", "演示：已暂停"),
    ]
    for handle, name, role, agent, ws, prio, note in bots:
        db.execute("INSERT INTO bots (id, handle, display_name, role, agent_id, home_ws_id, priority, runtimes, status, status_note,"
                   " last_active_at, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (BOTS[handle], handle, name, role, AGENTS[agent], WS[ws], prio, '["claude"]', "paused", note,
                    iso(ago(hours=6)), iso(ago(days=8)), iso(ago(hours=6))))
    db.execute("INSERT INTO bot_threads (id, bot_id, kind, title, summary, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
               (did(16, 1), BOTS["demo-trainer"], "main", "主线", "盯着 quasar 的训练和评测。", iso(ago(days=8)), iso(ago(hours=6))))
    db.execute("INSERT INTO bot_threads (id, bot_id, kind, title, task_id, summary, closed_at, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
               (did(16, 2), BOTS["demo-trainer"], "task", "学习率扫描", did(13, 4), "第一组已跑完。", None, iso(ago(days=2)), iso(ago(hours=7))))
    for i, (to, subject, body) in enumerate([
        ("demo-trainer", "学习率扫描", "lr=2e-4 那组跑完了，loss 曲线比基线低 3%。"),
        ("demo-reviewer", "评审请求", "atlas-api 的分页改动请看一下。"),
    ]):
        db.execute("INSERT INTO bot_mail (id, to_bot, from_kind, from_ref, subject, body, status, delivered_at, done_at, created_at)"
                   " VALUES (?,?,?,?,?,?,?,?,?,?)",
                   (did(17, i + 1), BOTS[to], "user", "", subject, body, "done", iso(ago(hours=8)), iso(ago(hours=7)), iso(ago(hours=8))))
    for i, (name, desc, body) in enumerate([
        ("demo-gpu-etiquette", "演示：GPU 使用约定", "训练前先认领 gpu:N，跑完立刻释放。"),
        ("demo-eval-baseline", "演示：评测基线", "v1 评测集上的基线分数是 71.4。"),
    ]):
        db.execute("INSERT INTO memories (id, scope, name, description, type, body, updated_by, updated_at) VALUES (?,?,?,?,?,?,?,?)",
                   (did(18, i + 1), "shared", name, desc, "project", body, "demo-trainer", iso(ago(days=i + 1))))

    facts = [
        ("demo/run/quasar-lr-2e-4", "run", {"step": 18400, "loss": 1.82, "eta_min": 14}, "ok"),
        ("demo/run/quasar-lr-3e-4", "run", {"step": 7200, "loss": 2.31, "note": "节点离线后没有更新"}, "stale"),
        (f"demo/gpu/{NODE}/0", "gpu", {"util": 0, "mem_gb": 0}, "lost"),
        ("demo/dataset/eval-v1", "dataset", {"rows": 12000, "sha": "0000demo"}, "ok"),
    ]
    for key, kind, value, status in facts:
        db.execute("INSERT INTO facts (key, kind, owner_bot, value, status, updated_at, created_at) VALUES (?,?,?,?,?,?,?)",
                   (key, kind, BOTS["demo-trainer"], json.dumps(value, ensure_ascii=False), status, iso(ago(minutes=25)), iso(ago(days=2))))
        db.execute("INSERT INTO fact_history (key, value, at) VALUES (?,?,?)", (key, json.dumps(value, ensure_ascii=False), iso(ago(minutes=25))))
    claims = [
        (BOTS["demo-trainer"], NODE_ID, None, "gpu:0,1", "exclusive", "学习率扫描"),
        (BOTS["demo-trainer"], local, WS["quasar-ml"], "dir:/tmp/blazar-ui-demo/quasar-ml/runs", "shared", "写评测结果"),
        (BOTS["demo-reviewer"], local, None, "port:18731", "exclusive", "预览 atlas-api"),
    ]
    for i, (bot, node, ws, scope, mode, note) in enumerate(claims):
        db.execute("INSERT INTO claims (id, bot_id, node_id, workspace_id, scope, mode, note, created_at) VALUES (?,?,?,?,?,?,?,?)",
                   (did(19, i + 1), bot, node, ws, scope, mode, note, iso(ago(hours=i + 1))))

    jobs = [
        ("演示：打包 wheel", "echo building && sleep 2 && echo done", "done", 0, local, WS["atlas-api"], 3),
        ("演示：导出评测表", "echo exporting; exit 1", "failed", 1, local, WS["orbit-docs"], 5),
        ("演示：清理缓存", "sleep 600", "killed", None, local, WS["quasar-ml"], 8),
        ("演示：远程评测", "sleep 3600", "lost", None, NODE_ID, WS["quasar-train"], 20),
    ]
    for i, (label, cmd, status, code, node, ws, hours) in enumerate(jobs):
        db.execute("INSERT INTO jobs (id, owner_kind, bot_id, workspace_id, node_id, kind, label, command, cwd, status, exit_code, notify,"
                   " last_error, created_at, updated_at, ended_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (did(20, i + 1), "bot" if i == 3 else "user", BOTS["demo-trainer"] if i == 3 else None, ws, node, "spawn", label, cmd,
                    None, status, code, "none", "机器离线，查不到进程" if status == "lost" else None,
                    iso(ago(hours=hours)), iso(ago(hours=hours - 1)), iso(ago(hours=hours - 1))))

    inbox = [
        ("run_done", "提交了分页改动，没有推送", "已提交：`3f9c2ab feat(users): paginate the users list and return 400 on bad params`。没有推送。", "atlas-api", THREAD, None, ago(minutes=20), False, False),
        ("approval", "等你批准：冒烟训练 200 步", "python train.py --config configs/base.yaml --steps 200", "quasar-ml", did(12, 2), APPROVAL, ago(minutes=4), False, False),
        ("question", "在问你：lr=3e-4 那组的节点离线了，要不要换到本机重跑？", "lr=3e-4 那组的节点离线了，要不要换到本机重跑？", "quasar-ml", did(12, 2), QUESTION, ago(minutes=3), False, False),
        ("run_failed", FAILED_LINE["busy"], "API Error: 529 overloaded", "orbit-site", did(12, 4), None, ago(hours=3), False, False),
    ] + [
        ("run_failed", FAILED_LINE[cause], message, name, did(12, n), None,
         next(last for ws, _, _, _, _, last, _ in wss if ws == name), False, False)
        for n, (name, cause, _, message, _, title) in enumerate(FAILED, 5) if name != "orbit-site"
    ] + [
        ("autopilot_paused", "自动化「夜间评测汇总」已暂停", "连续失败 3 次。", None, None, AUTOPILOTS[1], ago(hours=9), True, False),
        ("job", "后台任务「演示：导出评测表」失败了", "退出码 1", None, None, None, ago(hours=4), True, False),
        ("alert", "后台任务「演示：远程评测」失联", "它在等的机器 gpu-a.invalid 不在线。", None, None, "rule:job_orphan:demo", ago(hours=19), True, True),
        ("approval", "等你批准：给工作区派活", "给 nebula-web 派活：暗色模式收尾后，把主题切换按钮放到顶栏右侧", "对话", CHATS["handoff"], CHAT_APPROVAL, ago(minutes=2), False, False),
        ("run_failed", FAILED_LINE["network"], "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)", "对话", CHATS["gpu"], None, ago(hours=5), False, False),
        ("denied", "被拦下：远程命令写操作", REFUSED_CMD, "对话", CHATS["denied"], "toolu_demo_deny1", ago(minutes=8), False, False),
        ("question", "在问你：" + ASK["question"], "", "对话", CHATS["ask"], "ask:" + CHATS["ask"], ago(minutes=5), False, False),
    ]
    for i, (kind, title, body, ws, thread, ref, when, read, archived) in enumerate(inbox):
        db.execute("INSERT INTO inbox (id, kind, title, body, workspace_id, thread_id, session_id, ref_id, read_at, archived_at, created_at)"
                   " VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                   (did(21, i + 1), kind, title, body, assistant if ws == "对话" else WS.get(ws), thread,
                    thread if kind.startswith("run_") or kind == "denied" or (ref or "").startswith("ask:") else None, ref,
                    iso(when) if read else None, iso(when) if archived else None, iso(when)))
    snippets = [
        ("验收清单", "- 单元测试全部通过\n- 没有新的 clippy 警告\n- 改动的接口在 README 里有说明"),
        ("提交规范", "Conventional Commits，英文小写祈使句，标题不超过 72 个字符，细节写进正文。"),
    ]
    for i, (name, body) in enumerate(snippets):
        db.execute("INSERT INTO snippets (id, name, body, created_at, updated_at) VALUES (?,?,?,?,?)",
                   (did(22, i + 1), name, body, iso(ago(days=3)), iso(ago(days=1))))
    hooks = [
        ("turn_start", "hub", "scripts/check-disk.sh", 0, 20),
        ("session_end", "node", "git status --short > /tmp/blazar-last-status", 0, 30),
    ]
    for i, (event, target, command, blocking, timeout) in enumerate(hooks):
        db.execute("INSERT INTO hooks (id, event, command, target, blocking, timeout_secs, enabled, created_at) VALUES (?,?,?,?,?,?,?,?)",
                   (did(23, i + 1), event, command, target, blocking, timeout, 1, iso(ago(days=5))))
    rules = [
        ("Read", "", None, 1, 128),
        ("Bash", "pytest", WS["atlas-api"], 1, 17),
        ("Edit", "docs/", WS["orbit-docs"], 0, 0),
    ]
    for i, (tool, pattern, ws, enabled, hits) in enumerate(rules):
        db.execute("INSERT INTO approval_rules (id, tool, pattern, workspace_id, enabled, hits, created_at) VALUES (?,?,?,?,?,?,?)",
                   (did(24, i + 1), tool, pattern, ws, enabled, hits, iso(ago(days=2))))


def session():
    s = json.loads((DATA / "hub.session").read_text())
    return s["url"], s["token"]


def call(method, path, body=None):
    base, token = session()
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=90) as r:
        raw = r.read().decode()
    return json.loads(raw) if raw.strip() else {}


def ui(cmd):
    out = call("POST", "/api/dev/ui", cmd)
    if isinstance(out, dict) and out.get("error"):
        raise RuntimeError(f"{cmd.get('action')}: {out['error']}")
    return out


def try_ui(cmd):
    if cmd.get("optional") and cmd.get("selector") and not any(
            t["shown"] for t in ui({"action": "text", "selector": cmd["selector"]})["texts"]):
        return {"skipped": True}
    try:
        return ui(cmd)
    except RuntimeError as e:
        print(f"  ! {e}", file=sys.stderr)
        return None


class Shooter:
    def __init__(self, out, round_name, rehearse=False):
        self.root = Path(out)
        self.round = round_name
        self.rehearse = rehearse
        self.dir = self.root / round_name
        self.dir.mkdir(parents=True, exist_ok=True)
        self.entries = []
        self.issues = []
        self.broken = 0

    def arrive(self, route, settle):
        path = route.split("?")[0]
        want = section_title(route) if path == "/settings" else TITLES.get(path)
        for attempt in range(3):
            url = ui({"action": "init"}).get("url")
            head = (ui({"action": "snapshot"}).get("headings") or [""])[0] if want else None
            if url == path and head == want:
                return None
            if attempt < 2:
                ui({"action": "navigate", "path": route, "settle_ms": settle + 600 * (attempt + 1)})
        return f"想要 {route}（{want or '不查标题'}），实际停在 {url}（{head or '无标题'}）"

    def shoot(self, no, slug, theme, route, expect, state, warning=None):
        if "toast" not in slug and ui(TOAST_GONE).get("found"):
            print(f"  ! {slug}：提示一直没有消失", file=sys.stderr)
        layout = ui({"action": "layout"})
        errors = ui({"action": "errors"}).get("errors", [])
        name = f"{no:02d}-{slug}-{theme}.png"
        size = None
        if not self.rehearse:
            shot = ui({"action": "shot", "name": name, "settle_ms": 250})
            shutil.move(shot["path"], self.dir / name)
            size = [shot["width"], shot["height"]]
        over = layout.get("overflowing", [])
        rel = (self.dir / name).relative_to(ROOT) if (self.dir / name).is_relative_to(ROOT) else self.dir / name
        self.entries.append({"file": str(rel), "route": route, "expect": expect, "state": state,
                             "overflow": len(over), "errors": len(errors), "size": size, "warning": warning})
        if warning:
            self.issues.append(f"{name}\t路由或标题不对\t{warning}")
        for o in over:
            self.issues.append(f"{name}\t溢出\t{o.get('name', '')[:80]}\t{o.get('rect')}")
        for e in errors:
            self.issues.append(f"{name}\t错误\t{e.splitlines()[0][:300]}（完整内容见 console-errors.log）")
        if errors:
            with (self.dir / "console-errors.log").open("a", encoding="utf-8") as log:
                for e in errors:
                    log.write(f"== {name}\n{e}\n\n")
        print(f"{name}\t溢出 {len(over)}\t错误 {len(errors)}" + ("\t演练，没截图" if self.rehearse else ""))

    def check(self, name, step):
        if step["action"] == "check_ui":
            found = ui({"action": "text", "selector": step["selector"]})
            got = found["texts"][0]["text"].strip() if "text" in step and found["texts"] else found["matches"]
            want = step.get("text", step.get("want"))
            kind = "画面不对"
        else:
            got = db_rows(step["sql"], step.get("args", ()))[0][0]
            want = step["want"]
            kind = "数据没复原"
        if got == want:
            return
        print(f"  ! {name}：{step['what']}应为 {want!r}，实际 {got!r}", file=sys.stderr)
        self.issues.append(f"{name}\t{kind}\t{step['what']}应为 {want!r}，实际 {got!r}")
        self.broken += 1

    def page(self, no, slug, theme, route, expect, state, steps=(), scroll=True, settle=900):
        ui({"action": "errors"})
        try_ui({"action": "scroll", "selector": "main.main", "top": "start", "settle_ms": 50})
        if route:
            nav = ui({"action": "navigate", "path": route, "settle_ms": settle})
            if not nav.get("ok"):
                print(f"  ! 没跳到 {route}：{nav}，等页面就绪后再试一次", file=sys.stderr)
                ready()
                nav = ui({"action": "navigate", "path": route, "settle_ms": settle + 600})
            if not nav.get("ok"):
                print(f"  ! 没跳到 {route}：{nav}", file=sys.stderr)
        warning = self.arrive(route, settle) if route else None
        after = [st for st in steps if st.get("after")]
        for step in (st for st in steps if not st.get("after")):
            if step["action"] == "scroll_mid":
                info = ui({"action": "scroll", "selector": step["selector"], "top": "end", "settle_ms": 500})
                ui({"action": "scroll", "selector": step["selector"], "top": info["height"] // 2 - info["view"] // 2, "settle_ms": 600})
            elif try_ui(step) is None and not step.get("optional"):
                raise RuntimeError(f"准备步骤失败：{step}")
        if scroll:
            try_ui({"action": "scroll", "selector": "main.main", "top": "start", "settle_ms": 100})
        if route and not steps and not warning:
            warning = self.arrive(route, settle)
        if warning:
            print(f"  ! {slug}：{warning}", file=sys.stderr)
        self.shoot(no, slug, theme, route, expect, state, warning)
        for step in after:
            if step["action"] in ("check_db", "check_ui"):
                self.check(f"{no:02d}-{slug}-{theme}.png", step)
            else:
                try_ui(step)
        if not scroll:
            return
        info = try_ui({"action": "scroll", "selector": "main.main", "top": "end", "settle_ms": 300})
        if info and info["height"] > info["view"] * 1.1:
            if info["height"] > info["view"] * 2.5:
                ui({"action": "scroll", "selector": "main.main", "top": info["height"] // 2 - info["view"] // 2, "settle_ms": 300})
                self.shoot(no, f"{slug}-middle", theme, route, expect + "（页面中段）", state)
                ui({"action": "scroll", "selector": "main.main", "top": "end", "settle_ms": 300})
            self.shoot(no, f"{slug}-bottom", theme, route, expect + "（滚到底）", state)
        try_ui({"action": "scroll", "selector": "main.main", "top": "start", "settle_ms": 50})

    def save(self, viewport):
        (self.dir / "manifest.json").write_text(json.dumps({"viewport": viewport, "entries": self.entries, "issues": self.issues},
                                                           ensure_ascii=False, indent=1))


def ready():
    for selector in ("nav.side a.nav", "main.main .page, main.main .ws"):
        if not ui({"action": "wait", "selector": selector, "timeout_ms": 15000}).get("found"):
            sys.exit(f"页面没有就绪：等不到 {selector}")
    ui({"action": "wait", "ms": 300})


def viewport():
    probe = "".join(f"@media (min-resolution: {d}dppx){{body::after{{content:'{d}'}}}}" for d in (1, 1.5, 2, 3))
    ui({"action": "css", "value": probe})
    dpr = ui({"action": "style", "selector": "body", "pseudo": "::after", "props": ["content"]})["styles"][0]["content"].strip("\"'")
    ui({"action": "css", "value": ""})
    w, h = ui({"action": "layout"})["viewport"]
    return {"inner": [w, h], "dpr": dpr}


def theme(value):
    left = dismiss()
    if left:
        sys.exit(f"切主题前还有没关掉的{'、'.join(left)}，先处理再拍")
    ui({"action": "navigate", "path": "/settings?s=appearance", "settle_ms": 500})
    ui({"action": "type", "selector": "#settings-theme", "value": value, "settle_ms": 300})


TITLES = {"/": "中控台", "/workspaces": "工作区", "/chats": "全部对话", "/tasks": "任务", "/autopilots": "任务",
          "/jobs": "任务", "/bots": "bot", "/agents": "智能体", "/agents/new": "新建角色档案", "/skills": "智能体",
          "/apps": "接入的软件", "/runtimes": "智能体", "/nodes": "机器与组网", "/usage": "智能体"}


def section_title(route):
    query = route.split("?", 1)[1] if "?" in route else ""
    section = dict(part.split("=", 1) for part in query.split("&") if "=" in part).get("s", "appearance")
    return dict(SETTINGS).get(section, "设置")


TOAST_GONE = {"action": "wait", "selector": ".toasts .toast", "gone": True, "timeout_ms": 6000}
ESC = {"action": "key", "key": "Escape", "optional": True}
OVERLAYS = (("弹窗", "[data-modal-mask]"), ("右键菜单", ".ctx-menu"), ("状态弹层", ".side-pop"), ("被弹窗挡住的区域", "[inert]"))
ESC_AT = {"右键菜单": ".ctx-menu", "状态弹层": ".side-status", "弹窗": "body"}


def open_overlays():
    return [name for name, selector in OVERLAYS if ui({"action": "text", "selector": selector})["matches"]]


def dismiss():
    for _ in range(4):
        if not open_overlays():
            return []
        try_ui(dict(ESC))
        left = open_overlays()
        for name, selector in ESC_AT.items():
            if name in left:
                try_ui({"action": "key", "key": "Escape", "selector": selector, "settle_ms": 200})
    return open_overlays()


CLOSE_FILE = {"action": "click", "selector": "[aria-label=\"关闭 src/atlas/routes/users.py\"]", "optional": True}
RESTORE = {"action": "click", "selector": "button[aria-label=\"还原检查器\"]", "optional": True}
CLOSE_INSPECTOR = {"action": "click", "selector": "button[aria-label=\"关闭检查器\"]", "optional": True}
OPEN_FILES = {"action": "click", "selector": ".ws-head button[aria-label=\"文件\"][aria-pressed=\"false\"]", "optional": True}
EDITOR_TAB = {"action": "click", "selector": ".ws-insp .rtabs .rtab:first-child", "optional": True}
CLOSE_FILES = {"action": "click", "selector": ".ws-head button[aria-label=\"文件\"][aria-pressed=\"true\"]", "optional": True}
CLOSE_TERM = {"action": "click", "selector": "button[aria-label=\"收起终端\"]", "optional": True}
PREVIEW_KEY = {"action": "key", "key": "P", "code": "KeyP", "meta": True, "shift": True, "selector": "body"}
CHANGES = {"action": "click", "selector": "button.ws-changes"}


def calm(*after):
    return tuple(dict(step, after=True) if after else dict(step)
                 for step in (RESTORE, EDITOR_TAB, CLOSE_FILE, CLOSE_FILES, CLOSE_INSPECTOR, CLOSE_TERM))

ROUTES = [
    ("console", "/", "中控台：侧栏第一项「中控台」带橙色角标；页头「中控台 · 对话切换器 ⌄ · 新对话 ⌘T」，「全部对话…」在切换器菜单最下面；"
     "标题下一行小统计「N 在跑 · N 等你 · N 失败」；中间是最近一个助手对话；右栏 372px 常驻「需要你处理」卡片，下面是「动态」和「查看全部」"),
    ("home", "/workspaces", "工作区首页：需要你处理 / 运行中 / 最近 三组两行短卡，「最近」超过两行时折叠并在标题右侧给出「显示全部（N）」"),
    ("chats", "/chats", "全部对话：页头「中控台 / 全部对话」；每行是状态点、标题、最后一句的灰字摘要、时间，页头右侧「新对话」；没有对话时是统一的空状态"),
    ("chat-new", "/chats/new", "新对话页：中控台布局，对话切换器显示「新对话」，720 居中的空对话区和输入框，右栏照常"),
    ("inbox", "/?inbox=all", "中控台右栏展开成完整消息列表：「‹ 消息」返回键、显示 / 类型两个筛选、无框列表、未读蓝点"),
    ("tasks", "/tasks", "任务看板"),
    ("autopilots", "/autopilots", "自动化列表"),
    ("jobs", "/jobs", "后台任务列表"),
    ("bots", "/bots", "bot 列表"),
    ("agents", "/agents", "角色档案列表"),
    ("agents-new", "/agents/new", "新建角色档案表单"),
    ("skills", "/skills", "技能库"),
    ("apps", "/apps", "接入的软件（办公 / 仿真）"),
    ("runtimes", "/runtimes", "运行时与账号额度"),
    ("nodes", "/nodes", "机器与组网"),
    ("usage", "/usage", "用量统计（近 30 天）"),
    ("settings", "/settings", "设置首页（外观分区）"),
]

SETTINGS = [
    ("appearance", "外观"), ("chat", "对话与编辑"), ("notify", "提醒"), ("shortcuts", "快捷键"), ("rules", "自动批准"),
    ("snippets", "片段"), ("hooks", "自动执行脚本"), ("git", "Git 与 PR"), ("platforms", "技能来源"), ("about", "数据与关于"),
]

TABS = [
    ("tasks", "/tasks", [("all", "全部", "/tasks"), ("autopilots", "自动化", "/autopilots"), ("jobs", "后台任务", "/jobs")]),
    ("agents", "/agents", [("profiles", "角色档案", "/agents"), ("skills", "技能", "/skills"), ("runtimes", "运行时", "/runtimes"),
                           ("usage", "用量", "/usage")]),
]


def tab_step(href):
    return {"action": "click", "selector": f"nav.section-tabs a[href=\"{href}\"]"}


def brow(path):
    return f".browser .brow[title$=\"/{path.parent.name}/{path.name}\"]"


def browse_to(folder, expect):
    return ({"action": "type", "selector": "input[aria-label=\"目录路径\"]", "value": str(folder)},
            {"action": "click", "text": "前往"},
            {"action": "wait", "selector": brow(expect), "timeout_ms": 8000},
            {"action": "wait", "ms": 300})


def settings_parts(round_name):
    sheet = {"action": "wait", "selector": "[data-modal-root]", "timeout_ms": 4000}
    items = [
        ("settings-shortcut-recording", "/settings?s=shortcuts", "快捷键：点一下「任务」的键帽进入录入状态",
         ({"action": "click", "selector": "button[aria-label=\"更改「任务」快捷键\"]"},), False),
        ("settings-shortcut-conflict", "/settings?s=shortcuts", "快捷键：录入时按下已被「全部消息」占用的 ⌥I，行内给出冲突提示",
         ({"action": "click", "selector": "button[aria-label=\"更改「任务」快捷键\"]"},
          {"action": "key", "key": "i", "code": "KeyI", "alt": True, "selector": "button[aria-label=\"更改「任务」快捷键\"]"},
          {"action": "wait", "ms": 300}), False),
        ("settings-advanced-sheet", "/settings?s=chat", "「高级…」打开的说明面板（sheet）",
         ({"action": "click", "text": "高级…"}, sheet), False),
        ("settings-snippet-new", "/settings?s=snippets", "片段列表底部「+」打开的新建面板",
         ({"action": "click", "selector": "button[aria-label=\"添加片段\"]"}, sheet), False),
        ("settings-hook-new", "/settings?s=hooks", "自动执行脚本「+」打开的添加面板",
         ({"action": "click", "selector": "button[aria-label=\"添加自动执行脚本\"]"}, sheet), False),
    ]
    if round_name == "data":
        items.append(("settings-snippet-selected", "/settings?s=snippets", "片段列表选中一行（蓝色高亮，− 和「编辑…」可用）",
                      ({"action": "wait", "selector": ".pref-item", "timeout_ms": 8000}, {"action": "click", "selector": ".pref-item"}), False))
        items.append(("settings-snippet-edit", "/settings?s=snippets", "片段双击或「编辑…」打开的编辑面板",
                      ({"action": "wait", "selector": ".pref-item", "timeout_ms": 8000}, {"action": "click", "selector": ".pref-item"},
                       {"action": "click", "selector": "button[aria-label=\"编辑选中的片段\"]"}, sheet), False))
    return items


PALETTE_INPUT = ".palette .pal-search input"
PALETTE_OPEN = ({"action": "key", "key": "k", "code": "KeyK", "meta": True, "selector": "body"},
                {"action": "wait", "selector": ".palette", "timeout_ms": 4000}, {"action": "wait", "ms": 200})


def ws_rows_exist():
    return bool(db_rows("SELECT id FROM workspaces WHERE kind = 'project' LIMIT 1"))


def palette_parts(with_workspace):
    items = [
        ("palette", "/", "命令面板（⌘K）空输入：「最近」和「建议」两组", PALETTE_OPEN, False),
        ("palette-query", "/", "命令面板输入「设」：按工作区 / 页面 / 命令 / 机器分组，「设置」排在页面里",
         (*PALETTE_OPEN, {"action": "type", "selector": PALETTE_INPUT, "value": "设"}, {"action": "wait", "ms": 200}), False),
        ("palette-empty", "/", "命令面板输入没人匹配的「zzqv」：面板高度不变，结果区中间一行灰字「没有匹配的结果」",
         (*PALETTE_OPEN, {"action": "type", "selector": PALETTE_INPUT, "value": "zzqv"}, {"action": "wait", "ms": 200}), False),
    ]
    if with_workspace:
        items.append(("palette-actions", "/", "命令面板选中一个工作区后按 ⌘K 换成它的二级动作",
                      (*PALETTE_OPEN, {"action": "type", "selector": PALETTE_INPUT, "value": "a"},
                       {"action": "wait", "ms": 200},
                       {"action": "key", "key": "k", "code": "KeyK", "meta": True, "selector": PALETTE_INPUT},
                       {"action": "wait", "ms": 200}), False))
    return items


def row_menu_parts():
    rows = [
        ("tasks", "/tasks", "button.work-task", "任务行的右键菜单（打开 ↩、简介 ⌘I、开始、删除 ⌘⌫）"),
        ("autopilots", "/autopilots", "button.work-ap-row", "自动化行的右键菜单（打开、简介、立即运行、暂停、删除）"),
        ("jobs", "/jobs", "tr.job-row", "后台任务行的右键菜单（打开、复制命令、结束）"),
        ("bots", "/bots", "article.bot-card", "bot 行的右键菜单（打开、暂停、归档）"),
        ("agents", "/agents", "tr.agent-row", "角色档案行的右键菜单（打开、复制、归档）"),
        ("skills", "/skills", "tr.skill-row", "技能行的右键菜单（打开、更新、删除）"),
    ]
    return [(f"{slug}-row-menu", route, expect,
             ({"action": "wait", "selector": f"{row}[data-index=\"0\"]", "timeout_ms": 8000},
              {"action": "context_menu", "selector": f"{row}[data-index=\"0\"]"}), False)
            for slug, route, row, expect in rows]


ROW_OF = {"/tasks": "button.work-task", "/autopilots": "button.work-ap-row", "/jobs": "tr.job-row",
          "/agents": "tr.agent-row", "/skills": "tr.skill-row"}


def poke_menu(href):
    row = ROW_OF.get(href)
    if not row or not ui({"action": "text", "selector": f"{row}[data-index=\"0\"]"})["matches"]:
        return
    ui({"action": "context_menu", "selector": f"{row}[data-index=\"0\"]"})
    ui({"action": "wait", "selector": ".ctx-menu", "timeout_ms": 2000})
    ui({"action": "key", "key": "Escape", "selector": ".ctx-menu", "settle_ms": 150})


def poke_palette():
    for step in PALETTE_OPEN:
        ui(step)
    ui({"action": "key", "key": "Escape", "selector": PALETTE_INPUT, "settle_ms": 150})


def stress(rounds):
    ui({"action": "init"})
    ready()
    failed = False
    for t in ("dark", "light"):
        theme(t)
        ui({"action": "errors"})
        switches = 0
        errors = []
        for _, route, tabs in TABS:
            ui({"action": "navigate", "path": route, "settle_ms": 400})
            for n in range(rounds):
                for i, (_, label, href) in enumerate(tabs):
                    ui(tab_step(href))
                    switches += 1
                    if (n + i) % 3 == 0:
                        poke_menu(href)
                    if (n + i) % 5 == 0:
                        poke_palette()
                    left = dismiss()
                    if left:
                        errors.append(f"第 {n + 1} 轮「{label}」之后还有没关掉的{'、'.join(left)}")
                    for e in ui({"action": "errors"}).get("errors", []):
                        errors.append(f"第 {n + 1} 轮「{label}」之后：{e}")
        print(f"{t}\t切换 {switches} 次\t控制台错误 {len(errors)} 条")
        for e in errors:
            print(f"  ! {e}", file=sys.stderr)
        failed = failed or bool(errors)
    theme("system")
    if failed:
        sys.exit(1)


def chat_parts():
    if not db_rows("SELECT 1 FROM sessions WHERE id = ?", (CHATS["status"],)):
        return []
    row = ".chat-row"
    down = {"action": "key", "key": "ArrowDown", "code": "ArrowDown", "selector": ".chats-page"}
    chats_left = {"action": "check_db", "sql": "SELECT count(*) FROM sessions WHERE id LIKE ?", "args": (PREFIX + "%",),
                  "want": None, "what": "对话数"}
    listed = '.chat-room .cc-tool[data-name="list_workspaces"]'

    def shown(selector, want, what):
        return {"action": "check_ui", "selector": selector, "want": want, "what": what, "after": True}

    def says(selector, text, what):
        return {"action": "check_ui", "selector": selector, "text": text, "what": what, "after": True}

    return [
        ("chats-row-menu", "/chats", "对话行的右键菜单（打开 ↩ / 重命名… / 删除 ⌘⌫）",
         ({"action": "wait", "selector": f"{row}[data-index=\"0\"]", "timeout_ms": 8000},
          {"action": "context_menu", "selector": f"{row}[data-index=\"0\"]"}), False),
        ("chat-done", f"/chats/{CHATS['status']}", "对话页：中控台布局，页头「中控台 · 标题 ⌄ · 已完成」，720 居中的对话（「列出了 3 个工作区」带三个工作区小标签和「详情」），没有进度行和停止键",
         ({"action": "wait", "selector": ".chat-room .cc-log", "timeout_ms": 8000}, {"action": "wait", "ms": 800},
          says(".chat-room-head .status", "已完成", "页头状态"), shown(".chat-room .cc-spin", 0, "进度行"),
          shown('.chat-room .cb-send[data-mode="stop"]', 0, "停止键"), shown(f"{listed} .cc-wss .cc-ws", 3, "工作区小标签"),
          shown(f"{listed} .cc-raw", 1, "「详情」")), False),
        ("chat-approval", f"/chats/{CHATS['handoff']}", "等你批准的对话页：页头状态橙色「等你批准」，审批卡标题「允许派活给 nebula-web？」，正文是引用的话加目标工作区小标签",
         ({"action": "wait", "selector": ".chat-room .cc-appr", "timeout_ms": 8000}, {"action": "wait", "ms": 800},
          says(".chat-room-head .status", "等你批准", "页头状态"), says(".chat-room .cc-appr .ah", "允许派活给 nebula-web？", "审批卡标题"),
          shown(".chat-room .cc-appr .aquote", 1, "引用的话"), shown(".chat-room .cc-appr .abody .cc-ws", 1, "目标工作区小标签")), False),
        ("chat-failed", f"/chats/{CHATS['gpu']}", "失败的对话页：输入框换成失败卡（断网）",
         ({"action": "wait", "selector": ".chat-room .cc-log", "timeout_ms": 8000}, {"action": "wait", "ms": 800},
          says(".chat-room-head .status", "失败", "页头状态"), shown(".chat-room .cc-spin", 0, "进度行")), False),
        ("chat-denied", f"/chats/{CHATS['denied']}", "自动批准模式下被安全检查拦下的对话页：橙点提示行里是一张卡，标题「安全检查拦下了这条命令，没有执行」，"
         "完整命令（可滚动不截断）、「工作区 对话 · 机器 本机」和「原因：[Remote Shell Writes]」、「拦截说明」折叠，底部两个键「允许这一次并重试」「不用了」",
         ({"action": "wait", "selector": ".chat-room .cc-deny", "timeout_ms": 8000}, {"action": "wait", "ms": 800},
          says(".chat-room .cc-deny .cd-head", "安全检查拦下了这条命令，没有执行", "卡片标题"),
          says(".chat-room .cc-deny .acmd", REFUSED_CMD, "完整命令"),
          says(".chat-room .cc-deny .cd-meta span:last-child", "原因：[Remote Shell Writes]", "拦截原因"),
          shown(".chat-room .cc-deny .cd-acts .btn", 2, "两个按钮"), shown(".chat-room .cc-deny .ui-detail", 1, "拦截说明")), False),
        ("chat-denied-hard", f"/chats/{CHATS['hard']}", "批准后仍被拦下的对话页：第一张卡「已允许这一次（demo）」且没有按钮，中间是用户本人的放行消息，"
         "第二张卡红色标题「这条命令属于安全边界，批准也不放行」，命令右侧有复制键，底下一句「把命令复制到终端里自己运行」",
         ({"action": "wait", "selector": ".chat-room .cc-deny[data-state=\"hard\"]", "timeout_ms": 8000}, {"action": "wait", "ms": 800},
          shown(".chat-room .cc-deny[data-state=\"allowed\"]", 1, "已允许的卡"), shown(".chat-room .cc-deny .cd-acts", 0, "按钮"),
          says(".chat-room .cc-deny[data-state=\"hard\"] .cd-head", "这条命令属于安全边界，批准也不放行", "硬拦截标题"),
          shown(".chat-room .cc-deny[data-state=\"hard\"] .cc-copy", 1, "复制键"),
          shown(".chat-room .cc-user", 2, "用户消息")), False),
        ("chat-picker", f"/chats/{CHATS['status']}", "页头对话切换器的下拉：新对话 ⌘T、重命名对话…，下面是全部对话",
         ({"action": "wait", "selector": ".chat-pick .ws-thread-btn", "timeout_ms": 8000},
          {"action": "click", "selector": ".chat-pick .ws-thread-btn", "settle_ms": 300},
          {"action": "wait", "selector": ".ws-threads", "timeout_ms": 4000}, dict(ESC, after=True)), False),
        ("chats-undo-toast", "/chats", "⌘⌫ 删除一个对话：行立即消失，底部出现带「撤销」的提示",
         ({"action": "wait", "selector": f"{row}[data-index=\"2\"]", "timeout_ms": 8000}, down, down, down,
          {"action": "key", "key": "Backspace", "code": "Backspace", "meta": True, "selector": f"{row}[data-sel=\"true\"]"},
          {"action": "wait", "selector": ".toast.act", "timeout_ms": 4000},
          dict({"action": "click", "text": "撤销"}, after=True),
          dict(TOAST_GONE, after=True), dict({"action": "wait", "ms": 1500}, after=True),
          dict(chats_left, want=db_rows(chats_left["sql"], chats_left["args"])[0][0], after=True)), False),
    ]


def undo_parts():
    item = ".pref-item"
    restored = {"action": "check_db", "sql": "SELECT count(*) FROM snippets", "want": 2, "what": "片段数"}
    return [("settings-snippet-undo-toast", "/settings?s=snippets", "删除一个片段：行立即消失，底部出现带「撤销」的提示",
             ({"action": "wait", "selector": item, "timeout_ms": 8000}, {"action": "click", "selector": item},
              {"action": "key", "key": "Backspace", "code": "Backspace", "selector": f"{item}[aria-selected=\"true\"]"},
              {"action": "wait", "selector": ".toast.act", "timeout_ms": 4000},
              dict({"action": "click", "text": "撤销"}, after=True),
              dict(TOAST_GONE, after=True), dict({"action": "wait", "ms": 1500}, after=True),
              dict(restored, after=True)), False)]


def plan(round_name):
    state = "空状态（dev profile 现有数据）" if round_name == "empty" else "演示数据"
    items = [(slug, route, expect, (), True) for slug, route, expect in ROUTES]
    for key, label in SETTINGS:
        items.append((f"settings-{key}", f"/settings?s={key}", f"设置 → {label}", (), True))
    items += settings_parts(round_name)
    for group, route, tabs in TABS:
        for key, label, href in tabs:
            items.append((f"tab-{group}-{key}", route, f"页头标签「{label}」（从 {route} 点过去）", (tab_step(href),), False))
    items += palette_parts(ws_rows_exist())
    items.append(("status-popover", "/", "侧栏「设置」旁状态圆点的弹层（连接、机器、提醒）",
                  ({"action": "click", "selector": "button.side-dot"}, {"action": "wait", "ms": 300}), False))
    items.append(("nodes-advanced", "/nodes", "机器页表格下方「设置…」或「高级…」打开的机器发现面板",
                  ({"action": "wait", "selector": ".machines-foot .linkbtn", "timeout_ms": 8000},
                   {"action": "click", "selector": ".machines-foot .linkbtn"},
                   {"action": "wait", "selector": "[data-modal-root]", "timeout_ms": 4000}), False))
    mesh_more = ".nodes-page .pref-group .pref-more-btn"
    items.append(("nodes-mesh-advanced", "/nodes", "团队组网「本机组网」一组的「高级…」打开的组网详情面板",
                  ({"action": "wait", "selector": mesh_more, "timeout_ms": 8000},
                   {"action": "scroll", "selector": "main.main", "top": "end", "settle_ms": 300},
                   {"action": "click", "selector": mesh_more},
                   {"action": "wait", "selector": "[data-modal-root]", "timeout_ms": 4000}), False))
    items.append(("settings-return", "", "设置页按 Esc 回到进入前的页面（这里从中控台的完整消息列表进入）",
                  ({"action": "navigate", "path": "/?inbox=all", "settle_ms": 600},
                   {"action": "navigate", "path": "/settings?s=chat", "settle_ms": 600},
                   {"action": "wait", "selector": "nav.side .side-back", "timeout_ms": 4000},
                   {"action": "key", "key": "Escape", "code": "Escape", "selector": "body"},
                   {"action": "wait", "selector": ".console-rail[data-mode=\"all\"]", "timeout_ms": 4000}), False))
    ws_rows = db_rows("SELECT id, name, activity FROM workspaces WHERE kind = 'project' AND coalesce(project_id, '') != ?"
                      " ORDER BY created_at", (PROJECTS["demo-legacy"],))
    if not ws_rows:
        TITLES["/workspaces"] = "开始"
        items.append(("start", "/workspaces", "首次打开的「开始」清单：运行时、机器、第一个工作区", (), False))
        items.append(("start-new-workspace", "/workspaces", "开始清单里「选择项目文件夹…」打开的对话框",
                      ({"action": "click", "selector": ".start-actions .btn.primary"}, {"action": "wait", "ms": 600}), False))
        return state, items
    make_repos()
    new_ws = {"action": "click", "selector": ".home-page .page-actions .btn.primary"}
    items.append(("new-workspace", "/workspaces", "新建工作区对话框（页头「新建」），目录停在演示仓库",
                  (new_ws, {"action": "wait", "ms": 600}, *browse_to(DEMO, DEMO / "atlas-api")), False))
    items.append(("new-workspace-folder-menu", "/workspaces", "新建工作区对话框里目录的右键菜单（演示仓库里的 src）",
                  (new_ws, {"action": "wait", "ms": 600}, *browse_to(DEMO / "atlas-api", DEMO / "atlas-api" / "src"),
                   {"action": "context_menu", "selector": brow(DEMO / "atlas-api" / "src")}), False))
    first_row = ".home-card[data-index=\"0\"]"
    items.append(("home-selected", "/workspaces", "首页用 → 选中第一张卡：2px 蓝色描边，尺寸不变；审批卡第二行右侧换成「1 允许 3 拒绝」键帽",
                  ({"action": "wait", "selector": first_row, "timeout_ms": 8000},
                   {"action": "key", "key": "ArrowRight", "code": "ArrowRight", "selector": ".home-page"}), False))
    items.append(("home-row-menu", "/workspaces", "首页卡片的右键菜单（允许 / 总是允许 / 拒绝、打开 ↩、简介 ⌘I、改名、移除 ⌘⌫）",
                  ({"action": "wait", "selector": first_row, "timeout_ms": 8000},
                   {"action": "context_menu", "selector": first_row}), False))
    show_all = ".home-group[data-group=\"recent\"] .lgroup-actions button"
    if db_rows("SELECT 1 FROM projects WHERE id = ?", (PROJECTS["demo-legacy"],)):
        items.append(("home-recent-folded", "/workspaces", "「最近」超过两行时只显示两行，组标题右侧有「显示全部（N）」",
                      ({"action": "wait", "selector": show_all, "timeout_ms": 8000},), False))
        items.append(("home-recent-all", "/workspaces", "点「显示全部」后「最近」原地展开，不跳页",
                      ({"action": "wait", "selector": show_all, "timeout_ms": 8000},
                       {"action": "click", "selector": show_all}, {"action": "wait", "ms": 300}), False))
    items.append(("workspace-detail", "/workspaces", "右键菜单「简介」打开的属性弹层",
                  ({"action": "wait", "selector": first_row, "timeout_ms": 8000},
                   {"action": "context_menu", "selector": first_row}, {"action": "click", "text": "简介", "partial": True},
                   {"action": "wait", "ms": 600}), False))
    first_msg = ".console-rail .inbox-row[data-index=\"0\"]"
    items.append(("console-rail", "/", "中控台右栏：「需要你处理」里是等你批准（允许 1 / 拒绝 3）、在问你（选项加回复框）、被拦下（命令加「允许这一次并重试」「不用了」）、没跑成（下一步加「标为已处理」）的卡片，"
                  "橙色描边；下面「动态」是跑完了等条目，右侧「查看全部」",
                  ({"action": "wait", "selector": ".console-rail .inbox-row[data-kind=\"denied\"]", "timeout_ms": 8000},
                   {"action": "check_ui", "selector": ".console-rail .inbox-row[data-kind=\"denied\"] .inbox-act .btn", "want": 2, "what": "被拦下卡片的按钮数", "after": True},
                   {"action": "check_ui", "selector": ".console-rail .rail-feed .inbox-row[data-needs=\"true\"]", "want": 0, "what": "动态里不该有需要处理的卡", "after": True},
                   {"action": "check_ui", "selector": "nav.side .nav .n[data-pill=\"true\"]", "want": 1, "what": "侧栏橙色角标", "after": True}), False))
    items.append(("inbox-selected", "/?inbox=all", "完整消息列表用 ↓ 走到第二条（键盘逐条处理）",
                  ({"action": "wait", "selector": first_msg, "timeout_ms": 8000},
                   {"action": "key", "key": "ArrowDown", "code": "ArrowDown", "selector": first_msg},
                   {"action": "key", "key": "ArrowDown", "code": "ArrowDown", "selector": ".console-rail .inbox-row[data-sel=\"true\"]"}), False))
    items.append(("inbox-row-menu", "/?inbox=all", "完整消息列表里一条通知的右键菜单（打开、标为已读 U、归档 E）",
                  ({"action": "wait", "selector": first_msg, "timeout_ms": 8000},
                   {"action": "context_menu", "selector": first_msg}), False))
    items += chat_parts()
    failed = {name: label for name, _, label, _, _, _ in FAILED}
    for wid, name, activity in ws_rows:
        card = f"，输入框换成失败卡：第一行是原因，右下是这一类的下一步，左下「标为已处理」" if activity == "errored" else ""
        items.append((f"workspace-{slugify(name)}", f"/w/{wid}", f"工作区页：{name}{card}",
                      ({"action": "wait", "ms": 800}, *calm()), False))
    if round_name == "data":
        for name, cause, label, _, _, _ in FAILED:
            card = f".home-card[href=\"/w/{WS[name]}\"]"
            items.append((f"home-failed-{cause}", "/workspaces", f"首页「需要你处理」里{label}的卡片（第二行是原因）及其右键菜单：第一项是这一类的下一步，然后是「标为已处理 E」",
                          ({"action": "wait", "selector": card, "timeout_ms": 8000},
                           {"action": "context_menu", "selector": card}), False))
        items += detail_pages()
        items += workspace_parts()
        items += row_menu_parts()
        items += undo_parts()
    return state, items


def slugify(s):
    out = "".join(ch if ch.isalnum() and ch.isascii() else "-" for ch in s.lower())
    return "-".join(p for p in out.split("-") if p) or "x"


def db_rows(sql, args=()):
    db = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    try:
        return db.execute(sql, args).fetchall()
    finally:
        db.close()


def detail_pages():
    items = []
    for bid, handle in db_rows("SELECT id, handle FROM bots WHERE archived_at IS NULL ORDER BY handle"):
        items.append((f"bot-{slugify(handle)}", f"/bots/{bid}", f"bot 详情：{handle}", (), True))
    for aid, name in db_rows("SELECT id, name FROM agents WHERE archived_at IS NULL AND id LIKE ? ORDER BY id", (PREFIX + "%",)):
        items.append((f"agent-{aid[-2:]}", f"/agent/{aid}", f"角色档案详情：{name}", (), True))
    for app in ["lark", "github", "obsidian", "isaacsim", "mujoco"]:
        items.append((f"app-{app}", f"/apps/{app}", f"接入的软件详情：{app}", (), True))
    for rt in ["claude", "codex"]:
        items.append((f"runtime-{rt}", f"/runtimes/{rt}", f"运行时详情：{rt}", (), True))
    for node in ["local", NODE]:
        items.append((f"node-{slugify(node)}", f"/nodes/{node}", f"机器详情：{node}", (), True))
    for (name,) in db_rows("SELECT name FROM projects WHERE id LIKE ? ORDER BY name", (PREFIX + "%",)):
        items.append((f"project-{slugify(name)}", f"/project/{name}", f"项目详情：{name}", (), True))
    sheet = {"action": "wait", "selector": "[data-modal-root]", "timeout_ms": 4000}
    items.append(("app-github-advanced", "/apps/github", "接入的软件详情里「高级…」打开的连接详情面板",
                  ({"action": "wait", "selector": ".pref-more-btn", "timeout_ms": 8000},
                   {"action": "click", "selector": ".pref-more-btn"}, sheet), False))
    items.append(("runtime-claude-advanced", "/runtimes/claude", "运行时详情里「配置」组的「高级…」打开的高级配置面板",
                  ({"action": "wait", "selector": ".pref-group:last-child .pref-more-btn", "timeout_ms": 8000},
                   {"action": "click", "selector": ".pref-group:last-child .pref-more-btn"}, sheet), False))
    confirm_btn = ".nodes-page .page-actions .btn[aria-haspopup=\"dialog\"]"
    items.append(("nodes-confirm", "/nodes", "机器页页头「确认 N 台…」打开的批量确认面板（勾选机器、选开发机或个人电脑）",
                  ({"action": "wait", "selector": confirm_btn, "timeout_ms": 8000},
                   {"action": "click", "selector": confirm_btn}, sheet), False))
    items.append(("nodes-row-menu", "/nodes", "机器行的右键菜单（打开、新建工作区、设为开发机或个人电脑）",
                  ({"action": "wait", "selector": ".node-row[data-index=\"1\"]", "timeout_ms": 8000},
                   {"action": "context_menu", "selector": ".node-row[data-index=\"1\"]"}), False))
    pending_confirm = ".nodes-page.detail-page .page-actions .btn:not(.primary)[aria-haspopup=\"dialog\"]"
    items.append(("node-pending-confirm", f"/nodes/{NODE}", "待确认机器的详情：页头橙点「待确认」，次按钮「确认…」打开的确认面板",
                  ({"action": "wait", "selector": pending_confirm, "timeout_ms": 8000},
                   {"action": "click", "selector": pending_confirm}, sheet), False))
    items.append(("tree-menu", f"/w/{WS['atlas-api']}", "文件树的右键菜单",
                  ({"action": "wait", "ms": 800}, *calm(), dict(OPEN_FILES), {"action": "wait", "ms": 400},
                   {"action": "context_menu", "selector": "button.tn[title=\"README.md\"]"}, *calm(True)), False))
    return items


def workspace_parts():
    w = f"/w/{WS['atlas-api']}"
    wq = f"/w/{WS['quasar-ml']}"
    log = ".cc-log"
    wait = {"action": "wait", "ms": 900}
    reset = calm()
    after = calm(True)
    url = {"action": "type", "selector": "input[aria-label=\"预览地址\"]", "value": "http://localhost:18731/", "submit": True}
    tray = {"action": "click", "selector": ".cc-box .cb-todo", "settle_ms": 300}
    threads = {"action": "click", "selector": ".ws-head .ws-thread-btn", "settle_ms": 300}
    return [
        ("chat-bottom", w, "对话区底部（长对话，最后一轮）", (wait, *reset, {"action": "scroll", "selector": log, "top": "end", "settle_ms": 600}), False),
        ("chat-tray-open", w, "输入框顶部的托盘：点「待办 2/3」向上展开最多三行，右侧是「3 个文件 +45 −6 · 审阅」",
         (wait, *reset, {"action": "scroll", "selector": log, "top": "end", "settle_ms": 400}, tray, dict(tray, after=True)), False),
        ("chat-threads", w, "页头里会话名的下拉：打开的对话、新对话、重命名、关闭，下面是历史",
         (wait, *reset, threads, {"action": "wait", "selector": ".ws-threads", "timeout_ms": 4000}, dict(ESC, after=True)), False),
        ("chat-middle", w, "对话区中段（子智能体、diff、长代码块、表格）",
         (wait, *reset, {"action": "scroll", "selector": log, "top": "start", "settle_ms": 800}, {"action": "scroll", "selector": log, "top": "start", "settle_ms": 800},
          {"action": "scroll_mid", "selector": log}), False),
        ("chat-top", w, "对话区顶部（第一轮用户消息、思考、待办）",
         (wait, *reset, {"action": "scroll", "selector": log, "top": "start", "settle_ms": 800}, {"action": "scroll", "selector": log, "top": "start", "settle_ms": 800}), False),
        ("editor-tree", w, "编辑器里的文件树（⌘B）", (wait, *reset, dict(OPEN_FILES), {"action": "wait", "ms": 600}, *after), False),
        ("file-open", w, "从文件树打开 src/atlas/routes/users.py，文件树留在编辑器左边",
         (wait, *reset, dict(OPEN_FILES), {"action": "wait", "ms": 400},
          {"action": "click", "selector": "button.tn[title=\"src/atlas/routes/users.py\"]"}, {"action": "wait", "ms": 900}, *after), False),
        ("diff", w, "检查器「更改 → 差异」：工具栏只有「差异 / Git」「未提交 | 整条分支」、改动摘要、「显示 ⌄」和「让智能体审阅…」；每块改动以灰色细线「第 N 行」开头，旁边「展开上方 N 行」",
         (wait, *reset, dict(CHANGES), {"action": "wait", "ms": 900}, *after), False),
        ("diff-show-menu", w, "「更改」工具栏的「显示 ⌄」菜单：统一 / 并排、忽略空白、自动换行、改动文件列表、全部折叠，带勾选和快捷键",
         (wait, *reset, dict(CHANGES), {"action": "wait", "ms": 900}, {"action": "click", "selector": ".diffbar .dv-btn", "settle_ms": 300},
          {"action": "wait", "selector": ".dv-menu", "timeout_ms": 4000}, dict(ESC, after=True), *after), False),
        ("git", w, "检查器「更改 → Git」",
         (wait, *reset, dict(CHANGES), {"action": "click", "selector": ".insp-sub button:last-child"}, {"action": "wait", "ms": 900}, *after), False),
        ("terminal", w, "底部终端抽屉（⌘J）",
         (wait, *reset, {"action": "key", "key": "J", "code": "KeyJ", "meta": True, "selector": "body"}, {"action": "wait", "ms": 900}, *after), False),
        ("preview", w, "检查器「预览」：地址栏居中、最宽 640，左边「← →」，右边「在浏览器中打开」、刷新、⋯（本地 http.server 提供 public/index.html）",
         (wait, *reset, dict(PREVIEW_KEY), {"action": "wait", "ms": 400}, url, {"action": "wait", "ms": 1500}, *after), False),
        ("preview-max", w, "检查器放大后的预览",
         (wait, *reset, dict(PREVIEW_KEY), {"action": "wait", "ms": 400}, url, {"action": "wait", "ms": 1200},
          {"action": "click", "selector": "button[aria-label=\"放大检查器\"]"}, {"action": "wait", "ms": 800}, *after), False),
        ("inspector-max-approval", wq, "检查器放大后：底部 44px 迷你条（状态「等你批准」、一行输入框、「回到对话」），审批卡自动弹在迷你条上方，可按 1 / 2 / 3",
         (wait, *reset, dict(CHANGES), {"action": "wait", "ms": 600}, {"action": "click", "selector": "button[aria-label=\"放大检查器\"]"},
          {"action": "wait", "selector": ".ws-mini-dock .cc-appr", "timeout_ms": 4000}, {"action": "wait", "ms": 500},
          dict({"action": "click", "selector": ".ws-mini .btn", "optional": True}, after=True), *after), False),
    ]


def shots(out, round_name, rehearse=False):
    sh = Shooter(out, round_name, rehearse)
    ui({"action": "init"})
    ready()
    vp = viewport()
    if vp["dpr"] != "2":
        if not rehearse:
            sys.exit(f"devicePixelRatio 是 {vp['dpr']}，不是 2：窗口可能不在点亮的 Retina 显示器上，先处理再拍")
        print(f"演练：devicePixelRatio 是 {vp['dpr']}，不截图，只走流程", file=sys.stderr)
    state, items = plan(round_name)
    only = [part for part in os.environ.get("BLAZAR_AUDIT_ONLY", "").split(",") if part]
    if only:
        items = [it for it in items if any(it[0] == o or it[0].startswith(o + "-") for o in only)]
        if not items:
            sys.exit(f"BLAZAR_AUDIT_ONLY 没有匹配到任何项：{','.join(only)}")
    spawned = []
    preview = None
    if round_name == "data":
        for label, secs in zip(JOB_LABELS, (1800, 2400)):
            try:
                j = call("POST", "/api/jobs", {"workspace_id": WS["quasar-ml"], "command": f"echo 等待中; sleep {secs}",
                                               "label": label, "notify": "none"})
                spawned.append(j.get("id"))
            except Exception as e:
                print(f"  ! 派后台任务失败：{e}", file=sys.stderr)
        preview = subprocess.Popen(["python3", "-u", "-m", "http.server", "18731", "--bind", "localhost"],
                                   cwd=str(DEMO / "atlas-api" / "public"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)
    try:
        for t in ("dark", "light"):
            theme(t)
            for no, (slug, route, expect, steps, scroll) in enumerate(items, 1):
                try:
                    sh.page(no, slug, t, route, expect, state, steps, scroll)
                except Exception as e:
                    print(f"  ! {slug}-{t} 没截到：{e}", file=sys.stderr)
                    sh.issues.append(f"{no:02d}-{slug}-{t}.png\t没截到\t{e}")
                left = dismiss()
                if left:
                    print(f"  ! {slug}-{t}：拍完后收不起来的{'、'.join(left)}", file=sys.stderr)
                    sh.issues.append(f"{no:02d}-{slug}-{t}.png\t弹层没关掉\t{'、'.join(left)}")
        theme("system")
    finally:
        if preview:
            preview.terminate()
            preview.wait()
        for jid in spawned:
            if jid:
                try:
                    call("POST", f"/api/jobs/{jid}/kill", {})
                except Exception as e:
                    print(f"  ! 停后台任务 {jid} 失败：{e}", file=sys.stderr)
        sh.save(vp)
        index(Path(out))
    if sh.broken:
        sys.exit(f"有 {sh.broken} 处演示数据没有复原，见 index.md")


def index(out):
    files, lines, issues = [], [], []
    vp, size = None, None
    for round_name in ("empty", "data"):
        m = out / round_name / "manifest.json"
        if not m.exists():
            continue
        data = json.loads(m.read_text())
        vp = vp or data.get("viewport")
        lines.append(f"\n## {round_name}/\n")
        lines.append("| 文件 | 路由 | 画面里应当有什么 | 数据状态 | 溢出 | 控制台错误 | 警告 |")
        lines.append("| --- | --- | --- | --- | --- | --- | --- |")
        for e in data["entries"]:
            files.append(e["file"])
            size = size or e.get("size")
            lines.append(f"| `{e['file']}` | `{e['route']}` | {e['expect']} | {e['state']} | {e['overflow']} | {e['errors']} | {e.get('warning') or ''} |")
        issues += [f"- {round_name}/{i}".replace("\t", " · ") for i in data["issues"]]
    (out / "files.txt").write_text("\n".join(files) + "\n")
    head = ["# 界面审计截图索引", ""]
    if vp and size:
        head.append(f"视口：innerWidth×innerHeight = {vp['inner'][0]}×{vp['inner'][1]}，devicePixelRatio {vp['dpr']}，"
                    f"截图 {size[0]}×{size[1]} 像素（只含页面可见区域）。")
    head.append("")
    tail = ["", "## 驱动发现的溢出和控制台错误", ""] + (issues or ["（无）"])
    logs = [p.relative_to(out) for p in sorted(out.glob("*/console-errors.log"))]
    if logs:
        tail += ["", "控制台错误的完整文本（含堆栈）：" + "、".join(f"`{p}`" for p in logs)]
    (out / "index.md").write_text("\n".join(head + lines + tail) + "\n")


METRIC_ROUTES = [
    ("console", "/"), ("chats", "/chats"), ("workspaces", "/workspaces"), ("tasks", "/tasks"), ("bots", "/bots"),
    ("agents", "/agents"), ("runtimes", "/runtimes"), ("nodes", "/nodes"), ("usage", "/usage"), ("apps", "/apps"),
    ("settings", "/settings?s=appearance"), ("settings-notify", "/settings?s=notify"),
    ("chat-done", f"/chats/{CHATS['status']}"), ("chat-approval", f"/chats/{CHATS['handoff']}"),
    ("ws-atlas-api", f"/w/{WS['atlas-api']}"), ("ws-quasar-ml", f"/w/{WS['quasar-ml']}"),
]
METRIC_KEYS = ("fontSize", "fontWeight", "color", "background", "radius", "shadow", "border")


def metrics(out):
    res = {}
    ui({"action": "init"})
    ready()
    print("页面\t" + "\t".join(METRIC_KEYS))
    for t in ("dark", "light"):
        theme(t)
        for slug, route in METRIC_ROUTES:
            ui({"action": "navigate", "path": route, "settle_ms": 900})
            m = ui({"action": "metrics", "top": 8})
            res[f"{slug}-{t}"] = m
            print(f"{slug}-{t}\t" + "\t".join(str(m[k]["distinct"]) for k in METRIC_KEYS))
    theme("system")
    Path(out).write_text(json.dumps(res, ensure_ascii=False, indent=1))


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "seed":
        seed()
    elif cmd == "clean":
        unseed()
    elif cmd == "shots" and len(sys.argv) >= 4 and sys.argv[3] in ("empty", "data") and sys.argv[4:] in ([], ["--rehearse"]):
        shots(sys.argv[2], sys.argv[3], rehearse=len(sys.argv) == 5)
    elif cmd == "index" and len(sys.argv) == 3:
        index(Path(sys.argv[2]))
    elif cmd == "metrics" and len(sys.argv) == 3:
        metrics(sys.argv[2])
    elif cmd == "stress" and len(sys.argv) <= 3:
        stress(int(sys.argv[2]) if len(sys.argv) == 3 else 30)
    else:
        sys.exit("用法：ui_audit.py seed | clean | shots <目录> empty|data [--rehearse] | index <目录> | metrics <文件> | stress [轮数]")


if __name__ == "__main__":
    main()
