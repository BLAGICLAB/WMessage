//! 迁移模块 Tauri command 入口 + 日志读取纯函数。
//!
//! 共 5 个 #[tauri::command]（migration_rules_load / _import / _template_save /
//! migration_run / _log_read / migration_status）+ 3 个内部辅助
//! （parse_rules_csv 已在 rules.rs；read_migration_log / tail_log_lines 在此模块
//! 因与 _log_read 同生命周期）。

use std::fs;
use std::path::Path;

use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

use super::rules::{load_rules, parse_rules_csv, save_rules, template_csv};
use super::run::POLL_INTERVAL_SECS;
use super::types::{MigrationReport, MigrationStatus, RulesFile};

#[tauri::command]
pub fn migration_rules_load(app: AppHandle) -> CommandResult<RulesFile> {
    Ok(load_rules(&app)?)
}

/// 文件对话框导入规则表（CSV 表格 / 旧 JSON 都支持），返回导入的规则数。
/// ⚠️ blocking 对话框不能在主线程调用（会死锁卡死 App），必须走 spawn_blocking
#[tauri::command]
pub async fn migration_rules_import(app: AppHandle) -> CommandResult<usize> {
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        handle
            .dialog()
            .file()
            .add_filter("规则表 (CSV)", &["csv"])
            .add_filter("规则表 (JSON)", &["json"])
            .add_filter("全部文件", &["*"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| CommandError::Internal(format!("对话框线程失败：{e}")))?;
    let Some(file) = picked else {
        return Err(CommandError::ConfirmRejected); // 取消等同拒绝（无确认超时）
    };
    let path = file
        .into_path()
        .map_err(|e| CommandError::IoError(format!("对话框路径转换失败：{e}")))?;
    let bytes = fs::read(&path).map_err(|e| CommandError::IoError(format!("读取失败：{e}")))?;
    // 编码兜底链：UTF-8 → UTF-16 LE/BE（Excel「Unicode 文本」导出）→ GBK（Excel 默认导出）
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        match String::from_utf8(bytes.clone()) {
            Ok(s) => s,
            Err(_) => {
                let (s, _, _) = encoding_rs::GBK.decode(&bytes);
                s.into_owned()
            }
        }
    };
    let rules: RulesFile = if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败：{e}"))?
    } else {
        parse_rules_csv(&text)?
    };
    super::rules::validate_rules(&rules)?;
    // 破坏性规则导入确认：move=文件离开原位置 / delete=文件删除，均不可自动撤销。
    // 只数启用行（与执行器口径一致：禁用行不触发）；blocking 确认框必须
    // spawn_blocking（主线程阻塞对话框会死锁，同上 pick_file 先例）。
    let mut moves = 0usize;
    let mut deletes = 0usize;
    let mut disabled = 0usize;
    for r in &rules.rules {
        if !r.enabled {
            disabled += 1;
            continue;
        }
        match r.action.as_str() {
            "move" => moves += 1,
            "delete" => deletes += 1,
            _ => {}
        }
    }
    if moves + deletes > 0 {
        let mut msg = format!(
            "导入的规则表含 {} 条移动归档、{} 条删除动作（启用行）。\n\n执行后文件将离开原位置或被删除（不可自动撤销）。确认导入？",
            moves, deletes
        );
        if disabled > 0 {
            msg.push_str(&format!("\n另有 {disabled} 条禁用规则一并导入。"));
        }
        let handle2 = app.clone();
        let confirmed = tauri::async_runtime::spawn_blocking(move || {
            use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
            handle2
                .dialog()
                .message(msg)
                .title("破坏性规则确认")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "确认导入".to_string(),
                    "取消".to_string(),
                ))
                .blocking_show()
        })
        .await
        .map_err(|e| CommandError::Internal(format!("对话框线程失败：{e}")))?;
        if !confirmed {
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Info,
                "rules.import_rejected",
                "moves" => moves.to_string(),
                "deletes" => deletes.to_string(),
                "disabled" => disabled.to_string(),
                "count" => rules.rules.len().to_string(),
            );
            return Err(CommandError::ConfirmRejected);
        }
    }
    let count = rules.rules.len();
    save_rules(&app, &rules)?;
    super::ops::log_line(&app, &format!("导入规则表 {} 条", count));
    // confirmed 审计在 save_rules 成功后落——审计与实际落盘对账一致（先审计后写盘
    // 会出现「审计已确认、盘上无变更」的取证漂移）
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Info,
        "rules.import_confirmed",
        "moves" => moves.to_string(),
        "deletes" => deletes.to_string(),
        "disabled" => disabled.to_string(),
        "count" => count.to_string(),
    );
    Ok(count)
}

/// 保存对话框下载 CSV 表格模版（Excel/WPS 可直接编辑），返回保存路径（取消返回空串）。
/// ⚠️ blocking 对话框必须在 spawn_blocking 里跑（主线程会死锁卡死 App）
#[tauri::command]
pub async fn migration_rules_template_save(app: AppHandle) -> CommandResult<String> {
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        handle
            .dialog()
            .file()
            .set_file_name("wmessage-清理规则模板.csv")
            .add_filter("CSV 表格", &["csv"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| CommandError::Internal(format!("对话框线程失败：{e}")))?;
    let Some(file) = picked else {
        return Ok(String::new());
    };
    let path = file
        .into_path()
        .map_err(|e| CommandError::IoError(format!("对话框路径转换失败：{e}")))?;
    fs::write(&path, template_csv().as_bytes())
        .map_err(|e| CommandError::IoError(e.to_string()))?;
    Ok(path.to_string_lossy().to_string())
}

/// 手动触发一次迁移
#[tauri::command]
pub async fn migration_run(app: AppHandle) -> CommandResult<MigrationReport> {
    // B3: 主线程上不能跑长 IO（跨卷拷贝可冻结算分钟级）。
    // 扔到 spawn_blocking 线程池，AppHandle 在子线程里仍可用（是 Send）。
    tauri::async_runtime::spawn_blocking(move || super::run::run_migration(&app))
        .await
        .map_err(|e| CommandError::from(format!("迁移线程 join 失败：{e}")))?
}

/// 迁移日志读取：尾部 limit 行、最新在前（与机器人审计日志同模式，老板指定）
/// NEW-B-3: 日志最大 5MB，sync 读阻塞主线程 → async + spawn_blocking（B3 同模式）
/// F3（Phase 6b）：读失败（权限/磁盘/损坏）返回 Err(IoError) + ERROR 审计，
/// 不再静默吞成「暂无迁移日志」；仅「文件不存在」返回占位文案。
#[tauri::command]
pub async fn migration_log_read(app: AppHandle, limit: Option<usize>) -> CommandResult<String> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        read_migration_log(&super::rules::log_path(&app2), limit)
    })
    .await
    .map_err(|e| CommandError::Internal(format!("迁移日志读取线程 join 失败：{e}")))?;
    match r {
        Ok(s) => Ok(s),
        Err(e) => {
            // 带 code 写审计（err => 宏臂自动展开 code/recoverable/err）
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Error,
                "migration_log_read_fail",
                err => &e
            );
            Err(e)
        }
    }
}

/// F3（Phase 6b）：纯路径参数版便于单测（tauri command 绑定 Wry AppHandle）。
/// NotFound → Ok("（暂无迁移日志）")；其他 IO 错误 → Err(IoError)，不静默吞。
fn read_migration_log(path: &Path, limit: Option<usize>) -> CommandResult<String> {
    match fs::read_to_string(path) {
        Ok(raw) => Ok(tail_log_lines(&raw, limit)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("（暂无迁移日志）".into()),
        Err(e) => Err(CommandError::IoError(format!("读取迁移日志失败：{e}"))),
    }
}

/// 尾部 limit 行、最新在前（纯函数，可单测）
pub(crate) fn tail_log_lines(raw: &str, limit: Option<usize>) -> String {
    let limit = limit.unwrap_or(500).clamp(1, 5000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    lines.join("\n")
}

#[tauri::command]
pub fn migration_status(app: AppHandle) -> CommandResult<MigrationStatus> {
    let rules = load_rules(&app)?;
    Ok(MigrationStatus {
        rules_count: rules.rules.len(),
        poll_interval_secs: POLL_INTERVAL_SECS,
    })
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_log_lines_tail_reversed_and_clamped() {
        let raw = "l1\nl2\nl3\nl4";
        assert_eq!(tail_log_lines(raw, Some(2)), "l4\nl3");
        // 默认 500 > 总行数：全量倒序
        assert_eq!(tail_log_lines(raw, None), "l4\nl3\nl2\nl1");
        // clamp 下限 1
        assert_eq!(tail_log_lines(raw, Some(0)), "l4");
        // clamp 上限 5000（不越界）
        assert_eq!(tail_log_lines(raw, Some(9999)), "l4\nl3\nl2\nl1");
    }

    /// F3（Phase 6b）：文件不存在 → 占位文案（Ok），日志确实没东西不算错误。
    #[test]
    fn f3_read_migration_log_missing_returns_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("migration.log");
        assert_eq!(read_migration_log(&p, None).unwrap(), "（暂无迁移日志）");
    }

    /// F3（Phase 6b）：IO 错误（路径是目录 → read_to_string 失败且非 NotFound）
    /// → Err(IoError)，不再静默吞成「暂无迁移日志」。
    #[test]
    fn f3_read_migration_log_io_error_not_swallowed() {
        let tmp = tempfile::tempdir().unwrap();
        let err = read_migration_log(tmp.path(), None).unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::IoError);
        assert!(!err.is_recoverable());
    }

    /// F3（Phase 6b）：内容走 tail_log_lines 同逻辑；async wrapper 桥接 CommandResult 不丢内容。
    #[test]
    fn f3_read_migration_log_tail_and_async_bridge() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("migration.log");
        std::fs::write(&p, "a\nb\nc\n").unwrap();
        assert_eq!(read_migration_log(&p, Some(2)).unwrap(), "c\nb");
        let p2 = p.clone();
        let s: CommandResult<String> = tauri::async_runtime::block_on(async {
            tauri::async_runtime::spawn_blocking(move || read_migration_log(&p2, None))
                .await
                .map_err(|e| CommandError::Internal(format!("迁移日志读取线程 join 失败：{e}")))?
        });
        assert_eq!(s.unwrap(), "c\nb\na");
    }
}
