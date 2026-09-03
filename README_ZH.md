<div align="center">

# CC Switch — 远程服务器扩展版

### 基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 的独立 fork

[![Version](https://img.shields.io/github/v/release/xh20220630/cc-switch-agent?color=blue&label=version)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey.svg)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

[English](README.md) | 中文 | [日本語](README_JA.md) | [Deutsch](README_DE.md)

</div>

---

## 这是什么

本仓库是 [CC Switch](https://github.com/farion1231/cc-switch)（MIT）的独立 fork。上游是一个用 Tauri 2 构建的桌面应用，统一管理 Claude Code、Claude Desktop、Codex、Gemini CLI、Grok Build、OpenCode、OpenClaw、Hermes 等工具的多家 API 配置，支持一键切换、MCP/Prompts/Skills 管理、代理与故障转移、用量统计、云同步等。

**上游的完整功能与介绍请直接查看上游仓库：**

- 项目主页与文档：<https://github.com/farion1231/cc-switch>
- 用户手册：<https://github.com/farion1231/cc-switch/tree/main/docs/user-manual>
- 更新日志：<https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md>

本 fork **在保留上游能力的基础上**，新增了「远程服务器模式」：通过 SSH 连接远程 Linux 服务器，在远端运行临时 Agent，直接读写远端 `~/.cc-switch` 数据库，把远端 Claude/Gemini/OpenCode/Grok/Codex 的会话用量导入并展示在桌面端，并支持本地与远端之间的 Provider 同步。

---

## 本 fork 新增内容

### 远程服务器模式（Remote Runtime）

很多用户把 Claude Code / Codex / Gemini 跑在**远端 Linux 服务器**上（云主机、容器、开发机），而桌面端只管本机配置。本 fork 让桌面端直连远端，把「计算核心」搬运过去：

- **SSH 目标管理** — 从 `~/.ssh/config` 自动发现目标，或手动添加；连接测试、编辑、删除
- **临时 Agent 随连随走** — 桌面端内嵌 musl 静态编译的 Linux Agent（x86_64 / aarch64 双架构），连接时 scp 上传到 `/tmp`，`--stdio` 运行，结束时自动删除自身（trap 清理 + 桌面端兜底清理）
- **直接读写远端数据库** — Agent 打开远端用户自己的 `~/.cc-switch/cc-switch.db`，行为与本机完全一致（同一套 Core 导入器），不是「拉取副本」
- **远端 Usage 同步** — 连接成功后自动同步一次，进入 Usage 页面兜底再同步一次，可随时手动同步；导入 Claude / Gemini / OpenCode / Grok / Codex 五源会话日志到远端数据库
- **Provider 同步** — 本地与远端之间同步 Provider，带搜索、去重跳过、失败重试的同步对话框
- **运行时切换器** — 顶栏在「本机 / 远程目标」之间切换，实时状态指示（connecting / online / offline / reconnecting / incompatible）

```
┌─ 桌面端 (Windows / Linux) ─┐      SSH      ┌─ 远端服务器 (Linux) ───────────┐
│ Tauri 前端 UI               │   scp/ssh   │ 临时 Agent (cc-switch-agent)     │
│  appInvoke ─► remote_invoke │ ──────────► │  │ 打开远端 ~/.cc-switch/        │
│  RemoteRuntimeState         │ ◄────────── │  │  cc-switch.db                 │
│  内嵌 Agent 字节            │             │  │ 会话日志: ~/.claude ~/.gemini  │
└─────────────────────────────┘             │  │ ~/.local/share/opencode 等     │
                                            └──────────────────────────────────┘
```

### 安全设计

| 风险 | 对策 |
|---|---|
| 命令注入 | 远端命令全部由受控模板拼装（十六进制路径 / 十进制长度 / SHA-256），用户输入不进 shell；SSH 参数经 `OsString` 数组直传，不经过本地 shell |
| 越权命令 | 能力白名单集中定义（`cc-switch-protocol`，27 个命令），Agent 握手时回传，桌面端 `require()` 校验，未登记的本地命令禁止远程执行 |
| 串台（切目标后旧响应） | generation 令牌双检 + 前端查询缓存按 scope 隔离，目标切换期间的迟到响应被拒绝 |
| 密码泄露 | 密码仅作为本次连接的内存输入，永不写入 `remote-targets.json`；Windows 可选 DPAPI（CryptProtectData）加密保存到系统安全存储，SSH_ASKPASS 辅助无 TTY 认证 |
| 主机密钥 | 首次连接 `ssh-keyscan` 展示指纹，用户确认后才写入 `known_hosts`（XShell 式） |
| 远端残留 | Agent 自 trap 删除 + 桌面端 Drop 时兜底 SSH 删除 |
| 日志污染 stdout | Agent 只把诊断写 stderr，协议帧带 magic 提前识别串流 |
| 超时卡死 | 每个命令有白名单超时（30s~300s），超时发 Cancel（带 requestId + operationId 防误伤） |

### 远端支持范围

- **远端服务器**：仅 Linux（`uname -s` 预检），架构 x86_64 或 aarch64
- **桌面端**：Windows（x86_64 / ARM64）、Linux（x86_64 / ARM64）
- 远端被读取的数据：远端 `$HOME` 下的 `~/.claude`、`~/.gemini`、`~/.local/share/opencode`、`~/.grok`、`~/.codex` 会话文件及 `~/.cc-switch/` 数据库；桌面端本机文件不会被上传或读取

### 与上游的差异

| 项 | 上游 | 本 fork |
|---|---|---|
| 远程服务器模式 | 无 | ✅ 新增（本仓库核心价值） |
| Provider 同步 | 无 | ✅ 新增 |
| 构建平台 | Windows / macOS / Linux | Windows x64/ARM64 + Linux x64/ARM64 |
| macOS 签名/公证 | Apple Developer ID + notarization | 无（fork 无 Apple 证书，不构建 macOS） |
| 自动更新签名/端点 | 上游密钥与仓库 | 指向 fork 自身签名密钥与本仓库 |
| 版本号 | 3.x 跟随上游 | 0.0.1 独立发版 |

---

## 安装与下载

从 [Releases](https://github.com/xh20220630/cc-switch-agent/releases) 下载最新版：

| 平台 | 文件 |
|---|---|
| Windows (x86_64) | `CC-Switch-{version}-Windows.msi`（安装版）/ `Windows-Portable.zip`（绿色版） |
| Windows (ARM64) | `CC-Switch-{version}-Windows-arm64.msi` / `Windows-arm64-Portable.zip` |
| Linux (x86_64) | `CC-Switch-{version}-Linux-x86_64.AppImage` / `.deb` / `.rpm` |
| Linux (ARM64) | `CC-Switch-{version}-Linux-arm64.AppImage` / `.deb` / `.rpm` |

> 本 fork 不提供 macOS 构建（无 Apple 签名证书）。macOS 用户请使用上游发布版。
> `.tar.gz` 为 Tauri updater 自动更新专用，无需手动下载。

## 快速开始：连接远程服务器

1. **准备远端** — 一台 Linux 服务器（x86_64 / aarch64），开 SSH，能访问远端用户 `~/.cc-switch` 及各会话目录
2. **添加目标** — 设置 → Remote Targets，可自动扫描 `~/.ssh/config` 发现目标，或手动填写主机 / 用户 / 端口
3. **连接** — 首次连接会 `ssh-keyscan` 展示服务器公钥指纹，确认后才写入 `known_hosts`；密码仅用于本次连接（Windows 可选加密保存）
4. **切换运行时** — 顶栏运行时切换器选到目标，连接成功后远端 Usage 自动同步一次；进入 Usage 页面兜底再同步；之后可随时点「同步会话」
5. **Provider 同步** — 在 Provider 同步对话框中把本地 Provider 推到远端，或从远端拉取，支持搜索与去重

> 远端没有 60 秒常驻轮询：Agent 是随连接诞生的临时进程，同步时机为「连接成功 + 页面进入 + 手动」。

## 常见问题

<details>
<summary><strong>连接远端后 Usage 数据什么时候出现？</strong></summary>

连接成功的瞬间后端自动同步一次；进入 Usage 页面再兜底同步一次；之后可随时点「同步会话」按钮。远端没有 60 秒常驻轮询。

</details>

<details>
<summary><strong>远端会读取我本机的文件吗？</strong></summary>

不会。远端 Agent 只打开远端 `$HOME` 下的会话文件与 `~/.cc-switch/` 数据库；桌面端本机文件不会被上传或读取。

</details>

<details>
<summary><strong>密码存在哪里？</strong></summary>

连接密码只作为本次连接的内存输入，永不写入 `remote-targets.json`。Windows 上可选 DPAPI 加密保存到 `~/.cc-switch/remote-credentials.json`；非 Windows 平台暂不提供凭据保存。

</details>

<details>
<summary><strong>为什么没有 macOS 版本？</strong></summary>

fork 没有 Apple Developer ID 证书，无法签名/公证 macOS 应用。需要 macOS 版请用上游：<https://github.com/farion1231/cc-switch/releases>

</details>

## 文档

- **远程同步实现说明**：`docs/remote-sync-architecture-zh.md`
- **本地/远端请求隔离设计**：`docs/remote-route-isolation-design.md`
- 上游功能的手册、FAQ 与更新日志见上游仓库：<https://github.com/farion1231/cc-switch>

## 开发

### 环境要求

- Node.js 18+，pnpm 8+
- Rust 1.85+，Tauri CLI 2.8+

### 命令

```bash
pnpm install        # 安装依赖
pnpm dev            # 开发模式（热更新）
pnpm typecheck      # 前端类型检查
pnpm test:unit      # 前端单元测试
cd src-tauri
cargo test          # 后端测试
pnpm tauri build    # 构建应用
```

> 远程 Agent 是 musl 静态编译的 Linux 二进制（x86_64 / aarch64），由 `src-tauri/build.rs` 内嵌进桌面包；GitHub Actions 的 release workflow 负责交叉编译并打包。

## 许可证

MIT © Jason Young（上游）— 本 fork 在 [MIT License](LICENSE) 下分发。
