//! 任务卡导出/导入的纯逻辑段（路径闸门 / 信封组装 / 导入事务 / bounded 读）。
//!
//! 从 `db/tasks.rs` 拆出。本模块与 `crate::db::tasks` **同层**，
//! 两者的 `super::` 同指 `crate::db`——所以搬移不改任何路径语义。

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::{CommandError, CommandResult};

use super::tasks::{load_all, upsert_tasks, Task};

/// 导出/导入路径统一闸门（五调用方共用：workspace/tasks/workflow 导出导入 +
/// 审计导出）。path 直达自 invoke 参数，不能沿用「save dialog 亲手选 = 明确授权」假设
/// （同 W8 附件边界校验口径）。检查链：
/// ① 非空 → ② 词法拒 `..` 组件 → ③ 扩展名 .json（大小写不敏感）→ ④ 文件名合法
/// → ⑤ 父目录必须存在且为目录（canonicalize 解析软链）→ ⑥ 目标已存在时拒符号链
/// （symlink_metadata fail-closed，同 bot_fs resolve_writable 口径）。
///
/// TOCTOU 残余窗口（ 要求显式留档）：本闸门是时点检查，与实际
/// 读写之间文件可被替换（软链 swap/父目录改挂）——窗口比 resolve_writable 宽
/// （导出低频 + 路径来自用户 save dialog，风险接受口径与 bot_fs.rs:514 一致）。
/// 写侧由 atomic_write 落盘（临时文件+rename，不写半截）；读侧（import）只读
/// 自己打开的句柄，最坏读到被换的内容——完整性由 JSON 解析失败兜底，不炸主流程。
/// 导出/导入目标的路径闸门公共前导：fs 调用进阻塞线程（不占 Tokio worker），
/// 校验失败转 CommandError。各 *_export / *_import 命令共用。
pub(crate) async fn check_export_path_gate(path: String) -> Result<(), CommandError> {
    crate::py::document::spawn_blocking_map(move || {
        check_export_path(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(CommandError::from)
}

pub fn check_export_path(path: &str) -> CommandResult<()> {
    use std::path::Path;
    let trimmed = path.trim();
    let reject = |reason: &str| CommandError::InvalidArgument {
        field: "path".into(),
        value: path.chars().take(120).collect(),
        reason: reason.into(),
    };
    if trimmed.is_empty() {
        return Err(reject("路径为空"));
    }
    let p = Path::new(trimmed);
    // 词法拒绝 .. 组件（canonicalize 前置闸——逃逸写法根本不进后续检查）
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(reject("路径不允许包含 .."));
    }
    let ok = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if !ok {
        return Err(reject("必须是 .json 文件"));
    }
    // 文件名合法（/tmp/dir/ 这类以目录收尾的写法 extension 为空已被上一步拒绝，
    // 这里兜底 "."/".." 形态的 file_name）
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return Err(reject("路径必须以文件名结尾"));
    };
    if name == "." || name == ".." {
        return Err(reject("路径必须以文件名结尾"));
    }
    // 父目录必须存在且为目录（canonicalize 解析软链到真实目标，悬空父目录拒绝）
    let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return Err(reject("缺少父目录"));
    };
    let parent_canon = std::fs::canonicalize(parent)
        .map_err(|e| reject(&format!("父目录不存在或无法解析：{e}")))?;
    if !parent_canon.is_dir() {
        return Err(reject("父目录不是目录"));
    }
    // 目标已存在时拒符号链（写穿透风险，fail-closed；导入侧同样过闸——
    // 符号链 .json 罕见，报错可解释）
    if let Ok(meta) = std::fs::symlink_metadata(trimmed) {
        if meta.file_type().is_symlink() {
            return Err(reject("目标路径是符号链接，已拒绝"));
        }
        if !meta.is_file() {
            return Err(reject("目标路径已存在且不是常规文件"));
        }
    }
    Ok(())
}

/// 导出信封 v2（任务图谱设计 §1.4）：本人资料卡 + 已知成员 + 全量任务（owner 已盖章）
/// 同时是导入信封的 profile 字段类型（往返同构）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportProfileCard {
    pub person_id: String,
    pub name: String,
    /// 预设头像 ID（profile::AVATAR_IDS）：两端内置同一套图，导入端按 ID
    /// 解析回同一张图落 people.avatar。旧信封无此字段 → None → 按无头像处理
    #[serde(default)]
    pub avatar_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksExportFileV2 {
    pub version: u8,
    pub exported_at: i64,
    pub profile: ExportProfileCard,
    pub people: Vec<super::PeopleCard>,
    pub tasks: Vec<Task>,
}

/// 导入载荷：untagged 按序尝试——对象 = v2 信封，数组 = v1 裸任务（旧版导出永久可导）
#[derive(Deserialize)]
#[serde(untagged)]
pub enum TasksImportPayload {
    V2(Box<TasksImportEnvelope>),
    V1(Vec<Task>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksImportEnvelope {
    #[serde(default)]
    pub version: Option<u8>,
    #[serde(default)]
    pub profile: Option<ExportProfileCard>,
    #[serde(default)]
    pub people: Vec<super::PeopleCard>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

/// 组装 v2 导出 JSON（纯逻辑，单测锚点）：NULL owner 盖章为本人 pid；
/// people = people 表已知成员（除本人）+ 任务里引用但未注册的占位成员
pub fn export_tasks_json(
    conn: &rusqlite::Connection,
    self_pid: &str,
    self_name: &str,
    self_avatar_id: Option<&str>,
) -> Result<(String, usize), String> {
    let mut tasks = load_all(conn)?;
    for t in tasks.iter_mut() {
        if t.owner_id
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            t.owner_id = Some(self_pid.to_string());
        }
    }
    let mut people: Vec<super::PeopleCard> = super::people::people_load(conn)?
        .into_iter()
        .filter(|p| p.id != self_pid)
        .map(|p| super::PeopleCard {
            id: p.id,
            name: p.name,
        })
        .collect();
    let mut seen: std::collections::HashSet<String> = people.iter().map(|p| p.id.clone()).collect();
    for t in &tasks {
        if let Some(pid) = t
            .owner_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if pid != self_pid && seen.insert(pid.to_string()) {
                people.push(super::PeopleCard {
                    id: pid.to_string(),
                    name: "未知成员".into(),
                });
            }
        }
    }
    let count = tasks.len();
    let file = TasksExportFileV2 {
        version: 2,
        exported_at: crate::db::now_ms(),
        profile: ExportProfileCard {
            person_id: self_pid.to_string(),
            name: self_name.to_string(),
            avatar_id: self_avatar_id
                .filter(|id| crate::profile::is_valid_avatar_id(id))
                .map(str::to_string),
        },
        people,
        tasks,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    Ok((json, count))
}

/// 归属归一（任务图谱设计 §1.3）：有效 pid = 任务自带 ownerId 非空者，
/// 否则信封本人 pid；等于自己 → 库内 NULL，否则 Some(pid)
pub(crate) fn normalize_owner(
    t: &Task,
    envelope_pid: Option<&str>,
    self_pid: &str,
) -> Option<String> {
    let eff = t
        .owner_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| envelope_pid.map(str::to_string))
        .unwrap_or_else(|| self_pid.to_string());
    if eff == self_pid {
        None
    } else {
        Some(eff)
    }
}

/// 导入核心（纯逻辑 + 事务，单测锚点）：people 与 tasks 同事务写入；
/// 归并规则不变（按 id，同 id 取 updatedAt 更晚者）。调用方须持 DB_WRITE_LOCK。
pub fn import_tasks_conn(
    conn: &mut rusqlite::Connection,
    raw: &str,
    self_pid: &str,
) -> Result<usize, String> {
    let payload: TasksImportPayload =
        serde_json::from_str(raw).map_err(|e| format!("不是有效的任务数据 JSON：{e}"))?;
    let (tasks, envelope, people_cards) = match payload {
        TasksImportPayload::V1(v) => (v, None, Vec::new()),
        TasksImportPayload::V2(env) => {
            if let Some(v) = env.version {
                if v != 2 {
                    return Err(format!("不支持的导出格式版本：{v}（本应用支持 1/2）"));
                }
            }
            let profile = env.profile.ok_or_else(|| {
                "信封缺少 profile 资料卡，无法确定任务归属（文件可能被手改损坏）".to_string()
            })?;
            let card = (profile.person_id, profile.name, profile.avatar_id);
            (env.tasks, Some(card), env.people)
        }
    };
    // 本人 personId 若为空（防御）——信封 pid 也不该等于空串
    if self_pid.trim().is_empty() {
        return Err("本人 personId 未初始化".into());
    }
    let envelope_pid = envelope.as_ref().map(|(pid, _, _)| pid.clone());
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // 成员注册表先行落库（任务图谱设计 §1.4）：信封本人卡（= 对方；若对方 pid 恰为
    // 本人——导自己的旧信封——is_self 只升不降语义保住本人行）+ 信封成员卡。
    // 头像 ID 白名单过滤后落 people.avatar（信封内容不可信任，垃圾 ID 不落库）
    if let Some((pid, name, avatar_id)) = &envelope {
        let avatar = avatar_id
            .as_deref()
            .filter(|id| crate::profile::is_valid_avatar_id(id));
        super::people::people_upsert_entry(&tx, pid, name, false, avatar)?;
    }
    super::people::people_upsert_cards(&tx, &people_cards)?;
    // 任务引用但信封未携带资料的归属 → 占位行，图谱不出现悬空归属
    for pid in tasks
        .iter()
        .filter_map(|t| normalize_owner(t, envelope_pid.as_deref(), self_pid))
    {
        super::people::people_ensure_placeholder(&tx, &pid)?;
    }
    let mut merged = 0usize;
    for t in &tasks {
        if t.id.trim().is_empty() {
            continue;
        }
        let mut t2 = t.clone();
        t2.owner_id = normalize_owner(t, envelope_pid.as_deref(), self_pid);
        let cur: Option<Option<i64>> = tx
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = ?1",
                rusqlite::params![t.id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let cur_ua = cur.flatten().unwrap_or(0);
        let take = t.updated_at.unwrap_or(0) > cur_ua;
        if take {
            upsert_tasks(&tx, std::slice::from_ref(&t2))?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

/// 导入文件读入大小上限（防内存 DoS：用户误选多 GB 文件时 read_to_string 直接吃满内存）
pub(crate) const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;

/// 导入文件的 bounded 读（tasks_import / workspace_import 共用）：上限在读侧
/// 强制（不做 metadata 预检——check-then-act 之间文件可被换大，symlink swap），
/// 只有限制实际读入字节数才兜底。先读字节再转 String：Take 截断可能切断
/// UTF-8 码点边界，直接 read_to_string 会把超限文件误报成编码错误。
pub(crate) fn read_import_bounded(path: &str, max_bytes: u64) -> Result<String, CommandError> {
    let f = std::fs::File::open(path).map_err(|e| format!("无法读取所选文件：{e}"))?;
    let mut limited = std::io::Read::take(f, max_bytes + 1);
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut limited, &mut buf)
        .map_err(|e| format!("无法读取所选文件：{e}"))?;
    if buf.len() as u64 > max_bytes {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path.to_string(),
            reason: format!("导入文件超过大小上限（{max_bytes} 字节）"),
        });
    }
    String::from_utf8(buf)
        .map_err(|e| format!("不是有效的 UTF-8 文本：{e}"))
        .map_err(CommandError::from)
}
