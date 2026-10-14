//! 无界面会话计时：只使用日志中的请求边界和输出事件，绝不以同步时刻代替请求耗时。
//! Codex 规则与桌面 RequestTimer 保持一致；估算值保留来源标记，由界面显示 ≈。

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// 旧 Agent 的最小库可能没有 settings；沿用桌面同名元信息表保存计时解析版本。
/// 不清除扫描游标或计费行，修订号只决定是否对已同步文件补一次计时。
pub(super) fn revision_requires_replay(
    connection: &Connection,
    key: &str,
) -> rusqlite::Result<bool> {
    connection.execute(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )?;
    let revision = connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(revision.as_deref() != Some("1"))
}

pub(super) fn mark_revision_current(connection: &Connection, key: &str) -> rusqlite::Result<()> {
    revision_requires_replay(connection, key)?;
    // 已完成的版本不重复写入，避免无变化的后台扫描使日志计数缓存持续失效。
    connection.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1')
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        WHERE settings.value IS NULL OR settings.value <> excluded.value",
        [key],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EstimatedTiming {
    pub latency_ms: u64,
    pub first_token_ms: Option<u64>,
}

pub(super) fn timestamp_ms(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

/// 排除同批写盘的毫秒突发及超过一小时的休眠/重试间隔，缺失时间戳不推造计时。
pub(super) fn estimate(
    start: Option<i64>,
    end: Option<i64>,
    first: Option<i64>,
) -> Option<EstimatedTiming> {
    let start = start?;
    let latency = end?.checked_sub(start)?;
    if !(100..=3_600_000).contains(&latency) {
        return None;
    }
    Some(EstimatedTiming {
        latency_ms: latency as u64,
        first_token_ms: first
            .and_then(|first| first.checked_sub(start))
            .filter(|first| *first >= 0 && *first <= latency)
            .map(|first| first as u64),
    })
}

#[derive(Default)]
pub(super) struct CodexRequestTimer {
    boundary: Option<i64>,
    previous_count: Option<i64>,
    start: Option<i64>,
    last_output: Option<i64>,
    tool_after_output: Option<i64>,
    first_output: Option<i64>,
    usage_record: Option<(i64, Value)>,
}

impl CodexRequestTimer {
    pub fn observe(&mut self, value: &Value) {
        let Some(at) = value.get("timestamp").and_then(timestamp_ms) else {
            return;
        };
        let payload = &value["payload"];
        match value["type"].as_str() {
            Some("turn_context") => self.boundary = self.boundary.max(Some(at)),
            Some("token_usage_record") => {
                self.usage_record = payload.get("usage").cloned().map(|usage| (at, usage));
            }
            Some("event_msg") => match payload["type"].as_str() {
                Some("task_started" | "turn_aborted") => {
                    // 中断请求不留给下一轮；保存上一用量时间以识别旧格式补写的输出项。
                    *self = Self {
                        boundary: self.boundary.max(Some(at)),
                        previous_count: self.previous_count,
                        ..Self::default()
                    };
                }
                Some("item_completed")
                    if matches!(
                        payload["item"]["type"].as_str(),
                        Some("Reasoning" | "AgentMessage")
                    ) =>
                {
                    if let Some(first) = payload["started_at_ms"].as_i64() {
                        self.first_output =
                            Some(self.first_output.map_or(first, |old| old.min(first)));
                    }
                }
                _ => {}
            },
            Some("response_item") => {
                let kind = payload["type"].as_str().unwrap_or_default();
                if kind.ends_with("_output") {
                    self.boundary = self.boundary.max(Some(at));
                    if self.last_output.is_some() {
                        self.tool_after_output = Some(at);
                    }
                } else if kind == "reasoning"
                    || kind.ends_with("_call")
                    || (kind == "message" && payload["role"] == "assistant")
                {
                    // 旧日志在 token_count 后补写同批输出，不能把它算成下一次请求。
                    if self.start.is_none()
                        && self.previous_count.is_some_and(|old| at - old <= 100)
                    {
                        return;
                    }
                    self.start = self.start.or(self.boundary);
                    self.last_output = Some(at);
                    self.tool_after_output = None;
                } else if kind == "message" {
                    self.boundary = self.boundary.max(Some(at));
                }
            }
            _ => {}
        }
    }

    pub fn finish(&mut self, value: &Value, last_usage: Option<&Value>) -> Option<EstimatedTiming> {
        let at = value.get("timestamp").and_then(timestamp_ms);
        // 优先真实响应结束用量记录；延迟到工具执行结束才写 token_count 的旧日志，
        // 改用最后一个模型输出的时刻，避免把工具执行时间算进生成速度。
        let record_end = self
            .usage_record
            .take()
            .filter(|(_, usage)| {
                last_usage.is_none_or(|last| {
                    ["input_tokens", "cached_input_tokens", "output_tokens"]
                        .iter()
                        .all(|key| {
                            usage[*key].as_u64().unwrap_or(0) == last[*key].as_u64().unwrap_or(0)
                        })
                })
            })
            .map(|(at, _)| at);
        let waited_for_tool = self
            .tool_after_output
            .is_some_and(|tool| at.is_none_or(|at| at - tool <= 100));
        let end = record_end.or(if waited_for_tool {
            self.last_output
        } else {
            at
        });
        let timing = estimate(self.start.or(self.boundary), end, self.first_output);
        *self = Self {
            boundary: self.boundary.max(at),
            previous_count: at,
            ..Self::default()
        };
        timing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(at: u32, kind: &str, payload: Value) -> Value {
        json!({"timestamp": format!("2026-07-30T10:00:{at:02}Z"), "type": kind, "payload": payload})
    }

    /// 工具结果晚于模型响应结束，不能把工具运行时间塞进请求生成窗口。
    #[test]
    fn tool_wait_and_aborted_turn_do_not_inflate_the_next_request() {
        let mut timer = CodexRequestTimer::default();
        timer.observe(&event(0, "turn_context", json!({})));
        timer.observe(&event(2, "response_item", json!({"type": "function_call"})));
        timer.observe(&event(
            20,
            "response_item",
            json!({"type": "function_call_output"}),
        ));
        let timing = timer
            .finish(&event(20, "event_msg", json!({})), None)
            .expect("结束计时");
        assert_eq!(timing.latency_ms, 2_000);
        timer.observe(&event(21, "event_msg", json!({"type": "turn_aborted"})));
        timer.observe(&event(40, "event_msg", json!({"type": "task_started"})));
        timer.observe(&event(42, "response_item", json!({"type": "reasoning"})));
        assert_eq!(
            timer
                .finish(&event(44, "event_msg", json!({})), None)
                .expect("下一请求")
                .latency_ms,
            4_000
        );
    }

    #[test]
    fn missing_or_implausible_timestamps_stay_unknown() {
        assert!(estimate(None, Some(100), None).is_none());
        assert!(estimate(Some(0), Some(99), None).is_none());
        assert!(estimate(Some(0), Some(3_600_001), None).is_none());
        assert_eq!(
            estimate(Some(0), Some(2_000), Some(3_000))
                .expect("总耗时有效")
                .first_token_ms,
            None
        );
    }
}
