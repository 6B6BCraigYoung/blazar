use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use blazar_transport::ExecSpec;

use crate::api::Shared;

use super::dev::dev_dir;

const LISTEN_RETRY: Duration = Duration::from_secs(10);

fn listen_script(dir: &str) -> String {
    format!(
        r#"D={dir}; [ -f "$D/pid" ] || exit 0
echo "__ROOT__ $(cat "$D/pid")"
echo __PS__
ps -eo pid=,ppid=
if [ "$(uname -s)" = Darwin ] || ! command -v ss >/dev/null 2>&1; then echo __LSOF__; lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null
else echo __SS__; ss -ltnpH 2>/dev/null; fi
exit 0"#
    )
}

pub fn descendants(root: u32, tree: &[(u32, u32)]) -> HashSet<u32> {
    let mut found = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in tree {
            if ppid == parent && found.insert(pid) {
                queue.push(pid);
            }
        }
    }
    found
}

fn parse_ps(s: &str) -> Vec<(u32, u32)> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit_once(':')?.1.parse().ok()
}

pub fn parse_lsof(s: &str) -> Vec<(u32, u16)> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 9 || !l.contains("(LISTEN)") {
                return None;
            }
            Some((f[1].parse().ok()?, port_of(f[f.len() - 2])?))
        })
        .collect()
}

pub fn parse_ss(s: &str) -> Vec<(u32, u16)> {
    let pids = |l: &str| {
        l.split("pid=")
            .skip(1)
            .filter_map(|p| {
                p.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<u32>()
                    .ok()
            })
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    let mut port = None;
    for l in s.lines() {
        if let Some(p) = l
            .strip_prefix("LISTEN")
            .and_then(|r| r.split_whitespace().nth(2))
            .and_then(port_of)
        {
            port = Some(p);
        }
        if let Some(p) = port {
            out.extend(pids(l).into_iter().map(|pid| (pid, p)));
        }
    }
    out
}

pub fn listening_ports(out: &str) -> Vec<u16> {
    let root: Option<u32> = out
        .lines()
        .find_map(|l| l.strip_prefix("__ROOT__"))
        .and_then(|r| r.trim().parse().ok());
    let section = |name: &str, end: &[&str]| -> String {
        let mut lines = out.lines().skip_while(|l| *l != name).skip(1);
        lines
            .by_ref()
            .take_while(|l| !end.contains(l))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(root) = root else {
        return Vec::new();
    };
    let tree = descendants(root, &parse_ps(&section("__PS__", &["__LSOF__", "__SS__"])));
    let sockets = if out.lines().any(|l| l == "__SS__") {
        parse_ss(&section("__SS__", &[]))
    } else {
        parse_lsof(&section("__LSOF__", &[]))
    };
    let mut ports: Vec<u16> = sockets
        .into_iter()
        .filter(|(pid, _)| tree.contains(pid))
        .map(|(_, port)| port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

struct Listen {
    pid: String,
    ports: Vec<u16>,
    at: Instant,
}

fn listens() -> &'static std::sync::Mutex<HashMap<String, Listen>> {
    static L: OnceLock<std::sync::Mutex<HashMap<String, Listen>>> = OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub(super) async fn listening(st: &Shared, node: &str, ws: &str, pid: &str) -> Vec<u16> {
    let cached = listens()
        .lock()
        .unwrap()
        .get(ws)
        .filter(|l| l.pid == pid && (!l.ports.is_empty() || l.at.elapsed() < LISTEN_RETRY))
        .map(|l| l.ports.clone());
    if let Some(ports) = cached {
        return ports;
    }
    let out = st
        .transport(node)
        .exec(
            ExecSpec::new("bash")
                .arg("-lc")
                .arg(listen_script(&dev_dir(ws))),
        )
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();
    let ports = listening_ports(&out);
    listens().lock().unwrap().insert(
        ws.to_owned(),
        Listen {
            pid: pid.to_owned(),
            ports: ports.clone(),
            at: Instant::now(),
        },
    );
    ports
}

pub(super) fn forget_listen(ws: &str) {
    listens().lock().unwrap().remove(ws);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_ports_come_from_the_dev_process_tree_on_macos() {
        let out = "__ROOT__ 500\n__PS__\n  500     1\n  501   500\n  502   501\n  777     1\n__LSOF__\n\
COMMAND   PID  USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n\
Python    502 craig    4u  IPv4 0xafaa59714fcb7f25      0t0  TCP 127.0.0.1:8765 (LISTEN)\n\
node      501 craig   22u  IPv6 0x5f6946e4331ab31a      0t0  TCP [::1]:5173 (LISTEN)\n\
node      501 craig   23u  IPv4 0x5f6946e4331ab31b      0t0  TCP *:24678 (LISTEN)\n\
node      501 craig   24u  IPv6 0x5f6946e4331ab31c      0t0  TCP *:24678 (LISTEN)\n\
rapportd  777 craig   16u  IPv4 0xae788ff549d6b72e      0t0  TCP *:52719 (LISTEN)\n";
        assert_eq!(listening_ports(out), vec![5173, 8765, 24678]);
        assert_eq!(
            listening_ports("__PS__\n1 0\n__LSOF__\n"),
            Vec::<u16>::new()
        );
    }

    #[test]
    fn listening_ports_come_from_the_dev_process_tree_on_linux() {
        let out = "__ROOT__ 41200\n__PS__\n41200 1\n41234 41200\n41301 41200\n41355 41301\n1000 1\n__SS__\n\
LISTEN 0      4096       127.0.0.1:8000       0.0.0.0:*    users:((\"python3\",pid=41234,fd=3))\n\
LISTEN 0      511                *:5173             *:*    users:((\"node\",pid=41301,fd=22))\n\
LISTEN 0      511          0.0.0.0:3000       0.0.0.0:*    users:((\"node\",pid=41301,fd=23),(\"node\",pid=41355,fd=23))\n\
LISTEN 0      128          0.0.0.0:22         0.0.0.0:*    users:((\"sshd\",pid=1000,fd=3))\n\
LISTEN 0      4096   [::ffff:127.0.0.1]:9229       *:*\n\
                                                             users:((\"node\",pid=41301,fd=19))\n\
LISTEN 0      128             [::]:22            [::]:*    users:((\"sshd\",pid=1000,fd=4))\n";
        assert_eq!(listening_ports(out), vec![3000, 5173, 8000, 9229]);
    }

    #[test]
    fn descendants_include_every_generation_but_not_siblings() {
        let tree = [
            (1, 0),
            (10, 1),
            (11, 10),
            (12, 11),
            (13, 11),
            (20, 1),
            (21, 20),
        ];
        assert_eq!(descendants(10, &tree), HashSet::from([10, 11, 12, 13]));
        assert_eq!(descendants(21, &tree), HashSet::from([21]));
        assert_eq!(descendants(99, &[(5, 99), (99, 5)]), HashSet::from([99, 5]));
    }
}
