//! DB 访问样板收敛（P1 audit#2：183 处 mutex_poisoned eprintln + 124 处
//! spawn_blocking + DB_WRITE_LOCK + open_db + ensure_table 散布全仓）
//!
//! - [`lock_or_recover`]：替代 `mutex.lock().unwrap_or_else(|e| eprintln + into_inner)`
//!   全仓 183 处粘贴。poison 仍走仓库既有 `[mutex_poisoned]` 静默审计约定
//!   （`into_inner` 把内部数据吐出来继续跑；中毒意味着另一线程 panic，
//!   崩溃日志已经在那里，当前写者若看到非法数据仍会失败，那是 catch 的
//!   问题不是 lock 的）。
//!
//! - [`with_write_conn`]：异步包装 `spawn_blocking + DB_WRITE_LOCK + open_db`
//!   三件套，闭包内继续接 ensure_table / 业务 SQL；返回 `Result<R, String>`
//!   透传（错误包裹到 `CommandError::DbError` 由调用方按既有纪律自行映射）。
//!
//! **不做**：不动 inner 业务逻辑、不改 audit 行、不合并跨文件差异。auto_write_allowed
//! 等前置闸仍在调用方 await 之前跑，确保惰性场景零成本短路。

/// 防 mutex 中毒 — 静默审计中毒原因（按仓库惯例进 stderr），把内部数据
/// 吐出来继续跑（中毒意味着另一线程 panic，崩溃日志已经在那里；当前写者
/// 若看到「非法数据」仍会失败，那是 catch 的问题不是 lock 的）。
pub(crate) fn lock_or_recover<'a, T>(
    m: &'a std::sync::Mutex<T>,
    label: &str,
) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] {label}: {e:?}");
        e.into_inner()
    })
}

/// spawn_blocking 单写者接线：拿 `DB_WRITE_LOCK` + `open_db` + 注入 closure；
/// closure 返回 `Result<R, String>` 透传。线程 join 失败统一报「DB 写线程 join 失败」。
///
/// 拿锁走 `lock_db_write()` 而不是裸 `DB_WRITE_LOCK.lock()`——闭包里调
/// `upsert_tasks` 这类契约函数时，它断言的是「当前线程持锁」这个线程本地标记；
/// 裸锁拿到的互斥是真的，标记却是假的，debug 构建下当场 panic。
///
/// 配套语义：
/// - 闭包拿 `&mut Connection`（业务需要事务/非事务时按需 `&*conn` 借用即可）
/// - **不在 helper 内自动 ensure_table**：每个调用方表契约不同，闭包自己调一次
///   `ensure_table(&conn)?` 既保持原顺序、又避免 helper 撞上 `&Connection`/`&mut Connection`
///   借用差异（`ensure_table` 普遍接受 `&Connection`，闭包内 deref 即可）
/// - `auto_write_allowed` / `read_*_config` 等前置仍留在 await 之前，惰性短路零成本
pub(crate) async fn with_write_conn<R, F, T>(app: &tauri::AppHandle<R>, f: F) -> Result<T, String>
where
    R: tauri::Runtime,
    F: FnOnce(&mut rusqlite::Connection) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let app = app.clone();
    let join_result: Result<Result<T, String>, tauri::Error> =
        tauri::async_runtime::spawn_blocking(move || -> Result<T, String> {
            let _g = crate::db::lock_db_write();
            let mut conn = crate::db::open_db(&app)?;
            f(&mut conn)
        })
        .await;
    join_result.map_err(|e| format!("DB 写线程 join 失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// with_write_conn 的临界区必须让当前线程被标记为「持 DB 写锁」。
    ///
    /// `upsert_tasks` 之类的契约函数断言的是这个线程本地标记，不是锁本身；
    /// 拿锁方式与标记设置脱节时，互斥是真的、断言却是假的——debug 构建下
    /// 闭包里调 `upsert_tasks` 会当场 panic，release 构建里被编译掉看不见。
    #[tokio::test]
    async fn write_conn_marks_the_closing_thread_as_holding_the_db_write_lock() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let marked = with_write_conn(&handle, |_conn| Ok(crate::db::holding_db_write()))
            .await
            .expect("写连接应能拿到");
        assert!(
            marked,
            "with_write_conn 闭包内必须置位持锁标记——否则契约函数的断言会误判"
        );
    }
}
