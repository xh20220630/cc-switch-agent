use std::sync::{Arc, Mutex};

use super::capabilities::{CommandCapabilityRegistry, RemoteCapabilityError};
use super::client::RemoteClientError;
use super::credentials::{CredentialError, RemoteCredentialStore};
use super::models::{RemoteConnectionStatus, RemoteRuntimeSnapshot, RemoteTargetConfig};
use super::ssh::{preflight, LocalForwardSpec, OpenSshSession, RemotePlatform, RemoteSshError};
use super::target_store::{RemoteTargetStore, TargetStoreError};

/// 跨 Core、Agent、SSH 客户端与 runtime generation 的公开错误契约。
///
/// 前端可按这些 code 决定重试、重新连接或提示修复权限；维护时新增跨进程错误码必须先登记，
/// 禁止退回解析本地化 message 的脆弱做法。
pub const DOCUMENTED_REMOTE_ERROR_CODES: &[&str] = &[
    "AUTH_FAILED",
    "CAPABILITY_UNAVAILABLE",
    "COMMAND_NOT_EXPOSED",
    "DATABASE_BUSY",
    "DATABASE_INCOMPATIBLE",
    "INVALID_ARGUMENT",
    "LIVE_WRITE_FAILED",
    "PROVIDER_NOT_FOUND",
    "REMOTE_BUSINESS_ERROR",
    "REMOTE_CONNECTION_ERROR",
    "REMOTE_OFFLINE",
    "REMOTE_OPERATION_CANCELLED",
    "REMOTE_OPERATION_TIMEOUT",
    "REMOTE_PERMISSION_DENIED",
    "REMOTE_UNREACHABLE",
    "STALE_RUNTIME",
];

pub fn documented_error_codes() -> &'static [&'static str] {
    DOCUMENTED_REMOTE_ERROR_CODES
}

trait RuntimeCommandSession: Send + Sync {
    fn invoke(
        &self,
        request_id: &str,
        command: &str,
        args: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, RemoteSshError>;
}

impl RuntimeCommandSession for OpenSshSession {
    fn invoke(
        &self,
        request_id: &str,
        command: &str,
        args: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, RemoteSshError> {
        OpenSshSession::invoke(self, request_id, command, args, timeout_ms)
    }
}

type SharedRuntimeSession = Arc<dyn RuntimeCommandSession>;

pub struct RemoteRuntimeState {
    store: RemoteTargetStore,
    credentials: RemoteCredentialStore,
    snapshot: Mutex<RemoteRuntimeSnapshot>,
    session: Mutex<Option<SharedRuntimeSession>>,
}

impl RemoteRuntimeState {
    pub fn new(store: RemoteTargetStore) -> Result<Self, RemoteRuntimeError> {
        Self::with_credentials(store, RemoteCredentialStore::default_path())
    }

    pub fn with_credentials(
        store: RemoteTargetStore,
        credentials: RemoteCredentialStore,
    ) -> Result<Self, RemoteRuntimeError> {
        let document = store.load()?;
        let snapshot = match document.active_target_id {
            Some(target_id) => RemoteRuntimeSnapshot {
                status: RemoteConnectionStatus::Offline,
                generation: 0,
                active_target_id: Some(target_id),
                error_code: Some("NOT_CONNECTED".to_string()),
                error_message: Some("远程目标尚未连接".to_string()),
            },
            None => RemoteRuntimeSnapshot::local(0),
        };
        Ok(Self {
            store,
            credentials,
            snapshot: Mutex::new(snapshot),
            session: Mutex::new(None),
        })
    }

    pub fn default_store() -> Result<Self, RemoteRuntimeError> {
        Self::new(RemoteTargetStore::default_path())
    }

    pub fn snapshot(&self) -> Result<RemoteRuntimeSnapshot, RemoteRuntimeError> {
        Ok(self.lock_snapshot()?.clone())
    }

    pub fn list_targets(&self) -> Result<Vec<RemoteTargetConfig>, RemoteRuntimeError> {
        let mut targets = self.store.load()?.targets;
        for target in &mut targets {
            target.has_saved_password = self.credentials.has(&target.id);
        }
        Ok(targets)
    }

    pub fn upsert_target(&self, target: RemoteTargetConfig) -> Result<(), RemoteRuntimeError> {
        self.store.upsert(target)?;
        Ok(())
    }

    /// 仅执行平台与认证预检，不上传 Agent，也不替换当前会话。
    /// 设置页可据此验证尚未保存的连接参数，避免“测试”意外改变用户当前环境。
    pub fn test_target(
        &self,
        target: &RemoteTargetConfig,
    ) -> Result<RemotePlatform, RemoteRuntimeError> {
        preflight(target).map_err(RemoteRuntimeError::Ssh)
    }

    pub fn delete_target(&self, target_id: &str) -> Result<bool, RemoteRuntimeError> {
        let deleted = self.store.delete(target_id)?;
        if deleted {
            // 目标删除时一并清理其保存的密码凭据，避免遗留过期凭据。
            let _ = self.credentials.delete(target_id);
            let generation = {
                let snapshot = self.lock_snapshot()?;
                (snapshot.active_target_id.as_deref() == Some(target_id))
                    .then_some(snapshot.generation + 1)
            };
            if let Some(generation) = generation {
                // 删除活动目标等价于切回本机，必须同步终止旧 SSH 子进程，不能只改 UI 快照。
                *self.lock_session()? = None;
                *self.lock_snapshot()? = RemoteRuntimeSnapshot::local(generation);
            }
        }
        Ok(deleted)
    }

    pub fn save_target_password(&self, target_id: &str, password: &str) -> Result<(), RemoteRuntimeError> {
        Ok(self.credentials.set(target_id, password)?)
    }

    pub fn delete_target_password(&self, target_id: &str) -> Result<bool, RemoteRuntimeError> {
        Ok(self.credentials.delete(target_id)?)
    }

    pub fn has_target_password(&self, target_id: &str) -> Result<bool, RemoteRuntimeError> {
        Ok(self.credentials.has(target_id))
    }

    pub fn connect_target(
        &self,
        target_id: &str,
        password: Option<String>,
    ) -> Result<RemoteRuntimeSnapshot, RemoteRuntimeError> {
        self.connect_target_with_forward(target_id, password, None)
    }

    /// 连接远程目标，可选择附带端口转发规格（把远端 CLI 的本地路由请求经
    /// SSH 隧道送回桌面代理）。转发规格只影响本次连接，切换目标后自动失效。
    pub fn connect_target_with_forward(
        &self,
        target_id: &str,
        password: Option<String>,
        forward: Option<LocalForwardSpec>,
    ) -> Result<RemoteRuntimeSnapshot, RemoteRuntimeError> {
        let mut target = self
            .list_targets()?
            .into_iter()
            .find(|target| target.id == target_id)
            .ok_or_else(|| RemoteRuntimeError::TargetNotFound(target_id.to_string()))?;
        // 优先使用本次调用携带的密码，其次使用系统安全存储中已保存的密码；
        // 两者都没有时保持 None，让 SSH 层按公钥/ssh-agent 认证。
        target.password = match password {
            Some(password) if !password.is_empty() => Some(password),
            _ => self.credentials.get(target_id)?,
        };
        let generation = {
            let mut snapshot = self.lock_snapshot()?;
            let generation = snapshot.generation + 1;
            *snapshot = RemoteRuntimeSnapshot {
                status: RemoteConnectionStatus::Connecting,
                generation,
                active_target_id: Some(target_id.to_string()),
                error_code: None,
                error_message: None,
            };
            generation
        };
        // 新目标连接前先销毁旧会话，确保失败时不会保留一个与快照不一致的远端进程。
        *self.lock_session()? = None;
        self.store.set_active_target(Some(target_id.to_string()))?;

        match OpenSshSession::connect_with_forward(&target, forward) {
            Ok(session) => {
                *self.lock_session()? = Some(Arc::new(session));
                let mut snapshot = self.lock_snapshot()?;
                *snapshot = RemoteRuntimeSnapshot {
                    status: RemoteConnectionStatus::Online,
                    generation,
                    active_target_id: Some(target_id.to_string()),
                    error_code: None,
                    error_message: None,
                };
                Ok(snapshot.clone())
            }
            Err(error) => {
                let mut snapshot = self.lock_snapshot()?;
                *snapshot = RemoteRuntimeSnapshot {
                    status: RemoteConnectionStatus::Offline,
                    generation,
                    active_target_id: Some(target_id.to_string()),
                    error_code: Some(ssh_error_code(&error).to_string()),
                    error_message: Some(error.to_string()),
                };
                Err(RemoteRuntimeError::Ssh(error))
            }
        }
    }

    pub fn use_local(&self) -> Result<RemoteRuntimeSnapshot, RemoteRuntimeError> {
        // 保留已连 SSH 会话与其 -R 隧道：远程 CLI 的 live base_url 仍指向
        // 127.0.0.1:{port}/remote，切回本机不能杀掉这条链路，否则远程 CLI
        // 会因隧道消失而无法使用。仅在连接到其他远程目标(connect_* 前)时销毁。
        self.store.set_active_target(None)?;
        let mut snapshot = self.lock_snapshot()?;
        *snapshot = RemoteRuntimeSnapshot::local(snapshot.generation + 1);
        Ok(snapshot.clone())
    }

    pub fn invoke_remote(
        &self,
        expected_generation: u64,
        command: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteRuntimeError> {
        self.require_generation(expected_generation)?;
        let registry = CommandCapabilityRegistry::remote_supported();
        let capability = registry.require(command)?;
        // 克隆 Arc 后立即释放 runtime session 锁；目标切换可并发丢弃旧 session，
        // 正在执行的请求仍持有旧 Arc，返回时由第二次 generation 检查拒绝迟到结果。
        let session = self
            .lock_session()?
            .clone()
            .ok_or(RemoteRuntimeError::Offline)?;
        let result = session.invoke(
            &uuid::Uuid::new_v4().to_string(),
            command,
            args,
            capability.timeout_ms,
        );
        self.require_generation(expected_generation)?;
        result.map_err(RemoteRuntimeError::Ssh)
    }

    fn require_generation(&self, expected: u64) -> Result<(), RemoteRuntimeError> {
        let snapshot = self.lock_snapshot()?;
        if snapshot.generation != expected {
            return Err(RemoteRuntimeError::StaleRuntime {
                expected,
                actual: snapshot.generation,
            });
        }
        if snapshot.status != RemoteConnectionStatus::Online {
            return Err(RemoteRuntimeError::Offline);
        }
        Ok(())
    }

    #[cfg(test)]
    fn install_test_session(&self, generation: u64, session: Box<dyn RuntimeCommandSession>) {
        *self.session.lock().expect("锁定测试 session") = Some(Arc::from(session));
        *self.snapshot.lock().expect("锁定测试 snapshot") = RemoteRuntimeSnapshot {
            status: RemoteConnectionStatus::Online,
            generation,
            active_target_id: Some("test-target".to_string()),
            error_code: None,
            error_message: None,
        };
    }

    fn lock_snapshot(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RemoteRuntimeSnapshot>, RemoteRuntimeError> {
        self.snapshot
            .lock()
            .map_err(|error| RemoteRuntimeError::StatePoisoned(error.to_string()))
    }

    fn lock_session(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<SharedRuntimeSession>>, RemoteRuntimeError> {
        self.session
            .lock()
            .map_err(|error| RemoteRuntimeError::StatePoisoned(error.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteRuntimeError {
    #[error(transparent)]
    Store(#[from] TargetStoreError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error("远程运行时状态锁损坏: {0}")]
    StatePoisoned(String),
    #[error("远程目标不存在: {0}")]
    TargetNotFound(String),
    #[error("远程连接未建立")]
    Offline,
    #[error("远程运行时已切换: expected={expected}, actual={actual}")]
    StaleRuntime { expected: u64, actual: u64 },
    #[error(transparent)]
    Capability(#[from] RemoteCapabilityError),
    #[error(transparent)]
    Ssh(#[from] RemoteSshError),
    #[error(transparent)]
    Client(#[from] RemoteClientError),
}

impl RemoteRuntimeError {
    pub fn code(&self) -> &str {
        match self {
            Self::Store(_) => "REMOTE_TARGET_STORE_ERROR",
            Self::Credential(CredentialError::UnsupportedPlatform) => "CREDENTIAL_STORE_UNSUPPORTED",
            Self::Credential(_) => "CREDENTIAL_STORE_ERROR",
            Self::StatePoisoned(_) => "REMOTE_STATE_ERROR",
            Self::TargetNotFound(_) => "REMOTE_TARGET_NOT_FOUND",
            Self::Offline => "REMOTE_OFFLINE",
            Self::StaleRuntime { .. } => "STALE_RUNTIME",
            Self::Capability(_) => "COMMAND_NOT_EXPOSED",
            Self::Ssh(RemoteSshError::Validation(_)) => "REMOTE_TARGET_INVALID",
            Self::Ssh(error) => ssh_error_code(error),
            Self::Client(error) => error.code(),
        }
    }
}

fn ssh_error_code(error: &RemoteSshError) -> &str {
    error.code()
}

#[cfg(test)]
mod generation_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    struct BlockingSession {
        calls: Arc<AtomicUsize>,
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl RuntimeCommandSession for BlockingSession {
        fn invoke(
            &self,
            _request_id: &str,
            _command: &str,
            _args: serde_json::Value,
            _timeout_ms: u64,
        ) -> Result<serde_json::Value, RemoteSshError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.started.send(());
            self.release
                .lock()
                .expect("锁定 fake release")
                .recv_timeout(Duration::from_secs(2))
                .expect("等待释放 fake 响应");
            Ok(json!({ "source": "old-generation" }))
        }
    }

    fn connected_runtime(session: BlockingSession) -> (Arc<RemoteRuntimeState>, Arc<AtomicUsize>) {
        let temp = tempfile::tempdir().expect("创建 runtime fixture");
        let store = RemoteTargetStore::at(temp.keep().join("remote-targets.json"));
        let runtime = Arc::new(RemoteRuntimeState::new(store).expect("创建 runtime"));
        let calls = Arc::clone(&session.calls);
        runtime.install_test_session(7, Box::new(session));
        (runtime, calls)
    }

    #[test]
    fn stale_generation_is_rejected_before_reaching_session() {
        let (started_sender, _started_receiver) = mpsc::channel();
        let (_release_sender, release_receiver) = mpsc::channel();
        let (runtime, calls) = connected_runtime(BlockingSession {
            calls: Arc::new(AtomicUsize::new(0)),
            started: started_sender,
            release: Mutex::new(release_receiver),
        });

        let error = runtime
            .invoke_remote(6, "usage.summary", json!({}))
            .expect_err("旧 generation 必须在发送前被拒绝");
        assert_eq!(error.code(), "STALE_RUNTIME");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn response_is_rejected_when_generation_changes_during_request() {
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let (runtime, _calls) = connected_runtime(BlockingSession {
            calls: Arc::new(AtomicUsize::new(0)),
            started: started_sender,
            release: Mutex::new(release_receiver),
        });
        let invoking = Arc::clone(&runtime);
        let request =
            std::thread::spawn(move || invoking.invoke_remote(7, "usage.summary", json!({})));
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("fake session 收到请求");
        let snapshot = runtime.use_local().expect("请求期间切回本地");
        assert_eq!(snapshot.generation, 8);
        release_sender.send(()).expect("释放旧响应");

        let error = request
            .join()
            .expect("等待旧请求")
            .expect_err("迟到响应必须被拒绝");
        assert_eq!(error.code(), "STALE_RUNTIME");
    }
}

#[cfg(test)]
mod password_e2e_tests {
    use super::*;

    /// 端到端密码认证集成测试：需要 CC_SWITCH_TEST_SSH_TARGET、CC_SWITCH_TEST_SSH_USER、
    /// CC_SWITCH_TEST_SSH_PASSWORD 环境变量指向真实服务器。验证保存密码 → 连接时从
    /// 凭据存储注入 → askpass 认证 → Agent 协议握手全链路，默认忽略避免 CI 依赖网络。
    #[test]
    #[ignore]
    fn password_credentials_connect_end_to_end() {
        let host = std::env::var("CC_SWITCH_TEST_SSH_TARGET").expect("设置测试服务器地址");
        let username =
            std::env::var("CC_SWITCH_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());
        let password =
            std::env::var("CC_SWITCH_TEST_SSH_PASSWORD").expect("设置测试服务器密码");

        let temp = tempfile::tempdir().expect("创建 runtime fixture");
        let dir = temp.keep();
        let store = RemoteTargetStore::at(dir.join("remote-targets.json"));
        let credentials = RemoteCredentialStore::at(dir.join("credentials.json"));
        let runtime =
            RemoteRuntimeState::with_credentials(store, credentials).expect("创建 runtime");
        let target_id = "e2e-password-target";
        runtime
            .upsert_target(RemoteTargetConfig {
                id: target_id.to_string(),
                name: "e2e".to_string(),
                host_alias: host.clone(),
                username: Some(username),
                port: Some(22),
                identity_file: None,
                password: None,
                has_saved_password: false,
            })
            .expect("保存目标");

        // 保存密码后不携带 password 连接，验证凭据存储自动注入路径。
        runtime
            .save_target_password(target_id, &password)
            .expect("保存密码到凭据存储");
        assert!(runtime.has_target_password(target_id).expect("检查密码存在"));
        let snapshot = runtime.connect_target(target_id, None).expect("密码连接成功");
        assert_eq!(snapshot.status, RemoteConnectionStatus::Online);

        // 删除凭据后必须从存储中消失；后续连接（若有默认密钥）不再走密码路径。
        runtime.delete_target_password(target_id).expect("删除密码");
        assert!(!runtime.has_target_password(target_id).expect("检查密码已删除"));
        assert_eq!(
            runtime.list_targets().expect("读取目标列表")[0].has_saved_password,
            false
        );
    }

    /// 端到端主机密钥信任集成测试：需要 CC_SWITCH_TEST_SSH_TARGET 环境变量指向真实服务器。
    /// 备份 known_hosts → 移除该主机条目 → 调用 trust_host_key 写入 → 验证 ssh-keygen -F
    /// 能查到 → 最后恢复备份，默认忽略避免 CI 依赖网络或污染用户 known_hosts。
    #[test]
    #[ignore]
    fn trust_host_key_end_to_end() {
        let host = std::env::var("CC_SWITCH_TEST_SSH_TARGET").expect("设置测试服务器地址");
        let known_hosts = crate::config::get_home_dir().join(".ssh").join("known_hosts");
        let backup = known_hosts.with_extension("cc-switch-test-bak");
        if known_hosts.exists() {
            std::fs::copy(&known_hosts, &backup).expect("备份 known_hosts");
        }
        let mut restored = false;
        let restore = |restored: &mut bool| {
            if *restored {
                return;
            }
            *restored = true;
            if backup.exists() {
                std::fs::copy(&backup, &known_hosts).expect("恢复 known_hosts");
            } else if known_hosts.exists() {
                std::fs::remove_file(&known_hosts).expect("清理 known_hosts");
            }
            let _ = std::fs::remove_file(&backup);
        };

        let target = RemoteTargetConfig {
            id: "e2e-trust".to_string(),
            name: "e2e".to_string(),
            host_alias: host.clone(),
            username: None,
            port: Some(22),
            identity_file: None,
            password: None,
            has_saved_password: false,
        };

        // 用 ssh-keygen -R 移除现有条目，模拟首次连接。
        let status = std::process::Command::new("ssh-keygen")
            .args(["-R", &host])
            .status()
            .expect("移除已有主机密钥");
        assert!(status.success(), "ssh-keygen -R 执行失败");

        let result = crate::remote::ssh::trust_host_key(&target);
        if let Err(error) = &result {
            restore(&mut restored);
            panic!("trust_host_key 失败: {error}");
        }
        let fingerprints = result.expect("获取指纹");
        assert!(!fingerprints.is_empty(), "应返回至少一个密钥指纹");

        let check = std::process::Command::new("ssh-keygen")
            .args(["-F", &host])
            .output()
            .expect("查询 known_hosts");
        let check_out = String::from_utf8_lossy(&check.stdout);
        assert!(
            check_out.contains(&host),
            "known_hosts 中应能找到 {host}，实际输出: {check_out}"
        );
        restore(&mut restored);
    }
}
