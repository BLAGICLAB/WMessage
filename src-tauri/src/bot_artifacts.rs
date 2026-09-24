//! 任务卡执行流程产物登记表（D1a 内存 HashMap）
//!
//! 链路：`tool_link_file_to_task` 登记 → `run_task_in_chat_with` 收尾
//! 按 `TaskExecOrigin` 分流（D4d）→ emit `artifact-batch-ready` Tauri event
//! → 前端 `ArtifactBatchDialog` 弹窗勾选 → 用户确认后调
//! `confirm_artifact_batch` 落 db_upsert。
//!
//! 进程重启后未弹窗的产物清空（"没帮上"可接受，不持久化）。
//! 设计权衡见 workspace 内部讨论 2026-09-11 D1a 拍板。

use crate::bot_chat::TaskExecOrigin;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Final,
    Intermediate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisteredArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub registered_at: i64,
}

// 产物登记表已收进 `AppState`（`app.manage` 注入），访问器走 `try_state`；
// 未注入的路径退回进程级兜底实例，语义与 3.1 前完全一致。
use crate::app_state::artifact_registry;

/// 登记一个产物。同 task_id + path 去重。
pub async fn register<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    path: String,
    kind: ArtifactKind,
) {
    let mut map = artifact_registry(app).lock().await;
    let entry = map.entry(task_id.to_string()).or_default();
    if !entry.iter().any(|a| a.path == path) {
        entry.push(RegisteredArtifact {
            path,
            kind,
            registered_at: chrono::Local::now().timestamp_millis(),
        });
    }
}

/// 弹窗前取走：清空并返回当前 task_id 的全部登记
pub async fn take_all<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
) -> Vec<RegisteredArtifact> {
    let mut map = artifact_registry(app).lock().await;
    map.remove(task_id).unwrap_or_default()
}

/// 只看不取（调试 / 状态查询用）
#[allow(dead_code)]
pub async fn peek<R: tauri::Runtime>(app: &AppHandle<R>, task_id: &str) -> Vec<RegisteredArtifact> {
    let map = artifact_registry(app).lock().await;
    map.get(task_id).cloned().unwrap_or_default()
}

/// D4d 收尾分流：判定是否触发汇总弹窗，返回要弹的最终产物列表。
///
/// - Manual（🤖 按钮）：必须任务卡 column=done 才弹（用户主动点完成）
/// - Scheduled（⏰ 定时）：不论 column 都弹（不能让 LLM 误切 status 杀定时）
/// - Batch（📦 批量）：不论 column 都弹（同 Scheduled）
///
/// intermediate 不参与弹窗（schema 已说明：本会话在 AI_Gen_Files 目录
/// 没新建过的路径不参与绑定——这是 LLM 自报 kind 时的兜底描述，
/// 实际路径合法性仍由 `tool_link_file_to_task` 在登记前 canonicalize 校验）。
pub async fn should_emit<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    origin: TaskExecOrigin,
    task_column: Option<&str>,
) -> Option<Vec<RegisteredArtifact>> {
    let all = peek(app, task_id).await;
    let finals: Vec<_> = all
        .into_iter()
        .filter(|a| a.kind == ArtifactKind::Final)
        .collect();
    if finals.is_empty() {
        return None;
    }
    match origin {
        TaskExecOrigin::Manual => {
            if task_column == Some("done") {
                Some(finals)
            } else {
                None
            }
        }
        TaskExecOrigin::Scheduled | TaskExecOrigin::Batch => Some(finals),
    }
}

/// Tauri command：用户在 ArtifactBatchDialog 勾选确认后调用，落 db_upsert。
/// 返回成功绑定的文件数。
#[tauri::command]
pub async fn confirm_artifact_batch(
    app: AppHandle,
    task_id: String,
    paths: Vec<String>,
) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let task = crate::db::db_load_for(&app)
        .await
        .map_err(|e| format!("读取任务卡失败：{e}"))?
        .into_iter()
        .find(|t| t.id == task_id)
        .ok_or_else(|| format!("任务卡不存在：{task_id}"))?;
    let mut next = task.clone();
    let files: Vec<_> = paths
        .iter()
        .map(|p| crate::db::TaskFile {
            path: p.clone(),
            is_dir: false,
        })
        .collect();
    crate::bot::apply_files_to_task(&mut next, files);
    next.expected_updated_at = next.updated_at;
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    crate::db::db_upsert(app.clone(), vec![next.clone()])
        .await
        .map_err(|e| format!("绑定失败：{e}"))?;
    crate::bot::broadcast_after_mutation(&app, vec![next.clone()], vec![]);
    // 弹窗已确认，清登记表。TOCTOU 兜底：take_all 返回里「弹窗快照后、确认前
    // 新登记」的产物不在用户勾选内——重新登记留下一轮弹窗，不静默丢弃。
    let removed = take_all(&app, &task_id).await;
    for a in removed {
        if !paths.iter().any(|p| p == &a.path) {
            register(&app, &task_id, a.path, a.kind).await;
        }
    }
    Ok(paths.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 样板：给 mock app 注入**独立** `AppState`（每测试一个实例）。
    /// 不注入也能跑（会走兜底实例），但注入后测试之间互不可见。
    fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(crate::app_state::AppState::default());
        app.handle().clone()
    }

    fn kind_str(k: ArtifactKind) -> &'static str {
        match k {
            ArtifactKind::Final => "final",
            ArtifactKind::Intermediate => "intermediate",
        }
    }

    #[tokio::test]
    async fn register_dedup_same_path() {
        let app = test_handle();
        register(&app, "t1", "/a/b.txt".into(), ArtifactKind::Final).await;
        register(&app, "t1", "/a/b.txt".into(), ArtifactKind::Final).await;
        let v = take_all(&app, "t1").await;
        assert_eq!(v.len(), 1);
        assert_eq!(kind_str(v[0].kind), "final");
    }

    #[tokio::test]
    async fn should_emit_filters_intermediate() {
        let app = test_handle();
        register(&app, "t2", "/a/x.txt".into(), ArtifactKind::Intermediate).await;
        register(&app, "t2", "/a/y.txt".into(), ArtifactKind::Final).await;
        let v = should_emit(&app, "t2", TaskExecOrigin::Scheduled, None)
            .await
            .unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "/a/y.txt");
        // 清理
        take_all(&app, "t2").await;
    }

    #[tokio::test]
    async fn should_emit_manual_requires_done() {
        let app = test_handle();
        register(&app, "t3", "/a/z.txt".into(), ArtifactKind::Final).await;
        // todo 状态 → 不弹
        assert!(
            should_emit(&app, "t3", TaskExecOrigin::Manual, Some("todo"))
                .await
                .is_none()
        );
        // doing 状态 → 不弹
        assert!(
            should_emit(&app, "t3", TaskExecOrigin::Manual, Some("doing"))
                .await
                .is_none()
        );
        // done 状态 → 弹
        let v = should_emit(&app, "t3", TaskExecOrigin::Manual, Some("done"))
            .await
            .unwrap();
        assert_eq!(v.len(), 1);
        take_all(&app, "t3").await;
    }

    #[tokio::test]
    async fn should_emit_scheduled_ignores_status() {
        let app = test_handle();
        register(&app, "t4", "/a/s.txt".into(), ArtifactKind::Final).await;
        // 不论 column 都弹
        assert!(
            should_emit(&app, "t4", TaskExecOrigin::Scheduled, Some("todo"))
                .await
                .is_some()
        );
        assert!(
            should_emit(&app, "t4", TaskExecOrigin::Scheduled, Some("doing"))
                .await
                .is_some()
        );
        assert!(
            should_emit(&app, "t4", TaskExecOrigin::Scheduled, Some("done"))
                .await
                .is_some()
        );
        take_all(&app, "t4").await;
    }

    #[tokio::test]
    async fn should_emit_empty_when_no_final() {
        let app = test_handle();
        register(&app, "t5", "/a/i.txt".into(), ArtifactKind::Intermediate).await;
        assert!(should_emit(&app, "t5", TaskExecOrigin::Scheduled, None)
            .await
            .is_none());
        take_all(&app, "t5").await;
    }

    /// 验收：`manage` 注入的实例彼此隔离；未注入的路径走兜底实例，
    /// 也不被注入实例污染——即「注入 = 隔离，不注入 = 老行为（进程级共享）」。
    #[tokio::test]
    async fn app_state_per_test_instances_are_isolated() {
        let a = test_handle();
        let b = test_handle();
        register(&a, "iso", "/a/one.txt".into(), ArtifactKind::Final).await;
        assert_eq!(
            peek(&a, "iso").await.len(),
            1,
            "注入实例 a 应看到自己的登记"
        );
        assert_eq!(
            peek(&b, "iso").await.len(),
            0,
            "注入实例 b 不得看到 a 的登记"
        );
        let bare = tauri::test::mock_app().handle().clone();
        assert_eq!(
            peek(&bare, "iso").await.len(),
            0,
            "兜底实例不得被注入实例污染"
        );
    }
}
