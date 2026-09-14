//! 路径解析 + 日志轮转 + 原子写 + 老库拷贝边

use std::path::PathBuf;
use tauri::Manager;

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
    std::fs::rename(&tmp, path).map_err(|e| format!("落盘重命名失败：{e}"))?;
    Ok(())
}

/// 便携模式首启拷贝老库——checkpoint 失败记 warn 继续；同时拷 `-wal` / `-shm` 边车
pub fn copy_legacy_db(legacy_db: &std::path::Path, db_path: &std::path::Path) -> Vec<String> {
    let mut warns = Vec::new();
    match rusqlite::Connection::open(legacy_db) {
        Ok(conn) => match conn.query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| {
            r.get::<_, i64>(0)
        }) {
            Ok(0) => {}
            Ok(_) => warns.push(
                "老库 WAL checkpoint 被占（BUSY），WAL 未落主库——-wal/-shm 已一并拷贝".to_string(),
            ),
            Err(e) => warns.push(format!(
                "老库 WAL checkpoint 失败（继续拷贝，-wal/-shm 一并带走）：{e}"
            )),
        },
        Err(e) => warns.push(format!("老库打开失败（跳过 checkpoint 直接拷贝）：{e}")),
    }
    let tmp = db_path.with_extension("db.copying");
    let copied = std::fs::copy(legacy_db, &tmp)
        .map_err(|e| e.to_string())
        .and_then(|_| std::fs::rename(&tmp, db_path).map_err(|e| e.to_string()));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
        warns.push(format!("老库拷贝失败：{e}"));
        return warns;
    }
    for ext in ["wal", "shm"] {
        let src = wal_sidecar(legacy_db, ext);
        if src.exists() {
            if let Err(e) = std::fs::copy(&src, wal_sidecar(db_path, ext)) {
                warns.push(format!("老库 -{ext} 边车拷贝失败：{e}"));
            }
        }
    }
    warns
}

/// SQLite WAL 边车路径：`wmessage.db` → `wmessage.db-wal` / `wmessage.db-shm`
pub fn wal_sidecar(db: &std::path::Path, ext: &str) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push(format!("-{ext}"));
    std::path::PathBuf::from(s)
}
