pub mod live;
mod model;
mod operation_lock;
mod repository;

use crate::{CoreError, HeadlessState};

pub use live::{project_provider, LiveContext, SwitchResult, TargetPlatform};
pub use model::{ProviderRecord, ProviderSortUpdate};

/// Provider 领域门面：协调规范数据库事务与目标主机 live 配置投影。
pub struct ProviderService;

impl ProviderService {
    /// 按桌面排序规则读取完整 Provider，并把 endpoint 合并回开放 meta。
    pub fn list(
        state: &HeadlessState,
        app: &str,
    ) -> Result<indexmap::IndexMap<String, ProviderRecord>, CoreError> {
        // 等待在途投影完成，避免 UI 读到已提交但尚未生效的中间状态。
        let _operation = operation_lock::acquire(state)?;
        repository::list(state, app)
    }

    /// 返回指定应用的当前 Provider ID；尚未配置时保持既有空字符串协议。
    pub fn current(state: &HeadlessState, app: &str) -> Result<String, CoreError> {
        let _operation = operation_lock::acquire(state)?;
        repository::current(state, app)
    }

    /// 正常新增保持首项自动启用；交换传 false 时只存副本，目标为空也不能改当前项或 live。
    pub fn add(
        state: &HeadlessState,
        app: &str,
        provider: ProviderRecord,
        add_to_live: bool,
    ) -> Result<bool, CoreError> {
        let _operation = operation_lock::acquire(state)?;
        let had_current = !repository::current(state, app)?.is_empty();
        let projected_provider = provider.clone();
        let added = repository::add(state, app, provider, add_to_live)?;
        // 待启用副本不能成为首项，否则交换会意外改变远端 CLI 的配置。
        if add_to_live && !had_current && should_project_implicitly(state, app) {
            let context = live_context(state);
            live::project_provider(&context, app, &projected_provider)?;
        }
        Ok(added)
    }

    /// 更新完整 Provider；当前项的 live 投影会同步刷新，非当前项只修改规范数据库。
    pub fn update(
        state: &HeadlessState,
        app: &str,
        original_id: &str,
        provider: ProviderRecord,
    ) -> Result<bool, CoreError> {
        Self::update_with_projection(state, app, original_id, provider, None)
    }

    /// 更新完整 Provider, 并允许投影使用桌面附带的改写快照（本地路由模式）。
    /// 数据库始终落盘传入的原始 provider; 改写快照只用于 live 投影。
    pub fn update_with_projection(
        state: &HeadlessState,
        app: &str,
        original_id: &str,
        provider: ProviderRecord,
        projected: Option<ProviderRecord>,
    ) -> Result<bool, CoreError> {
        // current 判断、数据库更新和 live 投影必须属于同一临界区，避免编辑和切换交错。
        let _operation = operation_lock::acquire(state)?;
        let is_current = repository::current(state, app)? == original_id;
        let updated = repository::update(state, app, original_id, provider.clone())?;
        if is_current && should_project_implicitly(state, app) {
            let context = live_context(state);
            let projection = match projected {
                Some(projected) => live::project_provider(&context, app, &projected),
                None => live::project_provider(&context, app, &provider),
            };
            projection?;
        }
        Ok(updated)
    }

    /// 删除非当前 Provider；当前项必须先切换，防止目标 CLI 失去可追踪的配置来源。
    pub fn delete(state: &HeadlessState, app: &str, id: &str) -> Result<(), CoreError> {
        let _operation = operation_lock::acquire(state)?;
        repository::delete(state, app, id)
    }

    /// 在数据库中原子切换当前项，并把选中配置投影到目标 HOME。
    pub fn switch(state: &HeadlessState, app: &str, id: &str) -> Result<SwitchResult, CoreError> {
        Self::switch_with_projection_impl(state, app, id, None)
    }

    /// 切换当前项，但投影使用桌面附带的改写快照（本地路由模式：base_url 指向
    /// 桌面代理、token 为占位符）。数据库只记录 current 指向，不落盘投影快照。
    pub fn switch_with_projection(
        state: &HeadlessState,
        app: &str,
        id: &str,
        projected: ProviderRecord,
    ) -> Result<SwitchResult, CoreError> {
        Self::switch_with_projection_impl(state, app, id, Some(projected))
    }

    fn switch_with_projection_impl(
        state: &HeadlessState,
        app: &str,
        id: &str,
        projected: Option<ProviderRecord>,
    ) -> Result<SwitchResult, CoreError> {
        // 锁一直持有到 live 完成；只锁 SQLite 事务会让较早请求最后覆盖配置文件。
        let _operation = operation_lock::acquire(state)?;
        let context = live_context(state);
        // 条件能力必须在事务前检查；Linux 不可写 Claude Desktop 时数据库保持原 current。
        live::ensure_projection_supported(&context, app)?;
        let provider = repository::switch(state, app, id)?;
        match projected {
            Some(projected) => live::project_provider(&context, app, &projected),
            None => live::project_provider(&context, app, &provider),
        }
    }

    /// 原子更新同一应用的排序；任一未知 ID 都会回滚整批操作。
    pub fn update_sort_order(
        state: &HeadlessState,
        app: &str,
        updates: &[ProviderSortUpdate],
    ) -> Result<(), CoreError> {
        let _operation = operation_lock::acquire(state)?;
        repository::update_sort_order(state, app, updates)
    }
}

fn live_context(state: &HeadlessState) -> LiveContext<'_> {
    // Context 只携带目标状态自身的信息，不能读取桌面宿主机 HOME 或环境覆盖。
    LiveContext {
        home: state.home(),
        platform: state.platform(),
    }
}

fn should_project_implicitly(state: &HeadlessState, app: &str) -> bool {
    // Linux 可保存和编辑 Claude Desktop 数据，但没有可用客户端路径；只有显式 switch
    // 需要向用户返回条件能力错误，普通 CRUD 不应在数据库成功后伪装成失败。
    !(app == "claude-desktop" && state.platform() == TargetPlatform::Linux)
}
