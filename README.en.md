[简体中文](README.md) · **English**

# Blazar

One desktop workspace for code and AI coding agents on local and remote machines.

[![CI](https://img.shields.io/badge/CI-GitHub%20Actions-2088FF?logo=githubactions&logoColor=white)](.github/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.90-orange?logo=rust)](rust-toolchain.toml)

![Workspace with a file tree, editor, change review, and agent conversation](assets/workspace.png)

## The problem it solves

Code, terminals, and agent conversations are spread across machines, so switching machines also means switching context.
Blazar brings editing, conversations, change review, and PR creation for local and SSH workspaces into one window.

## Install

macOS: download the Apple Silicon or Intel `.dmg` from [Releases](../../releases), open it, drag Blazar into Applications, and launch it.
Current macOS builds are neither Developer ID signed nor notarized. After verifying the download source, if macOS blocks the first launch, run:

```sh
xattr -dr com.apple.quarantine /Applications/Blazar.app
```

Linux builds target x86_64 `.deb` / `.AppImage`; Windows x86_64 `.msi` builds are experimental. Check Releases for the assets actually available.
To run an agent, the machine running the task needs Claude Code, Codex, or another supported CLI. Blazar does not bundle these CLIs. When a remote machine is missing Claude Code or Codex, Blazar installs the official, checksum-verified release (matching your local version) into that machine's `~/.blazar`, the way Cursor installs its remote server; if the remote has no internet access, your local machine downloads it and copies it over. Other CLIs must be installed and signed in manually.

## Get started

1. **Add a machine**: use the local machine, or add an SSH host in Machines & Networking.
2. **Open a workspace**: select New workspace, choose the machine and project directory, and create a Git worktree if you need a separate branch.
3. **Give work to an agent**: choose a runtime, send a task, handle approvals in the conversation, and review the code changes.

## Multiple machines

Blazar uses EasyTier to connect machines. Join a network with a `.blazar` invite file or import an existing EasyTier configuration. The network service runs independently and requires administrator permission to enable; code and agents run on the selected machine.

## Integrations

- **Claude Code, Codex, and other CLIs**: run on the workspace machine; supported models, permissions, and session features vary by CLI.
- **Feishu / Lark**: connect through `lark-cli`, then opt in to agent read or write access for documents and other office content.
- **GitHub**: create PRs and view their status through `gh` on the workspace machine, or open the browser to create a PR.
- **Obsidian**: open a vault as a workspace, preview Markdown, and save tasks as notes.
- **Isaac Sim, MuJoCo**: pick a workspace and Blazar finds the software on that machine (the workspace's Python environment, a conda environment, or a path you set), launches it there, and streams that machine's display back with hardware encoding. If the software is missing, Blazar can install it: it creates its own Python environment on that machine, installs MuJoCo from PyPI or Isaac Sim from NVIDIA's package index (which requires accepting NVIDIA's license). A desktop session of yours is used directly; a headless machine needs a one-time root run of a script Blazar generates to set up a virtual display. When ffmpeg is missing, a checksum-verified static build is installed into that machine's `~/.blazar`.

## Resident bots

A bot is a persistent identity inside the hub: a duty (an agent profile), a home workspace, a mailbox, the resources it has claimed, and its conversation threads. A process only starts when there is work and exits when it is done.

- **Temporary or fixed**: a bot can be spun up for one job with a single sentence of duty (the "new bot" form, or `spawn_bot` from the lead); it is archived after a day idle with its home workspace and conversation kept, and "promote" keeps it for good.
- **Work arrives as mail**: send a bot mail from its page, or let another bot call `send_message`. An idle bot wakes up and resumes its previous conversation; urgent mail is injected into the turn it is running. Bots answer with `reply` and wait with `wait_reply`.
- **Lead and workers**: a `lead` can dispatch to any workspace; a `worker` can only message bots, read status and facts, and operate the workspaces it has claimed. The hub attaches the caller's identity to the orchestration tools, mail between bots names its sender, and only mail from the user counts as an instruction.
- **Facts**: bots publish the state they own with `facts_put` and refresh it on a heartbeat; two missed intervals mark a fact stale, five mark it lost and raise an alert. External scripts report through `POST /api/facts/report` with a token generated in the settings.
- **Claims**: `claim gpu:0,1`, `dir:/data/queue`, or `port:8766`; an overlapping exclusive claim is refused with the name of the owner to talk to. Claiming a whole workspace gives the bot remote hands on that machine.
- **Shared memory**: `memory_write` compares against existing entries first; a duplicate name or a very similar description is refused and the existing entry returned, and updates must carry its version.
- **Always-on hub**: run the hub on a machine that stays online, register it with `scripts/hub-service.sh install` as a launchd or systemd service bound to loopback only, and reach it from a laptop with `ssh -N -L 7777:127.0.0.1:7777 hub-host` and `http://localhost:7777`. The token lives in `hub.token` next to the database and survives restarts; point the CLI at it with `BLAZAR_HUB=http://127.0.0.1:7777 BLAZAR_HUB_TOKEN=<token>`.
- **Moving to the always-on hub**: on the laptop, export the configuration (agent profiles, skills, MCP servers, automations, approval rules and the like, without conversation history) from Settings → Data & About → "迁移配置" → "导出" or with `blazar export`, then import it on the hub machine with "导入" on the same card or `blazar import <file>`, reviewing the dry-run report before confirming; `--local-node <node>` places workspaces from the laptop itself on a node of the new hub. Sign in to the accounts again on the hub machine and fill in the secret values that the export leaves empty (environment variables, MCP env and headers). Imported automations arrive paused: stop them on the old hub first, then enable them on the new one so they do not run twice.
- **Desktop on a remote hub**: in Settings → Data & About → "这台电脑连哪个 hub", choose the remote machine, enter its host alias from `~/.ssh/config` and the port (7777 by default), test it, then switch and restart. The app stops running its own hub, keeps an SSH tunnel up itself (reconnecting when it drops), and the Dock badge and account menu follow the remote hub. The laptop must be able to log in to that machine over SSH without a password. To go back, use Blazar → "改用本机 hub 并重启" in the menu bar, which works whatever version the remote hub runs; the choice is stored in `hub.json` in the data directory, and deleting it also returns to the local hub.

## Architecture

| Directory | Responsibility |
| --- | --- |
| [apps/desktop](apps/desktop) | Tauri desktop entry point with an embedded local Hub |
| [crates/web](crates/web) | Leptos / WebAssembly UI |
| [crates/hub](crates/hub), [crates/db](crates/db) | HTTP / WebSocket APIs and SQLite data |
| [crates/runtime](crates/runtime), [crates/transport](crates/transport) | Agent startup and local / SSH execution |
| [crates/netmesh](crates/netmesh) | Management of the separate EasyTier service |
| [apps/cli](apps/cli), [crates/mcp](crates/mcp) | CLI and MCP entry points |

## Contributing

1. Keep code free of comments; put explanations in commit bodies or the README.
2. Use Conventional Commits: `type(scope): description`, with one logical change per commit.
3. Use Rust 1.90. Before submitting, run `leptosfmt --check crates/web/src` (formatting inside `view!`; run `leptosfmt` before `cargo fmt`), `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`; see [CI](.github/workflows/ci.yml) for the full checks. For UI changes, run `trunk build --release` in `crates/web` before `cargo build -p blazar-hub`.
4. Use generic fixtures such as `hub-host`, `gpu-1`, `10.99.0.1`, and `/home/me`. Do not commit personal environment details or credentials.
5. Only append database migrations; never change existing migrations.

## Security model

- The Hub binds to loopback by default. APIs and WebSockets validate sessions and origins; webhooks use separate tokens. Local processes are within the trust boundary. Do not expose the Hub directly to the public internet.
- Workspace settings and conversation records are stored in SQLite in the local data directory. Check backups and exports for sensitive content.
- Accounts use the corresponding CLI login configuration. User-provided Claude long-lived tokens are also saved locally and can be used for remote tasks through a credential proxy.
- Agents read and write files, run commands, and may send data to model providers or integrated software according to the selected permission mode. These modes do not provide a common operating-system sandbox.
- EasyTier invite files contain network credentials; share them only with trusted recipients. Installing the network service requires administrator permission.

## License

Blazar is licensed under [MIT](LICENSE); see [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES.md) for bundled third-party software.
