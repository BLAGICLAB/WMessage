//! 迁移引擎 + 后台轮询线程。
//!
//! 核心入口 `run_migration`：持 RAII 防重入 + 调 `run_migration_inner`。
//! `spawn_polling`：每 10min 跑一轮；启动时 replay journal 修复 DB。

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use tauri::AppHandle;

use crate::db;
use crate::error::CommandError;

use super::journal::{journal_cleared, journal_committed, journal_find_pending, journal_pending};
use super::ops::{
    emit_upserts, filename_matches, log_line, move_entry, move_remove_permanently_failed, now_ms,
    resolve_archive_dir, verify_archive_dir_created, MigrationGuard, MAX_MOVE_REMOVE_FAILURES,
};
use super::recovery::{decide_src_missing, journal_replay_pending, SrcMissingAction};
use super::rules::load_rules;
use super::types::MigrationReport;

/// 完成满 7 天进入归档（与前端 applyArchiveRule 的 ARCHIVE_AFTER_MS 一致）
pub const ARCHIVE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub(crate) const POLL_INTERVAL_SECS: u64 = 600;

/// 核心迁移：返回报告。silent 模式（轮询）不向 UI 抛错。
pub fn run_migration(app: &AppHandle) -> Result<MigrationReport, CommandError> {
    let _guard = MigrationGuard::acquire()?;
    run_migration_inner(app)
}

fn run_migration_inner(app: &AppHandle) -> Result<MigrationReport, CommandError> {
    // B3: db_load/db_upsert 改 async 了；run_migration_inner 在 spawn_polling 的 std::thread
    // 或 spawn_blocking(migration_run) 线程里跑，不在 tokio runtime 上 → 用 block_on 桥接。
    let mut report = MigrationReport {
        ts: now_ms(),
        ..Default::default()
    };
    // 规则不可读（损坏/权限）→ 本轮迁移中止：绝不以空规则执行任何 move/delete。
    // 本调用点保持 CommandError 结构化直传（IoError/DomainRule 不塌回 Internal）；
    // 函数内 DB 类错误维持既有 String/Internal 口径，全量改造属独立批不做
    let rules = load_rules(app)?;
    let tasks = tauri::async_runtime::block_on(async { db::db_load(app.clone()).await })
        .map_err(|e| e.to_string())?;
    // NEW-B-2: 本轮所有 journal 读写复用同一条连接（原来每次 journal 调用各 open_db 一次，
    // 迁一个文件付 3 次全套 schema 检查）；journal 写由 journal_* 内部持 DB_WRITE_LOCK 串行。
    let jconn = db::open_db(app).map_err(|e| e.to_string())?;
    let now = now_ms();
    let mut changed: Vec<db::Task> = vec![];
    // NEW-B-1: 解绑时确认关闭的 pending journal id——在 changed 批量落盘成功后统一提交，
    // 避免「journal 已提交但解绑未落盘」对账空洞。
    let mut journals_commit_after_batch: Vec<i64> = vec![];

    // 阶段一：完成满 7 天且未归档的任务 → 归档（兜底：主窗口关闭时也照常到期）
    let mut due: Vec<db::Task> = vec![];
    for t in tasks.iter() {
        if t.deleted_at.is_some()
            || t.archived == Some(true)
            || t.column != crate::db::TaskStatus::Done
        {
            continue;
        }
        if let Some(completed) = t.completed_at {
            if completed > 0 && now - completed >= ARCHIVE_AFTER_MS {
                let mut nt = t.clone();
                nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                nt.archived = Some(true);
                nt.updated_at = Some(now);
                due.push(nt);
            }
        }
    }
    if !due.is_empty() {
        tauri::async_runtime::block_on(async { db::db_upsert(app.clone(), due.clone()).await })
            .map_err(|e| e.to_string())?;
        // T1-1：due 已落盘（updated_at=now），changed 末尾统一重写的基线须重武装为
        // 刚写入的值，否则二次 upsert 会被自己的基线比对拒写
        for t in &mut due {
            t.expected_updated_at = t.updated_at;
        }
        report.archived = due.len();
        let line = format!("归档到期任务 {n} 个", n = due.len());
        report.log.push(line.clone());
        log_line(app, &line);
        emit_upserts(app, &due);
        changed.extend(due);
    }

    // 阶段二：对已归档任务执行规则迁移。
    // 注意：遍历的是阶段一之前读的旧快照——本轮刚归档的任务不参与本轮迁移，
    // 要到下一轮（10 分钟后）才会走规则。这是有意行为（审计 P3-5 补注释）：
    // 避免归档与迁移在同一次扫描里链式触发，用户看到的中间状态更少。
    for t in tasks.iter() {
        if t.archived != Some(true) || t.deleted_at.is_some() {
            continue; // 保护：非归档/回收站任务一律不动
        }
        let Some(src_str) = t.file_path.as_deref() else {
            continue;
        };
        let src = PathBuf::from(src_str);
        let Some(name) = src.file_name().map(|s| s.to_string_lossy().to_string()) else {
            report.skipped += 1;
            let line = format!("跳过「{}」：路径无文件名", t.title);
            report.log.push(line.clone());
            log_line(app, &line);
            continue;
        };
        // 规则顺序即优先级：取第一条命中
        let Some(rule) = rules
            .rules
            .iter()
            .find(|r| r.enabled && filename_matches(&name, r))
        else {
            continue; // 无命中规则：不动，也不记噪音日志
        };

        match rule.action.as_str() {
            "move" => {
                let dir = match resolve_archive_dir(app, &rule.archive_dir) {
                    Ok(d) => d,
                    Err(e) => {
                        report.skipped += 1;
                        let line = format!("跳过「{name}」：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                };
                if let Err(e) = fs::create_dir_all(&dir) {
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：创建归档目录失败：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                // 落地后复验：关闭 resolve→create 之间祖先被换成符号链接的窗口
                // （canonicalize 终态目录比对基准前缀；残余窗缩至 create→verify 间，
                // 文件系统原语限制归既有 follow-up）
                if let Err(e) = verify_archive_dir_created(app, &dir, &rule.archive_dir) {
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                if !src.exists() {
                    // NEW-B-1: 源不存在时先对账 journal——「上一轮 move 成功但 db_upsert 失败」
                    // 时 journal 仍 pending 且 dst 存在，此时应就地修复绑定到 dst，而非解绑
                    // （旧逻辑直接解绑 → 附件链接丢失一整个会话周期，要等重启 replay 才恢复）。
                    let pending = journal_find_pending(&jconn, &t.id, &src)?;
                    let dst_exists = pending
                        .as_ref()
                        .and_then(|e| e.dst.as_ref())
                        .map(|d| PathBuf::from(d).exists())
                        .unwrap_or(false);
                    match decide_src_missing(pending, dst_exists) {
                        SrcMissingAction::RepairMove { dst, journal_id } => {
                            let mut nt = t.clone();
                            nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                            nt.file_path = Some(dst.clone());
                            nt.updated_at = Some(now);
                            match tauri::async_runtime::block_on(async {
                                db::db_upsert(app.clone(), vec![nt.clone()]).await
                            }) {
                                Ok(()) => {
                                    journal_committed(&jconn, journal_id).ok();
                                    // T1-1：nt 已落盘，changed 末尾统一重写前基线重武装为刚写入值
                                    nt.expected_updated_at = nt.updated_at;
                                    changed.push(nt);
                                    report.moved += 1;
                                    let line = format!(
                                        "修复绑定「{name}」→ {dst}（上轮 move 落库失败，journal={journal_id} 对账恢复）"
                                    );
                                    report.log.push(line.clone());
                                    log_line(app, &line);
                                }
                                Err(e) => {
                                    report.skipped += 1;
                                    let line = format!(
                                        "跳过「{name}」：journal 修复落库失败（{e}），journal={journal_id} 待修复"
                                    );
                                    report.log.push(line.clone());
                                    log_line(app, &line);
                                }
                            }
                            continue;
                        }
                        SrcMissingAction::Unbind { commit_journal } => {
                            // 确认解绑：无 pending（用户外部删除/文件本就不存在），
                            // 或 pending 为 delete（解绑本就是其终态）/ move 但 dst 也丢失。
                            let mut nt = t.clone();
                            nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                            nt.file_path = None;
                            nt.file_is_dir = None;
                            nt.updated_at = Some(now);
                            changed.push(nt);
                            if let Some(id) = commit_journal {
                                journals_commit_after_batch.push(id);
                            }
                            report.skipped += 1;
                            let line = format!("解除绑定「{name}」：源文件已不存在（{src_str}）");
                            report.log.push(line.clone());
                            log_line(app, &line);
                            continue;
                        }
                    }
                }
                // NEW-B-5: 跨卷 copy 成功但 remove 持续失败（Windows 占用）的 src 永久跳过，
                // 不再每轮生成新冲突名再 copy（归档目录不累积副本）。首次越阈时已记 ERROR，
                // 后续轮静默跳过（10min/轮不刷屏）。
                if move_remove_permanently_failed(src_str) {
                    report.skipped += 1;
                    continue;
                }
                // P2-6：选定后落盘前复检（TOCTOU）——并发任务抢占同名时递增后缀重选
                let Some(dst) = super::ops::claim_dst_name(&dir, &name) else {
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：目标目录同名冲突过多（不覆盖、不删除）");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                };
                // B1: 写 journal pending → move_entry → db_upsert → journal_committed
                // 如果中间任一步崩了，启动时 journal_replay_pending 修复 DB
                let journal_id = match journal_pending(&jconn, "move", &src, Some(&dst), &t.id) {
                    Ok(id) => id,
                    Err(e) => {
                        report.skipped += 1;
                        let line = format!("跳过「{name}」：journal_pending 失败：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                };
                if let Err(e) = move_entry(&src, &dst) {
                    // move 未启动 / 明确失败 → clear journal，下次重试不需要修复
                    journal_cleared(&jconn, journal_id).ok();
                    // NEW-B-5: dst 已存在且 src 仍在 = copy 成功但 remove 失败
                    // （区别于 copy 本身失败：此时 dst 不存在，不计数）。
                    // 失败计数越阈 → 记 ERROR 审计并永久跳过该 src，防止归档副本累积。
                    if src.exists() && dst.exists() {
                        let n = super::ops::record_move_remove_failure(src_str);
                        if n >= MAX_MOVE_REMOVE_FAILURES {
                            let line = format!(
                                "ERROR「{name}」：跨卷复制后删除源持续失败 {n} 次（{src_str}），已永久跳过该源；归档目录可能已有副本，请手动处理"
                            );
                            report.log.push(line.clone());
                            log_line(app, &line);
                        }
                    }
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                let mut nt = t.clone();
                nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                nt.file_path = Some(dst.to_string_lossy().to_string());
                nt.updated_at = Some(now);
                if let Err(e) = tauri::async_runtime::block_on(async {
                    db::db_upsert(app.clone(), vec![nt.clone()]).await
                }) {
                    // move 成功但 db_upsert 失败 → journal 保持 pending，
                    // 下次启动 replay 时检测 dst 存在 + src 不存在，修复 DB
                    report.skipped += 1;
                    let line = format!(
                        "已移动「{name}」但 db_upsert 失败（{e}），journal={journal_id} 待修复"
                    );
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                journal_committed(&jconn, journal_id).ok();
                // T1-1：nt 已落盘，changed 末尾统一重写前基线重武装为刚写入值
                nt.expected_updated_at = nt.updated_at;
                changed.push(nt);
                report.moved += 1;
                let line = format!("已移动「{name}」→ {}", dst.display());
                report.log.push(line.clone());
                log_line(app, &line);
            }
            "delete" => {
                if !src.exists() {
                    // NEW-B-1: 同 move 分支——先对账 journal。pending delete 的终态本就是
                    // 解绑，落盘后提交 journal 关闭环路；pending move 且 dst 在 → 就地修复。
                    let pending = journal_find_pending(&jconn, &t.id, &src)?;
                    let dst_exists = pending
                        .as_ref()
                        .and_then(|e| e.dst.as_ref())
                        .map(|d| PathBuf::from(d).exists())
                        .unwrap_or(false);
                    match decide_src_missing(pending, dst_exists) {
                        SrcMissingAction::RepairMove { dst, journal_id } => {
                            let mut nt = t.clone();
                            nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                            nt.file_path = Some(dst.clone());
                            nt.updated_at = Some(now);
                            match tauri::async_runtime::block_on(async {
                                db::db_upsert(app.clone(), vec![nt.clone()]).await
                            }) {
                                Ok(()) => {
                                    journal_committed(&jconn, journal_id).ok();
                                    // T1-1：nt 已落盘，changed 末尾统一重写前基线重武装为刚写入值
                                    nt.expected_updated_at = nt.updated_at;
                                    changed.push(nt);
                                    report.moved += 1;
                                    let line = format!(
                                        "修复绑定「{name}」→ {dst}（上轮 move 落库失败，journal={journal_id} 对账恢复）"
                                    );
                                    report.log.push(line.clone());
                                    log_line(app, &line);
                                }
                                Err(e) => {
                                    report.skipped += 1;
                                    let line = format!(
                                        "跳过「{name}」：journal 修复落库失败（{e}），journal={journal_id} 待修复"
                                    );
                                    report.log.push(line.clone());
                                    log_line(app, &line);
                                }
                            }
                            continue;
                        }
                        SrcMissingAction::Unbind { commit_journal } => {
                            let mut nt = t.clone();
                            nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                            nt.file_path = None;
                            nt.file_is_dir = None;
                            nt.updated_at = Some(now);
                            changed.push(nt);
                            if let Some(id) = commit_journal {
                                journals_commit_after_batch.push(id);
                            }
                            report.skipped += 1;
                            let line = format!("解除绑定「{name}」：源文件已不存在");
                            report.log.push(line.clone());
                            log_line(app, &line);
                            continue;
                        }
                    }
                }
                // B1: write journal pending → delete → db_upsert → committed
                let journal_id = match journal_pending(&jconn, "delete", &src, None, &t.id) {
                    Ok(id) => id,
                    Err(e) => {
                        report.skipped += 1;
                        let line = format!("跳过「{name}」：journal_pending 失败：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                };
                let res = if t.file_is_dir == Some(true) {
                    fs::remove_dir_all(&src)
                } else {
                    fs::remove_file(&src)
                };
                if let Err(e) = res {
                    // delete 失败 → clear journal，下次重试
                    journal_cleared(&jconn, journal_id).ok();
                    report.skipped += 1;
                    let line = format!("删除「{name}」失败（可能被占用/权限不足）：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                let mut nt = t.clone();
                nt.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
                nt.file_path = None;
                nt.file_is_dir = None;
                nt.updated_at = Some(now);
                if let Err(e) = tauri::async_runtime::block_on(async {
                    db::db_upsert(app.clone(), vec![nt.clone()]).await
                }) {
                    // delete 成功但 db_upsert 失败 → journal 保持 pending，
                    // 下次启动 replay 时检测 src 不存在 + task 仍有 file_path，
                    // 修复 DB 清 file_path
                    report.skipped += 1;
                    let line = format!(
                        "已删除「{name}」但 db_upsert 失败（{e}），journal={journal_id} 待修复"
                    );
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                journal_committed(&jconn, journal_id).ok();
                // T1-1：nt 已落盘，changed 末尾统一重写前基线重武装为刚写入值
                nt.expected_updated_at = nt.updated_at;
                changed.push(nt);
                report.deleted += 1;
                let line = format!("已删除「{name}」（规则显式启用 delete）");
                report.log.push(line.clone());
                log_line(app, &line);
            }
            _ => {}
        }
    }

    // 统一落盘（阶段一的 due 已包含在 changed 中，重复 upsert 幂等无害）
    if !changed.is_empty() {
        tauri::async_runtime::block_on(async { db::db_upsert(app.clone(), changed.clone()).await })
            .map_err(|e| e.to_string())?;
        emit_upserts(app, &changed);
        // NEW-B-1: 解绑已落盘 → 提交对应 pending journal，关闭对账环路
        // （若落盘失败则上面已 return，journal 保持 pending，留待下轮/启动 replay）
        for id in journals_commit_after_batch {
            journal_committed(&jconn, id).ok();
        }
    }

    Ok(report)
}

/// 后台轮询线程：启动 60s 后先跑一次，此后每 POLL_INTERVAL_SECS 检测一次，失败只记日志
pub fn spawn_polling(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        // B1: 启动时 replay 上轮未提交的 pending journal 条目，修复 move/delete 成功但
        // db_upsert 失败造成的 DB 不一致。出错只记日志，不影响后续轮询。
        // MI-04b：replay 是 run_migration 之外唯一另一个 journal 写者，不挂守卫时其
        // find+act 会与进行中的迁移 run 在相同 (task_id, src) 上交错。等到拿到
        // MigrationGuard 再 replay——互斥后 journal 回到单写者语义。
        // 守卫作用域只包住 replay（guard 在 replay 结束后立即 drop）：polling loop 在
        // 同一闭包内，guard 若泄漏到 loop 会把 RUNNING 永久置 true，后台自动迁移全灭。
        // 等待设上限（60×5s=5min）：RUNNING 若因未来 bug 卡死，跳过本轮 replay 并记
        // 日志，而不是无限阻塞轮询线程。
        let mut guard = None;
        for _ in 0..60 {
            match MigrationGuard::acquire() {
                Ok(g) => {
                    guard = Some(g);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_secs(5)),
            }
        }
        match guard {
            Some(_g) => match journal_replay_pending(&app) {
                Ok((rec, err)) if rec > 0 || err > 0 => {
                    log_line(
                        &app,
                        &format!("journal replay 启动：恢复 {rec} 条，失败 {err} 条"),
                    );
                }
                Ok(_) => {}
                Err(e) => log_line(&app, &format!("journal replay 启动失败：{e}")),
            },
            None => log_line(
                &app,
                "journal replay 跳过：迁移守卫 5 分钟内未释放（疑似卡死）",
            ),
        }
        loop {
            std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
            // 防 panic 杀死轮询线程（审计 P2）：单轮崩溃只废这一轮，后台自动迁移永久可用
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_migration(&app)));
            if let Err(e) = r {
                let msg = e
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| e.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "未知 panic".into());
                log_line(&app, &format!("轮询迁移 panic（已恢复）：{msg}"));
            }
        }
    });
}
