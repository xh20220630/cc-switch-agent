use serde::Serialize;
use serde_json::Value;
use std::str::FromStr;
use tauri::{Emitter, Manager, State};

use crate::remote::models::{RemoteRuntimeSnapshot, RemoteTargetConfig};
use crate::remote::runtime::{RemoteRuntimeError, RemoteRuntimeState};
use crate::remote::ssh::{host_key_fingerprints, trust_host_key, LocalForwardSpec, RemotePlatform};
use crate::remote::ssh_config::{discover_current_user_ssh_targets, DiscoveredSshTarget};
use crate::store::AppState;

use crate::proxy::REMOTE_ROUTE_PREFIX;

#[tauri::command]
pub fn remote_discover_ssh_targets() -> Result<Vec<DiscoveredSshTarget>, String> {
    // 发现过程只读取本机用户配置，不依赖当前远程运行时，也不会发起网络连接。
    discover_current_user_ssh_targets().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remote_list_targets(
    state: State<'_, RemoteRuntimeState>,
) -> Result<Vec<RemoteTargetConfig>, String> {
    state.list_targets().map_err(serialize_error)
}

#[tauri::command]
pub fn remote_upsert_target(
    state: State<'_, RemoteRuntimeState>,
    target: RemoteTargetConfig,
) -> Result<bool, String> {
    state.upsert_target(target).map_err(serialize_error)?;
    Ok(true)
}

#[tauri::command]
pub async fn remote_test_target(
    app_handle: tauri::AppHandle,
    target: RemoteTargetConfig,
) -> Result<RemotePlatform, String> {
    // SSH 进程可能等待网络超时，必须离开 Tauri 命令线程，避免阻塞其他本地操作。
    tauri::async_runtime::spawn_blocking(move || {
        app_handle
            .state::<RemoteRuntimeState>()
            .test_target(&target)
            .map_err(serialize_error)
    })
    .await
    .map_err(|error| format!("远程连接测试任务失败: {error}"))?
}

#[tauri::command]
pub fn remote_save_target_password(
    state: State<'_, RemoteRuntimeState>,
    #[allow(non_snake_case)] targetId: String,
    password: String,
) -> Result<bool, String> {
    state
        .save_target_password(&targetId, &password)
        .map_err(serialize_error)?;
    Ok(true)
}

#[tauri::command]
pub fn remote_delete_target_password(
    state: State<'_, RemoteRuntimeState>,
    #[allow(non_snake_case)] targetId: String,
) -> Result<bool, String> {
    state
        .delete_target_password(&targetId)
        .map_err(serialize_error)
}

#[tauri::command]
pub fn remote_has_target_password(
    state: State<'_, RemoteRuntimeState>,
    #[allow(non_snake_case)] targetId: String,
) -> Result<bool, String> {
    state.has_target_password(&targetId).map_err(serialize_error)
}

#[tauri::command]
pub async fn remote_trust_target_host(
    target: RemoteTargetConfig,
) -> Result<Vec<String>, String> {
    // ssh-keyscan 可能等待网络超时，离开命令线程避免阻塞本地 UI。
    tauri::async_runtime::spawn_blocking(move || {
        trust_host_key(&target)
            .map_err(RemoteRuntimeError::Ssh)
            .map_err(serialize_error)
    })
    .await
    .map_err(|error| format!("信任主机密钥任务失败: {error}"))?
}

#[tauri::command]
pub async fn remote_get_host_key_fingerprints(
    target: RemoteTargetConfig,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        host_key_fingerprints(&target)
            .map_err(RemoteRuntimeError::Ssh)
            .map_err(serialize_error)
    })
    .await
    .map_err(|error| format!("读取主机密钥指纹任务失败: {error}"))?
}

#[tauri::command]
pub fn remote_delete_target(
    state: State<'_, RemoteRuntimeState>,
    #[allow(non_snake_case)] targetId: String,
) -> Result<bool, String> {
    state.delete_target(&targetId).map_err(serialize_error)
}

#[tauri::command]
pub fn remote_get_runtime_snapshot(
    state: State<'_, RemoteRuntimeState>,
) -> Result<RemoteRuntimeSnapshot, String> {
    state.snapshot().map_err(serialize_error)
}

#[tauri::command]
pub async fn remote_set_active_target(
    app_handle: tauri::AppHandle,
    #[allow(non_snake_case)] targetId: Option<String>,
    password: Option<String>,
) -> Result<RemoteRuntimeSnapshot, String> {
    // 桌面代理运行时，把远端 CLI 的本地路由请求经 SSH 隧道送回桌面代理，
    // 否则"本地路由"型供应商在远端会因 127.0.0.1:{port} 无人监听而失败。
    let forward = match targetId.as_deref() {
        Some(_) => {
            let status = app_handle
                .state::<AppState>()
                .proxy_service
                .get_status()
                .await
                .map_err(|error| format!("读取桌面代理状态失败: {error}"))?;
            status.running.then(|| LocalForwardSpec::same_port(status.port))
        }
        None => None,
    };
    let task_handle = app_handle.clone();
    let (target_id_connected, result) = tauri::async_runtime::spawn_blocking(move || {
        let state = task_handle.state::<RemoteRuntimeState>();
        match &targetId {
            Some(target_id) => (
                true,
                state.connect_target_with_forward(target_id, password, forward),
            ),
            None => (false, state.use_local()),
        }
    })
    .await
    .map_err(|error| format!("远程环境切换任务失败: {error}"))?;

    // 即使连接失败也广播最终快照，让前端进入明确的离线态而不是一直显示连接中。
    let snapshot = app_handle
        .state::<RemoteRuntimeState>()
        .snapshot()
        .map_err(serialize_error)?;
    let _ = app_handle.emit("remote-runtime-status", &snapshot);

    // 连接成功后自动触发一次远端会话用量同步：桌面本机会话同步依赖进程内 60 秒
    // 轮询，而远端 Agent 没有常驻调度，若不主动发起，远端库的 Usage 统计永远为空。
    // 同步在独立任务中运行，失败只记日志，不影响本次切换结果。
    if target_id_connected && result.is_ok() {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let generation = handle
                .state::<RemoteRuntimeState>()
                .snapshot()
                .map(|snapshot| snapshot.generation)
                .unwrap_or(0);
            let outcome = tauri::async_runtime::spawn_blocking(move || {
                handle
                    .state::<RemoteRuntimeState>()
                    .invoke_remote(generation, "usage.session_sync", serde_json::Value::Null)
            })
            .await;
            match outcome {
                Ok(Ok(_)) => log::info!("连接远端后自动同步会话用量完成"),
                Ok(Err(error)) => log::warn!("连接远端后自动同步会话用量失败: {error}"),
                Err(error) => log::warn!("连接远端后自动同步会话用量任务失败: {error}"),
            }
        });
    }

    result.map_err(serialize_error)
}

#[tauri::command]
pub async fn remote_invoke(
    app_handle: tauri::AppHandle,
    command: String,
    #[allow(non_snake_case)] args: Value,
    generation: u64,
) -> Result<Value, String> {
    // 本地路由模式：桌面代理运行中时，远程切换 provider 必须让远程 CLI 把请求
    // 打到 SSH 隧道(127.0.0.1:15721)送回桌面代理，由代理用 DB 真实 key 按
    // x-api-key 转发，否则直连网关会因 AUTH_TOKEN→Bearer 与网关不兼容而 401。
    // 因此在此改写投影快照并随命令下发，远端 DB 的原始配置保持不变。
    let (args, remote_provider_sync) =
        maybe_rewrite_provider_switch_for_remote_proxy(&app_handle, &command, args).await;
    let result = {
        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app_handle
                .state::<RemoteRuntimeState>()
                .invoke_remote(generation, &command, args)
                .map_err(serialize_error)
        })
        .await
        .map_err(|error| format!("远程命令任务失败: {error}"))?
    };

    // 远程切换成功后，把 provider 同步进桌面 DB 并设为 current，让本地代理能
    // 按该 provider 的真实配置转发（远程模式的数据在远端 DB，桌面代理看不到）。
    if result.is_ok() {
        if let Some(sync) = remote_provider_sync {
            let state = app_handle.state::<AppState>();
            match sync_provider_to_local_proxy(&state, sync).await {
                Ok(()) => log::info!("[remote] 已同步远程 provider 到桌面代理路由"),
                Err(error) => log::warn!("[remote] 同步远程 provider 到桌面代理失败: {error}"),
            }
        }
    }
    result
}

/// 把远程切换的 provider 注册进桌面 DB 并设为"远程 current"，供本地代理转发。
/// 与本地 current 完全隔离：不调用 set_current_provider，避免覆盖本地选择。
async fn sync_provider_to_local_proxy(
    state: &tauri::State<'_, AppState>,
    sync: RemoteProviderSync,
) -> Result<(), String> {
    let mut provider = sync.provider;
    // 标记为远程同步条目：桌面代理接管恢复时不能把本地 live 的 token 覆盖进来
    // （本地 live 可能是其它 provider 的占位/真实 token，覆盖会导致代理转发 401）。
    if let Some(meta) = provider.meta.as_mut() {
        meta.remote_synced = Some(true);
    } else {
        provider.meta = Some(crate::provider::ProviderMeta {
            remote_synced: Some(true),
            ..Default::default()
        });
    }
    // save_provider 为 upsert：桌面无此 id 时插入，已有(可能是旧同步记录)时全量更新。
    state
        .db
        .save_provider(&sync.app_type, &provider)
        .map_err(|e| format!("保存 provider 到桌面 DB 失败: {e}"))?;
    // 仅更新"远程 current"(代理按 /remote 前缀请求用它)，不触碰本地 current。
    state
        .proxy_service
        .set_remote_current(&sync.app_type, &provider.id)
        .await;
    Ok(())
}

/// 远程切换成功后需要同步到桌面代理的 provider 信息。
struct RemoteProviderSync {
    app_type: String,
    provider: crate::provider::Provider,
}

/// 远程 `provider.switch`/`provider.update` 且桌面代理运行中时，改写 provider
/// 快照指向桌面代理。返回 (改写后的 args, 需要同步到桌面代理的原始 provider)。
async fn maybe_rewrite_provider_switch_for_remote_proxy(
    app_handle: &tauri::AppHandle,
    command: &str,
    args: Value,
) -> (Value, Option<RemoteProviderSync>) {
    let command = match command {
        "provider.switch" => command,
        // 远程编辑当前 provider 时, agent 端 update 也会投影 live; 若桌面代理
        // 运行中同样需要改写为本地路由, 避免直连网关认证头不兼容。
        "provider.update" => command,
        _ => {
            log::debug!("[remote] 跳过本地路由改写: 命令 {command}");
            return (args, None);
        }
    };
    let Some(app_type) = args.get("app").and_then(Value::as_str) else {
        log::warn!("[remote] 跳过本地路由改写: {command} 缺少 app 参数");
        return (args, None);
    };
    let Some(id) = args
        .get("id")
        .or_else(|| args.get("originalId"))
        .and_then(Value::as_str)
    else {
        log::warn!("[remote] 跳过本地路由改写: {command} 缺少 id 参数");
        return (args, None);
    };
    let Ok(app_type) = crate::AppType::from_str(app_type) else {
        log::warn!("[remote] 跳过本地路由改写: 未知 app 类型 {app_type}");
        return (args, None);
    };
    let Ok(status) = app_handle
        .state::<AppState>()
        .proxy_service
        .get_status()
        .await
    else {
        log::warn!("[remote] 跳过本地路由改写: 读取桌面代理状态失败");
        return (args, None);
    };
    if !status.running || status.port == 0 {
        log::info!(
            "[remote] 跳过本地路由改写: 桌面代理未运行 (running={}, port={})",
            status.running,
            status.port
        );
        return (args, None);
    }
    // provider.update / provider.switch 的 payload 均可自带完整 provider(前端持有
    // 远程 provider 数据), 优先基于它改写, 避免依赖桌面 DB(远程模式数据在远端)。
    let provider_value = args.get("provider").cloned();
    let mut sync_provider = provider_value.clone();
    let mut provider_value = match provider_value {
        Some(value) => value,
        None => match app_handle
            .state::<AppState>()
            .db
            .get_provider_by_id(id, app_type.as_str())
            .ok()
            .flatten()
        {
            Some(provider) => {
                sync_provider = serde_json::to_value(&provider).ok();
                match serde_json::to_value(provider) {
                    Ok(value) => value,
                    Err(error) => {
                        log::warn!(
                            "[remote] 跳过本地路由改写: provider `{id}` 序列化失败: {error}"
                        );
                        return (args, None);
                    }
                }
            }
            None => {
                log::warn!("[remote] 跳过本地路由改写: 桌面 DB 未找到 provider `{id}`");
                return (args, None);
            }
        },
    };
    let Some(settings) = provider_value.get_mut("settingsConfig") else {
        log::warn!(
            "[remote] 跳过本地路由改写: provider `{id}` 缺少 settingsConfig"
        );
        return (args, None);
    };
    let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) else {
        log::warn!(
            "[remote] 跳过本地路由改写: provider `{id}` settingsConfig 缺少 env 对象"
        );
        return (args, None);
    };
    // 与桌面代理接管语义一致：base_url 指向本地代理，token 替换为占位符，
    // 代理侧从 DB 读取真实凭据并按 provider 的 apiKeyField 选择 x-api-key。
    // `/remote` 路径前缀标记远程来源：桌面代理据此按"远程 current"转发，
    // 与本地请求(无前缀,按本地 current)隔离。
    let proxy_base_url = format!(
        "http://127.0.0.1:{}{}",
        status.port, REMOTE_ROUTE_PREFIX
    );
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        Value::String(proxy_base_url.clone()),
    );
    for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        if env.contains_key(key) {
            env.insert(key.to_string(), Value::String("PROXY_MANAGED".to_string()));
        }
    }
    log::info!(
        "[remote] {command} `{id}` 已改写为本地路由: {}",
        proxy_base_url
    );
    let mut rewritten = args;
    if let Some(object) = rewritten.as_object_mut() {
        // switch: 快照即投影本体; update: 原始 provider 留在 `provider`(落盘 DB),
        // 改写快照放入 `projected`(仅用于 live 投影)。
        match command {
            "provider.update" => {
                object.insert("projected".to_string(), provider_value);
            }
            _ => {
                object.insert("provider".to_string(), provider_value);
            }
        }
    }
    let sync = match (sync_provider, command) {
        // update 时同步用户编辑后的原始 provider; switch 时同步桌面 DB 原始记录。
        (Some(provider_value), _) => {
            match serde_json::from_value::<crate::provider::Provider>(provider_value) {
                Ok(provider) => Some(RemoteProviderSync {
                    app_type: app_type.as_str().to_string(),
                    provider,
                }),
                Err(error) => {
                    log::warn!("[remote] 同步 provider 反序列化失败: {error}");
                    None
                }
            }
        }
        _ => None,
    };
    (rewritten, sync)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteErrorPayload<'a> {
    code: &'a str,
    message: &'a str,
}

fn serialize_error(error: RemoteRuntimeError) -> String {
    let message = error.to_string();
    serde_json::to_string(&RemoteErrorPayload {
        code: error.code(),
        message: &message,
    })
    .unwrap_or(message)
}
