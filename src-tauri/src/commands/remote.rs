use serde::Serialize;
use serde_json::Value;
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
    state
        .has_target_password(&targetId)
        .map_err(serialize_error)
}

#[tauri::command]
pub async fn remote_trust_target_host(target: RemoteTargetConfig) -> Result<Vec<String>, String> {
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
    let runtime = app_handle.state::<RemoteRuntimeState>();
    let _operation = runtime.lock_provider_operations().await;
    if targetId.is_some() {
        app_handle
            .state::<AppState>()
            .proxy_service
            .clear_remote_routes()
            .await;
    }
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
            status
                .running
                .then(|| LocalForwardSpec::same_port(status.port))
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
    // 建连即恢复远端当前路由，不要求用户再点一次切换。这里只读远端，不改写任何 CLI 配置。
    let mut route_error = None;
    if target_id_connected && result.is_ok() {
        for app in ["claude", "codex", "gemini", "grokbuild"] {
            if let Err(error) =
                refresh_remote_provider_route(&app_handle, snapshot.generation, app).await
            {
                log::warn!("[remote] 初始化 {app} 路由失败: {error}");
                route_error = Some(error);
            }
        }
    }
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
                handle.state::<RemoteRuntimeState>().invoke_remote(
                    generation,
                    "usage.session_sync",
                    serde_json::Value::Null,
                )
            })
            .await;
            match outcome {
                Ok(Ok(_)) => log::info!("连接远端后自动同步会话用量完成"),
                Ok(Err(error)) => log::warn!("连接远端后自动同步会话用量失败: {error}"),
                Err(error) => log::warn!("连接远端后自动同步会话用量任务失败: {error}"),
            }
        });
    }

    let snapshot = result.map_err(serialize_error)?;
    if let Some(error) = route_error {
        return Err(format!(
            "远程已连接，但供应商路由初始化失败，请重新连接：{error}"
        ));
    }
    Ok(snapshot)
}

#[tauri::command]
pub async fn remote_invoke(
    app_handle: tauri::AppHandle,
    command: String,
    #[allow(non_snake_case)] args: Value,
    generation: u64,
) -> Result<Value, String> {
    // generation 在任何读取/改写前校验；同一桌面连接的写入和路由确认不可交错。
    let runtime = app_handle.state::<RemoteRuntimeState>();
    let _operation = if command.starts_with("provider.") {
        Some(runtime.lock_provider_operations().await)
    } else {
        None
    };
    runtime
        .require_generation(generation)
        .map_err(serialize_error)?;
    let app = args.get("app").and_then(Value::as_str).map(str::to_string);
    let mutation = matches!(
        command.as_str(),
        "provider.add" | "provider.update" | "provider.switch" | "provider.delete"
    );
    let args = prepare_remote_provider_args(&app_handle, generation, &command, args).await?;
    let deleted_id = if command == "provider.delete" {
        args.get("id").and_then(Value::as_str).map(str::to_string)
    } else {
        None
    };
    if mutation {
        if let Some(app) = app.as_deref() {
            // 写入期间宁可明确提示稍后重试，也不能让新模型请求带着旧供应商凭据出站。
            app_handle
                .state::<AppState>()
                .proxy_service
                .set_remote_current(app, None)
                .await;
        }
    }
    let result = invoke_agent(&app_handle, generation, &command, args).await;
    if result.is_ok() && mutation {
        if let Some(app) = app.as_deref() {
            // 重新读取实际 current，不能把刚编辑的非当前供应商当作当前项。
            refresh_remote_provider_route(&app_handle, generation, app)
                .await
                .map_err(|error| {
                    format!("远端操作已完成，但代理路由同步失败，请重新连接目标主机：{error}")
                })?;
            if let Some(id) = deleted_id {
                let target = runtime
                    .snapshot()
                    .map_err(serialize_error)?
                    .active_target_id
                    .ok_or("远程目标未连接")?;
                // 只清理当前主机命名空间中的影子，不影响本机同 ID 的卡片。
                app_handle
                    .state::<AppState>()
                    .db
                    .delete_provider(app, &format!("remote:{target}:{id}"))
                    .map_err(|error| format!("清理远程供应商影子失败: {error}"))?;
            }
        }
    }
    // 失败可能发生在 DB 已提交、live 未完成之后；保持路由撤销，禁止继续使用旧凭据。
    result
}

/// 交换弹窗通过本机边界访问指定机器，目标选择不影响当前页面和本地代理的运行环境。
#[tauri::command]
pub async fn remote_invoke_provider_target(
    app_handle: tauri::AppHandle,
    #[allow(non_snake_case)] targetId: String,
    command: String,
    args: Value,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app_handle
            .state::<RemoteRuntimeState>()
            .invoke_provider_target(&targetId, &command, args)
            .map_err(serialize_error)
    })
    .await
    .map_err(|error| format!("供应商交换远程任务失败: {error}"))?
}

/// 所有远程读取沿用命令传入的 generation，不能在等待 SSH 时悄悄跟随新主机。
async fn invoke_agent(
    app_handle: &tauri::AppHandle,
    generation: u64,
    command: &str,
    args: Value,
) -> Result<Value, String> {
    let handle = app_handle.clone();
    let command = command.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        handle
            .state::<RemoteRuntimeState>()
            .invoke_remote(generation, &command, args)
            .map_err(serialize_error)
    })
    .await
    .map_err(|error| format!("远程命令任务失败: {error}"))?
}

/// 从远端规范数据确认选择，使用目标主机命名空间保存影子记录，避免覆盖本机同 ID 的卡片。
async fn refresh_remote_provider_route(
    app_handle: &tauri::AppHandle,
    generation: u64,
    app: &str,
) -> Result<(), String> {
    let current = invoke_agent(
        app_handle,
        generation,
        "provider.current",
        serde_json::json!({"app": app}),
    )
    .await?;
    let providers = invoke_agent(
        app_handle,
        generation,
        "provider.list",
        serde_json::json!({"app": app}),
    )
    .await?;
    let provider = remote_current_provider(&current, &providers)?;
    let runtime = app_handle.state::<RemoteRuntimeState>();
    runtime
        .require_generation(generation)
        .map_err(serialize_error)?;
    let target = runtime
        .snapshot()
        .map_err(serialize_error)?
        .active_target_id
        .ok_or("远程目标未连接")?;
    let state = app_handle.state::<AppState>();
    let provider = provider.map(|mut provider| {
        provider.id = format!("remote:{target}:{}", provider.id);
        // 影子只用于远程转发和日志归因，不能加入本机故障转移队列。
        provider.in_failover_queue = false;
        provider
            .meta
            .get_or_insert_with(Default::default)
            .remote_synced = Some(true);
        provider
    });
    if let Some(provider) = provider.as_ref() {
        // 日志需要 DB 中的名称与归因信息，路由本身直接持有完整快照，不依赖本机 current。
        state
            .db
            .save_provider(app, provider)
            .map_err(|error| format!("保存远程影子供应商失败: {error}"))?;
    }
    state.proxy_service.set_remote_current(app, provider).await;
    Ok(())
}

/// 空 current 是合法的未配置状态；悬空或格式错误必须拒绝，不能猜测本机供应商。
fn remote_current_provider(
    current: &Value,
    providers: &Value,
) -> Result<Option<crate::provider::Provider>, String> {
    let id = current.as_str().ok_or("远程 current 返回格式错误")?;
    if id.is_empty() {
        return Ok(None);
    }
    let value = providers
        .get(id)
        .ok_or("远程当前供应商不存在，请重新连接目标主机")?;
    let provider: crate::provider::Provider =
        serde_json::from_value(value.clone()).map_err(|_| "远程供应商返回格式错误".to_string())?;
    if provider.id != id {
        return Err("远程供应商 ID 不一致".into());
    }
    Ok(Some(provider))
}

/// Claude 代理投影与规范配置分开：切换的源从远端读取，编辑的源是用户提交的完整配置。
/// 其他应用保持各自现有 writer 语义，不能往 Gemini 等配置误塞 ANTHROPIC 字段。
async fn prepare_remote_provider_args(
    app_handle: &tauri::AppHandle,
    generation: u64,
    command: &str,
    args: Value,
) -> Result<Value, String> {
    if !matches!(command, "provider.switch" | "provider.update")
        || args.get("app").and_then(Value::as_str) != Some("claude")
    {
        return Ok(args);
    }
    let status = app_handle
        .state::<AppState>()
        .proxy_service
        .get_status()
        .await?;
    if !status.running || status.port == 0 {
        return Ok(args);
    }
    let provider = if command == "provider.update" {
        args.get("provider")
            .cloned()
            .ok_or("编辑供应商缺少完整配置")?
    } else {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("切换供应商缺少 ID")?;
        let providers = invoke_agent(
            app_handle,
            generation,
            "provider.list",
            serde_json::json!({"app": "claude"}),
        )
        .await?;
        providers.get(id).cloned().ok_or("远程供应商不存在")?
    };
    project_remote_claude_args(command, args, provider, status.port)
}

/// 投影只改变远端 live，真实密钥仍落在远端 DB；没有 env 的官方配置继续使用原生认证。
fn project_remote_claude_args(
    command: &str,
    mut args: Value,
    mut provider: Value,
    port: u16,
) -> Result<Value, String> {
    let Some(env) = provider
        .get_mut("settingsConfig")
        .and_then(|settings| settings.get_mut("env"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(args);
    };
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        Value::String(format!("http://127.0.0.1:{port}{REMOTE_ROUTE_PREFIX}")),
    );
    for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        if env.contains_key(key) {
            env.insert(key.into(), Value::String("PROXY_MANAGED".into()));
        }
    }
    let object = args.as_object_mut().ok_or("供应商参数必须为对象")?;
    object.insert(
        if command == "provider.update" {
            "projected"
        } else {
            "provider"
        }
        .into(),
        provider,
    );
    Ok(args)
}

#[cfg(test)]
mod provider_route_tests {
    use super::*;
    use serde_json::json;

    fn provider(id: &str) -> Value {
        json!({"id": id, "name": id, "settingsConfig": {"env": {
            "ANTHROPIC_BASE_URL": "https://remote.example.com",
            "ANTHROPIC_API_KEY": "remote-secret", "ANTHROPIC_MODEL": "remote-model"
        }}, "inFailoverQueue": false})
    }

    #[test]
    fn claude_edit_inactive_provider_preserves_actual_remote_selection() {
        let edited = provider("inactive");
        let args = project_remote_claude_args(
            "provider.update",
            json!({"app":"claude", "provider":edited}),
            edited.clone(),
            15721,
        )
        .unwrap();
        // 即使省略 originalId，也保留规范密钥，只让 projected 使用代理占位符。
        assert_eq!(args["provider"], edited);
        assert_eq!(
            args["projected"]["settingsConfig"]["env"]["ANTHROPIC_API_KEY"],
            "PROXY_MANAGED"
        );
        let actual = remote_current_provider(
            &json!("active"),
            &json!({"active":provider("active"),"inactive":edited}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(actual.id, "active");
    }

    #[test]
    fn claude_remote_selection_requires_valid_current_snapshot() {
        assert!(remote_current_provider(&json!("missing"), &json!({})).is_err());
        assert!(remote_current_provider(&json!("a"), &json!({"a":provider("b")})).is_err());
        assert!(remote_current_provider(&json!(null), &json!({})).is_err());
        assert!(remote_current_provider(&json!(""), &json!({}))
            .unwrap()
            .is_none());
    }
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
