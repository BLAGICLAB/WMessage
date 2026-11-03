//! 通用持久化副作用队列（effect_queue 表）：
//! 「状态已写、动作未做」的中间态都能被恢复——入队与业务写同事务提交，
//! 派发器（`bot::effects`）轮询投递，崩溃后 due 已过期的行自然重跑（at-least-once）。
//!
//! 内核语义对拍 superagent `effects.ts` 实源：
//! - 同 kind+key 只一行：再入队**保留 seq 队位**（不插队）、`generation+1`、
//!   `attempts` 归零（新 payload 重新计次）
//! - 派发收尾用 `kind+key+seq+generation` 四元组 CAS：运行中被再入队时，
//!   旧尝试的收尾写不命中，新 payload 原样保留
//! - 成功 = 删行（队列即待办，done 不留行）
//! - 失败 = `attempts+1` + `error`（截断落列）+ `due` 指数退避；超限停投递留行，
//!   同 key 再入队即复活
//! - 「现在跑不了」（Later）只顺延 due，不消耗重试配额

use rusqlite::OptionalExtension;

/// 单源 DDL：open_db 与测试建表共用
pub const EFFECT_QUEUE_DDL: &str = "CREATE TABLE IF NOT EXISTS effect_queue (
   kind        TEXT NOT NULL,
   key         TEXT NOT NULL,
   payload     TEXT NOT NULL,
   seq         INTEGER NOT NULL,
   generation  INTEGER NOT NULL DEFAULT 1,
   attempts    INTEGER NOT NULL DEFAULT 0,
   due         INTEGER NOT NULL,
   since       INTEGER NOT NULL,
   error       TEXT,
   PRIMARY KEY (kind, key)
 );
 CREATE INDEX IF NOT EXISTS idx_effects_due ON effect_queue(kind, due, seq);";

/// 入队（可传 `&Transaction`：与业务写同事务提交是本队列的正确用法）。
/// 同 kind+key 再入队保留 seq 队位、generation+1、attempts 归零。
/// `due` 通常传当前毫秒（立即可派发）；调用方 commit 后应调用
/// `bot::effects::kick()` 唤醒派发器。
pub fn enqueue(
    conn: &rusqlite::Connection,
    kind: &str,
    key: &str,
    payload: &str,
    due: i64,
) -> Result<(), String> {
    let seq = next_seq(conn)?;
    conn.execute(
        "INSERT INTO effect_queue (kind, key, payload, seq, generation, attempts, due, since)
         VALUES (?1, ?2, ?3, ?4, 1, 0, ?5, ?5)
         ON CONFLICT(kind, key) DO UPDATE SET
           payload = excluded.payload, generation = generation + 1,
           attempts = 0, error = NULL, due = excluded.due",
        rusqlite::params![kind, key, payload, seq, due],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 进程内单调 seq：首次以库内 MAX(seq) 为基线，此后纯内存自增。
/// Mutex 保护「读基线 + 自增」的原子性（并发首入队不会拿到重复 seq）。
fn next_seq(conn: &rusqlite::Connection) -> Result<i64, String> {
    static SEQ: std::sync::Mutex<Option<i64>> = std::sync::Mutex::new(None);
    let mut g = crate::db::lock::lock_or_recover(&SEQ, "db::effects::next_seq");
    let next = match *g {
        Some(v) => v + 1,
        None => {
            let max: i64 = conn
                .query_row("SELECT IFNULL(MAX(seq), 0) FROM effect_queue", [], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            max + 1
        }
    };
    *g = Some(next);
    Ok(next)
}

/// 派发器抓取的行快照（CAS 收尾所需的四元组随行带走）
#[derive(Debug, Clone)]
pub(crate) struct EffectRow {
    pub kind: String,
    pub key: String,
    pub payload: String,
    pub seq: i64,
    pub generation: i64,
    pub attempts: i64,
}

/// 抓到点待派发行（`due <= now` 且 `attempts < max_attempts`，按 seq 先来先做）
pub(crate) fn fetch_due(
    conn: &rusqlite::Connection,
    kind: &str,
    now: i64,
    max_attempts: i64,
    limit: usize,
) -> Result<Vec<EffectRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT kind, key, payload, seq, generation, attempts \
             FROM effect_queue \
             WHERE kind = ?1 AND due <= ?2 AND attempts < ?3 \
             ORDER BY seq LIMIT ?4",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![kind, now, max_attempts, limit as i64],
            |r| {
                Ok(EffectRow {
                    kind: r.get(0)?,
                    key: r.get(1)?,
                    payload: r.get(2)?,
                    seq: r.get(3)?,
                    generation: r.get(4)?,
                    attempts: r.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 指定 kind+key 是否有待派发行（僵尸清扫豁免判断等场景用）
pub(crate) fn pending(conn: &rusqlite::Connection, kind: &str, key: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM effect_queue WHERE kind = ?1 AND key = ?2 LIMIT 1",
        [kind, key],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
    .map_err(|e| e.to_string())
}

/// 最早 due（派发器睡眠时长依据；空队列返回 None）。
/// `attempts_lt`：只统计还可投递的行——超限留行的 due 停在过去，
/// 不过滤会让派发器以最小睡眠间隔（50ms）永久空转。
pub(crate) fn next_due(
    conn: &rusqlite::Connection,
    attempts_lt: i64,
) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT MIN(due) FROM effect_queue WHERE attempts < ?1",
        [attempts_lt],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(|o| o.flatten())
    .map_err(|e| e.to_string())
}

/// 成功 = 删行。CAS 四元组：运行中被再入队（generation 已变）时删不中，
/// 返回 false——新 payload 仍是待办，由派发器下一轮接手。
pub(crate) fn finish_ok(conn: &rusqlite::Connection, row: &EffectRow) -> Result<bool, String> {
    let n = conn
        .execute(
            "DELETE FROM effect_queue WHERE kind = ?1 AND key = ?2 AND seq = ?3 AND generation = ?4",
            rusqlite::params![row.kind, row.key, row.seq, row.generation],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// Later：只顺延 due，不消耗重试配额
pub(crate) fn reschedule(
    conn: &rusqlite::Connection,
    row: &EffectRow,
    due: i64,
) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE effect_queue SET due = ?5 WHERE kind = ?1 AND key = ?2 AND seq = ?3 AND generation = ?4",
            rusqlite::params![row.kind, row.key, row.seq, row.generation, due],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 真失败：attempts+1 + error 落列 + due 指数退避。
/// 返回 Some(new_attempts) = CAS 命中（含新计次）；None = 行已被再入队改写。
pub(crate) fn record_failure(
    conn: &rusqlite::Connection,
    row: &EffectRow,
    error: &str,
    due: i64,
) -> Result<Option<i64>, String> {
    let sql = "UPDATE effect_queue \
               SET attempts = attempts + 1, error = ?5, due = ?6 \
               WHERE kind = ?1 AND key = ?2 AND seq = ?3 AND generation = ?4 \
               RETURNING attempts";
    conn.query_row(
        sql,
        rusqlite::params![row.kind, row.key, row.seq, row.generation, error, due],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(EFFECT_QUEUE_DDL).unwrap();
        conn
    }

    fn row_of(conn: &rusqlite::Connection, key: &str) -> EffectRow {
        let mut stmt = conn
            .prepare("SELECT kind, key, payload, seq, generation, attempts FROM effect_queue WHERE key = ?1")
            .unwrap();
        stmt.query_row([key], |r| {
            Ok(EffectRow {
                kind: r.get(0)?,
                key: r.get(1)?,
                payload: r.get(2)?,
                seq: r.get(3)?,
                generation: r.get(4)?,
                attempts: r.get(5)?,
            })
        })
        .unwrap()
    }

    fn col_i(conn: &rusqlite::Connection, key: &str, col: &str) -> i64 {
        conn.query_row(
            &format!("SELECT {col} FROM effect_queue WHERE key = ?1"),
            [key],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn enqueue_then_reenqueue_keeps_seq_and_resets_attempts() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        enqueue(&conn, "notify", "k1", r#"{"a":1}"#, 100).unwrap();
        let first = row_of(&conn, "k1");
        // 模拟失败过几次
        conn.execute(
            "UPDATE effect_queue SET attempts = 3, error = 'x' WHERE key = 'k1'",
            [],
        )
        .unwrap();
        // 再入队：seq 队位保留（不插队）、generation+1、attempts 归零、error 清空
        enqueue(&conn, "notify", "k1", r#"{"a":2}"#, 200).unwrap();
        let again = row_of(&conn, "k1");
        assert_eq!(again.seq, first.seq, "同 key 再入队不得插队");
        assert_eq!(
            again.generation,
            first.generation + 1,
            "generation 必须递增"
        );
        assert_eq!(col_i(&conn, "k1", "attempts"), 0, "新 payload 重新计次");
        let err: Option<String> = conn
            .query_row("SELECT error FROM effect_queue WHERE key = 'k1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(err, None, "再入队清空旧错误");
        assert_eq!(col_i(&conn, "k1", "due"), 200, "due 取新值");
        let payload: String = conn
            .query_row(
                "SELECT payload FROM effect_queue WHERE key = 'k1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload, r#"{"a":2}"#, "payload 取新值");
    }

    #[test]
    fn fetch_due_respects_due_and_max_attempts() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        enqueue(&conn, "notify", "a", "{}", 50).unwrap();
        enqueue(&conn, "notify", "b", "{}", 150).unwrap();
        enqueue(&conn, "other", "c", "{}", 50).unwrap();
        let rows = fetch_due(&conn, "notify", 100, 5, 10).unwrap();
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["a"], "只取到点行、同 kind、seq 升序");
        // 超限行不再投递
        conn.execute("UPDATE effect_queue SET attempts = 5 WHERE key = 'a'", [])
            .unwrap();
        let rows = fetch_due(&conn, "notify", 100, 5, 10).unwrap();
        assert!(rows.is_empty(), "attempts 达到 max_attempts 的行停投递");
    }

    #[test]
    fn finish_ok_is_cas_on_generation() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        enqueue(&conn, "notify", "k", "{}", 50).unwrap();
        let row = row_of(&conn, "k");
        assert!(finish_ok(&conn, &row).unwrap(), "首次收尾应删中");
        assert!(!finish_ok(&conn, &row).unwrap(), "二次收尾 CAS 不命中");

        // 运行中被再入队 → 旧快照（旧 generation）删不中，新 payload 原样保留
        enqueue(&conn, "notify", "k", r#"{"new":1}"#, 50).unwrap();
        let stale = row_of(&conn, "k");
        enqueue(&conn, "notify", "k", r#"{"newer":1}"#, 60).unwrap();
        let fresh = row_of(&conn, "k");
        assert_ne!(
            stale.generation, fresh.generation,
            "再入队必须推进 generation"
        );
        assert!(!finish_ok(&conn, &stale).unwrap(), "旧 generation 删不中");
        assert!(
            row_of(&conn, "k").payload.contains("newer"),
            "新 payload 原样保留"
        );
        assert!(finish_ok(&conn, &fresh).unwrap(), "新 generation 删中");
    }

    #[test]
    fn record_failure_bumps_attempts_and_backs_off() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        enqueue(&conn, "notify", "k", "{}", 50).unwrap();
        let row = row_of(&conn, "k");
        let a1 = record_failure(&conn, &row, "boom-1", 1_000)
            .unwrap()
            .unwrap();
        assert_eq!(a1, 1);
        assert_eq!(col_i(&conn, "k", "due"), 1_000);
        let err: Option<String> = conn
            .query_row("SELECT error FROM effect_queue WHERE key = 'k'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(err.as_deref(), Some("boom-1"));
        // 行被再入队后，旧快照的收尾写不命中
        enqueue(&conn, "notify", "k", "{}", 60).unwrap();
        let fresh = row_of(&conn, "k");
        let stale_miss = record_failure(&conn, &row, "stale", 2_000).unwrap();
        assert_eq!(stale_miss, None, "旧快照 CAS 不命中");
        assert_eq!(fresh.attempts, 0);
    }

    #[test]
    fn pending_reports_presence_for_exempt_check() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        assert!(!pending(&conn, "notify", "k").unwrap());
        enqueue(&conn, "notify", "k", "{}", 50).unwrap();
        assert!(pending(&conn, "notify", "k").unwrap(), "待派发行应命中");
        assert!(
            !pending(&conn, "notify", "other").unwrap(),
            "不同 key 不命中"
        );
        assert!(!pending(&conn, "other", "k").unwrap(), "不同 kind 不命中");
    }

    #[test]
    fn next_due_excludes_give_up_rows() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        assert_eq!(next_due(&conn, 5).unwrap(), None, "空队列无 due");
        enqueue(&conn, "notify", "a", "{}", 100).unwrap();
        enqueue(&conn, "notify", "b", "{}", 300).unwrap();
        enqueue(&conn, "schedule", "c", "{}", 200).unwrap();
        assert_eq!(next_due(&conn, 5).unwrap(), Some(100));
        // 超限留行的 due 停在过去：不滤掉会把派发器逼成 50ms 空转
        conn.execute(
            "UPDATE effect_queue SET attempts = 5, due = 1, error = '耗尽' WHERE key = 'a'",
            [],
        )
        .unwrap();
        assert_eq!(next_due(&conn, 5).unwrap(), Some(200), "超限行不进睡眠计算");
        assert_eq!(next_due(&conn, 6).unwrap(), Some(1), "闸放宽后重新可见");
    }
}
