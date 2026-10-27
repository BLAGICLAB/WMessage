//! 扫描整理引擎（手动触发，不定时）：对启用来源地盘内的**无主**文件按全局规则
//! 归类。与绑定整理（run.rs）的分界：任务卡绑定物一律跳过（含回收站/归档卡——
//! 有主文件只由卡片归档时机整理，搬移与改地址同批落盘，连接永不断）。
//!
//! 扫描搬移无 journal：无主文件没有 DB 侧写，搬完即一致，崩溃最多丢一行记录。

use std::collections::HashSet;

use tauri::AppHandle;

use crate::db;
use crate::error::CommandError;

use super::ops::{
    claim_dst_name, filename_matches, log_line, move_entry, move_remove_permanently_failed,
    path_inside, record_move_remove_failure, resolve_archive_dir_checked, verify_dir_under,
    MigrationGuard, MAX_MOVE_REMOVE_FAILURES,
};
use super::rules::load_rules;
use super::sources::{load_sources, source_rule_base, source_scan_root};
use super::types::MigrationReport;

/// 手动扫描入口：持迁移守卫（与绑定整理同锁，防并行对同一文件竞争）
pub fn run_scan(app: &AppHandle) -> Result<MigrationReport, CommandError> {
    let _guard = MigrationGuard::acquire()?;
    run_scan_inner(app)
}

fn run_scan_inner(app: &AppHandle) -> Result<MigrationReport, CommandError> {
    let mut report = MigrationReport {
        ts: db::now_ms(),
        ..Default::default()
    };
    // 规则/来源不可读 → 本轮中止：绝不以空规则执行任何 move/delete（同绑定整理口径）
    let rules = load_rules(app)?;
    let sources = load_sources(app)?;
    // 排除清单：所有任务卡的绑定路径（含回收站/归档卡）。小写归一后精确比对，
    // 绑定物在扫描制眼中等同不存在，由绑定整理独占管理
    let tasks = tauri::async_runtime::block_on(async { db::db_load(app.clone()).await })
        .map_err(|e| e.to_string())?;
    let bound: HashSet<String> = tasks
        .iter()
        .flat_map(|t| t.effective_files())
        .map(|f| f.path.to_lowercase())
        .collect();

    for source in sources.sources.iter().filter(|s| s.enabled) {
        let root = match source_scan_root(app, source) {
            Ok(r) => r,
            Err(e) => {
                report.skipped += 1;
                let line = format!("[扫描]跳过来源「{}」：{e}", source_label(source));
                report.log.push(line.clone());
                log_line(app, &line);
                continue;
            }
        };
        let base = source_rule_base(source, &root);
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => {
                report.skipped += 1;
                let line = format!(
                    "[扫描]跳过来源「{}」：读取目录失败：{e}",
                    source_label(source)
                );
                report.log.push(line.clone());
                log_line(app, &line);
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(ty) = entry.file_type() else { continue };
            // 只整理文件；目录与符号链接不碰（后者跟读目标超出扫描语义）
            if ty.is_dir() || ty.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if bound.contains(&path.to_string_lossy().to_lowercase()) {
                continue; // 绑定物：静默跳过（每轮扫描都会复现，不记噪音日志）
            }
            let Some(rule) = rules
                .rules
                .iter()
                .find(|r| r.enabled && filename_matches(&name, r))
            else {
                continue; // 无命中规则：不动，也不记噪音日志
            };
            match rule.action.as_str() {
                "move" => {
                    let target = match resolve_archive_dir_checked(&base, &rule.archive_dir) {
                        Ok(t) => t,
                        Err(e) => {
                            report.skipped += 1;
                            let line = format!("[扫描]跳过「{name}」：{e}");
                            report.log.push(line.clone());
                            log_line(app, &line);
                            continue;
                        }
                    };
                    let (dir, anchor) = (target.dir, target.anchor);
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        report.skipped += 1;
                        let line = format!("[扫描]跳过「{name}」：创建归档目录失败：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                    if let Err(e) = verify_dir_under(&anchor, &dir) {
                        report.skipped += 1;
                        let line = format!("[扫描]跳过「{name}」：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                    // 已在归档目录内：上轮已归位，防同名冲突后缀自我膨胀（同绑定整理）
                    if path_inside(&path, &dir) {
                        continue;
                    }
                    // 跨卷 copy 成功但 remove 持续失败的 src 永久跳过（同绑定整理口径）
                    let path_str = path.to_string_lossy().to_string();
                    if move_remove_permanently_failed(&path_str) {
                        report.skipped += 1;
                        continue;
                    }
                    let Some(dst) = claim_dst_name(&dir, &name) else {
                        report.skipped += 1;
                        let line =
                            format!("[扫描]跳过「{name}」：目标目录同名冲突过多（不覆盖、不删除）");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    };
                    if let Err(e) = move_entry(&path, &dst) {
                        if path.exists() && dst.exists() {
                            let n = record_move_remove_failure(&path_str);
                            if n >= MAX_MOVE_REMOVE_FAILURES {
                                let line = format!(
                                    "[扫描]ERROR「{name}」：跨卷复制后删除源持续失败 {n} 次（{path_str}），已永久跳过该源"
                                );
                                report.log.push(line.clone());
                                log_line(app, &line);
                            }
                        }
                        report.skipped += 1;
                        let line = format!("[扫描]跳过「{name}」：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                    report.moved += 1;
                    let line = format!("[扫描]已移动「{name}」→ {}", dst.display());
                    report.log.push(line.clone());
                    log_line(app, &line);
                }
                "delete" => {
                    // 无主文件删除：无 DB 绑定可清，直接删 + 记录（journal 无对象可对账）
                    if let Err(e) = std::fs::remove_file(&path) {
                        report.skipped += 1;
                        let line = format!("[扫描]删除「{name}」失败（可能被占用/权限不足）：{e}");
                        report.log.push(line.clone());
                        log_line(app, &line);
                        continue;
                    }
                    report.deleted += 1;
                    let line = format!("[扫描]已删除「{name}」（规则显式启用 delete）");
                    report.log.push(line.clone());
                    log_line(app, &line);
                }
                _ => {}
            }
        }
    }

    let line = format!(
        "[扫描]手动扫描完成：移动 {} · 删除 {} · 跳过 {}",
        report.moved, report.deleted, report.skipped
    );
    report.log.push(line.clone());
    log_line(app, &line);
    Ok(report)
}

fn source_label(source: &super::types::ScanSource) -> String {
    match source.kind.as_str() {
        "desktop" => "桌面".to_string(),
        _ => source.path.clone(),
    }
}
