//! 老 free-form 审计入口冻结守卫：`audit_log` 已桥接到 write_event 单一写侧，
//! 存量调用点按白名单收敛（迁移完一文件摘一文件）；白名单外出现新的
//! `audit_log(` 调用即 fail——新代码一律 `audit_event!`。

const WHITELIST: &[&str] = &[
    // 定义与桥接实现本体
    "bot/config/audit.rs",
    // 存量调用文件（迁移一批摘一批；摘干净后本白名单清空、audit_log 退役）
    "apps/manage.rs",
    "bot/config/commands.rs",
    "bot/corrections.rs",
    "bot/mcp/builtin.rs",
    "bot/subagent_digest.rs",
    "bot/subagent_repair.rs",
    "bot/subagent_runner.rs",
    "bot/subagent_spawn.rs",
    "bot/subagent_tools.rs",
    "bot/mcp/commands.rs",
    // 工具实现层拆成 bot/tools/ 子模块后，存量调用点跟着搬了过来
    "bot/tools/common.rs",
    "bot/tools/doc.rs",
    "bot_fs.rs",
    "bot_chat.rs",
    // 聊天主流程拆成 bot_chat/ 子模块后，存量调用点跟着搬了过来
    "bot_chat/exec.rs",
    "bot_chat/flow.rs",
    "bot_chat/media.rs",
    "bot_desktop.rs",
    "bot_manage_file.rs",
    "bot_model_loop.rs",
    "bot_onions.rs",
    "bot_orchestrator.rs",
    "bot_reminders.rs",
    "bot_scheduler.rs",
    "bot_skills/files.rs",
    "bot_skills/manage.rs",
    // create_skill 链路从 bot_skills/manage.rs 拆出后，存量调用点跟着搬了过来
    "bot_skills/manage/create.rs",
    "bot_skills/remote.rs",
    "bot_skills/runtime.rs",
    "bot_slash.rs",
    "bot_ui.rs",
    "bot_web.rs",
    "db/bot_history.rs",
    "due_notify.rs",
    "exec_steps.rs",
    "ocr.rs",
    "py/document.rs",
];

fn scan_dir(
    dir: &std::path::Path,
    test_only: &std::collections::HashSet<String>,
    hits: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_dir(&p, test_only, hits);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let rel = p
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if test_only.contains(&rel) {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            let count = crate::bot::test_files::prod_slice(&body)
                .matches("audit_log(")
                .count();
            if count > 0 && !WHITELIST.contains(&rel.as_str()) {
                hits.push(format!("{rel}: {count} 处"));
            }
        }
    }
}

#[test]
fn audit_log_frozen_to_migration_whitelist() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let test_only = crate::bot::test_files::test_only_files(&src);
    let mut hits = Vec::new();
    scan_dir(&src, &test_only, &mut hits);
    assert!(
        hits.is_empty(),
        "白名单外出现 audit_log( 调用（新代码一律 audit_event!；存量迁移完从白名单摘除）：\n  {}",
        hits.join("\n  ")
    );
}

/// 口径自检：生产切片与代码位置无关。
///
/// 旧口径按「首个 `#[cfg(test)]` 截断」，于是「测试块在前、生产代码在后」
/// 的文件里，后半段生产代码整个扫不到——`bot_fs.rs` 的 20 处
/// `audit_log(` 调用就是这么躲过去的。口径退回到位置启发式时这个测试会红。
#[test]
fn prod_slice_ignores_code_position() {
    let body = "\
fn production_head() { audit_log(1); }
#[cfg(test)]
mod tests { fn t() { audit_log(2); } }
fn production_tail() { audit_log(3); }
";
    let prod = crate::bot::test_files::prod_slice(body);
    assert!(
        prod.contains("audit_log(1)"),
        "测试块之前的生产代码必须在扫描范围内"
    );
    assert!(
        prod.contains("audit_log(3)"),
        "测试块之后的生产代码必须在扫描范围内（旧口径在这里漏扫）"
    );
    assert!(
        !prod.contains("audit_log(2)"),
        "测试块内部的调用不属生产调用"
    );
}

/// 口径自检：测试专用文件靠模块声明识别，不靠文件名。
///
/// `audit/freeform_freeze.rs`（守卫自身）与 `bot/eval.rs` 都是测试专用
/// 但名字里没有 "test"；`bot/registry.rs` 名字像测试文件却是生产代码。
/// 靠猜名字两处都错，所以判据必须是 `#[cfg(test)] mod X;` 声明。
#[test]
fn test_only_files_resolved_from_module_declarations() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let test_only = crate::bot::test_files::test_only_files(&src);
    assert!(
        test_only.contains("audit/freeform_freeze.rs"),
        "守卫自身是测试专用文件（由 audit.rs 的 #[cfg(test)] mod freeform_freeze; 声明而来）"
    );
    assert!(
        test_only.contains("db/tests.rs"),
        "db/tests.rs 由 db/mod.rs 的 #[cfg(test)] mod tests; 声明而来"
    );
    assert!(
        !test_only.contains("bot/registry.rs"),
        "bot/registry.rs 是生产文件（名字像测试文件不代表是）"
    );
}
