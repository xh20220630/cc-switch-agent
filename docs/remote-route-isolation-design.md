# 远程路由隔离方案:base_url 路径前缀区分远程/本地请求

> 状态:已实现(2026-08-13)
> 背景:远程路由模式(SSH 隧道 + 桌面代理)已可用,但远程与本地共用同一
> 桌面代理端口与"唯一 current provider",两端切换会互相覆盖。

## 问题

当前链路:

```
本地 Claude Code → 127.0.0.1:15721(桌面)  ┐
                                           ├→ 桌面代理 → 按"唯一 current"转发
远程 Claude Code → SSH 隧道 → 127.0.0.1:15721(桌面) ┘
```

两个入口都打到桌面代理的同一端口,代理无法区分请求来自本地还是远程隧道
(SSH 转发后源地址均为 127.0.0.1),只能按当前 current provider 转发。
因此"本地选 DeepSeek、远程选 OpenCode Go"时,代理会跟随最后切换的一方,
两端互相干扰。

## 方案:base_url 路径前缀标记

远程投影时,把远程 live 的 base_url 写成带前缀的地址:

```
远程投影 base_url:  http://127.0.0.1:15721/remote
本地(桌面)base_url: http://127.0.0.1:15721(不变)
```

远程 Claude Code 请求路径:

```
远程: http://127.0.0.1:15721/remote/v1/messages   ← 带 /remote 前缀
本地: http://127.0.0.1:15721/v1/messages          ← 无前缀
```

桌面代理按路径前缀区分来源:

- 路径以 `/remote/` 开头 → **远程请求**:按"远程 current"选择 provider 转发,
  转发前剥离 `/remote` 前缀
- 否则 → **本地请求**:按"本地 current"选择 provider 转发

### 为什么不用自定义 header

Claude Code 客户端不支持注入自定义 HTTP header(请求头固定为
Authorization / anthropic-version / content-type / user-agent 等,
settings.json 的 env 不会变成请求头)。base_url 路径前缀是客户端原生支持
的等效"标记"手段,且随每个请求携带。

### 为什么不用双端口

双端口(15721 本地 / 15722 远程)也可行,但需要:
- 隧道参数改为 `-R 15722:127.0.0.1:15722`
- 桌面代理监听两个端口
- 路由逻辑按 listener 端口区分

路径前缀方案只需单端口、单 listener,改动更小。

## 实现要点

### 1. 远程投影改写(commands/remote.rs `maybe_rewrite_provider_switch_for_remote_proxy`)

当前改写逻辑把 base_url 写成 `http://127.0.0.1:{port}`。改为:

```rust
// 远程请求标记前缀
const REMOTE_ROUTE_PREFIX: &str = "/remote";

let proxy_base_url = format!("http://127.0.0.1:{}{}", status.port, REMOTE_ROUTE_PREFIX);
```

### 2. 桌面代理路由选择(proxy)

代理收到请求后,根据路径前缀选择 current:

```rust
// 伪代码:handler_context / provider_router 选择 provider 时
let is_remote = path.starts_with(REMOTE_ROUTE_PREFIX);
let provider = if is_remote {
    remote_current_provider(db)   // 远程同步的 current(已通过 sync_provider_to_local_proxy 维护)
} else {
    local_current_provider(db)    // 原逻辑
};
```

### 3. 远程 current 的维护

现有 `sync_provider_to_local_proxy` 已把远程 provider 设为桌面 DB current,
但它同时覆盖了本地 current——这正是冲突来源。需要拆分:

- 本地 current:继续由桌面自身切换维护(settings.current_provider_*)
- 远程 current:远程切换/编辑时写入**独立状态**(不覆盖本地 current),
  代理在 `/remote` 前缀路径时读取它

建议存储:代理进程内 `remote_current_providers: HashMap<app_type, provider_id>`,
由 `sync_provider_to_local_proxy` 更新;转发时 `/remote` 前缀走它。

### 4. 隧道与端口

保持不变:SSH `-R {port}:127.0.0.1:{port}`,单端口。仅投影 base_url 带前缀。

## 边界情况

- 代理未运行时:远程投影回退为直连(现状逻辑不变)
- 远程 provider 未同步到桌面:代理按远程 current 找不到 provider 时,
  回退本地 current 并 warn
- Codex/Gemini 等其他应用:路径前缀方案按 app 分别处理,前缀可统一
  `/remote`,路由选择按 app_type + 来源
- 前缀剥离:转发上游前去掉 `/remote`,避免上游 404

## 验收标准

1. 本地切 DeepSeek、远程切 OpenCode Go,两端同时对话互不干扰
2. 远程请求日志确认走 `/remote` 前缀且转发目标为远程 provider
3. 本地请求日志确认无前缀且转发目标为本地 provider
4. 代理未运行时远程回退直连(行为与现版一致)
