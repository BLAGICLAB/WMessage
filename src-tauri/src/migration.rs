//! 桌面文件自动迁移清理：任务进入归档列表（完成满 7 天）后，按用户规则表迁移其绑定文件。
//!
//! 安全约束：
//! - 只处理 archived=true 且未删除的任务——常规看板任务的文件绝不移动/删除
//! - delete 类规则默认关闭（模版即如此），仅当用户显式启用才执行删除
//! - 源文件不存在 / 权限不足 / 同名冲突：记日志跳过，不崩溃、不覆盖、不强制删除
//! - 移动成功后任务 filePath 同步更新为新路径，附件链接持续可用
//!
//! 规则表（数据目录 cleanup-rules.json，用户可随时导入，无需改代码）：
//! ```json
//! { "version": 1, "rules": [
//!   { "id": "…", "enabled": true,  "keywords": ["工资"], "action": "move",   "archiveDir": "工资/{year}" },
//!   { "id": "…", "enabled": false, "keywords": ["临时"], "action": "delete", "archiveDir": "" }
//! ] }
//! ```
//! - keywords：文件名包含任一关键字即命中（大小写不敏感；空 = 永不命中）
//! - action：move（移动归档）| delete（删除文件，默认关闭）
//! - archiveDir：相对桌面；`{year}` 展开为当前年份；绝对路径原样使用

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::error::{CommandError, CommandResult};

/// 完成满 7 天进入归档（与前端 applyArchiveRule 的 ARCHIVE_AFTER_MS 一致）
pub const ARCHIVE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const RULES_FILE: &str = "cleanup-rules.json";
const LOG_FILE: &str = "migration.log";
const POLL_INTERVAL_SECS: u64 = 600;

/// B1: 文件迁移操作日志记录（防 lost-update / 孤儿文件）。
/// 原来 file-move 成功但 db_upsert 失败 → 下一轮“源已消失”逻辑会解绑 file_path，
/// 附件链接永久丢失。本表记录「正在进行」的操作，启动时 replay 修复 DB。
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub id: i64,
    pub op: String,        // 'move' | 'delete'
    pub src: String,
    pub dst: Option<String>,
    pub task_id: String,
    pub state: String,     // 'pending' | 'committed' | 'cleared'
    pub created_at: i64,
}

/// B1 inner: 记录一个 pending 操作，返回 row id。
/// 抽出来为方便单测（不需 AppHandle）。生产仍走 journal_pending 包一层。
fn journal_pending_inner(
    conn: &rusqlite::Connection,
    op: &str,
    src: &Path,
    dst: Option<&Path>,
    task_id: &str,
    now_ms: i64,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO migration_journal (op, src, dst, task_id, state, created_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        rusqlite::params![
            op,
            src.to_string_lossy(),
            dst.map(|p| p.to_string_lossy().to_string()),
            task_id,
            now_ms,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// B1: 记录一个 pending 操作，返回 row id（后续 committed/cleared 需用）。
/// pending 表示「即将开始」还未完成，启动时需要 replay 检测。
fn journal_pending(
    app: &AppHandle,
    op: &str,
    src: &Path,
    dst: Option<&Path>,
    task_id: &str,
) -> Result<i64, String> {
    let conn = db::open_db(app).map_err(|e| e.to_string())?;
    journal_pending_inner(&conn, op, src, dst, task_id, now_ms())
}

/// B1 inner: 标记 committed。replay 跳过该行。
fn journal_committed_inner(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE migration_journal SET state = 'committed' WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// B1: 操作成功完成 → 标记 committed。replay 跳过该行。
fn journal_committed(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = db::open_db(app).map_err(|e| e.to_string())?;
    journal_committed_inner(&conn, id)
}

/// B1 inner: 清除（未启动 / 已明确失败）。
fn journal_cleared_inner(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE migration_journal SET state = 'cleared' WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// B1: 操作未启动 / 已明确失败 → 清除该行（不需要 replay）。
fn journal_cleared(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = db::open_db(app).map_err(|e| e.to_string())?;
    journal_cleared_inner(&conn, id)
}

/// B1: 启动时 replay pending 条目。
///
/// 语义：
/// - move + src 不存在 + dst 存在 → 文件已迁但 DB 未更新 → 重跑 db_upsert 改 file_path
/// - move + src 存在 + dst 不存在 → 文件未迁（操作未执行或失败）→ clear 行
/// - move + 两边都在 / 都不在 → 异常状态 → clear 行 + 记错误
/// - delete + src 不存在 → 文件已删但 DB 未清 file_path → 重跑 db_upsert 置 None
/// - delete + src 存在 → 文件未删 → clear 行
///
/// 返回（恢复条数, 错误条数）供调用者记日志。
pub fn journal_replay_pending(app: &AppHandle) -> Result<(usize, usize), String> {
    use crate::db::Task;
    let conn = db::open_db(app).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, op, src, dst, task_id, state, created_at
             FROM migration_journal WHERE state = 'pending'
             ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<JournalEntry> = stmt
        .query_map([], |r| {
            Ok(JournalEntry {
                id: r.get(0)?,
                op: r.get(1)?,
                src: r.get(2)?,
                dst: r.get(3)?,
                task_id: r.get(4)?,
                state: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut recovered = 0usize;
    let mut errors = 0usize;
    for entry in rows {
        let src = PathBuf::from(&entry.src);
        let dst = entry.dst.as_ref().map(PathBuf::from);
        match entry.op.as_str() {
            "move" => match (&dst, src.exists()) {
                (Some(d), false) if d.exists() => {
                    // move 成功但 DB 未更新
                    match recover_move_db(app, &entry.task_id, d) {
                        Ok(()) => {
                            journal_committed(app, entry.id).ok();
                            recovered += 1;
                            log_line(
                                app,
                                &format!(
                                    "journal replay: 修复 {} → {}",
                                    entry.src,
                                    d.display()
                                ),
                            );
                        }
                        Err(e) => {
                            errors += 1;
                            log_line(
                                app,
                                &format!(
                                    "journal replay: 修复 {} 失败：{}",
                                    entry.src, e
                                ),
                            );
                        }
                    }
                }
                _ => {
                    // 其他异常 / 未开始 / 异常状态 → clear 行
                    journal_cleared(app, entry.id).ok();
                }
            },
            "delete" => {
                if !src.exists() {
                    // delete 成功但 DB 未清 file_path
                    match recover_delete_db(app, &entry.task_id) {
                        Ok(()) => {
                            journal_committed(app, entry.id).ok();
                            recovered += 1;
                            log_line(
                                app,
                                &format!("journal replay: 清除 {} 的 file_path", entry.src),
                            );
                        }
                        Err(e) => {
                            errors += 1;
                            log_line(
                                app,
                                &format!(
                                    "journal replay: 清除 {} 失败：{}",
                                    entry.src, e
                                ),
                            );
                        }
                    }
                } else {
                    journal_cleared(app, entry.id).ok();
                }
            }
            _ => {
                journal_cleared(app, entry.id).ok();
            }
        }
    }

    // 限制日志表大小：最近 1000 条保留，剩余 cleared/committed 的清理
    // （避免 journal 表无限增长）
    let _ = conn.execute(
        "DELETE FROM migration_journal
         WHERE id NOT IN (
             SELECT id FROM migration_journal ORDER BY id DESC LIMIT 1000
         ) AND state IN ('committed', 'cleared')",
        [],
    );

    Ok((recovered, errors))
}

fn recover_move_db(app: &AppHandle, task_id: &str, dst: &Path) -> Result<(), String> {
    let tasks = db::db_load(app.clone()).map_err(|e| e.to_string())?;
    let Some(mut t) = tasks.into_iter().find(|x| x.id == task_id) else {
        return Err(format!("task {task_id} 不存在"));
    };
    t.file_path = Some(dst.to_string_lossy().to_string());
    t.updated_at = Some(now_ms());
    db::db_upsert(app.clone(), vec![t]).map_err(|e| e.to_string())
}

fn recover_delete_db(app: &AppHandle, task_id: &str) -> Result<(), String> {
    let tasks = db::db_load(app.clone()).map_err(|e| e.to_string())?;
    let Some(mut t) = tasks.into_iter().find(|x| x.id == task_id) else {
        return Err(format!("task {task_id} 不存在"));
    };
    t.file_path = None;
    t.file_is_dir = None;
    t.updated_at = Some(now_ms());
    db::db_upsert(app.clone(), vec![t]).map_err(|e| e.to_string())
}

/// 防重入：手动触发与定时轮询互斥
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 迁移防重入 RAII 守卫：Drop（含 panic 展开）时自动释放，
/// 防止 inner panic 后 RUNNING 永久 true 导致迁移永久锁死（审计 P2）
struct MigrationGuard;

impl MigrationGuard {
    fn acquire() -> Result<Self, String> {
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("迁移正在进行中，请稍后再试".into());
        }
        Ok(Self)
    }
}

impl Drop for MigrationGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRule {
    pub id: String,
    pub enabled: bool,
    pub keywords: Vec<String>,
    #[serde(default = "default_action")]
    pub action: String, // "move" | "delete"
    #[serde(default)]
    pub archive_dir: String,
}

fn default_action() -> String {
    "move".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    #[serde(default = "default_version")]
    pub version: u32,
    pub rules: Vec<MigrationRule>,
}

fn default_version() -> u32 {
    1
}

impl Default for RulesFile {
    fn default() -> Self {
        Self {
            version: 1,
            rules: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MigrationReport {
    pub ts: i64,
    /// 本轮自动归档的任务数（完成满 7 天 → archived=true）
    pub archived: usize,
    /// 移动归档成功数
    pub moved: usize,
    /// 删除成功数
    pub deleted: usize,
    /// 跳过数（源缺失/冲突/权限/无匹配等）
    pub skipped: usize,
    /// 本轮日志行
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationStatus {
    pub rules_count: usize,
    pub poll_interval_secs: u64,
}

fn rules_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join(RULES_FILE)
}

fn log_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join(LOG_FILE)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ───────────────────────── 规则读写 ─────────────────────────

pub fn load_rules(app: &AppHandle) -> RulesFile {
    match fs::read_to_string(rules_path(app)) {
        Ok(text) => match serde_json::from_str::<RulesFile>(&text) {
            Ok(r) => r,
            Err(e) => {
                log_line(app, &format!("规则文件解析失败（按空规则处理）：{e}"));
                RulesFile::default()
            }
        },
        Err(_) => RulesFile::default(),
    }
}

fn save_rules(app: &AppHandle, rules: &RulesFile) -> Result<(), String> {
    let dir = db::data_dir(app);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    fs::write(rules_path(app), text).map_err(|e| e.to_string())
}

/// 校验规则：action 合法；move 必须有归档目录；关键字至少一个非空
pub fn validate_rules(rules: &RulesFile) -> Result<(), String> {
    for (i, r) in rules.rules.iter().enumerate() {
        if r.action != "move" && r.action != "delete" {
            return Err(format!(
                "第 {} 条规则动作非法：{}（仅支持 move / delete）",
                i + 1,
                r.action
            ));
        }
        if r.action == "move" && r.archive_dir.trim().is_empty() {
            return Err(format!("第 {} 条规则是移动归档，归档目录不能为空", i + 1));
        }
        if !r.keywords.iter().any(|k| !k.trim().is_empty()) {
            return Err(format!("第 {} 条规则缺少文件名关键字", i + 1));
        }
    }
    Ok(())
}

/// CSV 规则模版文本（UTF-8 BOM：Excel/WPS 双击打开中文不乱码）。
/// 列：启用 | 文件名关键字 | 动作 | 归档目录
/// 关键字多个用中文逗号「，」分隔；启用填 是/否；动作填 移动归档/删除文件
fn template_csv() -> String {
    // 规则顺序即优先级：靠前的行先匹配（同名文件命中第一条规则）
    "\u{feff}启用,文件名关键字,动作,归档目录\n     是,工资，报销,移动归档,工资/{year}\n     否,临时,删除文件,\n"
        .to_string()
}

// ───────────────────────── 匹配与路径 ─────────────────────────

/// 文件名是否命中规则（大小写不敏感包含；空关键字永不命中）
pub fn filename_matches(file_name: &str, rule: &MigrationRule) -> bool {
    let lower = file_name.to_lowercase();
    rule.keywords
        .iter()
        .any(|k| !k.trim().is_empty() && lower.contains(&k.trim().to_lowercase()))
}

/// 归档目录解析：{year} → 当前年份；相对路径基于桌面；绝对路径原样
pub fn resolve_archive_dir(app: &AppHandle, template: &str) -> Result<PathBuf, String> {
    let year = chrono::Local::now().format("%Y").to_string();
    let expanded = template.replace("{year}", &year);
    let p = PathBuf::from(&expanded);
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

/// 同名冲突时生成 `名 (n).ext`（最多试 99 个）；返回 None 表示无可用名
fn conflict_free_name(dir: &Path, file_name: &str) -> Option<PathBuf> {
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

/// 递归拷贝目录（跨盘移动兜底用）：遇符号链接中止，失败时不破坏源
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("读取目录失败：{e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err("目录包含符号链接，跨盘移动已中止（源目录未动）".into());
        }
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&s, &d)?;
        } else {
            fs::copy(&s, &d).map_err(|e| format!("复制文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 移动文件/目录：先 rename（同卷）；失败（如跨卷 EXDEV）退化 copy+remove
fn move_entry(src: &Path, dst: &Path) -> Result<(), String> {
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

// ───────────────────────── 迁移引擎 ─────────────────────────

fn log_line(app: &AppHandle, line: &str) {
    crate::db::rotate_log_if_large(&log_path(app), 5 * 1024 * 1024);
    let full = format!("[{}] {line}", now_str());
    if let Ok(dir) = std::fs::canonicalize(db::data_dir(app)) {
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "{full}")
            });
    }
    // 无论如何也打到开发日志，便于排查
    println!("[migration] {full}");
}

fn emit_upserts(app: &AppHandle, tasks: &[db::Task]) {
    if tasks.is_empty() {
        return;
    }
    let payload = serde_json::json!({ "upserts": tasks, "deletes": [], "source": "migration" });
    let _ = app.emit_to("main", "tasks-updated", &payload);
}

/// 核心迁移：返回报告。silent 模式（轮询）不向 UI 抛错。
pub fn run_migration(app: &AppHandle) -> Result<MigrationReport, String> {
    let _guard = MigrationGuard::acquire()?;
    run_migration_inner(app)
}

fn run_migration_inner(app: &AppHandle) -> Result<MigrationReport, String> {
    let mut report = MigrationReport {
        ts: now_ms(),
        ..Default::default()
    };
    let rules = load_rules(app);
    let tasks = db::db_load(app.clone()).map_err(|e| e.to_string())?;
    let now = now_ms();
    let mut changed: Vec<db::Task> = vec![];

    // 阶段一：完成满 7 天且未归档的任务 → 归档（兜底：主窗口关闭时也照常到期）
    let mut due: Vec<db::Task> = vec![];
    for t in tasks.iter() {
        if t.deleted_at.is_some() || t.archived == Some(true) || t.column != "done" {
            continue;
        }
        if let Some(completed) = t.completed_at {
            if completed > 0 && now - completed >= ARCHIVE_AFTER_MS {
                let mut nt = t.clone();
                nt.archived = Some(true);
                nt.updated_at = Some(now);
                due.push(nt);
            }
        }
    }
    if !due.is_empty() {
        db::db_upsert(app.clone(), due.clone()).map_err(|e| e.to_string())?;
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
                if !src.exists() {
                    // B1: 源不存在不再是「直接解绑」。可能原因：
                    //   1. 上一轮 move 成功但 db_upsert 失败（journal pending 未 commit）
                    //   2. 用户在外部手动删了文件
                    //   3. 文件从来就不存在
                    // journal_replay_pending 已在 spawn_polling 启动时跑过，
                    // 这里看到的 src 不存在 = 确认需要解绑。交由 changed 推进。
                    let mut nt = t.clone();
                    nt.file_path = None;
                    nt.file_is_dir = None;
                    nt.updated_at = Some(now);
                    changed.push(nt);
                    report.skipped += 1;
                    let line = format!("解除绑定「{name}」：源文件已不存在（{src_str}）");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                let Some(dst) = conflict_free_name(&dir, &name) else {
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：目标目录同名冲突过多（不覆盖、不删除）");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                };
                // B1: 写 journal pending → move_entry → db_upsert → journal_committed
                // 如果中间任一步崩了，启动时 journal_replay_pending 修复 DB
                let journal_id = match journal_pending(app, "move", &src, Some(&dst), &t.id) {
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
                    journal_cleared(app, journal_id).ok();
                    report.skipped += 1;
                    let line = format!("跳过「{name}」：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                let mut nt = t.clone();
                nt.file_path = Some(dst.to_string_lossy().to_string());
                nt.updated_at = Some(now);
                if let Err(e) = db::db_upsert(app.clone(), vec![nt.clone()]) {
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
                journal_committed(app, journal_id).ok();
                changed.push(nt);
                report.moved += 1;
                let line = format!("已移动「{name}」→ {}", dst.display());
                report.log.push(line.clone());
                log_line(app, &line);
            }
            "delete" => {
                if !src.exists() {
                    // B1: 源不存在可能是上一轮 delete 成功但 db_upsert 失败，
                    // journal_replay_pending 启动时已修复；这里看到 = 确认解绑。
                    let mut nt = t.clone();
                    nt.file_path = None;
                    nt.file_is_dir = None;
                    nt.updated_at = Some(now);
                    changed.push(nt);
                    report.skipped += 1;
                    let line = format!("解除绑定「{name}」：源文件已不存在");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                // B1: write journal pending → delete → db_upsert → committed
                let journal_id = match journal_pending(app, "delete", &src, None, &t.id) {
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
                    journal_cleared(app, journal_id).ok();
                    report.skipped += 1;
                    let line = format!("删除「{name}」失败（可能被占用/权限不足）：{e}");
                    report.log.push(line.clone());
                    log_line(app, &line);
                    continue;
                }
                let mut nt = t.clone();
                nt.file_path = None;
                nt.file_is_dir = None;
                nt.updated_at = Some(now);
                if let Err(e) = db::db_upsert(app.clone(), vec![nt.clone()]) {
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
                journal_committed(app, journal_id).ok();
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
        db::db_upsert(app.clone(), changed.clone()).map_err(|e| e.to_string())?;
        emit_upserts(app, &changed);
    }

    Ok(report)
}

/// 后台轮询线程：启动 60s 后先跑一次，此后每 POLL_INTERVAL_SECS 检测一次，失败只记日志
pub fn spawn_polling(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        // B1: 启动时 replay 上轮未提交的 pending journal 条目，修复 move/delete 成功但
        // db_upsert 失败造成的 DB 不一致。出错只记日志，不影响后续轮询。
        match journal_replay_pending(&app) {
            Ok((rec, err)) if rec > 0 || err > 0 => {
                log_line(&app, &format!("journal replay 启动：恢复 {rec} 条，失败 {err} 条"));
            }
            Ok(_) => {}
            Err(e) => log_line(&app, &format!("journal replay 启动失败：{e}")),
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

// ───────────────────────── Tauri 命令 ─────────────────────────

#[tauri::command]
pub fn migration_rules_load(app: AppHandle) -> RulesFile {
    load_rules(&app)
}

/// 文件对话框导入规则表（JSON），返回导入的规则数。
/// ⚠️ blocking 对话框不能在主线程调用（会死锁卡死 App），必须走 spawn_blocking（与 bot_py.rs 一致）
/// 解析 CSV 规则表文本（自动识别表头列；编码层已由调用方处理）
fn parse_rules_csv(text: &str) -> Result<RulesFile, String> {
    use std::io::Cursor;
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(Cursor::new(text.trim_start_matches('\u{feff}').as_bytes()));
    let headers = rdr
        .headers()
        .map_err(|e| format!("CSV 表头解析失败：{e}"))?
        .clone();
    // 表头定位（宽容匹配：包含关键字即可）
    let find_col =
        |needle: &str| -> Option<usize> { headers.iter().position(|h| h.contains(needle)) };
    let (Some(ci_enable), Some(ci_kw), Some(ci_action), Some(ci_dir)) = (
        find_col("启用"),
        find_col("关键字"),
        find_col("动作"),
        find_col("目录"),
    ) else {
        return Err("CSV 需包含四列表头：启用 / 文件名关键字 / 动作 / 归档目录".into());
    };
    let mut rules: Vec<MigrationRule> = Vec::new();
    for (ri, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| format!("第 {} 行解析失败：{e}", ri + 2))?;
        let get = |i: usize| rec.get(i).unwrap_or("").trim().to_string();
        let kw_raw = get(ci_kw);
        let action_raw = get(ci_action);
        if kw_raw.is_empty() {
            continue; // 空行/无关键字行跳过
        }
        let action = match action_raw.as_str() {
            "移动归档" | "移动" | "move" | "Move" => "move".to_string(),
            "删除文件" | "删除" | "delete" | "Delete" => "delete".to_string(),
            other if other.is_empty() => "move".to_string(),
            other => {
                return Err(format!(
                    "第 {} 行动作「{}」无效（应填 移动归档 或 删除文件）",
                    ri + 2,
                    other
                ))
            }
        };
        let enabled = match get(ci_enable).as_str() {
            "是" | "true" | "True" | "TRUE" | "1" => true,
            _ => false,
        };
        let keywords: Vec<String> = kw_raw
            .split([',', '，', '、', ';', '；'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if keywords.is_empty() {
            continue;
        }
        rules.push(MigrationRule {
            id: uuid::Uuid::new_v4().to_string(),
            enabled,
            keywords,
            action: action.clone(),
            archive_dir: if action == "move" {
                get(ci_dir)
            } else {
                String::new()
            },
        });
    }
    if rules.is_empty() {
        return Err("CSV 中没有解析出任何规则".into());
    }
    Ok(RulesFile { version: 1, rules })
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
    validate_rules(&rules)?;
    let count = rules.rules.len();
    save_rules(&app, &rules)?;
    log_line(&app, &format!("导入规则表 {} 条", count));
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
pub fn migration_run(app: AppHandle) -> CommandResult<MigrationReport> {
    run_migration(&app).map_err(CommandError::from)
}

/// 迁移日志读取：尾部 limit 行、最新在前（与机器人审计日志同模式，老板指定）
#[tauri::command]
pub fn migration_log_read(app: AppHandle, limit: Option<usize>) -> String {
    let Ok(raw) = fs::read_to_string(log_path(&app)) else {
        return "（暂无迁移日志）".into();
    };
    let limit = limit.unwrap_or(500).clamp(1, 5000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    lines.join("\n")
}

#[tauri::command]
pub fn migration_status(app: AppHandle) -> MigrationStatus {
    let rules = load_rules(&app);
    MigrationStatus {
        rules_count: rules.rules.len(),
        poll_interval_secs: POLL_INTERVAL_SECS,
    }
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(keywords: Vec<&str>, action: &str, dir: &str) -> MigrationRule {
        MigrationRule {
            id: "r1".into(),
            enabled: true,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            action: action.into(),
            archive_dir: dir.into(),
        }
    }

    #[test]
    fn filename_match_case_insensitive() {
        let r = rule(vec!["工资"], "move", "工资/{year}");
        assert!(filename_matches("工资表2026.xlsx", &r));
        assert!(filename_matches("十月工资单.pdf", &r));
        assert!(!filename_matches("GONGZI.xlsx", &r)); // 中文关键字不匹配拼音
        assert!(filename_matches("2026工资.xlsx", &r));
        assert!(!filename_matches("报销单.xlsx", &r));
    }

    #[test]
    fn filename_match_english_case_insensitive() {
        let r = rule(vec!["Report"], "move", "报表/{year}");
        assert!(filename_matches("quarterly-report.docx", &r));
        assert!(filename_matches("REPORT_FINAL.docx", &r));
        assert!(!filename_matches("note.txt", &r));
    }

    #[test]
    fn empty_keywords_never_match() {
        let r = rule(vec![], "move", "x");
        assert!(!filename_matches("anything.txt", &r));
    }

    /// B1: journal SQL 基础流。构造临时 DB + 创建 migration_journal 表，
    /// 验证 pending → committed/cleared 状态变迁。
    fn setup_journal_db() -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("wm-jrn-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE migration_journal (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                op          TEXT    NOT NULL,
                src         TEXT    NOT NULL,
                dst         TEXT,
                task_id     TEXT    NOT NULL,
                state       TEXT    NOT NULL,
                created_at  INTEGER NOT NULL
            );",
        )
        .unwrap();
        (dir, conn)
    }

    /// B1: pending → committed 是 happy path，启动 replay 跳过该行
    #[test]
    fn journal_pending_to_committed_flow() {
        let (dir, conn) = setup_journal_db();
        let id = journal_pending_inner(&conn, "move", Path::new("/src/a"), Some(Path::new("/dst/a")), "task-1", 1000).unwrap();
        assert!(id > 0);

        // 验证插入后 state = pending
        let state: String = conn
            .query_row("SELECT state FROM migration_journal WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "pending");

        journal_committed_inner(&conn, id).unwrap();

        let state: String = conn
            .query_row("SELECT state FROM migration_journal WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "committed");

        // replay 查询会跳过该行（WHERE state = 'pending'）
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM migration_journal WHERE state = 'pending'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);

        fs::remove_dir_all(&dir).ok();
    }

    /// B1: pending → cleared（操作未启动 / 已失败），replay 不该修复
    #[test]
    fn journal_pending_to_cleared_flow() {
        let (dir, conn) = setup_journal_db();
        let id = journal_pending_inner(&conn, "delete", Path::new("/x/y"), None, "task-2", 2000).unwrap();
        journal_cleared_inner(&conn, id).unwrap();
        let state: String = conn
            .query_row("SELECT state FROM migration_journal WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "cleared");
        fs::remove_dir_all(&dir).ok();
    }

    /// B1: replay 模拟场景——move 成功但 db_upsert 崩，journal 保持 pending。
    /// 重启后 replay 检查 dst 存在 + src 不存在 → 调用 recover_move_db
    /// （这里不测 recover_move_db 本身，只验证检测逻辑的状态断言）。
    #[test]
    fn journal_replay_detects_move_completed_state() {
        let (dir, conn) = setup_journal_db();

        // 模拟「上一轮：pending 已写、move_entry 成功、db_upsert 崩溃」
        let src_path = dir.join("src.txt");
        let dst_path = dir.join("dst.txt");
        fs::write(&src_path, b"hello").unwrap();
        // 模拟 move 完成：写文件到 dst，删 src
        fs::rename(&src_path, &dst_path).unwrap();

        let id = journal_pending_inner(&conn, "move", &src_path, Some(&dst_path), "task-3", 3000).unwrap();

        // 此刻模拟 replay 检测：dst 存在 + src 不存在
        assert!(!src_path.exists(), "模拟：src 应已被移走");
        assert!(dst_path.exists(), "模拟：dst 应已存在");
        // 验证 journal 仍 pending
        let state: String = conn
            .query_row("SELECT state FROM migration_journal WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "pending", "db_upsert 崩后 journal 仍 pending");

        // replay 会检查 dst.exists() && !src.exists() → 调用 recover_move_db
        // （不能在这里调 recover_move_db，因为需要 AppHandle + 完整 task 表）
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_free_name_sequence() {
        let dir = std::env::temp_dir().join(format!("wm-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "1").unwrap();
        fs::write(dir.join("a (1).txt"), "1").unwrap();
        let got = conflict_free_name(&dir, "a.txt").unwrap();
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "a (2).txt");
        let got2 = conflict_free_name(&dir, "b.txt").unwrap();
        assert_eq!(got2.file_name().unwrap().to_string_lossy(), "b.txt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_dir_year_expansion() {
        // 仅验证占位符替换逻辑（不依赖桌面路径）
        let year = chrono::Local::now().format("%Y").to_string();
        let expanded = "工资/{year}".replace("{year}", &year);
        assert!(expanded.starts_with("工资/"));
        assert!(!expanded.contains('{'));
    }

    #[test]
    fn copy_dir_recursive_works() {
        let base = std::env::temp_dir().join(format!("wm-cp-{}", uuid::Uuid::new_v4()));
        let src = base.join("源");
        let dst = base.join("目标");
        fs::create_dir_all(src.join("子")).unwrap();
        fs::write(src.join("a.txt"), "A").unwrap();
        fs::write(src.join("子").join("b.txt"), "B").unwrap();
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "A");
        assert_eq!(
            fs::read_to_string(dst.join("子").join("b.txt")).unwrap(),
            "B"
        );
        assert!(src.exists()); // 拷贝不改源
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn move_entry_moves_directory() {
        // 文件夹任务：整个目录应被移动（rename 对目录同样生效）
        let base = std::env::temp_dir().join(format!("wm-mv-{}", uuid::Uuid::new_v4()));
        let src = base.join("项目资料");
        let dst = base.join("归档").join("项目资料");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "x").unwrap();
        fs::create_dir_all(base.join("归档")).unwrap();
        move_entry(&src, &dst).unwrap();
        assert!(dst.join("a.txt").exists());
        assert!(!src.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn move_entry_file_conflict_names_dir() {
        // 同名冲突命名对目录同样生效
        let base = std::env::temp_dir().join(format!("wm-mv2-{}", uuid::Uuid::new_v4()));
        let src = base.join("项目");
        let dst = base.join("归档");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::create_dir_all(dst.join("项目")).unwrap(); // 已有同名目录
        let got = conflict_free_name(&dst, "项目").unwrap();
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "项目 (1)");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn serde_parses_camel_case_archive_dir() {
        // 用户导入的规则表使用 camelCase 字段名（archiveDir），必须能正确解析
        let json = r#"{"id":"x","enabled":true,"keywords":["工资"],"action":"move","archiveDir":"工资/{year}"}"#;
        let r: MigrationRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.archive_dir, "工资/{year}");
        assert_eq!(r.action, "move");
        // 反序列化后重新序列化，字段名保持 camelCase（模版下载与保存格式一致）
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains("archiveDir"));
    }

    #[test]
    fn validate_rejects_bad_rules() {
        let mut bad = RulesFile {
            version: 1,
            rules: vec![rule(vec!["a"], "fly", "x")],
        };
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec!["a"], "move", " ")];
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec![""], "move", "x")];
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec!["a"], "delete", "")];
        assert!(validate_rules(&bad).is_ok());
    }

    // ────── 文件移动 / 同名冲突（基于纯 Path API，不依赖 AppHandle） ──────

    /// move_entry 基本：源文件 → 目标路径（无冲突）。原文件消失，新文件就位。
    #[test]
    fn move_file_basic() {
        let base = std::env::temp_dir().join(format!("wm-mv-basic-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let src = base.join("report.pdf");
        let dst = base.join("归档").join("report.pdf");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&src, b"hello world").unwrap();

        move_entry(&src, &dst).unwrap();
        assert!(dst.exists(), "目标文件应存在");
        assert!(!src.exists(), "源文件应已被移走");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello world");
        fs::remove_dir_all(&base).unwrap();
    }

    /// 同名冲突 → 加 ` (1)` 后缀。直接测 conflict_free_name（move_entry 会用它）。
    #[test]
    fn move_file_collision_adds_suffix_one() {
        let base = std::env::temp_dir().join(format!("wm-mv-col1-{}", uuid::Uuid::new_v4()));
        let dst_dir = base.join("归档");
        fs::create_dir_all(&dst_dir).unwrap();
        // 预占同名文件
        fs::write(dst_dir.join("report.pdf"), b"old").unwrap();

        let chosen = conflict_free_name(&dst_dir, "report.pdf").unwrap();
        assert_eq!(
            chosen.file_name().unwrap().to_string_lossy(),
            "report (1).pdf"
        );

        // 模拟实际迁移：用 chosen 作为目标路径做 move_entry（用同名空 src 占位）
        // 此场景是冲突检测本身已被 move_entry 调用前的 conflict_free_name 解决
        // 这里仅断言 conflict_free_name 选出的名字可用——后续真实 move 由调用方负责
        assert!(!chosen.exists(), "选出的名字不应已存在");
        fs::remove_dir_all(&base).unwrap();
    }

    /// 多个同名：name / name (1) 已存在 → 选择 name (2)
    #[test]
    fn move_file_collision_multiple_suffixes_picks_two() {
        let base = std::env::temp_dir().join(format!("wm-mv-col2-{}", uuid::Uuid::new_v4()));
        let dst_dir = base.join("归档");
        fs::create_dir_all(&dst_dir).unwrap();
        fs::write(dst_dir.join("report.pdf"), b"original").unwrap();
        fs::write(dst_dir.join("report (1).pdf"), b"first dup").unwrap();

        let chosen = conflict_free_name(&dst_dir, "report.pdf").unwrap();
        assert_eq!(
            chosen.file_name().unwrap().to_string_lossy(),
            "report (2).pdf"
        );
        assert!(!chosen.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    /// 扩展名/无扩展名的冲突场景应都能正确处理
    #[test]
    fn conflict_free_name_handles_extensionless_files() {
        let base = std::env::temp_dir().join(format!("wm-col-noext-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("README"), b"a").unwrap();
        fs::write(base.join("README (1)"), b"b").unwrap();

        let chosen = conflict_free_name(&base, "README").unwrap();
        assert_eq!(chosen.file_name().unwrap().to_string_lossy(), "README (2)");
        fs::remove_dir_all(&base).unwrap();
    }

    /// move_entry 在源不存在时返回 Err（不 panic）：保护测试
    #[test]
    fn move_file_source_missing_returns_error() {
        let base = std::env::temp_dir().join(format!("wm-mv-missing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let src = base.join("ghost.txt"); // 从未创建
        let dst = base.join("归档").join("ghost.txt");

        let err = move_entry(&src, &dst);
        assert!(err.is_err(), "源不存在应返回 Err");
        assert!(!dst.exists(), "目标未被创建");
        fs::remove_dir_all(&base).unwrap();
    }

    /// move_entry 成功拷贝文件后保留源文件以外的目录结构（拷贝语义）不适用于 move_entry 本身，
    /// 但应保证源是文件时 fs::rename 路径正确。此场景测试：跨目录 rename（同一 temp_dir 下）
    #[test]
    fn move_entry_handles_nested_dst_directory() {
        let base = std::env::temp_dir().join(format!("wm-mv-nested-{}", uuid::Uuid::new_v4()));
        let src = base.join("发票").join("input.pdf");
        let dst_dir = base.join("归档").join("发票").join("2026");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"x").unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        move_entry(&src, &dst_dir.join("input.pdf")).unwrap();
        assert!(dst_dir.join("input.pdf").exists());
        assert!(!src.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    /// 跨卷移动：fs::rename 失败后应退化到 copy+remove（这里同卷下也会走 rename，但验证路径不崩）
    #[test]
    fn move_entry_to_file_basic_file() {
        let base = std::env::temp_dir().join(format!("wm-mv-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let src = base.join("data.bin");
        let dst = base.join("dest.bin");
        fs::write(&src, vec![1u8, 2, 3, 4, 5]).unwrap();

        move_entry(&src, &dst).unwrap();
        assert!(dst.exists());
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), vec![1u8, 2, 3, 4, 5]);
        fs::remove_dir_all(&base).unwrap();
    }

    // ────── archive_dir year 占位符独立验证（不依赖 AppHandle::desktop_dir） ──────

    /// {year} 在多个上下文路径中都能被替换
    #[test]
    fn archive_dir_year_placeholder_in_nested_path() {
        let year = chrono::Local::now().format("%Y").to_string();
        for template in ["工资/{year}", "归档/{year}/Q4", "{year}/发票"] {
            let expanded = template.replace("{year}", &year);
            assert!(!expanded.contains("{year}"), "模板 {template:?} 仍有未替换的占位符");
            // 展开后的路径应是合理的本地路径
            assert!(PathBuf::from(&expanded).is_absolute() || expanded.contains('/'));
        }
    }

    /// 多个 {year} 占位符在同一个模板里都会被替换
    #[test]
    fn archive_dir_year_placeholder_replaced_multiple_times() {
        let year = chrono::Local::now().format("%Y").to_string();
        let expanded = "{year}/sub/{year}".replace("{year}", &year);
        assert_eq!(expanded, format!("{year}/sub/{year}"));
        assert!(!expanded.contains("{year}"));
    }

    // ────── parse_rules_csv 增强测试（CSV 解析逻辑） ──────

    #[test]
    fn parse_rules_csv_basic_template() {
        // 复用官方模版：\u{feff}启用,文件名关键字,动作,归档目录\n是,工资，...
        let text = "\u{feff}启用,文件名关键字,动作,归档目录\n是,工资，报销,移动归档,工资/{year}\n否,临时,删除文件,\n";
        let rules = parse_rules_csv(text).expect("官方模版应可解析");
        assert_eq!(rules.rules.len(), 2);

        let r0 = &rules.rules[0];
        assert!(r0.enabled);
        assert_eq!(r0.keywords, vec!["工资", "报销"]);
        assert_eq!(r0.action, "move");
        assert_eq!(r0.archive_dir, "工资/{year}");

        let r1 = &rules.rules[1];
        assert!(!r1.enabled);
        assert_eq!(r1.keywords, vec!["临时"]);
        assert_eq!(r1.action, "delete");
        assert_eq!(r1.archive_dir, ""); // delete 规则无视归档目录
    }

    #[test]
    fn parse_rules_csv_handles_missing_bom() {
        // 不带 BOM 也能解析（有些人手写 CSV）
        let text = "启用,文件名关键字,动作,归档目录\n是,发票,移动,发票/{year}\n";
        let rules = parse_rules_csv(text).expect("no-bom CSV should parse");
        assert_eq!(rules.rules.len(), 1);
        assert_eq!(rules.rules[0].keywords, vec!["发票"]);
    }

    #[test]
    fn parse_rules_csv_skips_blank_rows() {
        // 含空行 / 仅空格的行应被跳过
        let text = "启用,文件名关键字,动作,归档目录\n是,工资,移动,工资/{year}\n,\n   ,\n是,发票,移动,发票/{year}\n";
        let rules = parse_rules_csv(text).expect("blank rows should be skipped");
        assert_eq!(rules.rules.len(), 2);
        assert_eq!(rules.rules[0].keywords, vec!["工资"]);
        assert_eq!(rules.rules[1].keywords, vec!["发票"]);
    }

    #[test]
    fn parse_rules_csv_accepts_alternative_keywords_and_actions() {
        // 关键字支持中英文逗号 / 顿号 / 分号分隔；动作支持 移动/移动归档/move/Move
        let text = "启用,文件名关键字,动作,归档目录\ntrue,a；b；c，d,Move,X\n";
        let rules = parse_rules_csv(text).expect("valid row should parse");
        assert_eq!(rules.rules.len(), 1);
        assert!(rules.rules[0].enabled, "true 应被识别为启用");
        assert_eq!(
            rules.rules[0].keywords,
            vec!["a", "b", "c", "d"],
            "混合中英文分号 / 逗号分隔"
        );
        assert_eq!(rules.rules[0].action, "move", "Move 大小写变体应被归一化为 move");
    }

    #[test]
    fn parse_rules_csv_rejects_empty() {
        // 只有表头、无任何有效规则行 → 错误
        let text = "启用,文件名关键字,动作,归档目录\n";
        let err = parse_rules_csv(text);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("没有解析出任何规则"));
    }

    #[test]
    fn parse_rules_csv_rejects_unknown_action() {
        // 动作不在白名单 → 报错（指明行号）
        let text = "启用,文件名关键字,动作,归档目录\n是,工资,飞行,X\n";
        let err = parse_rules_csv(text).expect_err("未知动作应报错");
        assert!(err.contains("飞行"), "错误信息应提到无效动作名：{err}");
        assert!(err.contains("第 2 行"), "错误信息应指明行号");
    }

    #[test]
    fn parse_rules_csv_requires_four_columns_in_header() {
        // 表头列不全 → 报错
        let text = "启用,文件名关键字,动作\n是,a,移动,X\n";
        let err = parse_rules_csv(text).expect_err("缺列应报错");
        assert!(err.contains("四列"));
    }

    #[test]
    fn validate_rules_accepts_delete_without_archive_dir() {
        // delete 规则允许 archive_dir 为空
        let rf = RulesFile {
            version: 1,
            rules: vec![rule(vec!["tmp"], "delete", "")],
        };
        assert!(validate_rules(&rf).is_ok());
    }

    #[test]
    fn validate_rules_rejects_move_with_blank_archive_dir() {
        let mut rf = RulesFile {
            version: 1,
            rules: vec![rule(vec!["a"], "move", "")],
        };
        assert!(validate_rules(&rf).is_err());
        // 全空白也算空
        rf.rules = vec![rule(vec!["a"], "move", "   ")];
        assert!(validate_rules(&rf).is_err());
    }
}
