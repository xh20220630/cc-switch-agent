# 远程服务器 CC Switch 同步实现说明

> 技术文档:远程模式(SSH 连接远程服务器)下的 Usage/Provider 数据如何"同步"。
> 阅读对象:开发者;相关设计文档见 `docs/superpowers/specs/` 下的
> `ssh-remote-runtime-design`、`ephemeral-ssh-agent-design`、`remote-command-safe-path-design`、
> `remote-provider-usage-parity-design`。

## 1. 总体思路:不是"文件同步",而是"搬运计算核心"

远程同步没有把本机数据库/配置文件推到服务器,而是:

1. 桌面端通过 SSH 把**一次性临时 Agent**(musl 静态编译的 Linux 二进制,双架构
   x86_64 / aarch64,随桌面安装包内嵌)上传到远端;
2. Agent 以 `--stdio` 运行,通过 **SSH stdin/stdout 管道** 与桌面端跑私有二进制协议;
3. Agent 打开**远端用户自己的** `~/.cc-switch` 数据库,直接执行查询/写入;
4. "同步会话用量" = Agent 在远端把 Claude/Gemini/OpenCode/Grok/Codex 的会话日志
   导入远端数据库,与桌面端本机行为完全一致(同一套 Core 导入器)。

因此远程模式天然获得与本地完全一致的数据视图,而不是一个"拉取副本"。

```
┌─ 桌面端 (Windows/macOS/Linux) ─┐      SSH      ┌─ 远程服务器 (Linux) ─────────┐
│ Tauri 前端 UI                   │   ────────▶  │ 临时 Agent (cc-switch-agent)  │
│  appInvoke ──► remote_invoke    │   scp/ssh   │  │ 打开远端 ~/.cc-switch/      │
│  RemoteRuntimeState (generation)│◀────────    │  │   cc-switch.db              │
│  内嵌 Agent 字节 (include_bytes)│             │  │  会话日志:                  │
└─────────────────────────────────┘             │  │  ~/.claude ~/.gemini        │
                                                │  │  ~/.local/share/opencode    │
                                                │  │  ~/.grok ~/.codex           │
                                                └────────────────────────────────┘
```

## 2. 一次连接的完整时序

```
用户点击"连接"(remote_set_active_target)
│
├─ 1. preflight (ssh.rs::preflight)
│      ssh -o BatchMode=... -- host 'uname -s; uname -m'
│      → 必须是 Linux;识别 x86_64 / aarch64;错误分类:
│        HOST_KEY_NOT_TRUSTED / AUTH_FAILED / REMOTE_UNREACHABLE / ...
│
├─ 2. 选择 Agent (embedded_agent.rs)
│      按架构从 include_bytes! 内嵌字节中取 musl 静态二进制
│      (构建见 src-tauri/build.rs,release workflow 产物)
│
├─ 3. 上传 (ssh.rs::OpenSshSession::connect)
│      内嵌字节 → 本地随机临时文件 → scp 到 /tmp/cc-switch-agent-<uuid>
│      scp 失败时 EphemeralCleanupGuard 兜底删除远端残留
│
├─ 4. 启动 (ephemeral_deploy.rs::build_launch_command)
│      远端 bash:校验 wc -c 长度 + sha256sum → chmod 700 → 执行 --stdio
│      trap cleanup EXIT HUP INT TERM(Agent 退出自动删自身)
│      所有远端命令经过 with_remote_command_environment 锁定 PATH,
│      避免用户环境中的同名程序接管校验
│
├─ 5. 协议握手 (protocol.rs)
│      桌面 → Hello(appVersion, protocolMinor)
│      Agent → HelloAck(agentVersion, schemaVersion, platform,
│                        architecture, capabilities 白名单)
│      帧格式:magic "CCS1" + kind + major + payload_len + JSON payload
│
└─ 6. Runtime Online
       RemoteRuntimeState 快照 status=Online, generation=+1
       store 持久化 active_target_id;广播 remote-runtime-status 事件
```

## 3. 同步的三种触发路径

| 触发方式 | 本地模式 | 远程模式 |
|---|---|---|
| 后台自动 | 桌面进程内 60 秒轮询 + 启动时一次(`src-tauri/src/lib.rs` ~L1239) | 无常驻调度,连接成功时自动一次(`commands/remote.rs` `remote_set_active_target`) |
| 页面兜底 | — | UsageDashboard 进入页面每个 scope 自动同步一次(前端 ref 防重复) |
| 手动按钮 | `sync_session_usage` 本地命令 | 同一个 API 经 `appInvoke` 桥接自动走 `remote_invoke` |

关键点:远端 Agent 是**临时进程**,没有常驻调度器,所以必须由桌面端在
连接成功后主动发起一次 `usage.session_sync`,否则远端库的 Usage 永远为空。
`remote_set_active_target` 中该同步在独立任务执行,失败只记日志不影响切换结果。

## 4. 前端桥接:local 与 remote 的分流

`src/lib/runtime/invoke.ts::appInvoke`:

```ts
const runtime = getRuntimeSnapshot();
if (runtime.status === "local") return localInvoke(localCommand, args);
if (runtime.status !== "online") throw REMOTE_OFFLINE;
if (!options.remoteCommand) throw COMMAND_NOT_EXPOSED; // 未登记的本地命令禁止远程
return localInvoke("remote_invoke", { command: options.remoteCommand, args, generation });
```

- 每个 Usage/Provider API 都要显式声明 `remoteCommand`,未声明就拒绝远程执行;
- 请求携带调用时的 `generation`,后端发送前/响应后各校验一次
  (`runtime.rs::invoke_remote` 的 `require_generation` 双检),目标切换期间的
  迟到响应会被拒绝,绝不降级到本机执行;
- 前端查询缓存按 scope 隔离:`["local", gen]` / `["remote", targetId, gen]`,
  切目标自动丢弃旧缓存(`queryScope.ts`)。

## 5. 同步的具体内容(cc-switch-core)

命令入口 `usage.session_sync`,能力定义在 `cc-switch-protocol/src/capabilities.rs`
(27 个命令:provider.* / usage.*,含幂等性与超时)。执行体:

```
sync_sessions(state, cancellation)
├─ 进程级互斥锁 session_sync_guard(防止并发 worker 交错)
├─ sync_non_codex_sessions
│   ├─ Claude:   ~/.claude/projects/**/*.jsonl   (深度8, assistant 消息 usage)
│   ├─ Gemini:   ~/.gemini/tmp/session-*.json    (messages[].tokens)
│   ├─ OpenCode: ~/.local/share/opencode/opencode.db (只读 SQLite 查询)
│   └─ Grok:     ~/.grok/{sessions,archived_sessions}/**/updates.jsonl
│                (10 分钟 settle 窗口避免读到半截写入)
├─ sync_codex_sessions
│   └─ ~/.codex/sessions/** + archived_sessions/**/*.jsonl
└─ backfill_missing_costs(全部来源导入后统一按模型定价回填成本)
```

要点:

- **增量**:`session_log_sync` 表按文件记录 `last_modified`/`last_line_offset`,
  未变化的文件跳过(`watermark_requires_scan`);
- **去重**:导入前 `has_matching_proxy_usage` 按
  (app_type, model, 各类 tokens, created_at ±10 分钟)与代理计费行比对,
  已由代理记账的请求跳过,避免双算;Grok 因逐轮聚合只能按时间窗内是否有
  代理活动保守跳过;
- **upsert**:`INSERT ... ON CONFLICT(request_id) DO UPDATE`,幂等可重放;
- **取消**:所有循环点 `cancellation.check()`,SSH 断线/超时会取消整个同步。

## 6. 安全与可靠性设计

| 风险 | 对策 |
|---|---|
| 命令注入 | 远端命令全部由受控模板拼装(十六进制路径/十进制长度/SHA-256),用户输入不进 shell;ssh 参数经 `OsString` 数组直传,不经过本地 shell |
| 越权命令 | 能力白名单集中定义,Agent 握手回传,桌面 `require()` 校验 |
| 串台(切目标后旧响应) | generation 令牌双检 + 前端 scope 隔离 |
| 密码泄露 | 密码仅内存携带(不落 remote-targets.json),可选 DPAPI 加密存系统安全存储,SSH_ASKPASS 辅助无 TTY 认证 |
| 主机密钥 | 首次连接 ssh-keyscan 展示指纹,用户确认后才写 known_hosts(XShell 式) |
| 远端残留 | Agent 自 trap 删除 + 桌面 Drop 时兜底 SSH 删除(至多一次) |
| 日志污染 stdout | Agent 只把诊断写 stderr;协议帧带 magic 提前识别串流 |
| 超时卡死 | 每命令有白名单超时(30s~300s),超时发 Cancel(带 requestId+operationId 防误伤) |

## 7. 关键文件地图

| 文件 | 职责 |
|---|---|
| `src-tauri/src/remote/ssh.rs` | SSH/scp 连接、preflight、密码 askpass、主机密钥信任、错误分类 |
| `src-tauri/src/remote/ssh_config.rs` | 读取用户 `~/.ssh/config` 发现可导入目标 |
| `src-tauri/src/remote/ephemeral_deploy.rs` | 远端命令模板(preflight/launch/cleanup) |
| `src-tauri/src/remote/embedded_agent.rs` | 内嵌 Agent 字节目录 + SHA-256 |
| `src-tauri/src/remote/client.rs` | 桌面端协议客户端(请求/响应/事件/取消) |
| `src-tauri/src/remote/runtime.rs` | RemoteRuntimeState:快照/generation/凭据/会话管理 |
| `src-tauri/src/remote/target_store.rs` | remote-targets.json 持久化(落盘剥离密码) |
| `src-tauri/src/remote/credentials.rs` | Windows DPAPI 凭据存储 |
| `src-tauri/src/remote/capabilities.rs` | 重导出协议 crate 的能力白名单 |
| `src-tauri/crates/cc-switch-protocol/src/protocol.rs` | 帧格式/握手/RPC 结构 |
| `src-tauri/crates/cc-switch-protocol/src/capabilities.rs` | 27 个远程命令的定义 |
| `src-tauri/crates/cc-switch-agent/src/lib.rs` | Agent 会话循环:并发 worker、取消、能力应答 |
| `src-tauri/crates/cc-switch-core/src/usage/session.rs` | 五源会话导入器(远程/本地共用) |
| `src-tauri/crates/cc-switch-core/src/usage/mutation.rs` | sync_sessions/rebuild_codex/成本回填 |
| `src-tauri/src/commands/remote.rs` | Tauri 命令:list/upsert/test/connect/invoke/trust |
| `src-tauri/src/lib.rs` | 命令注册 + 本机 60 秒轮询调度 |
| `src/lib/runtime/invoke.ts` | 前端 local/remote 分流桥接 |
| `src/lib/runtime/queryScope.ts` | 查询缓存 scope 隔离 |
| `src/components/usage/UsageDashboard.tsx` | 手动同步按钮 + 远端自动同步兜底 |
| `src/components/remote/RemoteTargetsSettings.tsx` | 目标管理 UI(密码/信任主机密钥) |

## 8. 常见问题

**Q:远程连接成功后 Usage 数据什么时候出现?**
A:连接成功瞬间后端自动同步一次;进入 Usage 页面再兜底同步一次;之后可随时点
"同步会话"按钮。远端没有 60 秒常驻轮询。

**Q:为什么远程模式没有 60 秒自动同步?**
A:Agent 是随连接诞生的临时进程,不常驻;要保持自动同步需要保活一个持久 Agent,
当前设计是"按需同步"(连接时 + 页面进入时 + 手动)。

**Q:远端哪些数据被读取?**
A:远端 `$HOME` 下的 `~/.claude`、`~/.gemini`、`~/.local/share/opencode`、
`~/.grok`、`~/.codex` 会话文件,以及 `~/.cc-switch/` 数据库。桌面端本机文件不会被
上传或读取。
