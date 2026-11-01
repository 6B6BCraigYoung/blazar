import os
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
argv = sys.argv[1:]

if argv[:1] == ["-G"]:
    host = argv[-1]
    print(f"host {host}\nhostname {host}\nuser demo\nport 22")
    sys.exit(0)
if "-O" in argv:
    sys.exit(0)


def tunnel(spec, host_name):
    import socket
    _, remote, _, local = spec.split(":")
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        server.bind(("127.0.0.1", int(remote)))
    except OSError as e:
        print(f"Error: remote port forwarding failed for listen port {remote}: {e}", file=sys.stderr)
        sys.exit(255)
    server.listen(16)
    server.settimeout(0.5)

    def pipe(a, b):
        try:
            while True:
                data = a.recv(65536)
                if not data:
                    break
                b.sendall(data)
        except OSError:
            pass
        for c in (a, b):
            try:
                c.close()
            except OSError:
                pass

    while not ((ROOT / "offline" / host_name).exists() or (ROOT / "offline" / "all").exists()):
        try:
            conn, _ = server.accept()
        except socket.timeout:
            continue
        up = socket.create_connection(("127.0.0.1", int(local)))
        threading.Thread(target=pipe, args=(conn, up), daemon=True).start()
        threading.Thread(target=pipe, args=(up, conn), daemon=True).start()
    print(f"Connection to {host_name} closed by remote host.", file=sys.stderr)
    sys.exit(255)


if "-N" in argv and "-R" in argv:
    tunnel(argv[argv.index("-R") + 1], argv[-1])

if "--" in argv:
    rest = argv[argv.index("--") + 1:]
else:
    rest, skip = [], False
    for a in argv:
        if skip:
            skip = False
        elif a in ("-o", "-p", "-i", "-F", "-L", "-R", "-D", "-W", "-J", "-l"):
            skip = True
        elif a.startswith("-"):
            continue
        else:
            rest.append(a)
host, command = rest[0], " ".join(rest[1:])
home = ROOT / "remote" / host


def offline():
    return (ROOT / "offline" / host).exists() or (ROOT / "offline" / "all").exists()


if offline():
    time.sleep(1)
    print(f"ssh: connect to host {host} port 22: Operation timed out", file=sys.stderr)
    sys.exit(255)

env = dict(os.environ, HOME=str(home), PATH=f"{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
child = subprocess.Popen(["bash", "-c", command or "bash -l"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env, cwd=str(home), start_new_session=True)


def relay(src, fd):
    try:
        while True:
            chunk = os.read(src.fileno(), 65536)
            if not chunk:
                break
            os.write(fd, chunk)
    except OSError:
        pass


relays = [threading.Thread(target=relay, args=(child.stdout, 1), daemon=True),
          threading.Thread(target=relay, args=(child.stderr, 2), daemon=True)]
for t in relays:
    t.start()


def pump():
    try:
        while True:
            chunk = os.read(0, 65536)
            if not chunk:
                break
            child.stdin.write(chunk)
            child.stdin.flush()
    except (OSError, ValueError):
        pass
    try:
        child.stdin.close()
    except OSError:
        pass


threading.Thread(target=pump, daemon=True).start()
while child.poll() is None:
    if offline():
        try:
            child.stdin.close()
        except OSError:
            pass
        os.write(2, f"Connection to {host} closed by remote host.\n".encode())
        os._exit(255)
    time.sleep(0.2)
for t in relays:
    t.join(timeout=5)
sys.exit(child.returncode)
