use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::backup::Backup;
use rusqlite::Connection;

use crate::error::CoreError;
use crate::provider::live::TargetPlatform;
use crate::schema::{
    configure_connection, initialize_new_database, migrate_supported_database, read_schema_version,
    validate_existing_database, DESKTOP_SCHEMA_VERSION,
};

/// Agent 可安全构造的无界面状态，只持有目标主机数据库与显式 HOME。
///
/// HOME 不从进程全局环境变量重复读取，防止测试、桌面适配和并发远程会话互相污染。
pub struct HeadlessState {
    connection: Mutex<Connection>,
    home: PathBuf,
    platform: TargetPlatform,
}

impl HeadlessState {
    /// 打开目标 HOME 的真实数据库；新库直接初始化，桌面 v10+ 旧库先做一致备份再迁移。
    /// 更早、未来或版本正确但结构损坏的数据库仍在任何业务写入前拒绝。
    pub fn open(home: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_platform(home, TargetPlatform::current())
    }

    /// 使用显式目标平台打开数据库；桌面测试可模拟远端 Linux 的条件能力。
    pub fn open_with_platform(
        home: impl AsRef<Path>,
        platform: TargetPlatform,
    ) -> Result<Self, CoreError> {
        let home = home.as_ref().to_path_buf();
        let data_dir = home.join(".cc-switch");
        std::fs::create_dir_all(&data_dir).map_err(|source| CoreError::Io {
            path: data_dir.clone(),
            source,
        })?;
        let database_path = data_dir.join("cc-switch.db");
        let is_new = !database_path.exists();
        let connection = Connection::open(&database_path)?;
        configure_connection(&connection)?;
        if is_new {
            initialize_new_database(&connection)?;
        } else {
            let detected = read_schema_version(&connection)?;
            if (10..DESKTOP_SCHEMA_VERSION).contains(&detected) {
                // 备份先于任何 DDL，且使用当前连接的在线快照覆盖 WAL 内容；迁移失败时
                // 备份仍保留，便于用户用桌面端恢复，绝不能退化成直接复制主数据库文件。
                backup_before_schema_migration(&connection, &data_dir, detected)?;
                migrate_supported_database(&connection, &home.join(".codex"))?;
            }
            validate_existing_database(&connection)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
            home,
            platform,
        })
    }

    /// 构造与磁盘新库相同 schema 的隔离状态，供事务和协议测试使用。
    pub fn memory(home: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::memory_with_platform(home, TargetPlatform::current())
    }

    /// 构造指定目标平台的内存状态；只用于验证跨平台能力分支，不读取进程全局环境。
    pub fn memory_with_platform(
        home: impl AsRef<Path>,
        platform: TargetPlatform,
    ) -> Result<Self, CoreError> {
        let connection = Connection::open_in_memory()?;
        configure_connection(&connection)?;
        initialize_new_database(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            home: home.as_ref().to_path_buf(),
            platform,
        })
    }

    /// 公开只读连接闭包供桌面适配层复用 Core 查询，避免泄漏锁守卫和连接所有权。
    pub fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let connection = self.connection()?;
        operation(&connection)
    }

    /// 读取当前 schema 版本，供握手兼容判断和诊断使用。
    pub fn schema_version(&self) -> Result<i32, CoreError> {
        self.with_connection(|connection| Ok(read_schema_version(connection)?))
    }

    /// 返回显式目标 HOME；live writer 只能基于此路径，不能再次读取进程环境变量。
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// 返回建连时固定的目标平台，避免 live writer 错用桌面宿主机的编译平台。
    pub fn platform(&self) -> TargetPlatform {
        self.platform
    }

    /// Provider/Usage 写事务仍由各领域服务管理；仅在 crate 内暴露锁守卫。
    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, CoreError> {
        self.connection.lock().map_err(|_| CoreError::StatePoisoned)
    }
}

/// 为 Agent 自动迁移创建唯一的一致性快照。
///
/// 备份目录沿用桌面端的 `.cc-switch/backups`，但文件名带来源和版本范围，避免与
/// 常规定时备份混淆；若备份失败则中止迁移，宁可拒绝远程操作也不冒数据不可恢复风险。
fn backup_before_schema_migration(
    connection: &Connection,
    data_dir: &Path,
    detected: i32,
) -> Result<PathBuf, CoreError> {
    let backup_dir = data_dir.join("backups");
    std::fs::create_dir_all(&backup_dir).map_err(|source| CoreError::Io {
        path: backup_dir.clone(),
        source,
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup_path = backup_dir.join(format!(
        "cc-switch-agent-pre-migration-v{detected}-to-v{DESKTOP_SCHEMA_VERSION}-{nonce}.db"
    ));

    let mut destination = Connection::open(&backup_path)?;
    {
        let backup = Backup::new(connection, &mut destination)?;
        backup.run_to_completion(128, Duration::from_millis(5), None)?;
    }
    drop(destination);
    Ok(backup_path)
}
