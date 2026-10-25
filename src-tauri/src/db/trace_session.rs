//! 本会话文件改动面（聊天侧栏「文件」工作面）：清单 / 详情 / 累计 diff。
//!
//! 从 `db/trace.rs` 拆出。本模块与 `crate::db::trace` **同层**，
//! 两者的 `super::` 同指 `crate::db`——所以搬移不改任何路径语义。

use serde::Serialize;

use crate::error::CommandResult;

use super::trace::{file_change_from_row, FileChangeRow};

// ── 本会话文件改动（聊天侧栏「文件」工作面）────────────────────────────
//
// 口径：**本会话累计**——某文件「本会话第一次改之前」 vs 「现在盘上的内容」。
// 逐次 diff 只在累计给不出时降级用，且必须带降级原因：拿单次 diff 冒充累计，
// 比干脆不给更糟——数字看着有、实则不是那个口径，用户无从察觉。
//
// 为什么不是「最近一次改动」：侧栏标题承诺的是「本会话文件改动」，逐次口径下
// 标题与数字对不上（前端按 path 去重还是「后者覆盖前者」，多次改动只剩最后一次
// 的数字）。累计口径两处同源：列表 ± 与详情 diff 出自同一次计算。
//
// 读盘与算 diff 都是同步 IO：命令是 async，但整体挪进 blocking 线程池。

/// 本会话改动文件条数上限：侧栏是给人看的摘要面，全量在 `file_changes` 表里。
pub const SESSION_FILE_CHANGES_MAX: usize = 50;

/// 累计 diff 的文件大小上限（字节）：超了给不出「会话开始 vs 现在」的对比，
/// 降级成逐次 diff（逐次 diff 存在库里，本来就有界）。
const SESSION_DIFF_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 二进制探测窗口：与 `bot_skills::files` 的预览同口径（前 8KB 见 NUL 即二进制）。
const SESSION_BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 累计口径能给 / 不能给
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffMode {
    /// 能给「本会话第一次改之前 vs 现在」的累计 diff
    Session,
    /// 只能给逐次 diff（起始快照缺失 / 文件已不在 / 过大）
    PerChange,
    /// 该文件没有行级 diff（二进制 / 无实际改动）
    NoLineDiff,
}

impl DiffMode {
    fn as_str(self) -> &'static str {
        match self {
            DiffMode::Session => "session",
            DiffMode::PerChange => "perChange",
            DiffMode::NoLineDiff => "noLineDiff",
        }
    }
}

/// 文件内容的可取形态（基线与现状共用一套）
enum FileState {
    /// 内容拿到了
    Text(String),
    /// 二进制：没有行级 diff
    Binary,
    /// 取不到 + 人话原因（前端直接展示，不能吞）
    Missing(String),
}

/// 单次改动的摘要（降级视图里逐条列）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileChangeEntry {
    pub id: i64,
    pub kind: String,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<String>,
    pub truncated: bool,
}

/// 清单项：某文件在本会话的改动概览
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileChangeItem {
    pub path: String,
    pub kind: String,
    /// 累计口径下的 + 行数（降级时是各次之和，note 里说明）
    pub added: i64,
    pub deleted: i64,
    /// 本会话被改了几次
    pub changes: usize,
    /// "session" | "perChange" | "noLineDiff"
    pub mode: String,
    /// 文件在 AI 最后一次写盘后又被改过：累计 diff 含非 AI 的改动
    pub drifted: bool,
    /// 降级/特殊情况的原因（人话）
    pub note: Option<String>,
}

/// 单个文件的改动详情
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileDiff {
    pub path: String,
    pub kind: String,
    pub mode: String,
    pub added: i64,
    pub deleted: i64,
    /// 本会话被改了几次
    pub changes: usize,
    pub truncated: bool,
    pub drifted: bool,
    /// 累计 diff 文本（mode=session 时有值）
    pub diff: Option<String>,
    /// 逐次 diff：只有 mode=perChange 才带（session 档捎带它是白费载荷——
    /// 每次改动的 diff 都可能上千行）
    pub per_change: Vec<SessionFileChangeEntry>,
    pub note: Option<String>,
}

/// 命令返回体：不给 path 时是清单，给 path 时是该文件的详情
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileChanges {
    pub items: Vec<SessionFileChangeItem>,
    pub detail: Option<SessionFileDiff>,
}

/// 某路径的汇总（内部计算结果）
struct PathSummary {
    path: String,
    kind: String,
    changes: usize,
    mode: DiffMode,
    added: i64,
    deleted: i64,
    truncated: bool,
    drifted: bool,
    diff: Option<String>,
    per_change: Vec<SessionFileChangeEntry>,
    note: Option<String>,
}

impl PathSummary {
    fn to_item(&self) -> SessionFileChangeItem {
        SessionFileChangeItem {
            path: self.path.clone(),
            kind: self.kind.clone(),
            added: self.added,
            deleted: self.deleted,
            changes: self.changes,
            mode: self.mode.as_str().to_string(),
            drifted: self.drifted,
            note: self.note.clone(),
        }
    }

    fn to_detail(&self) -> SessionFileDiff {
        SessionFileDiff {
            path: self.path.clone(),
            kind: self.kind.clone(),
            mode: self.mode.as_str().to_string(),
            added: self.added,
            deleted: self.deleted,
            changes: self.changes,
            truncated: self.truncated,
            drifted: self.drifted,
            diff: self.diff.clone(),
            // 逐次 diff 只在降级档给：session 档捎带它是白费载荷
            per_change: if self.mode == DiffMode::PerChange {
                self.per_change.clone()
            } else {
                Vec::new()
            },
            note: self.note.clone(),
        }
    }
}

/// 文本/二进制判定（与 `bot_skills::files` 的预览同口径：前 8KB 见 NUL 即二进制）
fn decode_text_file(bytes: Vec<u8>) -> FileState {
    if bytes
        .iter()
        .take(SESSION_BINARY_SNIFF_BYTES)
        .any(|&b| b == 0u8)
    {
        return FileState::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(s) => FileState::Text(s),
        Err(_) => FileState::Missing("文件不是合法 UTF-8 文本".into()),
    }
}

/// 读当前文件内容（同步 IO，调用方在 blocking 线程）
fn read_current_file(path: &str) -> FileState {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return FileState::Missing(format!("读不到当前文件：{e}")),
    };
    if meta.len() > SESSION_DIFF_MAX_BYTES {
        return FileState::Missing(format!("文件较大（{} 字节），给不出累计对比", meta.len()));
    }
    match std::fs::read(path) {
        Ok(b) => decode_text_file(b),
        Err(e) => FileState::Missing(format!("读不到当前文件：{e}")),
    }
}

/// 会话起始内容（本会话第一次改之前）：快照闸与回滚同一口径。
///
/// 快照名来自 DB 行：带目录分隔符/父目录引用的一律拒绝（防读越界到任意文件）。
/// 指纹不符 = 快照已损坏：不能拿它当累计基线（否则累计 diff 是错的，且错得看不出来）。
fn read_before_snapshot(rows: &[FileChangeRow], checkpoint_dir: &std::path::Path) -> FileState {
    let Some(first) = rows.first() else {
        return FileState::Missing("本会话没有该文件的改动记录".into());
    };
    if first.kind == "create" {
        // 会话开始前文件不存在 → 基线是空串（累计 diff = 整个文件都是新增）
        return FileState::Text(String::new());
    }
    let Some(before_ref) = first.before_ref.as_deref() else {
        return FileState::Missing("本会话第一次修改前的快照缺失（写盘时快照失败降级）".into());
    };
    let snap = std::path::Path::new(before_ref);
    if before_ref.is_empty() || snap.file_name() != Some(snap.as_os_str()) {
        return FileState::Missing("起始快照名非法（应为纯文件名）".into());
    }
    let bytes = match std::fs::read(checkpoint_dir.join(before_ref)) {
        Ok(b) => b,
        Err(e) => return FileState::Missing(format!("起始快照不可读：{e}")),
    };
    let state = decode_text_file(bytes);
    if let (FileState::Text(t), Some(before_sha)) = (&state, first.before_sha.as_deref()) {
        if before_sha != crate::bot::sha256_hex(t) {
            return FileState::Missing("起始快照指纹不符（已损坏）".into());
        }
    }
    state
}

/// 某路径的累计/降级判定（纯函数：IO 都在外面做完）。
///
/// 基线与现状都得是文本才给得出累计 diff；否则分两档降级并各自说明原因：
/// 当前是二进制 → 没有行级 diff；基线或现状取不到 → 给逐次 diff。
fn summarize_path(
    path: &str,
    rows: &[FileChangeRow],
    before: FileState,
    current: FileState,
) -> PathSummary {
    let changes = rows.len();
    let kind = rows
        .first()
        .map(|r| r.kind.clone())
        .unwrap_or_else(|| "modify".to_string());
    let per_change: Vec<SessionFileChangeEntry> = rows
        .iter()
        .map(|r| SessionFileChangeEntry {
            id: r.id,
            kind: r.kind.clone(),
            added: r.added,
            deleted: r.deleted,
            diff: r.diff.clone(),
            truncated: r.truncated,
        })
        .collect();
    let base = PathSummary {
        path: path.to_string(),
        kind,
        changes,
        mode: DiffMode::PerChange,
        added: 0,
        deleted: 0,
        truncated: false,
        drifted: false,
        diff: None,
        per_change,
        note: None,
    };

    if let (FileState::Text(b), FileState::Text(cur)) = (&before, &current) {
        let (diff, added, deleted, truncated) = crate::bot::build_unified_diff(b, cur, path);
        // 漂移判定：当前内容指纹 vs AI 最后一次写盘时的 after_sha——
        // 不符 = AI 写完之后文件又被改过（人改的），累计 diff 里混着非 AI 的改动。
        let drifted = match rows.last().and_then(|r| r.after_sha.as_deref()) {
            Some(a) => a != crate::bot::sha256_hex(cur),
            None => false,
        };
        return PathSummary {
            mode: DiffMode::Session,
            added,
            deleted,
            truncated,
            drifted,
            diff,
            note: if drifted {
                Some("文件在 AI 最后一次修改后又被改过，此处包含非 AI 的改动".into())
            } else {
                None
            },
            ..base
        };
    }

    if matches!(&current, FileState::Binary) {
        return PathSummary {
            mode: DiffMode::NoLineDiff,
            note: Some("二进制文件没有行级 diff；可切「查看全文」用系统默认程序打开".into()),
            ..base
        };
    }

    let reason = match (&before, &current) {
        (FileState::Missing(r), _) => r.clone(),
        (_, FileState::Missing(r)) => r.clone(),
        _ => "起始快照与当前内容取不到累计对比所需的信息".to_string(),
    };
    PathSummary {
        mode: DiffMode::PerChange,
        added: rows.iter().map(|r| r.added).sum(),
        deleted: rows.iter().map(|r| r.deleted).sum(),
        note: Some(format!(
            "{reason}；下面是本会话的逐次改动（行数是各次之和，不是净变化）"
        )),
        ..base
    }
}

/// 本会话某会话的全部文件改动行（按落盘先后）
pub fn file_changes_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<FileChangeRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT fc.id, fc.trace_id, fc.span_id, fc.path, fc.kind, fc.added, fc.deleted,
                    fc.diff, fc.truncated, fc.before_ref, fc.before_sha, fc.after_sha, fc.created_at
             FROM file_changes fc JOIN exec_traces t ON fc.trace_id = t.id
             WHERE t.session_id = ?1 ORDER BY fc.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], file_change_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 按路径分组（保持首次出现顺序；组内按落盘先后）
fn group_by_path(rows: Vec<FileChangeRow>) -> Vec<(String, Vec<FileChangeRow>)> {
    let mut groups: Vec<(String, Vec<FileChangeRow>)> = Vec::new();
    for r in rows {
        match groups.iter_mut().find(|(p, _)| *p == r.path) {
            Some((_, g)) => g.push(r),
            None => groups.push((r.path.clone(), vec![r])),
        }
    }
    groups
}

/// 同步内核（DB + 读盘 + 算 diff 都在 blocking 线程里）
pub(crate) fn session_file_changes_sync(
    app: &tauri::AppHandle,
    session_id: &str,
    path: Option<&str>,
) -> CommandResult<SessionFileChanges> {
    let conn = super::open_db(app)?;
    let rows = file_changes_for_session(&conn, session_id)?;
    let groups = group_by_path(rows);
    let checkpoint_dir = crate::db::paths::data_dir(app).join("checkpoints");

    if let Some(want) = path {
        // 详情用法：只算这一个文件（清单由另一次调用给，前端缓存着）
        let Some((_, group)) = groups.iter().find(|(p, _)| p == want) else {
            return Ok(SessionFileChanges {
                items: Vec::new(),
                detail: None,
            });
        };
        let summary = summarize_path(
            want,
            group,
            read_before_snapshot(group, &checkpoint_dir),
            read_current_file(want),
        );
        return Ok(SessionFileChanges {
            items: Vec::new(),
            detail: Some(summary.to_detail()),
        });
    }

    // 清单用法：按落盘先后排；改动太多时只留最近的若干条
    let capped: Vec<&(String, Vec<FileChangeRow>)> = if groups.len() > SESSION_FILE_CHANGES_MAX {
        groups[groups.len() - SESSION_FILE_CHANGES_MAX..]
            .iter()
            .collect()
    } else {
        groups.iter().collect()
    };
    let mut items = Vec::with_capacity(capped.len());
    for (p, group) in capped {
        let summary = summarize_path(
            p,
            group,
            read_before_snapshot(group, &checkpoint_dir),
            read_current_file(p),
        );
        items.push(summary.to_item());
    }
    Ok(SessionFileChanges {
        items,
        detail: None,
    })
}

#[cfg(test)]
mod session_file_changes_tests;
