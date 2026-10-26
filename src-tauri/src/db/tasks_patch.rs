//! 任务卡定向补丁与列状态迁移（锁内纯逻辑段）。
//!
//! 从 `db/tasks.rs` 拆出。本模块与 `crate::db::tasks` **同层**，
//! 两者的 `super::` 同指 `crate::db`——所以搬移不改任何路径语义。

use crate::error::{CommandError, CommandResult};

use super::tasks::{due_format_error, load_task, upsert_tasks, TaskStatus};

/// task_set_column 的锁内段（纯 DB 逻辑，单测锚点）：**同一把写锁内**读现值 →
/// 应用列语义 → 打新基线（= 锁内现读 updated_at）→ 写。基线在锁内现读，
/// 对任何并发写者（规则定时器/迁移/其他实例）都不可能冲突。
/// 语义与前端 toggleDone 1:1：
/// → done：completed_at=now、archived=false、bot_assigned 清（用户完成显示用户头像）
/// → todo/doing：completed_at/archived 清（bot_assigned 保留）
pub(crate) fn task_set_column_locked(
    conn: &mut rusqlite::Connection,
    id: &str,
    status: TaskStatus,
    now: i64,
) -> CommandResult<super::Task> {
    // 单卡定点读（PK 索引），不走 load_all 全表扫——每次拖拽/完成都在写锁内
    let mut task =
        load_task(conn, id)?.ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    // 状态机守卫（数据层兜底）：回收站/归档卡不接受列迁移——完成/排期/拖拽
    // 一律先恢复。软删/归档写入自身走 task_patch（带状态位），不经此口。
    if task.deleted_at.is_some() || task.archived == Some(true) {
        let state = if task.deleted_at.is_some() {
            "回收站"
        } else {
            "归档"
        };
        return Err(CommandError::InvalidArgument {
            field: "col".into(),
            value: status.as_str().to_string(),
            reason: format!("任务卡在{state}中（已封存），不能迁移列状态；请先恢复到看板"),
        });
    }
    task.expected_updated_at = task.updated_at;
    task.updated_at = Some(now);
    match status {
        TaskStatus::Done => {
            task.column = TaskStatus::Done;
            task.completed_at = Some(now);
            task.archived = Some(false);
            task.bot_assigned = None;
        }
        TaskStatus::Todo | TaskStatus::Doing => {
            task.column = status;
            task.completed_at = None;
            task.archived = None;
        }
    }
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    upsert_tasks(&tx, std::slice::from_ref(&task)).map_err(CommandError::from)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(task)
}

/// TP-2：task_patch 的字段白名单应用器（纯逻辑，单测锚点）。
/// **null = 清空**、缺键 = 不动；未知键/受保护键/空标题 → InvalidArgument 响亮失败。
/// 受保护字段：id（主键）、updatedAt/expectedUpdatedAt（服务端统一打戳，前端快照不许带）、
/// createdAt（创建时间不可篡改）。
pub(crate) fn apply_task_patch(
    task: &mut super::Task,
    patch: &serde_json::Value,
) -> CommandResult<()> {
    let obj = patch
        .as_object()
        .ok_or_else(|| CommandError::InvalidArgument {
            field: "patch".into(),
            value: "非对象".into(),
            reason: "patch 必须是 JSON 对象".into(),
        })?;
    fn set_from<T: serde::de::DeserializeOwned>(
        slot: &mut T,
        v: &serde_json::Value,
        field: &str,
    ) -> CommandResult<()> {
        *slot = serde_json::from_value(v.clone()).map_err(|e| CommandError::InvalidArgument {
            field: field.into(),
            value: v.to_string(),
            reason: format!("类型不匹配: {e}"),
        })?;
        Ok(())
    }
    /// 计划时间格式校验：chrono 按定长 "%Y-%m-%dT%H:%M" 解析（月份/日期/时分越界
    /// 一并拒绝）；None（null 清空）直接放行
    fn validate_plan_time(
        v: Option<&str>,
        field: &str,
        raw: &serde_json::Value,
    ) -> CommandResult<()> {
        let Some(s) = v else {
            return Ok(());
        };
        if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").is_err() {
            return Err(CommandError::InvalidArgument {
                field: field.into(),
                value: raw.to_string(),
                reason: "计划时间格式必须是 YYYY-MM-DDTHH:mm".into(),
            });
        }
        Ok(())
    }
    /// 截止时间格式校验：口径见 [`due_format_error`]（三写口共用）。
    fn validate_due(v: Option<&str>, field: &str, raw: &serde_json::Value) -> CommandResult<()> {
        if let Some(reason) = v.and_then(due_format_error) {
            return Err(CommandError::InvalidArgument {
                field: field.into(),
                value: raw.to_string(),
                reason: reason.into(),
            });
        }
        Ok(())
    }
    for (k, v) in obj {
        match k.as_str() {
            // TaskStatus 的自定义 Deserialize 只支持借用字符串，from_value 走不通——
            // 特判经 FromStr 解析（wire = todo/doing/done）
            "column" => {
                let s = v.as_str().ok_or_else(|| CommandError::InvalidArgument {
                    field: k.into(),
                    value: v.to_string(),
                    reason: "column 必须是字符串".into(),
                })?;
                task.column = <TaskStatus as std::str::FromStr>::from_str(s).map_err(|e| {
                    CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: e,
                    }
                })?;
            }
            "completedAt" => set_from(&mut task.completed_at, v, k)?,
            "archived" => set_from(&mut task.archived, v, k)?,
            "deletedAt" => set_from(&mut task.deleted_at, v, k)?,
            "collapsed" => set_from(&mut task.collapsed, v, k)?,
            "note" => set_from(&mut task.note, v, k)?,
            "tags" => {
                set_from(&mut task.tags, v, k)?;
                // 双上限与 bot 工具 / HTTP 同一常量：UI 写入超限标签会让
                // 同一行数据在另外两个写口被拒（同字段两套标准互相打架）
                if let Some(tags) = &task.tags {
                    if tags.len() > crate::bot::MAX_TAGS {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: v.to_string(),
                            reason: format!("标签最多 {} 个", crate::bot::MAX_TAGS),
                        });
                    }
                    if let Some(t) = tags
                        .iter()
                        .find(|t| t.chars().count() > crate::bot::MAX_TAG_LEN)
                    {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: t.clone(),
                            reason: format!("单个标签最长 {} 字", crate::bot::MAX_TAG_LEN),
                        });
                    }
                }
                // 空数组 = 清除（与 bot/HTTP 两写口的「空数组清除」约定一致）
                if task.tags.as_ref().is_some_and(|t| t.is_empty()) {
                    task.tags = None;
                }
            }
            "subtasks" => set_from(&mut task.subtasks, v, k)?,
            "due" => {
                set_from(&mut task.due, v, k)?;
                validate_due(task.due.as_deref(), k, v)?;
            }
            "files" => set_from(&mut task.files, v, k)?,
            "order" => set_from(&mut task.order, v, k)?,
            "botAssigned" => set_from(&mut task.bot_assigned, v, k)?,
            // 计划起止（周时间网格排期）：单字段格式校验，null = 清除排期；
            // end > start 的跨字段一致性由 UI 成对提交保证
            "planStart" => {
                set_from(&mut task.plan_start, v, k)?;
                validate_plan_time(task.plan_start.as_deref(), k, v)?;
            }
            "planEnd" => {
                set_from(&mut task.plan_end, v, k)?;
                validate_plan_time(task.plan_end.as_deref(), k, v)?;
            }
            // 子 agent 编排三字段走 task_patch 既有通道；
            // null = 清空。assignee/budget/result 由服务端编排写，前端仅投影展示。
            // budget 落库前必须过 clamped()——硬顶契约在写口强制，防 task_patch
            // 旁路 maxTurns（ 采纳）。assignee 拒绝空串（它是串链
            // subagents 表的 join 键，空串会产生孤儿指向——）。
            "assignee" => {
                set_from(&mut task.assignee, v, k)?;
                if task
                    .assignee
                    .as_deref()
                    .is_some_and(|s| s.trim().is_empty())
                {
                    return Err(CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: "assignee 不能为空串（子 agent 串链键）".into(),
                    });
                }
            }
            "budget" => {
                set_from(&mut task.budget, v, k)?;
                task.budget = task.budget.take().map(|b| b.clamped());
            }
            "result" => set_from(&mut task.result, v, k)?,
            "dependsOn" => {
                set_from(&mut task.depends_on, v, k)?;
                if let Some(deps) = &task.depends_on {
                    if deps.iter().any(|d| d.trim().is_empty()) {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: v.to_string(),
                            reason: "dependsOn 元素不能为空串".into(),
                        });
                    }
                    // 自环：自己依赖自己。前端加依赖时会拒（成环），机器人走
                    // task_patch 这条路此前能写进去——写进去的卡在图谱里连出一条
                    // 自环边，度数虚高
                    if deps.iter().any(|d| d == &task.id) {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: v.to_string(),
                            reason: "dependsOn 不能包含任务自己".into(),
                        });
                    }
                }
            }
            "model" => {
                set_from(&mut task.model, v, k)?;
                if task.model.as_deref().is_some_and(|s| s.trim().is_empty()) {
                    return Err(CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: "model 不能为空串（清空用 null）".into(),
                    });
                }
            }
            "title" => {
                let t: String = serde_json::from_value(v.clone()).map_err(|e| {
                    CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: format!("类型不匹配: {e}"),
                    }
                })?;
                if t.trim().is_empty() {
                    return Err(CommandError::InvalidArgument {
                        field: "title".into(),
                        value: v.to_string(),
                        reason: "标题不能为空".into(),
                    });
                }
                task.title = t;
            }
            "id" | "updatedAt" | "expectedUpdatedAt" | "createdAt" => {
                return Err(CommandError::InvalidArgument {
                    field: k.into(),
                    value: v.to_string(),
                    reason: "受保护字段：id/updated_at/created_at 由服务端管理，不可经 patch 修改"
                        .into(),
                })
            }
            other => {
                return Err(CommandError::InvalidArgument {
                    field: other.into(),
                    value: v.to_string(),
                    reason: "未知 patch 字段".into(),
                })
            }
        }
    }
    Ok(())
}

/// 内容编辑类键：打在回收站/归档卡上 = 绕 UI 闸改已封存的数据。
/// 状态迁移类键（archived/deletedAt/column）不在列——恢复/归档/软删操作
/// 本身必须能打在封存卡上，否则回收站永远出不来。
const CONTENT_EDIT_KEYS: &[&str] = &[
    "title",
    "note",
    "tags",
    "subtasks",
    "files",
    "due",
    "planStart",
    "planEnd",
    "collapsed",
    "completedAt",
];

/// task_patch 的锁内段（纯 DB 逻辑，单测锚点）：读现值 → 白名单应用 →
/// 基线 = 锁内现读 updated_at → 服务端打戳 → 写。
pub(crate) fn task_patch_locked(
    conn: &mut rusqlite::Connection,
    id: &str,
    patch: &serde_json::Value,
    now: i64,
) -> CommandResult<super::Task> {
    // 单卡定点读（PK 索引），不走 load_all 全表扫——每次软删/归档/编辑/折叠都在写锁内
    let mut task =
        load_task(conn, id)?.ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    // 状态机守卫（数据层兜底）：回收站/归档卡不接受内容编辑——UI 闸挡得住
    // 正常操作，task_patch 直调（bot 工具/HTTP/脚本）绕得过 UI，这里挡数据。
    // 恢复操作（patch 带状态位）不受限，否则封存卡永远出不来。
    let seals_card = task.deleted_at.is_some() || task.archived == Some(true);
    if seals_card {
        let content_edits: Vec<&str> = CONTENT_EDIT_KEYS
            .iter()
            .filter(|k| patch.as_object().is_some_and(|o| o.contains_key(**k)))
            .copied()
            .collect();
        if !content_edits.is_empty() {
            let state = if task.deleted_at.is_some() {
                "回收站"
            } else {
                "归档"
            };
            return Err(CommandError::InvalidArgument {
                field: "patch".into(),
                value: content_edits.join(","),
                reason: format!("任务卡在{state}中（已封存），不能编辑内容；请先恢复到看板再操作"),
            });
        }
    }
    apply_task_patch(&mut task, patch)?;
    task.expected_updated_at = task.updated_at;
    task.updated_at = Some(now);
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    upsert_tasks(&tx, std::slice::from_ref(&task)).map_err(CommandError::from)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(task)
}
