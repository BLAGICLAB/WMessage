//! 用户指令文件（instructions.md）：进 system 的用户权威指令层
//!
//! 与 DB 语义记忆体分工：记忆体管自动沉淀（检索注入、衰减去重），
//! 本文件管用户手工维护的确定性指令——用户可枚举、可 diff、直接编辑。
//! 数据面单文件 `data_dir/instructions.md`。
//!
//! 读侧（每回合进 system）：文件缺失/为空 = 槽为空串；超限（外部编辑越过
//! 保存闸）= 载入截断带标注，首次截断发 WARN 审计、后续只进日志（回合级
//! 读盘，审计刷屏无意义）。
//! 写侧（设置页保存）：超限直接拒——保存闸拦得住的，就不留到载入侧兜底。

use std::path::PathBuf;

use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

/// 指令文件字符上限（载入截断与保存拒超共用同一常量，两层闸不许劈叉）
pub const INSTRUCTIONS_MAX_CHARS: usize = 16_000;

const INSTRUCTIONS_FILE_NAME: &str = "instructions.md";
const TRUNCATION_NOTE: &str = "\n\n（已截断：超过 16000 字上限，其余部分未载入）";

/// 截断渲染：超限取前 MAX 字符并补标注；未超限原样返回
fn render(raw: &str) -> String {
    if raw.chars().count() <= INSTRUCTIONS_MAX_CHARS {
        return raw.to_string();
    }
    let mut out: String = raw.chars().take(INSTRUCTIONS_MAX_CHARS).collect();
    out.push_str(TRUNCATION_NOTE);
    out
}

fn instructions_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::db::data_dir(app).join(INSTRUCTIONS_FILE_NAME)
}

/// 从目录读指令文件原文（未截断渲染）。文件缺失 = 空串（预期形态，首启即此）；
/// 其余读错误也回落空串但留 warn 日志——指令层不许打断回合，失败必须可见。
fn read_raw(dir: &std::path::Path) -> String {
    let path = dir.join(INSTRUCTIONS_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            log::warn!(
                "指令文件读取失败（{}）：{e}，本回合按空指令处理",
                path.display()
            );
            String::new()
        }
    }
}

/// 截断告警判据（纯函数便于单测）：原文超限即告警。
/// 不能用「rendered 与 raw 比长短」——截断只可能缩短或等长，那个条件恒假，
/// 告警会永久静默（截断态恰恰是最需要可见的时候）。
fn should_warn_truncated(raw: &str) -> bool {
    raw.chars().count() > INSTRUCTIONS_MAX_CHARS
}

/// 进 system 的指令段（已截断渲染）。空文件返回空串，调用方据此跳过槽位。
pub fn load_user_instructions<R: tauri::Runtime>(app: &AppHandle<R>) -> String {
    let raw = read_raw(&crate::db::data_dir(app));
    let rendered = render(&raw);
    if should_warn_truncated(&raw) {
        warn_truncated_once(app);
    }
    rendered
}

/// 载入全路径（读 + 截断渲染，无审计面）——fs 级测试入口
#[cfg(test)]
fn load_from_dir(dir: &std::path::Path) -> String {
    render(&read_raw(dir))
}

/// 截断告警：进程内只发一次 WARN 审计（截断态是持续状态，逐回合审计刷屏），
/// 之后回落 debug 日志。
fn warn_truncated_once<R: tauri::Runtime>(app: &AppHandle<R>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static AUDITED: AtomicBool = AtomicBool::new(false);
    if AUDITED.swap(true, Ordering::Relaxed) {
        log::debug!("指令文件仍处截断态（超过 {INSTRUCTIONS_MAX_CHARS} 字），请在设置页精简");
        return;
    }
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Warn,
        "instructions.truncated",
        "max_chars" => INSTRUCTIONS_MAX_CHARS,
    );
}

/// 指令文件视图（设置页编辑卡的数据面）
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsView {
    /// 文件原文（未截断渲染——编辑卡展示的就是盘上内容）
    pub content: String,
    pub chars: usize,
    pub max_chars: usize,
}

/// 读指令文件（设置页编辑卡回填）
#[tauri::command]
pub fn instructions_get(app: AppHandle) -> CommandResult<InstructionsView> {
    let path = instructions_path(&app);
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(CommandError::IoError(format!("指令文件读取失败：{e}"))),
    };
    Ok(InstructionsView {
        chars: content.chars().count(),
        max_chars: INSTRUCTIONS_MAX_CHARS,
        content,
    })
}

/// 写指令文件（设置页保存）。空内容 = 清空指令（合法，槽位随之消失）。
#[tauri::command]
pub fn instructions_set(app: AppHandle, content: String) -> CommandResult<InstructionsView> {
    let chars = content.chars().count();
    if chars > INSTRUCTIONS_MAX_CHARS {
        return Err(CommandError::IoError(format!(
            "指令文件超出上限：{chars} 字 > 上限 {INSTRUCTIONS_MAX_CHARS} 字，请精简后再保存"
        )));
    }
    crate::db::atomic_write(&instructions_path(&app), &content)
        .map_err(|e| CommandError::IoError(format!("指令文件保存失败：{e}")))?;
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Info,
        "instructions.updated",
        "chars" => chars,
    );
    Ok(InstructionsView {
        chars,
        max_chars: INSTRUCTIONS_MAX_CHARS,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_keeps_short_content_untouched() {
        assert_eq!(render("回复风格要简短。"), "回复风格要简短。");
        assert_eq!(render(""), "");
    }

    #[test]
    fn render_truncates_at_char_boundary_with_note() {
        // 中文按字符计（不是字节），截断不得切出半个字符
        let raw = "汉".repeat(INSTRUCTIONS_MAX_CHARS + 5);
        let out = render(&raw);
        assert_eq!(
            out.chars().count(),
            INSTRUCTIONS_MAX_CHARS + TRUNCATION_NOTE.chars().count()
        );
        assert!(out.ends_with(TRUNCATION_NOTE));
        assert!(out.starts_with(&"汉".repeat(INSTRUCTIONS_MAX_CHARS)));
    }

    #[test]
    fn render_keeps_exactly_max_chars_untouched() {
        let raw = "a".repeat(INSTRUCTIONS_MAX_CHARS);
        assert_eq!(render(&raw), raw);
    }

    #[test]
    fn read_raw_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_raw(dir.path()), "");
    }

    #[test]
    fn read_raw_reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(INSTRUCTIONS_FILE_NAME), "永远用中文回复").unwrap();
        assert_eq!(read_raw(dir.path()), "永远用中文回复");
    }

    #[test]
    fn load_from_dir_truncates_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let raw = "字".repeat(INSTRUCTIONS_MAX_CHARS + 1);
        std::fs::write(dir.path().join(INSTRUCTIONS_FILE_NAME), &raw).unwrap();
        let loaded = load_from_dir(dir.path());
        assert!(loaded.ends_with(TRUNCATION_NOTE));
        assert!(loaded.starts_with(&"字".repeat(INSTRUCTIONS_MAX_CHARS)));
        assert!(!loaded.contains(&"字".repeat(INSTRUCTIONS_MAX_CHARS + 1)));
    }

    #[test]
    fn warn_predicate_fires_exactly_when_raw_exceeds_max() {
        assert!(!should_warn_truncated(&"a".repeat(INSTRUCTIONS_MAX_CHARS)));
        assert!(should_warn_truncated(
            &"a".repeat(INSTRUCTIONS_MAX_CHARS + 1)
        ));
        // 深度超限时渲染结果比原文短；贴线小超限（超限量 < 标注长度）反而更长——
        // 所以「渲染前后比长短」的告警判据在两种方向上都不可靠，判据必须钉在原文上
        let raw = "汉".repeat(INSTRUCTIONS_MAX_CHARS + 100);
        let rendered = render(&raw);
        assert!(rendered.chars().count() < raw.chars().count());
        assert!(should_warn_truncated(&raw));
    }
}
