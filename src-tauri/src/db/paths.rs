//! 路径解析 + 日志轮转 + 原子写 + 老库拷贝边

use std::path::PathBuf;
use tauri::Manager;

use crate::error::CommandError;

/// 便携模式：数据库优先放 exe 同目录（U盘/绿色目录随走随带）；
/// 目录不可写（如 Program Files）时兜底到系统应用数据目录
pub fn db_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::paths::probe_log_dir(app)
}

/// 数据目录（供本地 HTTP API 存 token 等附属文件，便携模式跟随 exe）
pub fn data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    db_dir(app)
}

/// AI 产物目录唯一入口：data_dir/AI_Gen_Files，解析即建
pub fn gen_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> std::io::Result<std::path::PathBuf> {
    let dir = data_dir(app).join("AI_Gen_Files");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 日志轮转：超过 size_limit 字节就改名 .old（旧 .old 覆盖）
pub fn rotate_log_if_large(path: &std::path::Path, size_limit: u64) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() > size_limit {
            let old = path.with_extension("log.old");
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

    /// rename 失败（dst 为目录）→ tmp 不残留（NEW-5：不留 .tmp 垃圾）
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
