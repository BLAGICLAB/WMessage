//! 路径解析 + 日志轮转 + 原子写 + 老库拷贝边

use std::path::PathBuf;

use crate::error::CommandError;

/// 便携模式：数据库优先放 exe 同目录（U盘/绿色目录随走随带）；
/// 目录不可写（如 Program Files）时兜底到系统应用数据目录
pub fn db_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::paths::probe_log_dir(app)
}

/// 数据目录（供本地 HTTP API 存 token 等附属文件，便携模式跟随 exe）
pub fn data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    db_dir(app)
}

/// 程序模块根目录：data_dir/apps，apps 模块与 AI_Gen_Files 平级
/// （用户数据一部分；设计稿 `docs/APPS-DESIGN-2026-10-15.md` §2）
pub fn apps_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    data_dir(app).join("apps")
}

/// 程序模块单 app 目录：data_dir/apps/<app.id>/，与 `apps.rs::App` 字段
/// 一一对应（M1 起即定，AI 写代码据此契约）
pub fn app_dir_of<R: tauri::Runtime>(app: &tauri::AppHandle<R>, app_id: &str) -> PathBuf {
    apps_root(app).join(app_id)
}

/// AI 产物目录唯一入口：data_dir/AI_Gen_Files，解析即建
pub fn gen_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> std::io::Result<std::path::PathBuf> {
    let dir = data_dir(app).join("AI_Gen_Files");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 日志轮转：超过 size_limit 字节就整体改名让位，保留两代历史
/// （`X.log.old` → 再超限时挪到 `X.log.old.1`，最老一代被覆盖挤出）。
/// bot/api/migration 日志与 py、bot-config 两个旁路写入点共用本函数，
/// 代数提升对它们同时生效。rename 失败静默：双写者并发轮转最坏 =
/// 当次不轮转下次调用补上，无数据破坏（与原单代版同容错契约）。
pub fn rotate_log_if_large(path: &std::path::Path, size_limit: u64) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() > size_limit {
            let old = path.with_extension("log.old");
            if old.exists() {
                let older = path.with_extension("log.old.1");
                let _ = std::fs::rename(&old, &older);
            }
            let _ = std::fs::rename(path, &old);
        }
    }
}

/// 原子写文件——先写同目录 tmp 再 rename 覆盖目标
pub(crate) fn atomic_write(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("无效的目标路径：{}", path.display()))?;
    let tmp = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));
    std::fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // rename 失败不留 .tmp 垃圾（API-B fail-closed 测试暴露的残留面）
        let _ = std::fs::remove_file(&tmp);
        format!("落盘重命名失败：{e}")
    })?;
    Ok(())
}

/// 便携模式首启拷贝老库——Phase 1 (3 文件 to staging) + Phase 2 (rename order sidecar-first then main)；
/// 任一 Phase 失败 → clean tmp-* + 回滚 db_path-* → Err；源库 open 失败 → 直接 Err（fail-closed，
/// 损坏/占用/加密的源被字节级拷贝只会复制出坏主库），原始老库全程只读、原样保留。APW-02b。
/// 清理 copy_legacy_db 的三件 staging tmp-*（best-effort，任一失败不阻断回滚流程）
fn remove_staging(
    tmp_main: &std::path::Path,
    tmp_wal: &std::path::Path,
    tmp_shm: &std::path::Path,
) {
    let _ = std::fs::remove_file(tmp_main);
    let _ = std::fs::remove_file(tmp_wal);
    let _ = std::fs::remove_file(tmp_shm);
}

pub fn copy_legacy_db(
    legacy_db: &std::path::Path,
    db_path: &std::path::Path,
) -> Result<Vec<String>, CommandError> {
    // 契约守卫（此前只 trust caller 的 `if !db_path.exists()` 前置）：目标库已存在
    // 时，下面 Phase 2a/2b 的 rename 会覆盖既有 -wal/-shm，任一步失败后的回滚还会
    // 把被覆盖的边车删掉——等于销毁现有库的 WAL 状态。这里 fail-closed 直接拒绝。
    if db_path.exists() {
        return Err(CommandError::IoError(format!(
            "目标库已存在（{}），拒绝拷贝老库以保护现有数据；如需重新导入请先手动移走现有库文件",
            db_path.display()
        )));
    }
    // 孤儿边车清场：上次拷贝在 Phase 2a/2b（rename 边车）之后、2c（rename 主库）
    // 之前崩溃，会留下无主 -wal/-shm——不清场则本次新主库 rename 进来后恰与无关
    // WAL 配对（SQLite 按本库日志回放 = 损坏）。此处 db_path 必不存在（上方守卫），
    // 目标路径上任何边车都是孤儿，无保留价值；删除失败 fail-closed 拒拷贝
    // （静默放行会把巧合链原样留给 2c）。须在 2a 之前：2a/2b 之后目标边车已是
    // 本次新拷的，删了会把 BUSY 场景未 checkpoint 的 WAL 一并丢掉。
    for ext in ["wal", "shm"] {
        let orphan = wal_sidecar(db_path, ext);
        if orphan.exists() {
            std::fs::remove_file(&orphan).map_err(|e| {
                CommandError::IoError(format!(
                    "目标路径存在孤儿边车（{}）且删除失败，为防新库与无关 WAL 配对已拒绝拷贝：{e}",
                    orphan.display()
                ))
            })?;
        }
    }
    let mut warns = Vec::new();

    // checkpoint section（不变）
    match rusqlite::Connection::open(legacy_db) {
        Ok(conn) => match conn.query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| {
            r.get::<_, i64>(0)
        }) {
            Ok(0) => {}
            Ok(_) => warns.push(
                "老库 WAL checkpoint 被占（BUSY），WAL 未落主库——-wal/-shm 已一并拷贝".to_string(),
            ),
            Err(e) => {
                // BUSY（源库被其他连接占用）是良性：-wal/-shm 原样带走即可继续；
                // 其余（损坏 NOTADB / IO 等）= 源库不可信，与 open 失败同治：fail-closed。
                if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) {
                    warns.push(
                        "老库 WAL checkpoint 被占（BUSY），WAL 未落主库——-wal/-shm 已一并拷贝"
                            .to_string(),
                    );
                } else {
                    return Err(CommandError::IoError(format!(
                        "老库校验失败，已拒绝拷贝以保护数据（原始老库未改动）：{e}。\
                         请先人工检查老库是否损坏，再重试或删除老库。"
                    )));
                }
            }
        },
        Err(e) => {
            // 源库打不开（损坏/被占用/加密）时继续裸拷 = 把坏库复制成主库。
            // 拒绝拷贝：原库只读未动，留给人工排障后重试。
            return Err(CommandError::IoError(format!(
                "老库打开失败，已拒绝拷贝以保护数据（原始老库未改动）：{e}。\
                 请关闭可能占用该库的程序（如旧版应用/数据库工具）后重启重试；\
                 若反复出现，请先人工检查老库是否损坏。"
            )));
        }
    }

    // Phase 1: copy 3 files to staging tmp-*
    let legacy_wal = wal_sidecar(legacy_db, "wal");
    let legacy_shm = wal_sidecar(legacy_db, "shm");
    let tmp_main = db_path.with_extension("db.copying");
    let tmp_wal = wal_sidecar(&tmp_main, "wal");
    let tmp_shm = wal_sidecar(&tmp_main, "shm");

    let copy_main = std::fs::copy(legacy_db, &tmp_main);
    let copy_wal = if legacy_wal.exists() {
        std::fs::copy(&legacy_wal, &tmp_wal)
    } else {
        Ok(0)
    };
    let copy_shm = if legacy_shm.exists() {
        std::fs::copy(&legacy_shm, &tmp_shm)
    } else {
        Ok(0)
    };

    if let Some(e) = copy_main.err().or(copy_wal.err()).or(copy_shm.err()) {
        remove_staging(&tmp_main, &tmp_wal, &tmp_shm);
        return Err(CommandError::IoError(format!("老库 staging 拷贝失败：{e}")));
    }

    // Phase 2a: rename tmp_wal → db_path-wal（仅当 tmp_wal 存在；legacy 无 -wal 则 skip）
    if tmp_wal.exists() {
        if let Err(e) = std::fs::rename(&tmp_wal, wal_sidecar(db_path, "wal")) {
            remove_staging(&tmp_main, &tmp_wal, &tmp_shm);
            return Err(CommandError::IoError(format!("老库 -wal 提交失败：{e}")));
        }
    }

    // Phase 2b: rename tmp_shm → db_path-shm（仅当 tmp_shm 存在；legacy 无 -shm 则 skip）
    if tmp_shm.exists() {
        if let Err(e) = std::fs::rename(&tmp_shm, wal_sidecar(db_path, "shm")) {
            // 回滚 2a（db_path-wal 已建）+ 清 tmp-*
            let _ = std::fs::remove_file(wal_sidecar(db_path, "wal"));
            remove_staging(&tmp_main, &tmp_wal, &tmp_shm);
            return Err(CommandError::IoError(format!(
                "老库 -shm 提交失败（已回滚 -wal staging）：{e}"
            )));
        }
    }

    // Phase 2c: rename tmp_main → db_path（最后，main commit 是 final）
    if let Err(e) = std::fs::rename(&tmp_main, db_path) {
        // 回滚 2a + 2b（db_path-wal / db_path-shm 已建）
        let _ = std::fs::remove_file(wal_sidecar(db_path, "wal"));
        let _ = std::fs::remove_file(wal_sidecar(db_path, "shm"));
        remove_staging(&tmp_main, &tmp_wal, &tmp_shm);
        return Err(CommandError::IoError(format!(
            "老库主文件提交失败（已回滚 staging）：{e}"
        )));
    }

    Ok(warns)
}

/// SQLite WAL 边车路径：`wmessage.db` → `wmessage.db-wal` / `wmessage.db-shm`
pub fn wal_sidecar(db: &std::path::Path, ext: &str) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push(format!("-{ext}"));
    std::path::PathBuf::from(s)
}

#[cfg(test)]
mod atomic_write_tmp_tests {
    use super::*;

    /// rename 失败（dst 为目录）→ tmp 不残留（不留 .tmp 垃圾）
    #[test]
    fn atomic_write_rename_failure_cleans_tmp() {
        let dir = std::env::temp_dir().join(format!("wm-aw-tmp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("target"); // 目录：rename 必败
        std::fs::create_dir_all(&dst).unwrap();

        let r = atomic_write(&dst, "content");

        assert!(r.is_err(), "rename 到目录必须失败");
        let residue: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(residue.is_empty(), "tmp 不得残留：{residue:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod rotate_tests {
    use super::*;

    /// 未超限不轮转；超限后主文件让位 .old
    #[test]
    fn rotate_only_when_over_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("bot.log");
        std::fs::write(&log, "small").unwrap();
        rotate_log_if_large(&log, 100);
        assert!(log.exists(), "未超限不得轮转");
        assert!(!tmp.path().join("bot.log.old").exists());

        std::fs::write(&log, "0123456789abcdef").unwrap(); // 16 > 10
        rotate_log_if_large(&log, 10);
        assert!(!log.exists(), "超限后主文件应让位");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("bot.log.old")).unwrap(),
            "0123456789abcdef"
        );
    }

    /// 连滚三次：.old 挪到 .old.1（第一代），.old 是上一代，最老一代被挤出
    #[test]
    fn rotate_keeps_two_history_generations() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("bot.log");
        for gen in ["gen0", "gen1", "gen2"] {
            std::fs::write(&log, format!("{gen}-0123456789")).unwrap();
            rotate_log_if_large(&log, 10);
        }
        assert!(!log.exists(), "轮转后主文件让位（下次写入重建）");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("bot.log.old")).unwrap(),
            "gen2-0123456789"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("bot.log.old.1")).unwrap(),
            "gen1-0123456789"
        );
        assert!(
            !tmp.path().join("bot.log.old.2").exists(),
            "只保两代，更早历史应被挤出"
        );
    }
}
