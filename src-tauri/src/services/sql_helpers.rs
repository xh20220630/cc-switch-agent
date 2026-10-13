//! 桌面 Usage SQL 语义由 cc-switch-core 统一维护；本模块仅保留既有 crate 内导入路径。

// 上游的生产查询也使用 legacy 语义常量，不能再只在测试构建中导出。
pub(crate) use cc_switch_core::INPUT_TOKEN_SEMANTICS_LEGACY;
pub(crate) use cc_switch_core::{
    fresh_input_sql, is_cache_inclusive_app, INPUT_TOKEN_SEMANTICS_FRESH,
    INPUT_TOKEN_SEMANTICS_TOTAL,
};

/// 单行的「真实消耗 Tokens」：新增输入 + 输出 + 缓存写入 + 缓存命中。
///
/// 与 `usage_stats::derive_real_total_and_hit_rate`（指标卡 / 趋势图）同口径，
/// 供应商、模型等分组统计的 `total_tokens` 都用它，保证各行之和等于顶部总数。
pub fn real_total_tokens_sql(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    let fresh_input = fresh_input_sql(alias);
    format!(
        "(({fresh_input}) + {prefix}output_tokens \
          + {prefix}cache_creation_tokens + {prefix}cache_read_tokens)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn shared_fresh_input_sql_keeps_desktop_semantics() {
        let connection = Connection::open_in_memory().expect("创建内存数据库");
        connection
            .execute_batch(
                "CREATE TABLE proxy_request_logs (
                    app_type TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    cache_read_tokens INTEGER NOT NULL,
                    cache_creation_tokens INTEGER NOT NULL,
                    input_token_semantics INTEGER NOT NULL
                 );
                 INSERT INTO proxy_request_logs VALUES ('codex', 1000, 300, 200, 1);
                 INSERT INTO proxy_request_logs VALUES ('claude', 100, 900, 0, 2);",
            )
            .expect("写入 token fixture");
        let expression = fresh_input_sql("l");
        let sql = format!("SELECT SUM({expression}) FROM proxy_request_logs l");
        let total: i64 = connection
            .query_row(&sql, [], |row| row.get(0))
            .expect("计算 fresh input");
        assert_eq!(total, 600);
        assert!(is_cache_inclusive_app("codex"));
        assert!(!is_cache_inclusive_app("claude"));
        assert_eq!(INPUT_TOKEN_SEMANTICS_LEGACY, 0);
    }
}
