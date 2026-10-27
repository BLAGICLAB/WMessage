//! 文件操作公用 + 迁移 ops（路径解析 / 冲突命名 / 跨卷 move / 日志）。
//!
//! 跨模块复用的小工具（now_str）也放这里——是 IO 侧工具，迁移引擎
//! 和 journal 都按需调用，避免在 mod.rs 堆小工具。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tauri::{AppHandle, Emitter};

use crate::db;
use crate::error::CommandError;

use super::types::MigrationRule;

/// 全局单例：迁移运行时 RUNNING 标志（防重入 RAII 守卫借用）
pub(crate) static RUNNING: AtomicBool = AtomicBool::new(false);

/// 防重入 RAII 守卫：Drop（含 panic 展开）时自动释放，
/// 防止 inner panic 后 RUNNING 永久 true 导致迁移永久锁死（审计 ）
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

pub(crate) fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// 匹配与路径

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

/// 归档目标：落盘目录 + 锚点（落盘目录必须落在锚点之内）。
///
/// 锚点按归档目录形态分两种，解析时即捕获：
/// - 相对路径 → 基准目录（归档位置随来源走）
/// - 绝对路径 → 该路径自身的最深已存在祖先（归档位置固定；锚点必须在解析时
///   捕获——落盘后再取「最深已存在祖先」拿到的就是目录自己，等于自证，挡不住
///   祖先在解析与落盘之间被换成符号链接的竞态）
#[derive(Debug)]
pub(crate) struct ArchiveTarget {
    /// 归档目录最终落盘路径（绝对）
    pub(crate) dir: PathBuf,
    /// 锚点（已 canonicalize）：落盘目录必须落在它之内
    pub(crate) anchor: PathBuf,
}

/// 归档目录解析纯函数内核（供调用方以任意基准直调）——四道闸：
/// 1. `..` 组件拒绝（相对路径逃逸基准；绝对路径同样拒，两种形态语义统一）；
/// 2. 根式/盘符相对路径拒绝（Windows 下 `/foo`、`C:foo` 的落点随当前盘符漂移，
///    同一份规则表换台机器就落到别处——必须写完整盘符路径或纯相对目录）；
/// 3. 绝对路径：目录自身即落点（不拼基准），锚点 = 其最深已存在祖先；祖先必须
///    存在且是目录——盘符未挂载时响亮失败，不在本地盘同名位置静默建出目录树；
/// 4. 相对路径：落点 = 基准 + 目录，symlink 逃逸校验沿用（沿目标最深已存在祖先
///    canonicalize，须仍位于 canonical 基准目录内；叶子通常尚不存在，只能校到
///    最深已存在祖先——祖先在基准外即拒；新建叶层走 create_dir_all 常规目录创建，
///    落在 canonical 祖先之下；以符号链接充当新建叶层的场景超出本闸能力，与
///    exists→canonicalize 的 TOCTOU 残余同属文件系统原语限制，归既有
///    「Phase 6 TOCTOU 统一策略」follow-up）。
pub(crate) fn resolve_archive_dir_checked(
    base: &Path,
    template: &str,
) -> Result<ArchiveTarget, String> {
    let expanded = expand_year_placeholder(template);
    let p = PathBuf::from(&expanded);
    reject_parent_dir_components(&p, template)?;
    if p.is_absolute() {
        return absolute_target(&p, template);
    }
    // 根式路径（unix 下同时是绝对路径，已在上面返回；此处只剩 Windows 语义）：
    // "/foo" 无盘符但 join 会替换基准，"C:foo" 是盘符相对路径——两者落点都随
    // 当前盘符漂移，必须拒
    if p.has_root() || is_drive_relative(&p) {
        return Err(format!(
            "归档目录不能是根式/盘符相对路径（落点随当前盘符漂移）：{template}。请写完整绝对路径（如 E:\\归档\\{{year}}）或相对目录（如 工资/{{year}}）"
        ));
    }
    let resolved = base.join(&p);
    let anc = deepest_existing_ancestor(&resolved)
        .ok_or_else(|| format!("归档目录解析失败（基准目录不存在）：{}", base.display()))?;
    let canon_anc = anc
        .canonicalize()
        .map_err(|e| format!("归档目录解析失败（{}）：{e}", anc.display()))?;
    let canon_base = base
        .canonicalize()
        .map_err(|e| format!("基准目录解析失败（{}）：{e}", base.display()))?;
    if !canon_anc.starts_with(&canon_base) {
        return Err(format!(
            "归档目录解析后逃逸基准目录（疑似符号链接指向基准外）：{} → {}",
            resolved.display(),
            canon_anc.display()
        ));
    }
    Ok(ArchiveTarget {
        dir: resolved,
        anchor: canon_base,
    })
}

/// 绝对路径归档目标：锚点 = 该路径的最深已存在祖先。
///
/// 祖先必须存在：Windows 下盘符未挂载时整条链都不存在，此时响亮失败，
/// 而不是让 create_dir_all 在本地盘的同名位置静默建出一棵目录树。
fn absolute_target(p: &Path, template: &str) -> Result<ArchiveTarget, String> {
    if p.exists() && !p.is_dir() {
        return Err(format!("归档目录已存在且不是目录：{}", p.display()));
    }
    let anc = deepest_existing_ancestor(p)
        .ok_or_else(|| format!("归档目录所在路径不存在（盘符未挂载或上级目录缺失）：{template}"))?;
    if !anc.is_dir() {
        return Err(format!("归档目录的上级路径不是目录：{}", anc.display()));
    }
    let anchor = anc
        .canonicalize()
        .map_err(|e| format!("归档目录解析失败（{}）：{e}", anc.display()))?;
    Ok(ArchiveTarget {
        dir: p.to_path_buf(),
        anchor,
    })
}

/// 盘符相对路径判定（Windows 专有语义）：`C:foo` 不是绝对路径，但落点随当前
/// 盘符漂移（相对 C 盘当前工作目录），同一份规则表换台机器就落到别处——与
/// 根式路径 "/foo" 同类，必须拒绝。
/// unix 无此概念（`:` 是合法文件名字符），故只在 Windows 下判定。
pub(crate) fn is_drive_relative(p: &Path) -> bool {
    #[cfg(windows)]
    {
        let Some(s) = p.to_str() else {
            return false;
        };
        let b = s.as_bytes();
        // 盘符后不是路径分隔符即盘符相对（"C:foo"、"C:"）；"C:\foo" 是绝对路径
        b.len() >= 2
            && b[1] == b':'
            && (b[0] as char).is_ascii_alphabetic()
            && !matches!(b.get(2), Some(b'\\') | Some(b'/'))
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        false
    }
}

/// 最深已存在祖先：自底向上找到第一个存在的路径；整条链都不存在返回 None
/// （Windows 下即盘符未挂载）。
fn deepest_existing_ancestor(p: &Path) -> Option<PathBuf> {
    let mut anc = p;
    loop {
        if anc.exists() {
            return Some(anc.to_path_buf());
        }
        let parent = anc.parent()?;
        anc = parent;
    }
}

/// 归档目录落地后复验（终态 canonicalize 前缀校验）：落盘目录必须落在锚点之内。
/// 锚点由 resolve 捕获（见 ArchiveTarget）——落盘后再取「最深已存在祖先」会拿到
/// 目录自己，等于自证；此处的残余窗是 create→verify 之间（文件系统原语限制，
/// 归既有「Phase 6 TOCTOU 统一策略」follow-up）。
pub fn verify_dir_under(anchor: &Path, dir: &Path) -> Result<(), String> {
    let canon_dir = dir
        .canonicalize()
        .map_err(|e| format!("归档目录复验失败（{}）：{e}", dir.display()))?;
    if !canon_dir.starts_with(anchor) {
        return Err(format!(
            "归档目录创建后逃逸锚点目录（疑似符号链接指向锚点外）：{} → {}",
            dir.display(),
            canon_dir.display()
        ));
    }
    Ok(())
}

/// 归档目录 containment：拒绝 `..` 组件（相对路径会逃逸基准；对绝对路径同样生效——
/// /tmp/../etc 可直接写 /etc 表达，无能力损失）。契约边界：本函数只管 `..` 组件，
/// 绝对路径与根式路径由调用方闸（resolve_archive_dir_checked / validate_rules）负责。
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

/// 源路径是否已位于归档目录内（双方 canonicalize 后组件级前缀比较）。
/// Windows 大小写、junction/符号链接拼写差异由 canonicalize 归一；任一侧
/// canonicalize 失败（路径不存在/不可达）按「不在内」处理——调用方守卫宁可
/// 放行一轮搬移，不得因判定失败误跳过待归档文件。
pub(crate) fn path_inside(src: &Path, dir: &Path) -> bool {
    match (src.canonicalize(), dir.canonicalize()) {
        (Ok(s), Ok(d)) => s.starts_with(&d),
        _ => false,
    }
}

///  TOCTOU 防护：conflict_free_name 选定与 move_entry 实际创建之间存在竞态窗口——
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
    // 残留 staging 拒绝且不静默清——上次失败可能留下含用户数据的 .copying，
    // 直接复用会把旧文件混进本次归档副本，报错让人处置
    if staging.symlink_metadata().is_ok() {
        return Err(CommandError::DomainRule {
            domain: "migration".to_string(),
            reason: format!(
                "检测到残留暂存目录 {}，请确认无需保留后手动删除再重试",
                staging.display()
            ),
        });
    }
    // 4. 递归直写 staging，失败清 staging
    if let Err(e) = copy_dir_recursive_into(src, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    // 5. 顶层 fs::rename(staging, dst) 原子 commit；失败时 staging 是本次新建，
    //    可安全清掉，不留 .copying 残骸堵死重试
    fs::rename(&staging, dst).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        CommandError::Internal(format!("staging 提交失败：{e}"))
    })?;
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
    if let Err(e) = fs::rename(src, dst) {
        // 回退会替代掉 rename 的原始错误，失败原因必须留痕（EXDEV 跨卷属预期，
        // 权限/占用类异常只有这里看得到）；不可收紧为仅 EXDEV 回退——dst 父目录
        // 缺失等场景靠 copy 回退补建
        eprintln!(
            "[migration] rename 失败，回退 copy（src: {}，dst: {}）：{e}",
            src.display(),
            dst.display()
        );
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

// 跨卷 remove 持续失败防护

/// 跨卷回退「copy 成功但 remove 持续失败」（Windows 文件被占用）时，
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
    let mut m = crate::db::lock::lock_or_recover(
        move_remove_fail_counts(),
        "migration::ops move_remove_fail_counts",
    );
    let n = m.entry(src.to_string()).or_insert(0);
    *n += 1;
    *n
}

/// 该 src 是否已因 remove 持续失败被永久跳过（不再 copy，防止副本累积）
pub(crate) fn move_remove_permanently_failed(src: &str) -> bool {
    let m = crate::db::lock::lock_or_recover(
        move_remove_fail_counts(),
        "migration::ops move_remove_fail_counts",
    );
    m.get(src).copied().unwrap_or(0) >= MAX_MOVE_REMOVE_FAILURES
}

// 日志 / 事件

pub(crate) fn log_line(app: &AppHandle, line: &str) {
    // rotation 与 append 共用同一次 canonicalize 的结果：旧实现 rotation 走
    // rules::log_path（未 canonicalize）、append 走 canonicalize 后的路径，
    // 数据目录穿 symlink 时两者可分裂（rotation 改名一个拼写、写入仍进另一个）。
    let raw = db::data_dir(app);
    let dir = std::fs::canonicalize(&raw).unwrap_or(raw);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-arch-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 正常相对模板 → 落在基准目录内
    #[test]
    fn resolve_checked_relative_ok() {
        let base = tmp_dir("rel");
        let t = resolve_archive_dir_checked(&base, "工资/{year}").unwrap();
        assert!(t.dir.starts_with(&base), "应落在基准目录内：{:?}", t.dir);
        // 锚点是 canonical 后的基准（macOS 的 /var → /private/var 符号链接会让
        // 原始路径与 canonical 路径不相等，故按 canonical 基准比对）
        assert_eq!(t.anchor, base.canonicalize().unwrap(), "锚点应是基准目录");
        std::fs::remove_dir_all(&base).ok();
    }

    /// `..` 组件拒绝（既有闸，回归锁）
    #[test]
    fn resolve_checked_rejects_parent_components() {
        let base = tmp_dir("dotdot");
        let err = resolve_archive_dir_checked(&base, "../escape").unwrap_err();
        assert!(err.contains(".."), "got: {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 绝对路径放行：目录自身即落点，不拼基准（桌面 → E 盘归档的基本能力）
    #[test]
    fn resolve_checked_accepts_absolute() {
        let base = tmp_dir("abs");
        let abs = std::env::temp_dir().join("wmessage-arch-abs");
        let t = resolve_archive_dir_checked(&base, &abs.to_string_lossy()).unwrap();
        assert_eq!(t.dir, abs, "绝对路径原样作为落点");
        assert_eq!(
            t.anchor,
            std::env::temp_dir().canonicalize().unwrap(),
            "锚点应是该路径的最深已存在祖先（canonical 口径）"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// 绝对路径含 `..` → 拒绝（两种形态同一把尺，语义统一）
    #[test]
    fn resolve_checked_absolute_rejects_parent_components() {
        let base = tmp_dir("absdotdot");
        let abs = std::env::temp_dir()
            .join("wmessage-arch-abs")
            .join("..")
            .join("etc");
        let err = resolve_archive_dir_checked(&base, &abs.to_string_lossy()).unwrap_err();
        assert!(err.contains(".."), "got: {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 绝对路径的 {year} 占位符照常展开（E:\归档\{year} 形态）
    #[test]
    fn resolve_checked_absolute_expands_year() {
        let base = tmp_dir("absyear");
        let abs = std::env::temp_dir().join("wmessage-arch-year-{year}");
        let t = resolve_archive_dir_checked(&base, &abs.to_string_lossy()).unwrap();
        assert!(!t.dir.to_string_lossy().contains("{year}"), "占位符应展开");
        assert!(t
            .dir
            .to_string_lossy()
            .contains(&chrono::Local::now().format("%Y").to_string()));
        std::fs::remove_dir_all(&base).ok();
    }

    /// 绝对路径已存在且是文件 → 拒绝（不是目录）
    #[test]
    fn resolve_checked_absolute_file_target_rejected() {
        let base = tmp_dir("absfile");
        let f = std::env::temp_dir().join(format!("wm-arch-file-{}", uuid::Uuid::new_v4()));
        std::fs::write(&f, b"x").unwrap();
        let err = resolve_archive_dir_checked(&base, &f.to_string_lossy()).unwrap_err();
        assert!(err.contains("不是目录"), "got: {err}");
        std::fs::remove_file(&f).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    /// 根式路径（unix 下同时是绝对路径，放行；Windows 语义另测）
    #[cfg(unix)]
    #[test]
    fn resolve_checked_unix_root_path_is_absolute() {
        let base = tmp_dir("unixroot");
        let t = resolve_archive_dir_checked(&base, "/tmp/wm-arch-root").unwrap();
        assert_eq!(t.dir, PathBuf::from("/tmp/wm-arch-root"));
        std::fs::remove_dir_all(&base).ok();
    }

    /// Windows 根式/盘符相对路径拒绝：落点随当前盘符漂移
    #[cfg(windows)]
    #[test]
    fn resolve_checked_rejects_root_style_paths() {
        let base = tmp_dir("rootstyle");
        for tpl in ["/wm-arch-root", "C:wm-arch-drive"] {
            let err = resolve_archive_dir_checked(&base, tpl).unwrap_err();
            assert!(err.contains("根式"), "got: {err}");
        }
        std::fs::remove_dir_all(&base).ok();
    }

    /// Windows 盘符未挂载 → fail-closed：不在本地盘同名位置静默建目录树
    #[cfg(windows)]
    #[test]
    fn resolve_checked_missing_drive_fails_closed() {
        let base = tmp_dir("missingdrive");
        // 找一个不存在的盘符（Z 往下找）；全都被占用则本用例无意义，跳过
        let drive = ('Z'..='A')
            .rev()
            .find(|c| !PathBuf::from(format!("{c}:\\")).exists());
        let Some(d) = drive else {
            std::fs::remove_dir_all(&base).ok();
            return;
        };
        let p = format!("{d}:\\wm-arch-missing\\x");
        let err = resolve_archive_dir_checked(&base, &p).unwrap_err();
        assert!(err.contains("不存在"), "got: {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 锚点复验：目录落在锚点内 → 通过；落在锚点外 → 拒绝
    #[test]
    fn verify_dir_under_checks_anchor() {
        let base = tmp_dir("verify");
        let inside = base.join("inside");
        std::fs::create_dir_all(&inside).unwrap();
        let anchor = base.canonicalize().unwrap();
        assert!(verify_dir_under(&anchor, &inside).is_ok(), "锚点内 → 通过");
        let outside =
            std::env::temp_dir().join(format!("wm-arch-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        assert!(
            verify_dir_under(&anchor, &outside).is_err(),
            "锚点外 → 拒绝"
        );
        std::fs::remove_dir_all(&outside).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    /// symlink 逃逸：基准内符号链接指向基准外 → 拒绝
    #[cfg(unix)]
    #[test]
    fn resolve_checked_rejects_symlink_escape() {
        let base = tmp_dir("symlink");
        let outside = tmp_dir("outside");
        std::os::unix::fs::symlink(&outside, base.join("link")).unwrap();
        let err = resolve_archive_dir_checked(&base, "link/sub").unwrap_err();
        assert!(err.contains("逃逸"), "got: {err}");
        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    /// 自搬防线内核：归档目录内的文件判内；组件级比较不吃字符串前缀陷阱
    /// （`archx` 与 `arch` 是兄弟目录）；路径不存在按不在内处理
    #[test]
    fn path_inside_component_compare_not_string_prefix() {
        let base = tmp_dir("inside");
        let dir = base.join("arch");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(path_inside(&f, &dir), "归档目录内 → 判内");

        let trap = base.join("archx");
        std::fs::create_dir_all(&trap).unwrap();
        let tf = trap.join("b.txt");
        std::fs::write(&tf, b"x").unwrap();
        assert!(!path_inside(&tf, &dir), "archx 不是 arch 内（组件级比较）");

        let sf = {
            let s = base.join("other");
            std::fs::create_dir_all(&s).unwrap();
            let p = s.join("c.txt");
            std::fs::write(&p, b"x").unwrap();
            p
        };
        assert!(!path_inside(&sf, &dir), "同级目录 → 判外");
        assert!(
            !path_inside(&dir.join("nope.txt"), &dir),
            "src 不存在 → canonicalize 失败 → 按不在内"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// 预置残留 .copying 暂存目录 → copy 报错拒绝：不静默清（可能含用户数据）、
    /// 不产出混有旧文件的目标
    #[test]
    fn copy_dir_recursive_rejects_stale_staging() {
        let base = tmp_dir("stale-staging");
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        let staging = base.join("dst.copying");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("old.txt"), b"stale").unwrap();
        let err = copy_dir_recursive(&src, &dst).unwrap_err();
        assert!(
            err.to_string().contains("残留"),
            "错误须指向残留暂存目录，got: {err}"
        );
        assert!(!dst.exists(), "残留存在时不得产出目标");
        assert!(staging.exists(), "残留不得被静默清除");
        std::fs::remove_dir_all(&base).ok();
    }
}
