use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::client::{RemoteClientError, RemoteSession};
use super::embedded_agent::{embedded_agent_bytes, EmbeddedAgentError, EphemeralAgentSpec};
use super::ephemeral_deploy::{
    build_cleanup_command, build_launch_command, build_preflight_command, build_scp_args,
    CleanupScheduler, EphemeralCleanupGuard,
};
use super::models::{RemoteTargetConfig, RemoteTargetValidationError};
use super::protocol::ProtocolError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlatform {
    pub os: String,
    pub architecture: String,
}

/// 远端端口转发规格：把远端 `remote_listen_port` 上收到的连接经 SSH 隧道送回
/// 桌面（SSH 客户端）本机的 `local_port`。用于让远端 CLI 的本地路由请求
/// （如 `127.0.0.1:15721`）落到桌面正在运行的代理服务上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalForwardSpec {
    pub remote_listen_port: u16,
    pub local_port: u16,
}

impl LocalForwardSpec {
    /// 桌面分发端口与远端 CLI 请求端口一致时的快捷构造。
    pub fn same_port(port: u16) -> Self {
        Self {
            remote_listen_port: port,
            local_port: port,
        }
    }

    /// 生成 OpenSSH `-R` 参数数组。远端转发默认只绑定远端 loopback，
    /// 正好匹配 CLI 的 `127.0.0.1:{port}` 请求地址。
    fn ssh_args(&self) -> [OsString; 2] {
        [
            OsString::from("-R"),
            OsString::from(format!(
                "{}:127.0.0.1:{}",
                self.remote_listen_port, self.local_port
            )),
        ]
    }
}

/// 只构造参数数组，不经过本地 shell。远端命令只能来自受控模板；扩展任意命令能力前必须
/// 增加独立白名单，不能把用户脚本直接传入该入口。
///
/// 无密码时保持 BatchMode=yes（禁止交互与 askpass）；提供密码时改为显式允许
/// publickey+password，配合 SSH_ASKPASS 完成无 TTY 密码认证。
pub fn build_ssh_args(
    target: &RemoteTargetConfig,
    remote_command: &[String],
) -> Result<Vec<OsString>, RemoteTargetValidationError> {
    let target = target.clone().normalize()?;
    let use_password = target.password.is_some();
    let mut args = vec![OsString::from("-o")];
    args.push(if use_password {
        OsString::from("BatchMode=no")
    } else {
        OsString::from("BatchMode=yes")
    });
    args.push(OsString::from("-o"));
    args.push(OsString::from("StrictHostKeyChecking=yes"));
    if use_password {
        args.push(OsString::from("-o"));
        args.push(OsString::from("PreferredAuthentications=publickey,password"));
        args.push(OsString::from("-o"));
        args.push(OsString::from("NumberOfPasswordPrompts=1"));
    }

    if let Some(username) = target.username {
        args.push(OsString::from("-l"));
        args.push(OsString::from(username));
    }
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    if let Some(identity_file) = target.identity_file {
        args.push(OsString::from("-i"));
        args.push(OsString::from(identity_file));
    }

    args.push(OsString::from("--"));
    args.push(OsString::from(target.host_alias));
    args.extend(remote_command.iter().map(OsString::from));
    Ok(args)
}

/// SSH_ASKPASS 辅助：把密码写入临时目录，生成的 .cmd 通过 `type` 读取并输出，
/// 避免 cmd 对 `& |>^%` 等字符的二次解释。OpenSSH 8.4+ 支持 SSH_ASKPASS_REQUIRE=force，
/// 无 TTY 时会强制调用 askpass 完成密码认证。
struct AskPassGuard {
    _dir: tempfile::TempDir,
    script_path: std::path::PathBuf,
}

impl AskPassGuard {
    fn create(password: &str) -> Result<Self, RemoteSshError> {
        let dir = tempfile::Builder::new()
            .prefix("cc-switch-askpass-")
            .tempdir()
            .map_err(RemoteSshError::AskPassStaging)?;
        let password_file = dir.path().join("password.txt");
        std::fs::write(&password_file, password.as_bytes())
            .map_err(RemoteSshError::AskPassStaging)?;
        let script_path = dir.path().join("askpass.cmd");
        std::fs::write(
            &script_path,
            format!("@echo off\r\ntype \"{}\"\r\n", password_file.display()).as_bytes(),
        )
        .map_err(RemoteSshError::AskPassStaging)?;
        Ok(Self {
            _dir: dir,
            script_path,
        })
    }

    fn apply(&self, command: &mut Command) {
        command
            .env("SSH_ASKPASS", &self.script_path)
            .env("SSH_ASKPASS_REQUIRE", "force");
    }
}

/// 扫描目标主机公钥,返回 (keyscan 原始输出, 指纹列表)。
fn scan_host_keys(target: &RemoteTargetConfig) -> Result<(String, Vec<String>), RemoteSshError> {
    let target = target.clone().normalize()?;
    let mut scan = Command::new("ssh-keyscan");
    if let Some(port) = target.port {
        scan.arg("-p").arg(port.to_string());
    }
    scan.arg(&target.host_alias);
    let output = scan.output().map_err(RemoteSshError::Spawn)?;
    if !output.status.success() {
        return Err(RemoteSshError::CommandFailed(sanitize_stderr(
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    let keys = String::from_utf8_lossy(&output.stdout).into_owned();
    if keys.trim().is_empty() {
        return Err(RemoteSshError::CommandFailed("未从服务器获取到主机密钥".to_string()));
    }
    let fingerprints = keys
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            Some(parts.get(1).copied().unwrap_or(trimmed).to_string())
        })
        .collect::<Vec<_>>();
    Ok((keys, fingerprints))
}

/// 仅读取目标主机公钥指纹,供前端在确认信任前展示(类似 XShell 首次连接确认)。
pub fn host_key_fingerprints(target: &RemoteTargetConfig) -> Result<Vec<String>, RemoteSshError> {
    scan_host_keys(target).map(|(_, fingerprints)| fingerprints)
}

/// 使用 `ssh-keyscan` 获取目标主机公钥并写入用户 known_hosts，实现类似 XShell 的
/// "首次连接信任主机密钥"。返回写入的密钥指纹。
pub fn trust_host_key(target: &RemoteTargetConfig) -> Result<Vec<String>, RemoteSshError> {
    let (keys, fingerprints) = scan_host_keys(target)?;
    let known_hosts = crate::config::get_home_dir().join(".ssh").join("known_hosts");
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent).map_err(RemoteSshError::Spawn)?;
    }
    let mut existing = std::fs::read_to_string(&known_hosts).unwrap_or_default();
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&keys);
    std::fs::write(&known_hosts, existing).map_err(RemoteSshError::Spawn)?;
    Ok(fingerprints)
}

/// 为任意 ssh/scp 子进程接入密码，返回需要搭配传给 build_ssh_args 的辅助对象。
fn prepare_password_auth(target: &RemoteTargetConfig) -> Result<Option<AskPassGuard>, RemoteSshError> {
    match target.password.as_deref() {
        Some(password) if !password.is_empty() => {
            Ok(Some(AskPassGuard::create(password)?))
        }
        _ => Ok(None),
    }
}

pub struct OpenSshSession {
    child: Child,
    protocol: RemoteSession<ChildStdout, ChildStdin>,
    _cleanup: EphemeralCleanupGuard,
    // 密码认证期间需要 askpass 脚本在子进程存续期间保持存在。
    _askpass: Option<AskPassGuard>,
    pub platform: RemotePlatform,
}

impl OpenSshSession {
    pub fn connect(target: &RemoteTargetConfig) -> Result<Self, RemoteSshError> {
        Self::connect_with_forward(target, None)
    }

    pub fn connect_with_forward(
        target: &RemoteTargetConfig,
        forward: Option<LocalForwardSpec>,
    ) -> Result<Self, RemoteSshError> {
        let target = target.clone().normalize()?;
        let platform = preflight(&target)?;
        let agent_bytes = select_agent_bytes(&platform.architecture)?;
        let spec = EphemeralAgentSpec::for_architecture(&platform.architecture, agent_bytes)
            .map_err(|error| RemoteSshError::AgentEmbeddedArtifactMissing {
                architecture: error.to_string(),
            })?;

        // 守卫必须在上传前创建：scp 失败时远端仍可能已经留下部分文件，需要兜底删除。
        let cleanup = EphemeralCleanupGuard::new(
            target.clone(),
            spec.remote_path.clone(),
            Arc::new(OpenSshCleanupScheduler),
        );

        // scp 只接受文件路径；嵌入字节先写入本地随机临时文件，上传完成后立即自动删除。
        let mut local_agent = tempfile::NamedTempFile::new().map_err(RemoteSshError::Staging)?;
        local_agent
            .write_all(agent_bytes)
            .map_err(RemoteSshError::Staging)?;
        local_agent.flush().map_err(RemoteSshError::Staging)?;
        let scp_args = build_scp_args(&target, local_agent.path(), &spec.remote_path)?;
        let mut upload_command = Command::new("scp");
        let upload_askpass = prepare_password_auth(&target)?;
        if let Some(askpass) = upload_askpass.as_ref() {
            askpass.apply(&mut upload_command);
        }
        let upload = upload_command
            .args(scp_args)
            .output()
            .map_err(|error| RemoteSshError::AgentUploadFailed(error.to_string()))?;
        if !upload.status.success() {
            return Err(RemoteSshError::AgentUploadFailed(sanitize_stderr(
                &String::from_utf8_lossy(&upload.stderr),
            )));
        }

        let args = build_ssh_args(&target, &[build_launch_command(&spec)])?;
        let mut command = Command::new("ssh");
        let askpass = prepare_password_auth(&target)?;
        if let Some(askpass) = askpass.as_ref() {
            askpass.apply(&mut command);
        }
        // 桌面代理运行时，把远端 CLI 的本地路由请求经隧道送回桌面代理。
        if let Some(forward) = forward {
            command.args(forward.ssh_args());
        }
        let mut child = command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(RemoteSshError::Spawn)?;
        let writer = child
            .stdin
            .take()
            .ok_or(RemoteSshError::MissingPipe("stdin"))?;
        let reader = child
            .stdout
            .take()
            .ok_or(RemoteSshError::MissingPipe("stdout"))?;
        let stderr = capture_stderr(child.stderr.take());

        let protocol = match RemoteSession::connect(reader, writer, env!("CARGO_PKG_VERSION")) {
            Ok(protocol) => protocol,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let stderr = stderr
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap_or_default();
                return Err(classify_agent_launch_failure(error, &stderr));
            }
        };

        Ok(Self {
            child,
            protocol,
            _cleanup: cleanup,
            _askpass: askpass,
            platform,
        })
    }

    pub fn invoke(
        &self,
        request_id: &str,
        command: &str,
        args: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, RemoteSshError> {
        let operation_id = uuid::Uuid::new_v4().to_string();
        self.protocol
            .invoke_with_id(request_id, &operation_id, command, args, timeout_ms)
            .map_err(RemoteSshError::Client)
    }
}

impl Drop for OpenSshSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // 子进程退出后字段按顺序释放，_cleanup 会调度一次独立 SSH 删除作为 trap 的兜底。
    }
}

pub fn preflight(target: &RemoteTargetConfig) -> Result<RemotePlatform, RemoteSshError> {
    let args = build_ssh_args(target, &[build_preflight_command()])?;
    let mut command = Command::new("ssh");
    let askpass = prepare_password_auth(target)?;
    if let Some(askpass) = askpass.as_ref() {
        askpass.apply(&mut command);
    }
    let output = command
        .args(args)
        .output()
        .map_err(RemoteSshError::Spawn)?;
    if !output.status.success() {
        return Err(classify_ssh_failure(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    parse_remote_platform(&String::from_utf8_lossy(&output.stdout))
}

fn select_agent_bytes(architecture: &str) -> Result<&'static [u8], RemoteSshError> {
    embedded_agent_bytes(architecture).map_err(|error| match error {
        EmbeddedAgentError::ArtifactMissing(architecture) => {
            RemoteSshError::AgentEmbeddedArtifactMissing { architecture }
        }
        EmbeddedAgentError::UnsupportedArchitecture(architecture) => {
            RemoteSshError::ArchitectureUnsupported(architecture)
        }
    })
}

fn capture_stderr(stderr: Option<impl Read + Send + 'static>) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut buffer);
        }
        if !buffer.trim().is_empty() {
            log::warn!("Remote Agent stderr: {}", sanitize_stderr(&buffer));
        }
        let _ = sender.send(buffer);
    });
    receiver
}

fn classify_agent_launch_failure(error: RemoteClientError, stderr: &str) -> RemoteSshError {
    let sanitized = sanitize_stderr(stderr);
    if sanitized.contains("AGENT_INTEGRITY_FAILED") {
        return RemoteSshError::AgentIntegrityFailed(sanitized);
    }
    if matches!(
        &error,
        RemoteClientError::UnexpectedFrame(_)
            | RemoteClientError::Protocol(ProtocolError::UnsupportedMajor(_))
    ) {
        return RemoteSshError::AgentIncompatible(error.to_string());
    }
    let message = if sanitized.is_empty() {
        error.to_string()
    } else {
        sanitized
    };
    RemoteSshError::AgentStartFailed(message)
}

struct OpenSshCleanupScheduler;

impl CleanupScheduler for OpenSshCleanupScheduler {
    fn schedule(&self, target: &RemoteTargetConfig, remote_path: &str) {
        let target = target.clone();
        let remote_path = remote_path.to_string();
        let Ok(args) = build_ssh_args(&target, &[build_cleanup_command(&remote_path)]) else {
            return;
        };
        // Drop 不能长期阻塞 UI；独立 ssh 进程负责兜底删除，远端 launch trap 是第一道清理。
        std::thread::spawn(move || {
            let mut command = Command::new("ssh");
            let askpass = match prepare_password_auth(&target) {
                Ok(askpass) => askpass,
                Err(error) => {
                    log::warn!("Failed to prepare cleanup SSH auth: {error}");
                    return;
                }
            };
            if let Some(askpass) = askpass.as_ref() {
                askpass.apply(&mut command);
            }
            if let Err(error) = command.args(args).status() {
                log::warn!("Failed to schedule remote Agent cleanup: {error}");
            }
        });
    }
}

pub fn parse_remote_platform(output: &str) -> Result<RemotePlatform, RemoteSshError> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let os = lines
        .next()
        .ok_or_else(|| RemoteSshError::InvalidPlatformOutput(output.to_string()))?;
    let architecture = lines
        .next()
        .ok_or_else(|| RemoteSshError::InvalidPlatformOutput(output.to_string()))?;
    if !os.eq_ignore_ascii_case("linux") {
        return Err(RemoteSshError::PlatformUnsupported(os.to_string()));
    }
    let architecture = match architecture {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => return Err(RemoteSshError::ArchitectureUnsupported(other.to_string())),
    };
    Ok(RemotePlatform {
        os: "linux".to_string(),
        architecture: architecture.to_string(),
    })
}

pub fn classify_ssh_failure(stderr: &str) -> RemoteSshError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("host key verification failed") || lower.contains("no host key is known") {
        return RemoteSshError::HostKeyNotTrusted;
    }
    if lower.contains("permission denied") || lower.contains("authentication failed") {
        return RemoteSshError::AuthenticationFailed;
    }
    if lower.contains("connection timed out")
        || lower.contains("connection refused")
        || lower.contains("could not resolve hostname")
        || lower.contains("no route to host")
    {
        return RemoteSshError::Unreachable(sanitize_stderr(stderr));
    }
    RemoteSshError::CommandFailed(sanitize_stderr(stderr))
}

fn sanitize_stderr(stderr: &str) -> String {
    stderr
        .chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .take(4_096)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteSshError {
    #[error("SSH 主机密钥尚未信任")]
    HostKeyNotTrusted,
    #[error("SSH 公钥认证失败")]
    AuthenticationFailed,
    #[error("远程服务器不可达: {0}")]
    Unreachable(String),
    #[error("远程平台不受支持: {0}")]
    PlatformUnsupported(String),
    #[error("远程架构不受支持: {0}")]
    ArchitectureUnsupported(String),
    #[error("远程平台探测输出无效: {0}")]
    InvalidPlatformOutput(String),
    #[error("SSH 命令失败: {0}")]
    CommandFailed(String),
    #[error("无法启动 OpenSSH: {0}")]
    Spawn(std::io::Error),
    #[error("SSH 子进程缺少 {0} 管道")]
    MissingPipe(&'static str),
    #[error("桌面构建缺少内嵌 Agent，架构: {architecture}")]
    AgentEmbeddedArtifactMissing { architecture: String },
    #[error("临时 Agent 上传失败: {0}")]
    AgentUploadFailed(String),
    #[error("临时 Agent 完整性校验失败: {0}")]
    AgentIntegrityFailed(String),
    #[error("临时 Agent 启动失败: {0}")]
    AgentStartFailed(String),
    #[error("临时 Agent 协议不兼容: {0}")]
    AgentIncompatible(String),
    #[error("无法暂存内嵌 Agent: {0}")]
    Staging(std::io::Error),
    #[error("无法暂存密码辅助脚本: {0}")]
    AskPassStaging(std::io::Error),
    #[error(transparent)]
    Validation(#[from] RemoteTargetValidationError),
    #[error(transparent)]
    Client(#[from] RemoteClientError),
}

impl RemoteSshError {
    pub fn code(&self) -> &str {
        match self {
            Self::HostKeyNotTrusted => "HOST_KEY_NOT_TRUSTED",
            Self::AuthenticationFailed => "AUTH_FAILED",
            Self::Unreachable(_) => "REMOTE_UNREACHABLE",
            Self::PlatformUnsupported(_) => "REMOTE_PLATFORM_UNSUPPORTED",
            Self::ArchitectureUnsupported(_) => "REMOTE_ARCH_UNSUPPORTED",
            Self::AgentEmbeddedArtifactMissing { .. } => "AGENT_EMBEDDED_ARTIFACT_MISSING",
            Self::AgentUploadFailed(_) => "AGENT_UPLOAD_FAILED",
            Self::AgentIntegrityFailed(_) => "AGENT_INTEGRITY_FAILED",
            Self::AgentStartFailed(_) => "AGENT_START_FAILED",
            Self::AgentIncompatible(_) => "AGENT_INCOMPATIBLE",
            Self::Validation(_) => "REMOTE_TARGET_INVALID",
            Self::Client(error) => error.code(),
            _ => "REMOTE_CONNECTION_ERROR",
        }
    }
}
