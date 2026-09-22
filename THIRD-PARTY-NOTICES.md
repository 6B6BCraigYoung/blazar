# Third-party notices

Blazar's own code is released under the MIT License (see [LICENSE](LICENSE)). The components
below are **distributed with Blazar** under their own licenses. Rust dependencies compiled into
the binaries are listed in `Cargo.lock`; `cargo license` or `cargo about` produces a full report.

## Mesh engine: EasyTier

| | |
| --- | --- |
| What | Peer-to-peer networking (virtual interface, NAT traversal, relay, routing, encryption). Blazar's node discovery, invites and config import are built on it. |
| Version | 2.6.4 (`ENGINE_VERSION` in `crates/netmesh/src/engine.rs`) |
| License | **LGPL-3.0**. The full text ships next to the engine as `engine/LICENSE.txt`; the GPL-3.0 it references is at <https://www.gnu.org/licenses/gpl-3.0.html>. |
| Form | The upstream release binaries `easytier-core` / `easytier-cli`, **unmodified**. `scripts/fetch-easytier.sh` downloads them from the project's GitHub Releases and verifies pinned SHA-256 sums. |
| Source | <https://github.com/EasyTier/EasyTier/tree/v2.6.4> |
| Relationship | A **separate process** installed as a system service. Blazar talks to it over its command line and local RPC; it is not linked into Blazar. To substitute your own build, replace the two files under `engine/` in the application bundle before joining a network, or replace the copy in the system service directory and restart the service. |

## Vendored front-end libraries (`crates/hub/static/vendor/`)

| Component | Version | License | Source |
| --- | --- | --- | --- |
| Monaco Editor (`vs/`) | 0.52.2 | MIT | <https://github.com/microsoft/monaco-editor> |
| xterm.js (`xterm.js`, `xterm.css`, `addon-fit.js`) | — | MIT | <https://github.com/xtermjs/xterm.js> |
| IBM Plex Sans (`fonts/ibm-plex-sans-*`) | — | SIL OFL 1.1 (`fonts/OFL-ibm-plex.txt`) | <https://github.com/IBM/plex> |
| JetBrains Mono (`fonts/jetbrains-mono-*`) | — | SIL OFL 1.1 (`fonts/OFL-jetbrains-mono.txt`) | <https://github.com/JetBrains/JetBrainsMono> |

## Trademarks and product icons

The *Apps* section identifies integrated products with their own icons
(`crates/hub/static/vendor/brands/`). These marks belong to their owners: Feishu / Lark to
ByteDance / Lark Technologies, GitHub and its logo to GitHub, Inc., Obsidian to Dynalist Inc.
They are used only to indicate that Blazar can connect to those products and do not imply
endorsement or affiliation. The GitHub mark comes from [Octicons](https://github.com/primer/octicons)
(MIT; trademark use governed by [GitHub Logos and Usage](https://github.com/logos)). The Feishu
and Obsidian icons are the application icons. Redistributors who prefer not to carry these marks
can delete the directory and replace `APP_LOGO` in `crates/hub/static/js/office.js` with text.

## Called, not distributed

These are installed by the user; Blazar invokes the copy already on the machine with the user's
own login state: the agent CLIs (Claude Code, Codex, …); `git`, `ssh`, `curl`; `gh`
(GitHub CLI); `lark-cli` (Feishu / Lark); `npx` and `brew` (only when the user clicks *Install*);
Obsidian (Blazar reads only its vault registry `obsidian.json` and opens notes via `obsidian://`).
