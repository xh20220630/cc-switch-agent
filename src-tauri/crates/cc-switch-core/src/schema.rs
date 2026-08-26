use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

/// 与桌面数据库 `src/database/mod.rs` 保持一致；Agent 不再维护独立版本号。
pub const DESKTOP_SCHEMA_VERSION: i32 = 17;

const CANONICAL_REQUIRED_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS providers (
    id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    name TEXT NOT NULL,
    settings_config TEXT NOT NULL,
    website_url TEXT,
    category TEXT,
    created_at INTEGER,
    sort_index INTEGER,
    notes TEXT,
    icon TEXT,
    icon_color TEXT,
    meta TEXT NOT NULL DEFAULT '{}',
    is_current BOOLEAN NOT NULL DEFAULT 0,
    in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (id, app_type)
);
CREATE TABLE IF NOT EXISTS provider_endpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    url TEXT NOT NULL,
    added_at INTEGER,
    FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS proxy_request_logs (
    request_id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    model TEXT NOT NULL,
    request_model TEXT,
    pricing_model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    input_token_semantics INTEGER NOT NULL DEFAULT 0,
    input_cost_usd TEXT NOT NULL DEFAULT '0',
    output_cost_usd TEXT NOT NULL DEFAULT '0',
    cache_read_cost_usd TEXT NOT NULL DEFAULT '0',
    cache_creation_cost_usd TEXT NOT NULL DEFAULT '0',
    total_cost_usd TEXT NOT NULL DEFAULT '0',
    latency_ms INTEGER NOT NULL,
    first_token_ms INTEGER,
    duration_ms INTEGER,
    status_code INTEGER NOT NULL,
    error_message TEXT,
    session_id TEXT,
    provider_type TEXT,
    is_streaming INTEGER NOT NULL DEFAULT 0,
    cost_multiplier TEXT NOT NULL DEFAULT '1.0',
    created_at INTEGER NOT NULL,
    data_source TEXT NOT NULL DEFAULT 'proxy'
);
CREATE INDEX IF NOT EXISTS idx_request_logs_provider
    ON proxy_request_logs(provider_id, app_type);
CREATE INDEX IF NOT EXISTS idx_request_logs_created_at
    ON proxy_request_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_request_logs_model
    ON proxy_request_logs(model);
CREATE INDEX IF NOT EXISTS idx_request_logs_session
    ON proxy_request_logs(session_id);
CREATE INDEX IF NOT EXISTS idx_request_logs_status
    ON proxy_request_logs(status_code);
CREATE TABLE IF NOT EXISTS model_pricing (
    model_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    input_cost_per_million TEXT NOT NULL,
    output_cost_per_million TEXT NOT NULL,
    cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
    cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
);
CREATE TABLE IF NOT EXISTS usage_daily_rollups (
    date TEXT NOT NULL,
    app_type TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model TEXT NOT NULL,
    request_model TEXT NOT NULL DEFAULT '',
    pricing_model TEXT NOT NULL DEFAULT '',
    request_count INTEGER NOT NULL DEFAULT 0,
    success_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    input_token_semantics INTEGER NOT NULL DEFAULT 0,
    total_cost_usd TEXT NOT NULL DEFAULT '0',
    avg_latency_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
);
CREATE TABLE IF NOT EXISTS session_log_sync (
    file_path TEXT PRIMARY KEY,
    last_modified INTEGER NOT NULL,
    last_line_offset INTEGER NOT NULL DEFAULT 0,
    last_synced_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS session_usage_dedup (
    data_source TEXT NOT NULL,
    request_id TEXT NOT NULL,
    semantic_id TEXT NOT NULL,
    has_entry_id INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (data_source, request_id)
);
CREATE INDEX IF NOT EXISTS idx_session_usage_dedup_semantic
ON session_usage_dedup(data_source, semantic_id, has_entry_id);
"#;

const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
    (
        "providers",
        &[
            "id",
            "app_type",
            "name",
            "settings_config",
            "website_url",
            "category",
            "created_at",
            "sort_index",
            "notes",
            "icon",
            "icon_color",
            "meta",
            "is_current",
            "in_failover_queue",
        ],
    ),
    (
        "provider_endpoints",
        &["id", "provider_id", "app_type", "url", "added_at"],
    ),
    (
        "proxy_request_logs",
        &[
            "request_id",
            "provider_id",
            "app_type",
            "model",
            "request_model",
            "pricing_model",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_creation_tokens",
            "input_token_semantics",
            "input_cost_usd",
            "output_cost_usd",
            "cache_read_cost_usd",
            "cache_creation_cost_usd",
            "total_cost_usd",
            "latency_ms",
            "first_token_ms",
            "duration_ms",
            "status_code",
            "error_message",
            "session_id",
            "provider_type",
            "is_streaming",
            "cost_multiplier",
            "created_at",
            "data_source",
        ],
    ),
    (
        "model_pricing",
        &[
            "model_id",
            "display_name",
            "input_cost_per_million",
            "output_cost_per_million",
            "cache_read_cost_per_million",
            "cache_creation_cost_per_million",
        ],
    ),
    (
        "usage_daily_rollups",
        &[
            "date",
            "app_type",
            "provider_id",
            "model",
            "request_model",
            "pricing_model",
            "request_count",
            "success_count",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_creation_tokens",
            "input_token_semantics",
            "total_cost_usd",
            "avg_latency_ms",
        ],
    ),
    (
        "session_log_sync",
        &[
            "file_path",
            "last_modified",
            "last_line_offset",
            "last_synced_at",
        ],
    ),
];

/// 配置每条 Core 连接的并发行为。busy timeout 有界等待另一个 CC Switch 进程释放写锁，
/// 避免瞬时竞争直接失败，也避免 SSH 会话无限挂起。
pub(crate) fn configure_connection(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

/// 全新远端没有数据库时只创建本阶段使用的规范表，列名和约束与桌面 v16 一致。
/// 已有数据库绝不能经过此入口，防止 `CREATE TABLE IF NOT EXISTS` 掩盖缺列或旧私有 schema。
pub(crate) fn initialize_new_database(connection: &Connection) -> Result<(), SchemaError> {
    connection.execute_batch(CANONICAL_REQUIRED_SCHEMA)?;
    connection.pragma_update(None, "user_version", DESKTOP_SCHEMA_VERSION)?;
    Ok(())
}

/// 将桌面端仍在支持范围内的既有数据库迁移到当前版本。
///
/// 该入口由桌面进程和临时 Agent 共同调用，是 v10 之后迁移 SQL 的唯一事实来源。
/// 调用方必须在进入此函数前完成一致性备份；这里用 SAVEPOINT 保证任一步失败时，
/// 业务数据、DDL 与 `user_version` 一起回滚，不能留下“版本已新、结构仍旧”的半迁移库。
pub fn migrate_supported_database(
    connection: &Connection,
    codex_dir: &Path,
) -> Result<(), SchemaError> {
    let detected = read_schema_version(connection)?;
    if !(10..=DESKTOP_SCHEMA_VERSION).contains(&detected) {
        return Err(SchemaError::Incompatible {
            detected,
            supported: DESKTOP_SCHEMA_VERSION,
            reason: "仅支持从桌面 schema v10 及以上版本安全迁移".to_string(),
        });
    }
    if detected == DESKTOP_SCHEMA_VERSION {
        return Ok(());
    }

    connection.execute("SAVEPOINT core_schema_migration", [])?;
    let result = (|| {
        let mut version = detected;
        while version < DESKTOP_SCHEMA_VERSION {
            match version {
                10 => migrate_v10_to_v11(connection)?,
                11 => migrate_v11_to_v12(connection)?,
                12 => migrate_v12_to_v13(connection)?,
                13 => migrate_v13_to_v14(connection)?,
                14 => migrate_v14_to_v15(connection)?,
                15 => reset_codex_usage_on_connection(connection, codex_dir)?,
                16 => migrate_v16_to_v17(connection)?,
                _ => {
                    return Err(SchemaError::Incompatible {
                        detected: version,
                        supported: DESKTOP_SCHEMA_VERSION,
                        reason: "迁移链中存在未知版本".to_string(),
                    });
                }
            }
            version += 1;
            connection.pragma_update(None, "user_version", version)?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            connection.execute("RELEASE core_schema_migration", [])?;
            Ok(())
        }
        Err(error) => {
            connection
                .execute("ROLLBACK TO core_schema_migration", [])
                .ok();
            connection.execute("RELEASE core_schema_migration", []).ok();
            Err(error)
        }
    }
}

/// v10 -> v11：Usage 汇总增加请求模型与计价模型维度，明细增加计价模型。
/// SQLite 不能原地修改主键，因此必须重建汇总表；历史行无法恢复别名，统一填空串。
fn migrate_v10_to_v11(connection: &Connection) -> Result<(), SchemaError> {
    if table_exists(connection, "proxy_request_logs")? {
        add_column_if_missing(connection, "proxy_request_logs", "pricing_model", "TEXT")?;
    }
    if !table_exists(connection, "usage_daily_rollups")? {
        return Ok(());
    }

    connection.execute_batch(
        "ALTER TABLE usage_daily_rollups RENAME TO usage_daily_rollups_v10;
         CREATE TABLE usage_daily_rollups (
             date TEXT NOT NULL,
             app_type TEXT NOT NULL,
             provider_id TEXT NOT NULL,
             model TEXT NOT NULL,
             request_model TEXT NOT NULL DEFAULT '',
             pricing_model TEXT NOT NULL DEFAULT '',
             request_count INTEGER NOT NULL DEFAULT 0,
             success_count INTEGER NOT NULL DEFAULT 0,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens INTEGER NOT NULL DEFAULT 0,
             cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
             total_cost_usd TEXT NOT NULL DEFAULT '0',
             avg_latency_ms INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
         );
         INSERT INTO usage_daily_rollups
             (date, app_type, provider_id, model, request_model, pricing_model,
              request_count, success_count, input_tokens, output_tokens,
              cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms)
         SELECT date, app_type, provider_id, model, '', '',
              request_count, success_count, input_tokens, output_tokens,
              cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
         FROM usage_daily_rollups_v10;
         DROP TABLE usage_daily_rollups_v10;",
    )?;
    Ok(())
}

/// v11 -> v12：补齐项目 Profiles 表；`IF NOT EXISTS` 保证迁移可在桌面预建表后运行。
fn migrate_v11_to_v12(connection: &Connection) -> Result<(), SchemaError> {
    connection.execute(
        "CREATE TABLE IF NOT EXISTS profiles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            payload TEXT NOT NULL,
            sort_order INTEGER,
            created_at INTEGER,
            updated_at INTEGER
        )",
        [],
    )?;
    Ok(())
}

/// v12 -> v13：记录输入 token 是否包含缓存写入；0 保留旧数据的未知语义。
fn migrate_v12_to_v13(connection: &Connection) -> Result<(), SchemaError> {
    if table_exists(connection, "proxy_request_logs")? {
        add_column_if_missing(
            connection,
            "proxy_request_logs",
            "input_token_semantics",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    if table_exists(connection, "usage_daily_rollups")? {
        add_column_if_missing(
            connection,
            "usage_daily_rollups",
            "input_token_semantics",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

/// v13 -> v14：重建代理配置约束，使 Grok Build 拥有独立配置行。
/// 复制列按旧库实际形状动态选择，兼容发布版本和历史开发版本之间的字段差异。
fn migrate_v13_to_v14(connection: &Connection) -> Result<(), SchemaError> {
    if !table_exists(connection, "proxy_config")? {
        return Ok(());
    }

    connection.execute("DROP TABLE IF EXISTS proxy_config_v14", [])?;
    connection.execute(
        "CREATE TABLE proxy_config_v14 (
            app_type TEXT PRIMARY KEY CHECK (app_type IN ('claude','codex','gemini','grokbuild')),
            proxy_enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 15721,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 0,
            auto_failover_enabled INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
            streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
            non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
            circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
            circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
            circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
            circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
            circuit_min_requests INTEGER NOT NULL DEFAULT 10,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response',
            live_takeover_active INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        [],
    )?;

    let copied_columns = [
        ("app_type", "'claude'"),
        ("proxy_enabled", "0"),
        ("listen_address", "'127.0.0.1'"),
        ("listen_port", "15721"),
        ("enable_logging", "1"),
        ("enabled", "0"),
        ("auto_failover_enabled", "0"),
        ("max_retries", "3"),
        ("streaming_first_byte_timeout", "60"),
        ("streaming_idle_timeout", "120"),
        ("non_streaming_timeout", "600"),
        ("circuit_failure_threshold", "4"),
        ("circuit_success_threshold", "2"),
        ("circuit_timeout_seconds", "60"),
        ("circuit_error_rate_threshold", "0.6"),
        ("circuit_min_requests", "10"),
        ("default_cost_multiplier", "'1'"),
        ("pricing_model_source", "'response'"),
        ("live_takeover_active", "0"),
        ("created_at", "datetime('now')"),
        ("updated_at", "datetime('now')"),
    ]
    .into_iter()
    .map(|(column, fallback)| {
        column_exists(connection, "proxy_config", column).map(|exists| {
            if exists {
                format!("\"{column}\"")
            } else {
                fallback.to_string()
            }
        })
    })
    .collect::<Result<Vec<_>, rusqlite::Error>>()?
    .join(", ");

    connection.execute(
        &format!(
            "INSERT INTO proxy_config_v14 (
                app_type, proxy_enabled, listen_address, listen_port, enable_logging,
                enabled, auto_failover_enabled, max_retries,
                streaming_first_byte_timeout, streaming_idle_timeout, non_streaming_timeout,
                circuit_failure_threshold, circuit_success_threshold, circuit_timeout_seconds,
                circuit_error_rate_threshold, circuit_min_requests,
                default_cost_multiplier, pricing_model_source, live_takeover_active,
                created_at, updated_at
            ) SELECT {copied_columns} FROM proxy_config"
        ),
        [],
    )?;
    connection.execute("DROP TABLE proxy_config", [])?;
    connection.execute("ALTER TABLE proxy_config_v14 RENAME TO proxy_config", [])?;
    connection.execute(
        "INSERT OR IGNORE INTO proxy_config (app_type) VALUES ('grokbuild')",
        [],
    )?;
    Ok(())
}

/// v14 -> v15：为统一 Skills/MCP 管理补齐 Grok Build 启用标记。
fn migrate_v14_to_v15(connection: &Connection) -> Result<(), SchemaError> {
    if table_exists(connection, "mcp_servers")? {
        add_column_if_missing(
            connection,
            "mcp_servers",
            "enabled_grokbuild",
            "BOOLEAN NOT NULL DEFAULT 0",
        )?;
    }
    if table_exists(connection, "skills")? {
        add_column_if_missing(
            connection,
            "skills",
            "enabled_grokbuild",
            "BOOLEAN NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

/// 清除需要按分叉历史重新导入的 Codex 会话用量。
///
/// v15 -> v16 迁移和桌面端手动重建都必须使用这一入口；Provider 代理日志、
/// Gemini/Claude 会话和非 rollout cursor 必须原样保留。
pub fn reset_codex_usage_on_connection(
    connection: &Connection,
    codex_dir: &Path,
) -> Result<(), SchemaError> {
    if table_exists(connection, "proxy_request_logs")?
        && column_exists(connection, "proxy_request_logs", "data_source")?
    {
        connection.execute(
            "DELETE FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
        )?;
    }
    if table_exists(connection, "usage_daily_rollups")?
        && column_exists(connection, "usage_daily_rollups", "provider_id")?
    {
        connection.execute(
            "DELETE FROM usage_daily_rollups WHERE provider_id = '_codex_session'",
            [],
        )?;
    }
    if table_exists(connection, "session_log_sync")?
        && column_exists(connection, "session_log_sync", "file_path")?
    {
        let paths = {
            let mut statement = connection.prepare("SELECT file_path FROM session_log_sync")?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            paths
        };
        for file_path in paths
            .into_iter()
            .filter(|path| is_codex_cursor_path(path, codex_dir))
        {
            connection.execute(
                "DELETE FROM session_log_sync WHERE file_path = ?1",
                [file_path],
            )?;
        }
    }
    Ok(())
}

/// v16 -> v17: preserve session request identities after detail rollup.
/// 明细 prune 后 request_id → semantic_id 映射需要持久化账本，否则去重失效。
fn migrate_v16_to_v17(connection: &Connection) -> Result<(), SchemaError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_usage_dedup (
            data_source TEXT NOT NULL,
            request_id TEXT NOT NULL,
            semantic_id TEXT NOT NULL,
            has_entry_id INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (data_source, request_id)
         );
         CREATE INDEX IF NOT EXISTS idx_session_usage_dedup_semantic
         ON session_usage_dedup(data_source, semantic_id, has_entry_id);",
    )?;
    Ok(())
}

fn is_codex_cursor_path(file_path: &str, codex_dir: &Path) -> bool {
    let path = Path::new(file_path);
    let file_name = file_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if !is_rollout_filename(file_name) {
        return false;
    }
    if path.starts_with(codex_dir.join("sessions"))
        || path.starts_with(codex_dir.join("archived_sessions"))
    {
        return true;
    }
    // CODEX_HOME 改动后旧文件可能已不存在；目录段与 UUID rollout 文件名必须同时命中，
    // 避免仅凭路径子串误删 Gemini 或 Claude importer 的同步游标。
    file_path
        .replace('\\', "/")
        .split('/')
        .any(|segment| matches!(segment, "sessions" | "archived_sessions"))
}

fn is_rollout_filename(file_name: &str) -> bool {
    if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
        return false;
    }
    let stem = file_name.trim_end_matches(".jsonl");
    stem.get(stem.len().saturating_sub(36)..)
        .is_some_and(|candidate| uuid::Uuid::parse_str(candidate).is_ok())
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
}

fn column_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        rusqlite::params![table, column],
        |row| row.get(0),
    )
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    if column_exists(connection, table, column)? {
        return Ok(());
    }
    // 表名、列名和定义只来自迁移代码中的编译期常量，不接收 RPC 或数据库内容。
    connection.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

/// 迁移完成后执行严格结构检查；版本正确但缺列的损坏库仍必须在业务写入前拒绝。
pub(crate) fn validate_existing_database(connection: &Connection) -> Result<(), SchemaError> {
    let detected = read_schema_version(connection)?;
    if detected != DESKTOP_SCHEMA_VERSION {
        return Err(SchemaError::Incompatible {
            detected,
            supported: DESKTOP_SCHEMA_VERSION,
            reason: "user_version 与 Agent 支持版本不一致".to_string(),
        });
    }

    for (table, required) in REQUIRED_COLUMNS {
        let actual = table_columns(connection, table)?;
        if actual.is_empty() {
            return Err(SchemaError::Incompatible {
                detected,
                supported: DESKTOP_SCHEMA_VERSION,
                reason: format!("缺少必需表 {table}"),
            });
        }
        if let Some(column) = required.iter().find(|column| !actual.contains(**column)) {
            return Err(SchemaError::Incompatible {
                detected,
                supported: DESKTOP_SCHEMA_VERSION,
                reason: format!("表 {table} 缺少必需列 {column}"),
            });
        }
    }
    Ok(())
}

pub(crate) fn read_schema_version(connection: &Connection) -> Result<i32, rusqlite::Error> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>, rusqlite::Error> {
    // 表名来自上方编译期常量，不接收 RPC 或用户输入；SQLite 的 PRAGMA 表名无法参数绑定。
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get(1))?;
    rows.collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("远端数据库结构不兼容: detected={detected}, supported={supported}, reason={reason}")]
    Incompatible {
        detected: i32,
        supported: i32,
        reason: String,
    },
    #[error("远端数据库结构检查失败: {0}")]
    Database(#[from] rusqlite::Error),
}
