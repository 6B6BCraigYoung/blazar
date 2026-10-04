**简体中文** · [English](README.en.md)

# Blazar

把本地和远端机器上的代码与 AI 编码智能体放进同一个桌面工作台。

[![CI](https://img.shields.io/badge/CI-GitHub%20Actions-2088FF?logo=githubactions&logoColor=white)](.github/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.90-orange?logo=rust)](rust-toolchain.toml)

![工作区：文件树、编辑器、改动审阅与智能体对话](assets/workspace.png)

## 它解决什么问题

代码、终端和智能体会话分散在多台机器上，切换机器时也要切换上下文。
Blazar 在一个窗口里提供本地与 SSH 工作区的编辑、对话、改动审阅和创建 PR 入口。

## 安装

macOS：从 [Releases](../../releases) 下载适合 Apple Silicon 或 Intel 的 `.dmg`，打开后把 Blazar 拖入“应用程序”，再启动应用。
当前 macOS 构建未使用 Apple Developer ID 签名，也未公证；确认下载来源后，若首次启动被系统拦截，可运行：

```sh
xattr -dr com.apple.quarantine /Applications/Blazar.app
```

Linux 的构建目标为 x86_64 `.deb` / `.AppImage`；Windows x86_64 `.msi` 为实验性构建，下载以 Releases 实际附件为准。
要运行智能体，请先在执行任务的机器上安装并登录 Claude Code、Codex 等 CLI；Blazar 不包含这些 CLI。

## 开始使用

1. **加一台机器**：直接使用本机，或在“机器与组网”中添加 SSH 主机。
2. **打开一个工作区**：点击“新建工作区”，选择机器和项目目录；需要独立分支时创建 Git worktree。
3. **把活交给智能体**：选择运行时并发送任务，在对话中处理审批，再审阅代码改动。

## 多机器

Blazar 使用 EasyTier 连接机器，支持通过 `.blazar` 邀请文件加入网络，也能导入已有 EasyTier 配置。组网服务独立运行；启用服务需要管理员权限，代码与智能体仍在选定的机器上运行。

## 接入的软件

- **Claude Code、Codex 等 CLI**：在工作区所在机器运行；各 CLI 支持的模型、权限和会话能力不同。
- **飞书**：通过 `lark-cli` 接入，按需开启智能体读或写权限，处理文档等办公内容。
- **GitHub**：通过工作区机器上的 `gh` 创建 PR 并查看状态；也可跳转网页创建 PR。
- **Obsidian**：把笔记库作为工作区，预览 Markdown，并将任务保存为笔记。

## 架构

| 目录 | 职责 |
| --- | --- |
| [apps/desktop](apps/desktop) | Tauri 桌面入口，内嵌本机 Hub |
| [crates/web](crates/web) | Leptos / WebAssembly 界面 |
| [crates/hub](crates/hub)、[crates/db](crates/db) | HTTP / WebSocket API 与 SQLite 数据 |
| [crates/runtime](crates/runtime)、[crates/transport](crates/transport) | 智能体启动与本机 / SSH 执行 |
| [crates/netmesh](crates/netmesh) | 管理独立运行的 EasyTier 服务 |
| [apps/cli](apps/cli)、[crates/mcp](crates/mcp) | 命令行与 MCP 入口 |

## 参与贡献

1. 代码零注释，解释写进提交正文或 README。
2. 使用 Conventional Commits：`type(scope): description`，一个逻辑改动一个提交。
3. 使用 Rust 1.90，提交前运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和 `cargo test --workspace`；完整检查见 [CI](.github/workflows/ci.yml)。前端改动先在 `crates/web` 执行 `trunk build --release`，再执行 `cargo build -p blazar-hub`。
4. 测试和示例使用通用值，例如 `hub-host`、`gpu-1`、`10.99.0.1`、`/home/me`，不提交个人环境信息或凭据。
5. 数据库迁移只追加，不修改已有迁移。

## 安全模型

- Hub 默认只监听本机；API 与 WebSocket 校验会话和来源，Webhook 使用独立令牌。本机进程属于信任边界内，不要直接将 Hub 暴露到公网。
- 工作区设置和会话记录存储在本机数据目录的 SQLite 中；备份与导出前应检查其中的敏感内容。
- 账号使用对应 CLI 的登录配置；用户提供的 Claude 长期令牌也会保存到本机，并可通过凭据代理用于远端任务。
- 智能体按所选权限模式读写文件、执行命令，并可能向模型或接入的软件发送数据；这些模式不构成统一的系统沙箱。
- EasyTier 邀请文件包含入网凭据，应只交给可信接收者；安装组网服务需要管理员权限。

## 许可证

Blazar 使用 [MIT 许可证](LICENSE)；随包分发的第三方软件见 [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES.md)。
