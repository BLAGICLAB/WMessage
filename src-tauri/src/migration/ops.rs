//! 文件操作公用 + 迁移 ops（路径解析 / 冲突命名 / 跨卷 move / 日志）。
//!
//! 跨模块复用的小工具（now_ms / now_str）也放这里——是 IO 侧工具，迁移引擎
//! 和 journal 都按需调用，避免在 mod.rs 堆小工具。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::error::CommandError;

use super::types::MigrationRule;

/// 全局单例：迁移运行时 RUNNING 标志（防重入 RAII 守卫借用）
pub(crate) static RUNNING: AtomicBool = AtomicBool::new(false);

/// 防重入 RAII 守卫：Drop（含 panic 展开）时自动释放，
/// 防止 inner panic 后 RUNNING 永久 true 导致迁移永久锁死（审计 P2）
pub(crate) struct MigrationGuard;

impl MigrationGuard {
    pub(crate) fn acquire() -> Result<Self, CommandError> {
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CommandError::DomainRule {
                domain: "migration".to_string(),
                reason: "迁移正在进行中，请稍后再试".to_string(),
            });
        }
        Ok(Self)
    }
}

impl Drop for MigrationGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ───────────────────────── 匹配与路径 ─────────────────────────

/// 文件名是否命中规则（大小写不敏感包含；空关键字永不命中）
pub fn filename_matches(file_name: &str, rule: &MigrationRule) -> bool {
    let lower = file_name.to_lowercase();
    rule.keywords
        .iter()
        .any(|k| !k.trim().is_empty() && lower.contains(&k.trim().to_lowercase()))
}

/// {year} 占位符展开为当前年份（纯函数，抽出供单测直调——AppHandle 无法单测构造）。
pub(crate) fn expand_year_placeholder(template: &str) -> String {
    let year = chrono::Local::now().format("%Y").to_string();
    template.replace("{year}", &year)
}

/// 归档目录解析：{year} → 当前年份；相对路径基于桌面；绝对路径原样
pub fn resolve_archive_dir(app: &AppHandle, template: &str) -> Result<PathBuf, String> {
    let expanded = expand_year_placeholder(template);
    let p = PathBuf::from(&expanded);
    reject_parent_dir_components(&p, template)?;
    if p.is_absolute() {
        Ok(p)
    } else {
        let desktop = app
            .path()
            .desktop_dir()
            .map_err(|e| format!("无法定位桌面目录：{e}"))?;
        Ok(desktop.join(p))
    }
}

/// 归档目录 containment：拒绝 `..` 组件（相对路径会逃逸桌面基准；对绝对路径同样生效——
/// /tmp/../etc 可直接写 /etc 表达，无能力损失）。绝对路径本身放行（文档化特性），
/// 其收窄与 symlink 逃逸（需 canonicalize + 前缀校验）同属 B 类待拍项。
pub(crate) fn reject_parent_dir_components(p: &Path, template: &str) -> Result<(), String> {
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("归档目录不允许含 \"..\" 组件：{template}"));
    }
    Ok(())
}

/// 同名冲突时生成 `名 (n).ext`（最多试 99 个）；返回 None 表示无可用名
pub(crate) fn conflict_free_name(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let direct = dir.join(file_name);
    if !direct.exists() {
        return Some(direct);
    }
    let (stem, ext) = match file_name.rfind('.') {
        Some(pos) => (&file_name[..pos], &file_name[pos..]),
        None => (file_name, ""),
    };
    for n in 1..100 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// P2-6 TOCTOU 防护：conflict_free_name 选定与 move_entry 实际创建之间存在竞态窗口——
/// 两个并发迁移任务可能选中同一 dst，后写覆盖前写。实际创建前再 exists 一次；
/// 窗口内被并发抢占（文件已出现）则递增后缀重选（上限 99 轮防活锁）。
pub(crate) fn claim_dst_name(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let mut candidate = conflict_free_name(dir, file_name)?;
    for _ in 0..99 {
        if !candidate.exists() {
            return Some(candidate);
        }
        // 竞态窗口内被抢占：重选——conflict_free_name 跳过已存在名，必然递增后缀
        let next = conflict_free_name(dir, file_name)?;
        if next == candidate {
            return None; // 理论不可达（exists 时必换新名），防死循环兜底
        }
        candidate = next;
    }
    None
}

/// 递归拷贝目录（跨盘移动兜底用）：遇符号链接中止，失败时不破坏源
///
/// APW-02a 改造（atomicity-partial-write family +1）：
/// 1. top-level src symlink 检查 —— read_dir 跟读目标 = move 语义失控 → 返 Err
/// 2. top-level dst 已存在检查 —— 保守语义，不改现有"dst 不存在"行为 → 返 Err
/// 3. 顶层建 staging sibling（dst.parent().join(format!("{}.copying", file_name))，
///    显式追加不用 with_extension），递归直写 staging，顶层 fs::rename 原子 commit
/// 4. 失败 fs::remove_dir_all(&staging) 清 staging，dst 未被触碰
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), CommandError> {
    // 1. top-level src symlink 检查（read_dir 跟读目标 = move 语义失控）
    if src
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(CommandError::DomainRule {
            domain: "migration".to_string(),
            reason: format!(
                "src 是符号链接，read_dir 跟读目标，entries 与用户预期不一致：{}",
                src.display()
            ),
        });
    }
    // 2. top-level dst 已存在检查（保守语义）
    if dst.symlink_metadata().is_ok() {
        return Err(CommandError::DomainRule {
            domain: "migration".to_string(),
            reason: format!("dst 已存在，不允许覆盖：{}", dst.display()),
        });
    }
    // 3. 顶层 staging sibling 构造（不用 with_extension，避免 .tar/.pdf 等扩展名被替换错）
    let parent = dst.parent().ok_or_else(|| CommandError::DomainRule {
        domain: "migration".to_string(),
        reason: format!("dst 无 parent：{}", dst.display()),
    })?;
    let file_name = dst.file_name().ok_or_else(|| CommandError::DomainRule {
        domain: "migration".to_string(),
        reason: format!("dst 无 file_name：{}", dst.display()),
    })?;
    let staging = parent.join(format!("{}.copying", file_name.to_string_lossy()));
    // 4. 递归直写 staging，失败清 staging
    if let Err(e) = copy_dir_recursive_into(src, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    // 5. 顶层 fs::rename(staging, dst) 原子 commit
    fs::rename(&staging, dst).map_err(|e| format!("staging 提交失败：{e}"))?;
    Ok(())
}

/// 内部 helper：直写目标路径（不建子级 staging，完整镜像 src 目录结构）
fn copy_dir_recursive_into(src: &Path, dst: &Path) -> Result<(), CommandError> {
    fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("读取目录失败：{e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err(CommandError::DomainRule {
                domain: "migration".to_string(),
                reason: "目录包含符号链接，跨盘移动已中止（源目录未动）".to_string(),
            });
        }
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive_into(&s, &d)?;
        } else {
            fs::copy(&s, &d).map_err(|e| format!("复制文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 移动文件/目录：先 rename（同卷）；失败（如跨卷 EXDEV）退化 copy+remove
pub(crate) fn move_entry(src: &Path, dst: &Path) -> Result<(), String> {
    if let Err(_e) = fs::rename(src, dst) {
        // 跨卷回退：文件 copy+remove；目录递归拷贝后再删源
        if src.is_dir() {
            copy_dir_recursive(src, dst).map_err(|e| format!("跨卷复制失败：{e}"))?;
            fs::remove_dir_all(src).map_err(|e| format!("跨卷复制成功但删除源目录失败：{e}"))?;
            return Ok(());
        }
        fs::copy(src, dst).map_err(|e| format!("复制失败：{e}"))?;
        fs::remove_file(src).map_err(|e| format!("复制成功但删除源文件失败：{e}"))?;
        return Ok(());
    }
    Ok(())
}

// ───────────────────────── NEW-B-5: 跨卷 remove 持续失败防护 ─────────────────────────

/// NEW-B-5: 跨卷回退「copy 成功但 remove 持续失败」（Windows 文件被占用）时，
/// 若不加防护，每轮轮询 conflict_free_name 会生成新名再 copy → 归档目录累积副本。
/// 这里按 src 路径计数：失败超 MAX_MOVE_REMOVE_FAILURES（5 次 × 10min/轮 ≈ 50min
/// 持续失败）后永久跳过该 src 并记 ERROR 审计。计数进程内有效，重启清零重新尝试
/// （占用可能已解除，重启后重试是期望行为）。
static MOVE_REMOVE_FAILS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
    OnceLock::new();

pub(crate) const MAX_MOVE_REMOVE_FAILURES: u32 = 5;

pub(crate) fn move_remove_fail_counts(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, u32>> {
    MOVE_REMOVE_FAILS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 记录一次「copy 成功但 remove 失败」，返回该 src 累计失败次数
pub(crate) fn record_move_remove_failure(src: &str) -> u32 {
    let mut m = move_remove_fail_counts()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let n = m.entry(src.to_string()).or_insert(0);
    *n += 1;
    *n
}

/// 该 src 是否已因 remove 持续失败被永久跳过（不再 copy，防止副本累积）
pub(crate) fn move_remove_permanently_failed(src: &str) -> bool {
    let m = move_remove_fail_counts()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    m.get(src).copied().unwrap_or(0) >= MAX_MOVE_REMOVE_FAILURES
}

// ───────────────────────── 日志 / 事件 ─────────────────────────

pub(crate) fn log_line(app: &AppHandle, line: &str) {
    // rotation 与 append 共用同一次 canonicalize 的结果：旧实现 rotation 走
    // rules::log_path（未 canonicalize）、append 走 canonicalize 后的路径，
    // 数据目录穿 symlink 时两者可分裂（rotation 改名一个拼写、写入仍进另一个）。
    let raw = db::data_dir(app);
    let dir = std::fs::canonicalize(&raw).unwrap_or_else(|_| raw);
    let log_path = dir.join(super::rules::LOG_FILE);
    crate::db::rotate_log_if_large(&log_path, 5 * 1024 * 1024);
    let full = format!("[{}] {line}", now_str());
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{full}")
        });
    // 无论如何也打到开发日志，便于排查
    println!("[migration] {full}");
}

pub(crate) fn emit_upserts(app: &AppHandle, tasks: &[db::Task]) {
    if tasks.is_empty() {
        return;
    }
    let payload = serde_json::json!({ "upserts": tasks, "deletes": [], "source": crate::mutation::MutationOrigin::Migration.as_str() });
    let _ = app.emit_to("main", "tasks-updated", &payload);
}
