//! DB 写事务样板：在 [`with_write_conn`][super::lock::with_write_conn] 的
//! 「锁 + 连接」之上再开一层事务。区别只在「是否开事务」——锁粒度、连接
//! 语义、错误透传一致；拿锁同样走 `lock_db_write()`（契约函数断言的持锁
//! 标记依赖它，裸锁「锁是真的、标记是假的」）。

/// 在 DB_WRITE_LOCK + 连接之上再开一层事务：闭包返回 Err 即回滚（事务随 Drop 回滚）。
///
/// 闭包约束（违反的后果比写失败更糟，code review 对着这几条看）：
/// - 同步闭包：SQLite 事务不可跨 await 持有；
/// - 闭包内不得再走任何取锁路径（`with_write_conn` / `db_locked` / journal 写）——
///   `DB_WRITE_LOCK` 是 `std::sync::Mutex`，不可重入，嵌套即死锁；
///   需要业务 SQL 就内联，或抽收 `&Transaction` 的 `*_in_tx` helper；
/// - 事务里不得起外部副作用（发通知、起子进程）——外部副作用走 Effects 队列；
/// - 时长建议 100ms 内：全局写锁互斥所有写者，超过要拆。
///
/// 幂等/去重语义仍由调用方负责：事务提供「都成或都不成」，不提供业务幂等。
pub(crate) async fn with_tx<R, F, T>(app: &tauri::AppHandle<R>, f: F) -> Result<T, String>
where
    R: tauri::Runtime,
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let app = app.clone();
    let join_result: Result<Result<T, String>, tauri::Error> =
        tauri::async_runtime::spawn_blocking(move || -> Result<T, String> {
            let _g = crate::db::lock_db_write();
            let mut conn = crate::db::open_db(&app)?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            let out = f(&tx)?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(out)
        })
        .await;
    join_result.map_err(|e| format!("DB 写事务线程 join 失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 成功路径：闭包内全部落库，commit 后其他连接可见。
    #[tokio::test]
    async fn with_tx_commits_on_ok() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        with_tx(&handle, |tx| {
            tx.execute("CREATE TABLE tx_cm_probe (id INTEGER)", [])
                .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO tx_cm_probe (id) VALUES (1)", [])
                .map_err(|e| e.to_string())
        })
        .await
        .expect("事务应成功提交");
        let n: i64 = crate::db::lock::with_write_conn(&handle, |conn| {
            conn.query_row("SELECT count(*) FROM tx_cm_probe", [], |r| r.get(0))
                .map_err(|e| e.to_string())
        })
        .await
        .expect("查询应成功");
        assert_eq!(n, 1, "commit 后行必须可见");
    }

    /// 失败路径：闭包返回 Err → 不 commit → 整体回滚（建表也一并回滚）。
    #[tokio::test]
    async fn with_tx_rolls_back_on_err() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let err = with_tx(&handle, |tx| -> Result<(), String> {
            tx.execute("CREATE TABLE tx_rb_probe (id INTEGER)", [])
                .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO tx_rb_probe (id) VALUES (1)", [])
                .map_err(|e| e.to_string())?;
            Err("故意失败".to_string())
        })
        .await
        .expect_err("应回滚");
        assert_eq!(err, "故意失败", "闭包错误原样透传");
        let n: i64 = crate::db::lock::with_write_conn(&handle, |conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'tx_rb_probe'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .await
        .expect("查询应成功");
        assert_eq!(n, 0, "回滚后表不得存在");
    }

    /// 闭包必须运行在「持 DB_WRITE_LOCK」的线程上：契约函数
    /// （debug_assert holding_db_write）在事务内应能通过。
    #[tokio::test]
    async fn with_tx_marks_the_closing_thread_as_holding_the_db_write_lock() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let marked = with_tx(&handle, |_tx| Ok(crate::db::holding_db_write()))
            .await
            .expect("事务应能拿到");
        assert!(marked, "with_tx 闭包内必须置位持锁标记");
        assert!(!crate::db::holding_db_write(), "结束后持锁标记必须清位");
    }

    /// 闭包 panic 映射成 join 失败错误，不上抛 panic、不挂起等待方；
    /// 中毒锁由 lock_or_recover 恢复路径接管（同 with_write_conn 约定）。
    #[tokio::test]
    async fn closure_panic_maps_to_join_error_not_propagation() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let err = with_tx(&handle, |_tx| -> Result<(), String> {
            panic!("闭包 panic（测试手段）");
        })
        .await
        .expect_err("panic 应映射成 Err 而不是传播");
        assert!(
            err.contains("DB 写事务线程 join 失败"),
            "错误应为 join 失败语义：{err}"
        );
    }
}
