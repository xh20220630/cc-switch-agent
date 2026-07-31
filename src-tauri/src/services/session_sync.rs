//! 桌面会话同步的进程协调。
//!
//! Usage 解析、写入和迁移清理已统一迁到 `cc-switch-core`；本模块只保留
//! Tauri 进程级异步互斥，防止桌面端并发执行多轮会话同步。

use std::sync::OnceLock;

pub fn session_sync_mutex() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
