use std::path::Path;

use cc_switch_core::{CoreError, HeadlessState, SchemaError, DESKTOP_SCHEMA_VERSION};
use rusqlite::Connection;

const CANONICAL_REQUIRED_SCHEMA: &str = r#"
CREATE TABLE providers (
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
CREATE TABLE provider_endpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    url TEXT NOT NULL,
    added_at INTEGER,
    FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
);
CREATE TABLE proxy_request_logs (
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
CREATE TABLE model_pricing (
    model_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    input_cost_per_million TEXT NOT NULL,
    output_cost_per_million TEXT NOT NULL,
    cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
    cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
);
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
    input_token_semantics INTEGER NOT NULL DEFAULT 0,
    total_cost_usd TEXT NOT NULL DEFAULT '0',
    avg_latency_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
);
CREATE TABLE session_log_sync (
    file_path TEXT PRIMARY KEY,
    last_modified INTEGER NOT NULL,
    last_line_offset INTEGER NOT NULL DEFAULT 0,
    last_synced_at INTEGER NOT NULL,
    last_byte_offset INTEGER,
    last_tail_fingerprint INTEGER
);
"#;

#[test]
fn opens_existing_v16_schema_without_mutating_it() {
    let home = tempfile::tempdir().expect("创建临时 HOME");
    let db_path = database_path(home.path());
    let connection = create_database(&db_path);
    connection
        .execute_batch(CANONICAL_REQUIRED_SCHEMA)
        .expect("创建桌面 v16 fixture");
    connection
        .pragma_update(None, "user_version", DESKTOP_SCHEMA_VERSION)
        .expect("设置桌面 schema 版本");
    drop(connection);

    let state = HeadlessState::open(home.path()).expect("打开桌面规范数据库");
    assert_eq!(
        state.schema_version().expect("读取 schema 版本"),
        DESKTOP_SCHEMA_VERSION
    );
    drop(state);

    let reopened = Connection::open(db_path).expect("重新打开 fixture");
    assert!(column_exists(&reopened, "providers", "app_type"));
    assert!(!column_exists(&reopened, "providers", "app"));
    assert!(!table_exists(&reopened, "current_providers"));
}

#[test]
fn migrates_existing_desktop_v10_with_consistent_backup_and_preserves_business_data() {
    let home = tempfile::tempdir().expect("创建临时 HOME");
    let db_path = database_path(home.path());
    let connection = create_database(&db_path);
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("启用 WAL 以验证在线备份包含未合并内容");

    // 该 fixture 保留 v10 到 v16 之间真正发生变化的表形状，同时包含远程列表与
    // Usage 查询依赖的数据。后续新增迁移时应扩充此处，避免 Agent 只更新版本号。
    connection
        .execute_batch(
            r#"
            CREATE TABLE providers (
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
            CREATE TABLE provider_endpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                url TEXT NOT NULL,
                added_at INTEGER
            );
            CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                model TEXT NOT NULL,
                request_model TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
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
            CREATE TABLE model_pricing (
                model_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                input_cost_per_million TEXT NOT NULL,
                output_cost_per_million TEXT NOT NULL,
                cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
                cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
            );
            CREATE TABLE usage_daily_rollups (
                date TEXT NOT NULL,
                app_type TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model TEXT NOT NULL,
                request_count INTEGER NOT NULL DEFAULT 0,
                success_count INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                total_cost_usd TEXT NOT NULL DEFAULT '0',
                avg_latency_ms INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, app_type, provider_id, model)
            );
            CREATE TABLE session_log_sync (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_line_offset INTEGER NOT NULL DEFAULT 0,
                last_synced_at INTEGER NOT NULL
            );
            CREATE TABLE proxy_config (
                app_type TEXT PRIMARY KEY CHECK (app_type IN ('claude','codex','gemini')),
                enabled INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 3
            );
            CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY,
                enabled_codex BOOLEAN NOT NULL DEFAULT 0
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                enabled_codex BOOLEAN NOT NULL DEFAULT 0
            );

            INSERT INTO providers
                (id, app_type, name, settings_config, meta, is_current)
            VALUES ('remote-provider', 'claude', 'Remote Provider', '{}', '{}', 1);
            INSERT INTO usage_daily_rollups
                (date, app_type, provider_id, model, request_count, success_count,
                 input_tokens, output_tokens, total_cost_usd, avg_latency_ms)
            VALUES ('2026-07-30', 'claude', 'remote-provider', 'legacy-model',
                    7, 6, 1000, 500, '0.07', 120);
            INSERT INTO proxy_config (app_type, enabled, max_retries)
            VALUES ('codex', 1, 9);
            INSERT INTO mcp_servers (id, enabled_codex) VALUES ('remote-mcp', 1);
            INSERT INTO skills (id, enabled_codex) VALUES ('remote-skill', 1);
            PRAGMA user_version = 10;
            "#,
        )
        .expect("创建桌面 v10 fixture");

    // 保持写入连接存活，让 Provider/Usage 仍可能位于 WAL；Agent 必须从另一连接
    // 完成一致性备份与迁移，不能依赖关闭数据库触发 checkpoint。
    let state = HeadlessState::open(home.path()).expect("Agent 应安全迁移桌面 v10 数据库");
    assert_eq!(
        state.schema_version().expect("读取迁移后 schema 版本"),
        DESKTOP_SCHEMA_VERSION
    );
    drop(state);
    drop(connection);

    let migrated = Connection::open(&db_path).expect("打开迁移后的数据库");
    let provider: (String, i64) = migrated
        .query_row(
            "SELECT name, is_current FROM providers WHERE id = 'remote-provider'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("读取迁移后的 Provider");
    assert_eq!(provider, ("Remote Provider".to_string(), 1));
    let usage: (String, String, i64, String) = migrated
        .query_row(
            "SELECT request_model, pricing_model, request_count, total_cost_usd
             FROM usage_daily_rollups WHERE provider_id = 'remote-provider'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("读取迁移后的 Usage 汇总");
    assert_eq!(usage, (String::new(), String::new(), 7, "0.07".to_string()));
    assert!(column_exists(
        &migrated,
        "usage_daily_rollups",
        "input_token_semantics"
    ));
    assert!(table_exists(&migrated, "profiles"));
    assert_eq!(
        migrated
            .query_row(
                "SELECT enabled, max_retries FROM proxy_config WHERE app_type = 'codex'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("读取迁移后的代理配置"),
        (1, 9)
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'grokbuild'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("读取 Grok Build 代理配置"),
        1
    );
    assert!(column_exists(&migrated, "mcp_servers", "enabled_grokbuild"));
    assert!(column_exists(&migrated, "skills", "enabled_grokbuild"));
    drop(migrated);

    // 备份必须是迁移前的一致快照：版本和旧 rollup 形状都不能被 WAL 或后续 DDL 污染。
    let backup_dir = home.path().join(".cc-switch").join("backups");
    let backups = std::fs::read_dir(&backup_dir)
        .expect("迁移前应创建备份目录")
        .collect::<Result<Vec<_>, _>>()
        .expect("读取迁移前备份");
    assert_eq!(backups.len(), 1, "一次迁移只应生成一个安全备份");
    let backup = Connection::open(backups[0].path()).expect("打开迁移前备份");
    let backup_version: i32 = backup
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("读取备份版本");
    assert_eq!(backup_version, 10);
    assert!(!column_exists(
        &backup,
        "usage_daily_rollups",
        "request_model"
    ));
    assert_eq!(
        backup
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("读取备份 Provider"),
        1
    );
}

#[test]
fn rejects_legacy_agent_schema_before_business_writes() {
    let home = tempfile::tempdir().expect("创建临时 HOME");
    let db_path = database_path(home.path());
    let connection = create_database(&db_path);
    connection
        .execute_batch(
            "CREATE TABLE providers (
                app TEXT NOT NULL,
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                settings_config TEXT NOT NULL,
                PRIMARY KEY (app, id)
             );
             CREATE TABLE current_providers (
                app TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL
             );
             PRAGMA user_version = 1;",
        )
        .expect("创建旧 Agent schema");
    drop(connection);

    let error = match HeadlessState::open(home.path()) {
        Ok(_) => panic!("旧 Agent schema 必须在业务写入前被拒绝"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        CoreError::Schema(SchemaError::Incompatible {
            detected: 1,
            supported: DESKTOP_SCHEMA_VERSION,
            ..
        })
    ));
}

#[test]
fn creates_canonical_required_schema_for_a_new_remote_home() {
    let home = tempfile::tempdir().expect("创建临时 HOME");
    let state = HeadlessState::open(home.path()).expect("初始化全新远端数据库");
    assert_eq!(
        state.schema_version().expect("读取新库版本"),
        DESKTOP_SCHEMA_VERSION
    );
    drop(state);

    let connection = Connection::open(database_path(home.path())).expect("打开新数据库");
    for table in [
        "providers",
        "provider_endpoints",
        "proxy_request_logs",
        "model_pricing",
        "usage_daily_rollups",
        "session_log_sync",
    ] {
        assert!(table_exists(&connection, table), "缺少规范表 {table}");
    }
    assert!(column_exists(&connection, "providers", "app_type"));
    assert!(column_exists(&connection, "providers", "is_current"));
    assert!(!column_exists(&connection, "providers", "app"));
    assert!(!table_exists(&connection, "current_providers"));
}

fn database_path(home: &Path) -> std::path::PathBuf {
    home.join(".cc-switch").join("cc-switch.db")
}

fn create_database(path: &Path) -> Connection {
    std::fs::create_dir_all(path.parent().expect("数据库目录")).expect("创建数据库目录");
    Connection::open(path).expect("创建数据库")
}

fn table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )
        .expect("检查表")
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> bool {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("读取列信息");
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("查询列信息");
    let found = columns
        .map(|name| name.expect("读取列名"))
        .any(|name| name == column);
    drop(statement);
    found
}
