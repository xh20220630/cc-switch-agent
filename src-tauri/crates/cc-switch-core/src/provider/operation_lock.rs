//! 按目标 HOME 串行化供应商写入，覆盖同一进程的线程和多个 SSH Agent 进程。

use std::fs::{File, OpenOptions};
use std::time::{Duration, Instant};

use crate::{CoreError, HeadlessState};

/// 每次独立打开文件，避免复用/克隆文件句柄导致平台锁语义不同；关闭句柄自动释放锁。
/// 锁文件不能删除，否则等待者可能锁住旧 inode，让后来的进程绕过互斥。
pub(super) fn acquire(state: &HeadlessState) -> Result<File, CoreError> {
    let directory = state.home().join(".cc-switch");
    std::fs::create_dir_all(&directory).map_err(|source| CoreError::Io {
        path: directory.clone(),
        source,
    })?;
    let path = directory.join("provider-operation.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| CoreError::Io {
            path: path.clone(),
            source,
        })?;
    let started = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                // 保持在短 RPC 的超时预算内，竞争超时不执行任何数据库或 live 写入。
                if started.elapsed() >= Duration::from_secs(5) {
                    return Err(CoreError::DatabaseBusy);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(source) => return Err(CoreError::Io { path, source }),
        }
    }
}
