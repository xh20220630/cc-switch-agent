//! 供应商并发回归全部使用隔离 HOME，只覆盖 Claude，不读取实际客户端配置。

use std::process::Command;
use std::sync::{Arc, Barrier};

use cc_switch_core::{HeadlessState, ProviderRecord, ProviderService};
use serde_json::{json, Value};

fn provider(id: &str) -> ProviderRecord {
    serde_json::from_value(json!({
        "id": id, "name": id, "settingsConfig": {"env": {
            "ANTHROPIC_BASE_URL": format!("https://{id}.example.com"),
            "ANTHROPIC_API_KEY": format!("key-{id}"),
            "ANTHROPIC_MODEL": format!("model-{id}"),
            // 放大 live 序列化时间，重现数据库已提交但旧请求仍在写盘的窗口。
            "TEST_PADDING": "x".repeat(if id == "a" { 64 * 1024 } else { 0 })
        }}, "inFailoverQueue": false
    }))
    .unwrap()
}

fn assert_current_matches_live(state: &HeadlessState) {
    let id = ProviderService::current(state, "claude").unwrap();
    let providers = ProviderService::list(state, "claude").unwrap();
    let live: Value =
        serde_json::from_slice(&std::fs::read(state.home().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(live["env"], providers[&id].settings_config["env"]);
}

#[test]
fn claude_concurrent_switch_and_edit_keep_database_and_live_consistent() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(HeadlessState::open(home.path()).unwrap());
    for id in ["a", "b"] {
        ProviderService::add(&state, "claude", provider(id), true).unwrap();
    }
    for _ in 0..20 {
        let barrier = Arc::new(Barrier::new(3));
        std::thread::scope(|scope| {
            for id in ["a", "b"] {
                let state = state.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    ProviderService::switch(&state, "claude", id).unwrap();
                    ProviderService::update(&state, "claude", id, provider(id)).unwrap();
                });
            }
            barrier.wait();
        });
        assert_current_matches_live(&state);
    }
}

/// 通过独立测试进程复用同一磁盘 HOME，防止仅增加进程内 Mutex 的假修复通过回归。
#[test]
fn claude_independent_processes_share_provider_operation_lock() {
    let home = tempfile::tempdir().unwrap();
    let state = HeadlessState::open(home.path()).unwrap();
    for id in ["a", "b"] {
        ProviderService::add(&state, "claude", provider(id), true).unwrap();
    }
    for _ in 0..8 {
        let mut children: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| {
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "claude_provider_process_worker", "--nocapture"])
                    .env("CC_SWITCH_PROVIDER_LOCK_TEST_HOME", home.path())
                    .env("CC_SWITCH_PROVIDER_LOCK_TEST_ID", id)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_current_matches_live(&state);
    }
}

#[test]
fn claude_provider_process_worker() {
    // 普通测试运行不做操作，只有父测试明确提供的随机临时 HOME 才允许写入。
    let Some(home) = std::env::var_os("CC_SWITCH_PROVIDER_LOCK_TEST_HOME") else {
        return;
    };
    let id = std::env::var("CC_SWITCH_PROVIDER_LOCK_TEST_ID").unwrap();
    let state = HeadlessState::open(home).unwrap();
    for _ in 0..3 {
        ProviderService::switch(&state, "claude", &id).unwrap();
        ProviderService::update(&state, "claude", &id, provider(&id)).unwrap();
    }
}

/// 独立文件锁持有期间，整个切换都必须等待，不能提前提交 current 再等文件写入。
#[test]
fn claude_operation_waits_before_database_commit() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(HeadlessState::open(home.path()).unwrap());
    for id in ["a", "b"] {
        ProviderService::add(&state, "claude", provider(id), true).unwrap();
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.path().join(".cc-switch/provider-operation.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let worker_state = state.clone();
    let worker = std::thread::spawn(move || {
        sent.send(()).unwrap();
        ProviderService::switch(&worker_state, "claude", "b").unwrap();
    });
    received.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));
    let current: String = state
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT id FROM providers WHERE app_type='claude' AND is_current=1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(current, "a");
    drop(lock);
    worker.join().unwrap();
    assert_current_matches_live(&state);
}

/// 高竞争时允许返回可重试的忙错误，但超时的请求不能留下任何半完成的切换。
#[test]
fn claude_lock_timeout_leaves_database_and_live_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let state = HeadlessState::open(home.path()).unwrap();
    for id in ["a", "b"] {
        ProviderService::add(&state, "claude", provider(id), true).unwrap();
    }
    let path = home.path().join(".claude/settings.json");
    let before = std::fs::read(&path).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.path().join(".cc-switch/provider-operation.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let result = ProviderService::switch(&state, "claude", "b");
    assert!(matches!(
        result,
        Err(cc_switch_core::CoreError::DatabaseBusy)
    ));
    drop(lock);
    assert_eq!(ProviderService::current(&state, "claude").unwrap(), "a");
    assert_eq!(std::fs::read(path).unwrap(), before);
    // 超时返回不残留锁，竞争结束后正常切换仍可完成。
    ProviderService::switch(&state, "claude", "b").unwrap();
    assert_current_matches_live(&state);
}
