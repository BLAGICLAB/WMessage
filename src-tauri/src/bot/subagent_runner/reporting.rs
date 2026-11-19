//! 子 agent 收尾辅助族（自 subagent_runner.rs 切出）：任务包装渲染 / 终态上报
//! / 交付件探测与轮末提醒。`subagent_runner` 经 `pub(crate) use` 转出本模块
//! 的 pub 项，`crate::bot::subagent_runner::X` 旧路径不变。

use crate::bot_orchestrator::{acceptance_criteria_of, db_locked, finish_notify};
use crate::db::{SubagentRow, SubagentStatus};
use tauri::AppHandle;
use tauri::Emitter;

/// 任务包装（设计 §8.5）：orchestrator 从 subagents 行渲染，不接受 LLM 自由拼接。
/// 目标 / 验收标准 / 上下文摘要全部来自行上字段。
pub(crate) fn render_task_wrapper(row: &SubagentRow, artifact_dir: &std::path::Path) -> String {
    let criteria: Vec<String> = acceptance_criteria_of(row);
    let mut w = String::new();
    w.push_str("[子任务派发]\n");
    w.push_str(&format!("subagent_id: {}\n", row.id));
    w.push_str(&format!("objective: {}\n", row.objective));
    w.push_str("\nacceptance_criteria:\n");
    for c in &criteria {
        w.push_str(&format!("- {c}\n"));
    }
    if let Some(ctx) = row
        .context_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        w.push_str("\n上下文摘要:\n");
        w.push_str(ctx);
        w.push('\n');
    }
    w.push_str(&format!("\n产物目录:\n{}\n", artifact_dir.display()));
    // 交卷指令（二期乙）：收尾走 submit_result 工具——参数即收尾 JSON，协议级
    // 载体免去 markdown 围栏/散文/截断三害；文本 JSON 由补答链兜底（仍在）。
    // 字段契约单源在工具 schema 与系统提示工作流第 4 步，任务包装只指路不复述
    //（复述即第二份要同步的契约）。
    w.push_str(
        "\n完成后调用 submit_result 工具提交收尾（字段与规则见该工具的说明、\
         系统提示工作流第 4 步）：调用成功即任务结束，不要再输出收尾文本。\n",
    );
    w
}
/// 上报用终态：行已写入终态时以行为准（cancel 抢先 → cancelled），
/// 收尾闭包自身失败拿不到行时才回落 classify 的观感值
pub(crate) fn effective_status(
    row: Option<SubagentStatus>,
    classified: SubagentStatus,
) -> SubagentStatus {
    row.unwrap_or(classified)
}
/// 回读一行子 agent 的状态：读失败或行已不存在都返 None（调用方据此决定
/// 「按真实状态上报」还是「只留审计」）
pub(super) async fn load_subagent_status(app: &AppHandle, id: &str) -> Option<SubagentStatus> {
    let id = id.to_string();
    db_locked(app, move |conn| {
        Ok(crate::db::load_subagent(conn, &id)
            .ok()
            .flatten()
            .map(|r| r.status))
    })
    .await
    .unwrap_or(None)
}
/// `subagent-finished` 唯一收尾出口：主会话视图据此刷新子 agent 列表 + 打完成
/// 提示。所有把行写成终态的路径都必须走到这里（含排队期取消、启动中止、
/// 行加载失败的 ghost 路径）——前端只认事件，不发就永远停在 queued
pub(super) fn emit_subagent_finished(
    app: &AppHandle,
    subagent_id: &str,
    parent_session_id: Option<&str>,
    status: SubagentStatus,
    summary: &str,
) {
    let _ = app.emit_to(
        "main",
        "subagent-finished",
        serde_json::json!({
            "subagentId": subagent_id,
            "parentSessionId": parent_session_id,
            "status": status.as_str(),
            "summary": summary,
        }),
    );
    // 叫醒 check 等待方：本函数是所有转终态路径的唯一出口，通知挂在这里
    // 就不会漏（含排队期取消 / 启动中止 / ghost 路径）。
    finish_notify().notify_waiters();
}

/// 需自评判据数：acceptanceCriteria 里非机器判据（`file_exists:` /
/// `file_nonempty:` 前缀）的条数。机器判据由服务端收尾代验、提示词明说模型
/// 可不自评——交付件探测的覆盖基准因此不含它们，否则 checks 数永远差几条。
pub(super) fn self_judged_criteria_count(row: &SubagentRow) -> usize {
    acceptance_criteria_of(row)
        .iter()
        .filter(|c| crate::bot::subagent_digest::parse_machine_criterion(c.trim()).is_none())
        .count()
}

/// 机器验收的豁免口径：没有收尾正文（执行返错）、正文带任一熔断标记
/// （`FUSE_MARKER` 是轮数标记的前缀子串，查短标记即覆盖两者）、或停滞强停
/// ——没有「跑完」就没有验收可言。恢复链交出的收尾正文不带标记，照常判定。
pub(super) fn machine_verdicts_allowed(final_text: Option<&str>, stalled: bool) -> bool {
    let Some(text) = final_text else {
        return false;
    };
    !stalled && !text.contains(crate::bot_model_loop::FUSE_MARKER)
}

/// 盘上交付件报告（丙：熔断出口 verdict 推导的数据源；B：轮末提醒的数据源）。
pub(crate) struct DeliverableReport {
    pub path: std::path::PathBuf,
    pub value: serde_json::Value,
    pub checks: usize,
}

/// 盘上交付件探测：产物目录与 gen 根目录里名字含 verif 的 .json，
/// `checks` 数组条数 ≥ 基准判据数 → 验收已齐，返回**数据**（路径 + 解析后的
/// JSON）。全部探测点都不满足 → None。
///
/// 消费方两个：轮末收尾提醒（[verifier_deliverable_nudge]，B）与熔断出口的
/// verdict 推导（subagent_repair::recover_fused_exit，丙）——同一套「算不算
/// 齐了」的口径只写一份，提醒说「该收尾」、恢复说「结论我拿回来了」。
///
/// 只认「覆盖条数」不比对条目内容：单条判据被拆成多条 checks 只会让条件更早
/// 满足，而「满足」正是想要的结果；反向的漏判（写少了）由下一轮重探兜住，
/// 探测是每轮重跑的，不是一次定终身。半截写入解析失败同样按未就绪处理。
///
/// `not_before` 是 mtime 下限：gen 根目录是跨会话共享区，历史会话的同名旧
/// 报告（实测：上轮任务的 verified.json 在根目录滞留数日）不能算这次的交付
/// 件——本轮开跑之前落盘的一律不算。
pub(crate) fn find_fresh_deliverable_report(
    artifact_dir: &std::path::Path,
    gen_dir: Option<&std::path::Path>,
    required_checks: usize,
    not_before: std::time::SystemTime,
) -> Option<DeliverableReport> {
    if required_checks == 0 {
        return None;
    }
    let mut dirs = vec![artifact_dir];
    if let Some(g) = gen_dir {
        dirs.push(g);
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let lower = name.to_string_lossy().to_ascii_lowercase();
            if !lower.contains("verif") || !lower.ends_with(".json") {
                continue;
            }
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t >= not_before)
                .unwrap_or(false);
            if !fresh {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let checks_len = v.get("checks").and_then(|c| c.as_array()).map(|a| a.len());
            let Some(checks_len) = checks_len else {
                continue;
            };
            if checks_len >= required_checks {
                return Some(DeliverableReport {
                    path: entry.path(),
                    value: v,
                    checks: checks_len,
                });
            }
        }
    }
    None
}

/// 轮末收尾提醒文案（B 的注入口）：同一探测口径，产出面向模型的提醒。
pub(crate) fn verifier_deliverable_nudge(
    artifact_dir: &std::path::Path,
    gen_dir: Option<&std::path::Path>,
    required_checks: usize,
    not_before: std::time::SystemTime,
) -> Option<String> {
    let report = find_fresh_deliverable_report(artifact_dir, gen_dir, required_checks, not_before)?;
    Some(format!(
        "【系统提示】验收报告已齐：{} 的 checks 已覆盖全部需自评判据（{} 条）。\
         请立即输出收尾 JSON 结束任务，不要再调用任何核对、取证或加固类工具。",
        report.path.display(),
        report.checks
    ))
}

#[cfg(test)]
mod machine_gate_tests {
    use super::*;

    /// 熔断（轮数与工具两轴）与停滞行不给机器验收——注释承诺的「预算触顶/
    /// 异常退出的行没有验收可言」由这条守卫收口。恢复链交出的干净正文照常判。
    #[test]
    fn machine_verdicts_skip_fused_and_stalled_runs() {
        // 轮数熔断正文
        let rounds = format!(
            "已核对 40+ 个数字\n\n{}",
            crate::bot_model_loop::ROUNDS_FUSE_MARKER
        );
        assert!(
            !machine_verdicts_allowed(Some(&rounds), false),
            "轮数熔断不让验收"
        );
        // 工具熔断正文（原守卫只挡轮数标记，这条是本次收口的主目标）
        let tool = format!(
            "进度记录\n\n{}：本轮 Function 调用超过上限",
            crate::bot_model_loop::FUSE_MARKER
        );
        assert!(
            !machine_verdicts_allowed(Some(&tool), false),
            "工具熔断同样不让验收"
        );
        // 停滞强停：没有收尾 JSON，不给验收
        assert!(
            !machine_verdicts_allowed(Some("停止前的最后一段正文"), true),
            "停滞行不让验收"
        );
        // 执行返错：没有正文
        assert!(!machine_verdicts_allowed(None, false));
        // 恢复链交出的干净收尾 JSON：照常判定
        assert!(machine_verdicts_allowed(
            Some(r#"{"status":"succeeded"}"#),
            false
        ));
    }
}

#[cfg(test)]
mod deliverable_probe_tests {
    use super::*;

    /// 就绪判定只认本轮新写的 verif*.json：旧文件（跨会话残留）不算，
    /// checks 数不足不算，非 JSON / 无 checks 数组不算。
    #[test]
    fn deliverable_nudge_requires_fresh_full_checks() {
        // 截断先取、文件后写——与生产一致（下限在循环开跑前定，交付件在其后落盘）
        let now = std::time::SystemTime::now();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("verified.json");
        std::fs::write(&p, r#"{"checks":[{}, {}, {}]}"#).unwrap();
        // 3 条 checks ≥ 基准 3 → 就绪
        let hit = verifier_deliverable_nudge(dir.path(), None, 3, now);
        assert!(hit.is_some(), "满额 checks 应判就绪");
        assert!(
            hit.unwrap().contains(&p.display().to_string()),
            "文案带路径"
        );
        // 基准 4 → 未满
        assert!(verifier_deliverable_nudge(dir.path(), None, 4, now).is_none());
        // 名字不含 verif 的 json 不探
        let other = dir.path().join("evidence.json");
        std::fs::write(other, r#"{"checks":[{}, {}, {}]}"#).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(verifier_deliverable_nudge(dir.path(), None, 3, now).is_none());
        // 半截 JSON（写一半被抓到）不算就绪
        let half = dir.path().join("verified.json");
        std::fs::write(&half, r#"{"checks":[{}, {"#).unwrap();
        assert!(verifier_deliverable_nudge(dir.path(), None, 3, now).is_none());
    }

    /// gen 根目录的跨会话旧报告不冒充本次交付件：mtime 在本轮开跑之前一律跳过，
    /// 就算文件名与内容都齐也不算。
    #[test]
    fn deliverable_nudge_ignores_stale_gen_root_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("verified.json");
        std::fs::write(&stale, r#"{"checks":[{}, {}, {}]}"#).unwrap();
        let file_older_than_run =
            std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        assert!(
            verifier_deliverable_nudge(dir.path(), None, 3, file_older_than_run).is_some(),
            "下限早于文件 mtime 时照常命中（对照组）"
        );
        assert!(
            verifier_deliverable_nudge(dir.path(), None, 3, std::time::SystemTime::now()).is_none(),
            "下限晚于文件 mtime 时旧文件不算"
        );
        // 基准 0（全部是机器判据）整档让开：探测根本不该跑
        assert!(
            verifier_deliverable_nudge(dir.path(), None, 0, std::time::SystemTime::now()).is_none()
        );
    }

    /// 产物目录与 gen 根目录两处都探（判据允许报告落任一处），产物目录优先。
    #[test]
    fn deliverable_nudge_probes_both_dirs() {
        let artifact = tempfile::tempdir().unwrap();
        let gen = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now();
        // 只落 gen 根：产物目录没有 → 探得到
        let in_gen = gen.path().join("verified_final.json");
        std::fs::write(&in_gen, r#"{"checks":[{}]}"#).unwrap();
        let hit = verifier_deliverable_nudge(artifact.path(), Some(gen.path()), 1, now);
        assert!(hit.is_some(), "gen 根目录的交付件应被探到");
        assert!(hit.unwrap().contains("verified_final.json"));
        // 两处都有：产物目录的先命中
        let in_artifact = artifact.path().join("verified.json");
        std::fs::write(&in_artifact, r#"{"checks":[{}, {}]}"#).unwrap();
        let hit2 = verifier_deliverable_nudge(artifact.path(), Some(gen.path()), 1, now);
        let hit2 = hit2.unwrap();
        assert!(
            hit2.contains("verified.json") && !hit2.contains("verified_final"),
            "产物目录优先命中：{hit2}"
        );
    }
}
