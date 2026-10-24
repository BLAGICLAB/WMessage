//! 子 agent 摘要构建 / 注入 / 已消化标记，以及验收判据的机器判定。
//!
//! 摘要注入父会话的时机与内容口径见 subagent_digest_block；
//! 机器判据（file_exists / file_nonempty）走独立判定通道，
//! 不与模型自报的达成数混算。

use crate::bot_orchestrator::{clip, db_locked, safe_rel_path, CHECK_WAIT_MS_CAP};
use crate::db::{SubagentRow, SubagentStatus};
use tauri::AppHandle;

/// 回报段的渲染上限：一次注入最多这几条 / 这么多字。
///
/// 上限不是洁癖：父会话的历史预算是共享的，一次注入十条长摘要就能把本轮
/// 对话挤掉。宁可少注入几条（下一轮还会补），也不要撑爆当轮上下文。
pub(crate) const DIGEST_MAX_ITEMS: usize = 8;
pub(crate) const DIGEST_MAX_SUMMARY_CHARS: usize = 600;
const DIGEST_MAX_ARTIFACTS: usize = 5;
/// 失败/超预算行的残留产出渲染上限（字）。
///
/// 比成功行的摘要短：残留产出的作用是让父 agent 判断「值不值得带残留重试」，
/// 不是让它读完——读太长会把当轮上下文挤爆。
const DIGEST_MAX_RESIDUAL_CHARS: usize = 300;
/// 机器验收未过的条目（只认服务端核验过的：`machine: true` 且 `met: false`）。
///
/// 不认模型自报的 `met: false`：那是子 agent 的自我评价，服务端没核验过，
/// 摆出来会让父 agent 分不清「服务端验了没过」与「模型说自己没过」。
fn machine_unmet_criteria(result: &serde_json::Value) -> Vec<String> {
    let arr = result
        .get("acceptance_check")
        .or_else(|| result.get("checks"))
        .and_then(|v| v.as_array());
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter(|it| {
            it.get("machine").and_then(|m| m.as_bool()) == Some(true)
                && it.get("met").and_then(|m| m.as_bool()) == Some(false)
        })
        .filter_map(|it| {
            it.get("criterion")
                .and_then(|c| c.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| clip(s, 60))
        })
        .collect()
}
/// 一段渲染好的子任务回报 + 本次注入后应记为「父上下文已吸收」的子 agent id。
pub(crate) struct SubagentDigest {
    pub text: String,
    pub absorb: Vec<String>,
}
/// 父侧装配（设计 §3.7「下一轮 assemble 检查」的落点）。
///
/// 口径三条，都是为了「齐了/没齐」在模型侧都看得见：
/// 1. **只注 `result_json`，绝不注子会话历史**——子 agent 的完整对话留在它自己
///    的会话行里，父上下文只吸收结构化收尾。这是 §3.7 要点 3 的硬要求。
/// 2. **终态即吸收**：终态行注入后记入 `absorb`，下一轮不再重复念同一份结果。
/// 3. **非终态不吸收**：还在跑的行下轮还要看它有没有完成，所以只报一句计数，
///    不逐条铺开。
///
/// 返回 None = 没有新东西可注入（该父会话的子任务都已汇报过，或一个都没有），
/// 调用方据此不加这段 system 消息。
pub(crate) fn build_subagent_digest(
    rows: &[SubagentRow],
    absorbed: &std::collections::HashSet<String>,
) -> Option<SubagentDigest> {
    let mut items = String::new();
    let mut absorb = Vec::new();
    let mut running = 0usize;
    let mut listed = 0usize;

    for row in rows {
        if absorbed.contains(&row.id) {
            continue;
        }
        let objective = clip(&row.objective, 40);
        if !row.status.is_terminal() {
            // 在途不逐条列：铺开只会挤占上下文，模型只需要知道「还有几个没回」
            running += 1;
            continue;
        }
        if listed >= DIGEST_MAX_ITEMS {
            continue;
        }
        listed += 1;
        absorb.push(row.id.clone());
        match row.status {
            SubagentStatus::Succeeded => {
                items.push_str(&format!("{listed}. {objective} → 成功\n"));
                if let Some(r) = row.result_json.as_deref().and_then(parse_result_json) {
                    if let Some(summary) = r
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        items.push_str(&format!(
                            "   摘要：{}\n",
                            clip(summary, DIGEST_MAX_SUMMARY_CHARS)
                        ));
                    }
                    if let Some(list) = r.get("artifacts").and_then(|v| v.as_array()) {
                        let shown: Vec<String> = list
                            .iter()
                            .take(DIGEST_MAX_ARTIFACTS)
                            .filter_map(|a| {
                                let path = a.get("path").and_then(|v| v.as_str())?;
                                let desc = a
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty());
                                Some(match desc {
                                    Some(d) => format!("{}（{}）", clip(path, 80), clip(d, 60)),
                                    None => clip(path, 80),
                                })
                            })
                            .collect();
                        if !shown.is_empty() {
                            items.push_str(&format!("   产物：{}\n", shown.join("、")));
                        }
                    }
                    // 机检未过项：「成功」与「机检没过」两件事同时成立时，父 agent
                    // 必须看见后者——否则它只会以为这活干成了，把没过的项一起收下。
                    let unmet = machine_unmet_criteria(&r);
                    if !unmet.is_empty() {
                        items.push_str(&format!(
                            "   ⚠️ 机检未过 {} 项：{}\n",
                            unmet.len(),
                            unmet.join("、")
                        ));
                    }
                }
            }
            other => {
                // 失败/取消/超预算也必须回报：父 agent 否则会一直以为子任务还在跑，
                // 按「等齐」的预期一直等下去。
                let reason = row
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("：{}", clip(s, 200)))
                    .unwrap_or_default();
                items.push_str(&format!(
                    "{listed}. {objective} → {}{reason}\n",
                    match other {
                        SubagentStatus::Failed => "失败",
                        SubagentStatus::Cancelled => "已取消",
                        SubagentStatus::BudgetExceeded => "超预算",
                        _ => unreachable!("非终态已在上分支返回"),
                    }
                ));
                // 残留产出：失败/超预算 ≠ 产出为零。跑了几十分钟查到的东西对父
                // agent 判断「该不该带残留重试」有决定性价值——不摆出来，父 agent
                // 只能从零重烧一遍。
                if let Some(r) = row.result_json.as_deref().and_then(parse_result_json) {
                    if let Some(summary) = r
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        items.push_str(&format!(
                            "   残留产出：{}\n",
                            clip(summary, DIGEST_MAX_RESIDUAL_CHARS)
                        ));
                    }
                }
            }
        }
    }

    if listed == 0 && running == 0 {
        return None;
    }
    let mut text = String::from("[子任务回报] 以下是你此前派出的子任务结果，可直接据此继续：\n");
    if listed > 0 {
        text.push_str(&items);
        text.push_str("（以上已汇报，不必再用 check_subagent 重复查。）\n");
    }
    if running > 0 {
        text.push_str(&format!(
            "另有 {running} 个子任务仍在排队或执行中，完成后会在下一轮自动汇总；\
             现在可以先基于以上结果推进，不必空等。\n"
        ));
    }
    Some(SubagentDigest { text, absorb })
}
/// 解析收尾 JSON：解析失败不当成「没有结果」，而是当没有——宁可少报一条，
/// 也不要把半截 JSON 念进提示词（`finalize` 侧已保证 result_json 结构完整，
/// 这里只是不假设它一定完整）。
pub(crate) fn parse_result_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw).ok()
}
/// 父会话 → 已经汇报过的子 agent id。
///
/// **进程内、不落库**：这是一次「父上下文是否已吸收某结果」的去重标记，不是
/// 业务事实——DB 里的终态才是事实。落库要多一列 + 迁移，换来的只是重启后
/// 不重复念一遍；而重启后重复念的后果是提示词里多几段旧结果（啰嗦，不是
/// 错误），重启一次也只发生一次。这个取舍是刻意的，重启后重复汇报的口子
/// 记在设计文档 §十。
pub(crate) static DIGESTED: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>,
> = std::sync::OnceLock::new();
fn digested(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>
{
    DIGESTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
/// 锁中毒按 into_inner 取数据（全仓 mutex 口径）：去重表坏了的后果只是
/// 「可能多汇报一次」，不该让整个对话发不出请求。
fn lock_digested() -> std::sync::MutexGuard<
    'static,
    std::collections::HashMap<String, std::collections::HashSet<String>>,
> {
    crate::db::lock::lock_or_recover(digested(), "bot_orchestrator::DIGESTED")
}
/// 本次会话要注入的子任务回报段（没有新回报时 None）。
///
/// 只读自己的父会话行，查库失败/无会话一律 None——父上下文缺一段汇报照样能
/// 发请求，用户也还能用 `check_subagent` 手动查。
///
/// **不在这里记已吸收**：本函数只把「要注入什么」和「该记哪些 id」交出去，记
/// 动作由调用方在模型循环成功之后做（`mark_digested`）。在这里就记，等于把
/// 「这段回报已经说过了」提前宣布——而模型循环失败时它压根没送进模型，汇报轮
/// 也会因此以为已经说过、从此不再重提，子任务结果在本次进程内永久消失。
pub(crate) async fn subagent_digest_block(
    app: &AppHandle,
    session_id: Option<&str>,
) -> Option<SubagentDigest> {
    let sid = session_id?;
    let app2 = app.clone();
    let sid_owned = sid.to_string();
    let rows = tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app2).ok()?;
        crate::db::load_subagents_by_parent(&conn, &sid_owned, DIGEST_SCAN_LIMIT as u32).ok()
    })
    .await
    .ok()
    .flatten()?;

    let built = {
        let mut guard = lock_digested();
        let set = guard.entry(sid.to_string()).or_default();
        build_subagent_digest(&rows, set)?
    };
    crate::bot::audit_log(
        app,
        &format!(
            "subagent_digest | session={sid} absorbed={}",
            built.absorb.len()
        ),
    );
    Some(built)
}
/// 标记「父会话已吸收某子 agent 的结果」。
///
/// 父 agent 直接用 `check_subagent` 问到终态时同样算吸收：结果已经在父上下文
/// 里，digest 再念一遍只是同一结论塞两遍；不记的话，自动汇报轮还会以为
/// 「还没人说过」而把同一件事再讲一次。
pub(crate) fn mark_digested(session_id: Option<&str>, subagent_id: &str) {
    let Some(sid) = session_id else { return };
    lock_digested()
        .entry(sid.to_string())
        .or_default()
        .insert(subagent_id.to_string());
}
pub(crate) fn is_digested(session_id: &str, subagent_id: &str) -> bool {
    lock_digested()
        .get(session_id)
        .is_some_and(|s| s.contains(subagent_id))
}
/// 每次会话扫多少个子 agent 行（最新在前的 LIMIT）。
///
/// 与 `DIGEST_MAX_ITEMS`（一次渲染几条）不同：前者是「看多新」，后者是
/// 「说多少」。前者要够大，免得长期挂着的排队行把新完成的结果挤出候选。
const DIGEST_SCAN_LIMIT: usize = 30;
/// 机器可判定的验收谓词（相对子 agent 产物目录解析）：
/// - `file_exists:<相对路径>`：该文件存在
/// - `file_nonempty:<相对路径>`：该文件存在且非空
///
/// 为什么要有这一层：verifier 收尾里的 `met` 是**自述**，服务端无从核实——
/// 模型写 `met:true` 就显示通过。而「产物文件到底在不在」这类标准恰好能机械
/// 判定。不带前缀的条目维持原样由 verifier 自评（很多标准本来就得看内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachineCheck {
    FileExists(String),
    FileNonEmpty(String),
}
/// 谓词前缀 → 判定种类
pub(crate) const MACHINE_PREFIXES: &[(&str, u8)] = &[("file_exists:", 0), ("file_nonempty:", 1)];
/// 解析一条验收标准里的机器可判定谓词；None = 自由文本条目（交给 verifier 自评）
pub(crate) fn parse_machine_criterion(c: &str) -> Option<MachineCheck> {
    let t = c.trim();
    for (prefix, kind) in MACHINE_PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rel = safe_rel_path(rest)?;
            return Some(match kind {
                0 => MachineCheck::FileExists(rel.to_string()),
                _ => MachineCheck::FileNonEmpty(rel.to_string()),
            });
        }
    }
    None
}
/// 单条机器判定结论
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineVerdict {
    /// 原样保留已声明的 criteria 文本（回填/比对用）
    pub criterion: String,
    pub met: bool,
    pub reason: String,
}
/// 逐条执行机器可判定谓词（纯 FS 读，不写）。
/// 路径不合法（写了谓词但路径逃逸）判 false 并说明——fail-closed：
/// 宁可记未通过，也不能把一条不可核验的标准当通过。
pub(crate) fn eval_machine_criteria(
    criteria: &[String],
    dir: &std::path::Path,
) -> Vec<MachineVerdict> {
    let mut out = Vec::new();
    for c in criteria {
        let t = c.trim();
        let Some(check) = parse_machine_criterion(t) else {
            continue;
        };
        let (rel, want_nonempty) = match &check {
            MachineCheck::FileExists(r) => (r.as_str(), false),
            MachineCheck::FileNonEmpty(r) => (r.as_str(), true),
        };
        let path = dir.join(rel);
        let (met, reason) = match std::fs::metadata(&path) {
            Err(e) => (false, format!("产物不存在：{}", e.kind())),
            Ok(md) => {
                if md.is_dir() {
                    (false, "路径是目录，不是产物文件".to_string())
                } else if want_nonempty && md.len() == 0 {
                    (false, "产物存在但为空文件".to_string())
                } else {
                    (
                        true,
                        format!("服务端核验：{} 存在（{} 字节）", rel, md.len()),
                    )
                }
            }
        };
        out.push(MachineVerdict {
            criterion: t.to_string(),
            met,
            reason,
        });
    }
    out
}
/// 把机器判定并入收尾 JSON：以服务端结论覆盖 verifier 的自评，
/// 找不到对应条目就补一条（不依赖模型主动上报这些条目）。
pub(crate) fn merge_machine_verdicts(result: &mut serde_json::Value, verdicts: &[MachineVerdict]) {
    if verdicts.is_empty() {
        return;
    }
    let key = if result.get("checks").is_some() {
        "checks"
    } else {
        "acceptance_check"
    };
    let arr = result
        .as_object_mut()
        .map(|o| o.entry(key).or_insert_with(|| serde_json::json!([])));
    let Some(arr) = arr else { return };
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let arr = arr.as_array_mut().expect("上一步已归一成数组");
    for v in verdicts {
        let mut found = false;
        for item in arr.iter_mut() {
            let same = item
                .get("criterion")
                .and_then(|c| c.as_str())
                .map(|s| s.trim())
                == Some(v.criterion.as_str());
            if same {
                item["met"] = serde_json::Value::Bool(v.met);
                item["machine"] = serde_json::Value::Bool(true);
                item["machine_reason"] = serde_json::Value::String(v.reason.clone());
                found = true;
                break;
            }
        }
        if !found {
            arr.push(serde_json::json!({
                "criterion": v.criterion,
                "met": v.met,
                "machine": true,
                "machine_reason": v.reason,
            }));
        }
    }
}
/// 进度统计的交叉核对：只认**已声明过**的验收标准，且每条最多计一次。
/// 少了这层，模型报一条没派发过的 `met:true` 就能把 done 灌满
/// （min(total) 只是封顶，不挡虚报）。
pub(crate) fn count_met_against_declared(
    declared: &[String],
    reported: &[serde_json::Value],
) -> usize {
    let mut hit: Vec<&str> = Vec::new();
    for item in reported {
        if item.get("met").and_then(|m| m.as_bool()) != Some(true) {
            continue;
        }
        let Some(c) = item
            .get("criterion")
            .and_then(|c| c.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if declared.iter().any(|d| d.trim() == c) && !hit.contains(&c) {
            hit.push(c);
        }
    }
    hit.len()
}
/// 机器判定并入收尾产物（纯函数，单测锚点）：改写收尾 JSON 的验收结论，
/// 并在最终正文末尾点名未通过项——正文是模型与用户唯一同时看得到的地方，
/// 只写进 result_json 等于把结论藏起来。
pub(crate) fn apply_machine_verdicts(
    result: Option<serde_json::Value>,
    final_text: String,
    verdicts: &[MachineVerdict],
) -> (Option<serde_json::Value>, String) {
    if verdicts.is_empty() {
        return (result, final_text);
    }
    let mut result = result;
    if let Some(v) = result.as_mut() {
        merge_machine_verdicts(v, verdicts);
    }
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| !v.met)
        .map(|v| format!("{}（{}）", v.criterion, v.reason))
        .collect();
    if failed.is_empty() {
        return (result, final_text);
    }
    let mut t = final_text;
    t.push_str(&format!(
        "\n\n⚠️ 服务端机器验收未通过 {} 项：{}",
        failed.len(),
        failed.join("；")
    ));
    (result, t)
}
/// 机器验收推翻「收尾 JSON 解析失败」造成的假失败（纯函数，单测锚点）。
///
/// 起因：终态在 `classify_outcome` 里只凭子 agent 最后一条消息的收尾 JSON 判，
/// 正文通篇散文就落进 `result_json_unparseable` → failed。可服务端手里明明有
/// 更硬的证据——产物落盘了、机器可判定标准逐条验过——却排在终态之后算，
/// 没有任何一步回头修正。于是「产物齐、验收全过、状态失败」能同时成立。
///
/// 只推翻 `result_json_unparseable` 一种：模型自报 failed（`model_reported_*`）
/// 说的可能是服务端验不了的自由文本条目，机器全过不等于它同意。
/// 也要求至少有一条机器可判定结论——一条都没有时服务端零证据，推翻就是放行。
/// 不带前缀的自由文本条目本就没有服务端结论，按读者视角处理，原始正文仍
/// 完整留在 `result_json.summary` 里。
///
/// 返回 (终态, error, 是否被推翻)：error 保留解析失败标记并补上推翻理由，
/// 让「判成功」这件事在行上与审计里都留得下痕迹，而不是被悄悄抹平。
pub(crate) fn reconcile_machine_override(
    status: SubagentStatus,
    error: Option<String>,
    verdicts: &[MachineVerdict],
) -> (SubagentStatus, Option<String>, bool) {
    let parse_failed =
        matches!(status, SubagentStatus::Failed) && error.as_deref() == Some(UNPARSEABLE_MARKER);
    if !parse_failed || verdicts.is_empty() || !verdicts.iter().all(|v| v.met) {
        return (status, error, false);
    }
    (
        SubagentStatus::Succeeded,
        Some(format!(
            "{UNPARSEABLE_MARKER}:机器验收 {} 项全通过，按产物判成功",
            verdicts.len()
        )),
        true,
    )
}
/// 收尾 JSON 解析失败的 error 标记：判定与推翻两侧共用同一个字面量，
/// 改名时不会只改一处导致推翻静默失效。
pub(crate) const UNPARSEABLE_MARKER: &str = "result_json_unparseable";
/// 汇报轮等空档的上限：与 `check_subagent` 的等待预算同档。主会话那一轮跑了
/// 十分钟还没结束，用户早该知道子任务结果了，再等更久没有意义。
const REPORT_WAIT_MS: u64 = CHECK_WAIT_MS_CAP;
/// 汇报轮轮询主会话是否空闲的间隔。
const REPORT_RETRY_MS: u64 = 2000;
/// 派发方会话是否还在（读失败按「不在」处理：宁可少报一次，也不写孤儿消息）
///
/// bot_messages 没有外键约束，会话删了之后往里插一条谁也读不到的消息就是
/// 纯死数据，且会话已删时汇报轮本就没有对象。
pub(crate) fn bot_session_exists_locked(conn: &rusqlite::Connection, session_id: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM bot_sessions WHERE id = ?1",
        [session_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}
async fn bot_session_exists(app: &AppHandle, session_id: &str) -> bool {
    let sid = session_id.to_string();
    db_locked(app, move |conn| Ok(bot_session_exists_locked(conn, &sid)))
        .await
        .unwrap_or(false)
}
/// 子 agent 收尾后要不要给父会话起汇报轮。
///
/// 用户自己取消的不汇报：用户刚点过停止，不需要模型再解释一遍
/// （设计文档「用户取消不汇报」）。判据必须用**收尾时的真实终态**——
/// 收尾前那行的 status 恒为 Queued（行是派发时建的），拿行快照判取消
/// 永远判不成立，用户取消后照样起汇报轮。
pub(crate) fn should_report_to_parent(status: SubagentStatus) -> bool {
    status != SubagentStatus::Cancelled
}
/// 子 agent 收尾后让主 agent 主动向用户转述（fire-and-forget）。
///
/// 为什么要这条：主 agent 原本只在「下一轮组装消息时注入 digest」这一个时机
/// 看到子任务结果。可父会话那一轮往往已经结束了——实测里主 agent 用 4 次 30s
/// 等待都没等到，把「仍在调研中」当成整轮答复收尾，子 agent 49 秒后才跑完，
/// 此后用户没再说话，这个结果就永远躺在库里没人提。
///
/// 门控是「主会话空闲」而不是「子 agent 一收尾」：主会话还在跑的时候，它自己
/// 会再 check 一次拿到终态并当轮转述（那条路径已记进去重表），这时再起一轮
/// 就是同一件事说两遍。
pub(crate) fn spawn_parent_report(app: &AppHandle, row: &SubagentRow, status: SubagentStatus) {
    let Some(parent) = row.parent_session_id.clone() else {
        return;
    };
    // 用户自己取消的：用户刚点过停止，不需要模型再解释一遍
    if !should_report_to_parent(status) {
        return;
    }
    let app = app.clone();
    let sub_id = row.id.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(REPORT_WAIT_MS);
        loop {
            if is_digested(&parent, &sub_id) {
                return;
            }
            // 会话可能在这段等待里被用户删了：bot_messages 没有外键约束，
            // 硬跑一轮只会给一个不存在的会话写一条没人读得到的孤儿消息
            if !bot_session_exists(&app, &parent).await {
                crate::bot::audit_log(
                    &app,
                    &format!("subagent_report_skipped | id: {sub_id} | 派发方会话已不存在"),
                );
                return;
            }
            match crate::bot_chat::report_subagent_result(&app, &parent).await {
                crate::bot_chat::ReportOutcome::Ran => return,
                // 失败也重试：失败的那轮没把子任务结果说出去，直接放弃等于这条
                // 结果永远没人汇报（digest 也不会记成已吸收，下一轮还有机会）
                crate::bot_chat::ReportOutcome::Failed | crate::bot_chat::ReportOutcome::Busy => {}
            }
            if std::time::Instant::now() >= deadline {
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_report_gave_up | id: {sub_id} | 父会话一直没空下来，或汇报一直失败"
                    ),
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(REPORT_RETRY_MS)).await;
        }
    });
}
