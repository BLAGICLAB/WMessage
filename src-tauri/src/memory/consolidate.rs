//! 定时记忆整理（consolidation）：
//!
//! 候选 = 上次整理以来更新的条目 + access_count≥3 的活跃条目，按淘汰分降序取前 100
//! → 非流式 LLM 调用 → 解析 JSON 指令列表 → 一个事务内应用：
//! - merge{ids, content}：合并到 importance 最高（平手取最新）的现存条目，删其余来源
//! - contradiction{keep, drop, content}：keep 更新（content/向量重算），drop 删除
//! - distill{ids, content}：新建 kind=reflection / importance=4 / source=system 条目
//!   （引用 id 全不存在时跳过，防 LLM 幻觉 id 凭空造规律）
//!
//! 解析健壮性：剥代码围栏/截取首尾花括号；整体解析失败 → 本轮静默放弃记审计；
//! 单条缺字段/未知 action 跳过不影响其它。
//!
//! 调度：bot_scheduler 同模式（tauri async_runtime + tokio interval，每 10 分钟检查一次）。
//! 频率 off/12h/daily/weekly（默认 daily）。
//! 配置：`bot-config.json` 的 `memoryConsolidation` 字段 ({enabled, interval, lastRunAt})。
//! 命令：`memory_consolidate_now`（手动立即整理，返回 {merged, distilled, contradictions}）。
//!
//! 后台定期把近期/活跃记忆发给 LLM 做一轮反思——合并相关条目、裁决矛盾、提炼规律，
//! 而不是只等摘要攒够 10 条触发 Reflection。
//!
//! 纪律：
//! - LLM 输出解析失败 / LLM 不可用 → 本轮静默放弃 + 审计，绝不影响正常功能；
//! - 指令应用在一个事务里（半完成不留中间态）；
//! - 定时循环参照 bot_scheduler 模式（tauri async_runtime + tokio interval，10 分钟检查一次）。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::store::{self, MemItem, NewItem};
use super::{embed, now_ms, truncate_chars, MAX_CONTENT_CHARS};
use crate::error::{CommandError, CommandResult};
use crate::prompts::CONSOLIDATE_PROMPT;

/// 一批整理的候选上限
pub const CONSOLIDATE_BATCH_LIMIT: usize = 100;

/// 「活跃条目」门槛：access_count ≥ 3 的条目无论新旧都进候选
pub const CONSOLIDATE_ACTIVE_ACCESS: i64 = 3;

/// 默认回看窗口：上次整理时间缺失时取近 7 天
const CONSOLIDATE_DEFAULT_WINDOW_MS: i64 = 7 * 86_400_000;

/// 调度器检查周期（10 分钟扫一次是否到点，同 bot_scheduler 的 30s 扫描模式）
const SCHED_TICK_SECS: u64 = 600;

// ───────────────────────── 配置 ─────────────────────────

/// 记忆整理配置（存 bot-config.json 的 memoryConsolidation 字段）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ConsolidationConfig {
    /// 开关（与频率下拉「关闭」共同生效：enabled && interval != "off" 才会定时跑）
    pub enabled: bool,
    /// 频率：off / 12h / daily / weekly
    pub interval: String,
    /// 上次整理时间（epoch ms；None = 从未整理过）
    pub last_run_at: Option<i64>,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: "daily".into(),
            last_run_at: None,
        }
    }
}

/// 频率字符串 → 间隔时长；off/非法值 → None（不定时跑）
pub fn interval_duration(interval: &str) -> Option<std::time::Duration> {
    match interval.trim() {
        "12h" => Some(std::time::Duration::from_secs(12 * 3600)),
        "daily" => Some(std::time::Duration::from_secs(24 * 3600)),
        "weekly" => Some(std::time::Duration::from_secs(7 * 24 * 3600)),
        _ => None,
    }
}

/// 到点判定（纯函数，可测）：
/// - Off：开关关 / 频率 off / 非法频率；
/// - InitBaseline：从未整理过——先把基线记为现在（首个周期间隔后才真正跑，
///   防首次启动/刚开启时立刻白跑一次 LLM）；
/// - Run：距上次整理已达一个间隔。
#[derive(Debug, PartialEq, Eq)]
pub enum DueVerdict {
    Off,
    InitBaseline,
    Run,
}

pub fn classify_due(cfg: &ConsolidationConfig, now_ms: i64) -> DueVerdict {
    if !cfg.enabled {
        return DueVerdict::Off;
    }
    let Some(iv) = interval_duration(&cfg.interval) else {
        return DueVerdict::Off;
    };
    match cfg.last_run_at {
        None => DueVerdict::InitBaseline,
        Some(last) => {
            if now_ms.saturating_sub(last) >= iv.as_millis() as i64 {
                DueVerdict::Run
            } else {
                DueVerdict::Off
            }
        }
    }
}

// ───────────────────────── 整理指令 ─────────────────────────

/// 整理报告（前端 toast 与审计共用）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidateReport {
    /// merge 合并掉的来源条目数
    pub merged: usize,
    /// distill 提炼出的新规律条数
    pub distilled: usize,
    /// contradiction 裁决掉的矛盾条目数
    pub contradictions: usize,
}

/// LLM 输出的整理指令（解析后）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsolidateOp {
    /// 把 ids 合并成一条：内容更新到目标条目（importance 最高者），来源条目删除
    Merge { ids: Vec<String>, content: String },
    /// keep 与 drop_id 矛盾：按裁决保留 keep（内容更新为 content），删除 drop_id
    Contradiction {
        keep: String,
        drop_id: String,
        content: String,
    },
    /// 从 ids 提炼一条规律/反思：新建 kind=reflection 条目（importance=4）
    Distill { ids: Vec<String>, content: String },
}

impl ConsolidateOp {
    /// 指令的新内容文本（嵌入预计算遍历用）
    fn content(&self) -> &str {
        match self {
            ConsolidateOp::Merge { content, .. }
            | ConsolidateOp::Contradiction { content, .. }
            | ConsolidateOp::Distill { content, .. } => content,
        }
    }
}

/// 解析 LLM 输出为指令列表（纯函数，健壮优先）：/// 剥 ```json 代码围栏 / 截取首个 { 到末个 }；整体解析失败 → 空列表（本轮放弃）；
/// 单条指令缺字段 / 未知 action → 跳过该条，不影响其它指令。
pub fn parse_ops(text: &str) -> Vec<ConsolidateOp> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let start = t.find('{');
    let end = t.rfind('}');
    let Some((s, e)) = start.zip(end).filter(|(s, e)| s < e) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t[s..=e]) else {
        return Vec::new();
    };
    let Some(ops) = v["ops"].as_array() else {
        return Vec::new();
    };
    let str_list = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for op in ops {
        let action = op["action"].as_str().unwrap_or("").trim();
        let content = op["content"].as_str().unwrap_or("").trim().to_string();
        match action {
            "merge" => {
                let ids = str_list(&op["ids"]);
                if ids.len() >= 2 && !content.is_empty() {
                    out.push(ConsolidateOp::Merge { ids, content });
                }
            }
            "contradiction" => {
                let keep = op["keep"].as_str().unwrap_or("").trim().to_string();
                let drop_id = op["drop"].as_str().unwrap_or("").trim().to_string();
                if !keep.is_empty() && !drop_id.is_empty() && keep != drop_id && !content.is_empty()
                {
                    out.push(ConsolidateOp::Contradiction {
                        keep,
                        drop_id,
                        content,
                    });
                }
            }
            "distill" => {
                let ids = str_list(&op["ids"]);
                if !ids.is_empty() && !content.is_empty() {
                    out.push(ConsolidateOp::Distill { ids, content });
                }
            }
            _ => {} // 未知 action 跳过
        }
    }
    out
}

/// 候选收集（纯连接，可单测）：上次整理以来更新过的（无上次则近 7 天）
/// + access_count ≥ 3 的活跃条目；按淘汰分降序（重要/活跃/新近优先）取前 limit 条。
pub fn gather_candidates(
    conn: &rusqlite::Connection,
    since_ms: Option<i64>,
    now_ms: i64,
    limit: usize,
) -> Result<Vec<MemItem>, String> {
    let threshold = since_ms.unwrap_or(now_ms - CONSOLIDATE_DEFAULT_WINDOW_MS);
    let mut items: Vec<MemItem> = store::load_all(conn)?
        .into_iter()
        .filter(|m| m.updated_at_ms >= threshold || m.access_count >= CONSOLIDATE_ACTIVE_ACCESS)
        .collect();
    items.sort_by(|a, b| {
        store::evict_score(b, now_ms)
            .partial_cmp(&store::evict_score(a, now_ms))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(limit);
    Ok(items)
}

/// 候选条目 → prompt 用文本行
fn format_candidates(items: &[MemItem]) -> String {
    items
        .iter()
        .map(|m| format!("{} | {} | {} | {}", m.id, m.kind, m.importance, m.content))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指令应用（一个事务；向量由调用方在持锁/开事务前预计算好随 embs 传入——
/// ONNX 推理不占 DB 写锁临界区，与 mod.rs 门面纪律一致）：
/// - merge：目标 = ids 中 importance 最高（平手取最新更新）的现存条目，content/向量/时间
///   更新到目标，其余来源删除；
/// - contradiction：keep 内容/向量/时间更新，drop 删除；
/// - distill：新建 kind=reflection、importance=4、source=system 条目。
/// 指令引用的 id 不存在/不足以执行 → 跳过该条（计数不增）。
/// embs 与 ops 平行（embs[i] = ops[i].content 的嵌入；None = 嵌入失败按降级处理）。
pub fn apply_ops(
    conn: &mut rusqlite::Connection,
    ops: &[ConsolidateOp],
    embs: &[Option<Vec<f32>>],
    now_ms: i64,
) -> Result<ConsolidateReport, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut report = ConsolidateReport::default();
    for (i, op) in ops.iter().enumerate() {
        let emb = embs.get(i).and_then(|e| e.as_deref());
        match op {
            ConsolidateOp::Merge { ids, content } => {
                let items: Vec<MemItem> = store::load_all(&tx)?
                    .into_iter()
                    .filter(|m| ids.contains(&m.id))
                    .collect();
                if items.len() < 2 {
                    continue;
                }
                let target = items
                    .iter()
                    .max_by(|a, b| {
                        a.importance
                            .cmp(&b.importance)
                            .then(a.updated_at_ms.cmp(&b.updated_at_ms))
                    })
                    .expect("len>=2 已判定")
                    .clone();
                store::update_by_id(
                    &tx,
                    &target.id,
                    &truncate_chars(content, MAX_CONTENT_CHARS),
                    target.importance,
                    &target.source,
                    &target.kind,
                    emb,
                    now_ms,
                )?;
                let drop_ids: Vec<String> = items
                    .into_iter()
                    .filter(|m| m.id != target.id)
                    .map(|m| m.id)
                    .collect();
                report.merged += store::delete_by_ids(&tx, &drop_ids)?;
            }
            ConsolidateOp::Contradiction {
                keep,
                drop_id,
                content,
            } => {
                let all = store::load_all(&tx)?;
                let Some(keep_item) = all.iter().find(|m| &m.id == keep) else {
                    continue;
                };
                if !all.iter().any(|m| &m.id == drop_id) {
                    continue;
                }
                store::update_by_id(
                    &tx,
                    &keep_item.id,
                    &truncate_chars(content, MAX_CONTENT_CHARS),
                    keep_item.importance,
                    &keep_item.source,
                    &keep_item.kind,
                    emb,
                    now_ms,
                )?;
                report.contradictions += store::delete_by_ids(&tx, &[drop_id.clone()])?;
            }
            ConsolidateOp::Distill { ids, content } => {
                let all = store::load_all(&tx)?;
                if !ids.iter().any(|id| all.iter().any(|m| &m.id == id)) {
                    continue; // 引用的条目全不存在 → 跳过（防 LLM 幻觉 id 凭空造规律）
                }
                let item = NewItem {
                    kind: "reflection".to_string(),
                    content: truncate_chars(content, MAX_CONTENT_CHARS),
                    tags: vec!["reflection".to_string(), "consolidation".to_string()],
                    importance: 4,
                    source: "system".to_string(),
                };
                store::insert_item(&tx, &item, emb, now_ms)?;
                report.distilled += 1;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(report)
}

// ───────────────────────── 编排（LLM 调用 + 调度） ─────────────────────────

/// 跑一轮整理（手动命令与定时调度共用）：
/// 候选为空 → 零报告直接成功；LLM 不可用/输出解析失败 → Err（调用方决定记审计还是透传）。
/// 成功后由调用方更新 last_run_at（本函数不写配置，保持单一职责）。
pub async fn run_consolidation(app: &AppHandle) -> CommandResult<ConsolidateReport> {
    let cfg = crate::bot::load_config(app)
        .memory_consolidation
        .unwrap_or_default();
    let app2 = app.clone();
    let candidates =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<MemItem>, String> {
            let _g = crate::db::DB_WRITE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let conn = crate::db::open_db(&app2)?;
            store::ensure_table(&conn)?;
            gather_candidates(&conn, cfg.last_run_at, now_ms(), CONSOLIDATE_BATCH_LIMIT)
        })
        .await
        .map_err(|e| CommandError::from(format!("记忆整理取数线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    if candidates.len() < 2 {
        return Ok(ConsolidateReport::default()); // 少于 2 条无可整理
    }
    let msgs = vec![crate::bot_chat::ChatMsg {
        role: "user".into(),
        content: format_candidates(&candidates),
    }];
    // LLM 配置不可用（空 key 等）在这里自然报 ApiKeyMissing
    let text = crate::bot_chat::summarize_messages(app, CONSOLIDATE_PROMPT, &msgs).await?;
    let ops = parse_ops(&text);
    if ops.is_empty() {
        return Ok(ConsolidateReport::default());
    }
    // 嵌入在持锁前批量算好（独立阻塞闭包、不持 DB 写锁，与 mod.rs「嵌入计算一律在
    // 持锁前算好」纪律一致）；单项嵌入失败 = None，事务内按降级（无向量）处理。
    let embs = {
        let contents: Vec<String> = ops.iter().map(|op| op.content().to_string()).collect();
        tauri::async_runtime::spawn_blocking(move || {
            contents
                .iter()
                .map(|c| embed::embed_text(c))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| CommandError::from(format!("记忆整理嵌入线程 join 失败：{e}")))?
    };
    let app3 = app.clone();
    let report =
        tauri::async_runtime::spawn_blocking(move || -> Result<ConsolidateReport, String> {
            let _g = crate::db::DB_WRITE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut conn = crate::db::open_db(&app3)?;
            store::ensure_table(&conn)?;
            apply_ops(&mut conn, &ops, &embs, now_ms())
        })
        .await
        .map_err(|e| CommandError::from(format!("记忆整理写入线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    Ok(report)
}

/// 立即手动整理（设置页「立即整理」按钮）。成功后回写 last_run_at。
#[tauri::command]
pub async fn memory_consolidate_now(app: AppHandle) -> CommandResult<ConsolidateReport> {
    let report = run_consolidation(&app).await?;
    persist_last_run(&app);
    crate::audit_event!(&app, crate::audit::AuditLevel::Info, "memory.consolidate_now",
        "merged" => report.merged, "distilled" => report.distilled,
        "contradictions" => report.contradictions);
    Ok(report)
}

/// 回写 last_run_at = now（配置写失败只记日志，不影响整理结果）
fn persist_last_run(app: &AppHandle) {
    let now = now_ms();
    if let Err(e) = crate::bot::update_config_file(app, |cfg| {
        cfg.memory_consolidation
            .get_or_insert_with(ConsolidationConfig::default)
            .last_run_at = Some(now);
    }) {
        eprintln!("[memory] 整理时间回写失败：{}", e.message());
    }
}

/// 启动记忆整理调度器（App 启动时调用；bot_scheduler 同模式）：
/// 每 10 分钟检查一次配置，到点就跑一轮。LLM/解析失败静默记审计，不影响正常功能。
pub fn start_consolidation_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(SCHED_TICK_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            let cfg = crate::bot::load_config(&app)
                .memory_consolidation
                .unwrap_or_default();
            match classify_due(&cfg, now_ms()) {
                DueVerdict::Off => {}
                DueVerdict::InitBaseline => persist_last_run(&app),
                DueVerdict::Run => {
                    match run_consolidation(&app).await {
                        Ok(report) => {
                            crate::audit_event!(&app, crate::audit::AuditLevel::Info,
                                "memory.consolidate",
                                "merged" => report.merged, "distilled" => report.distilled,
                                "contradictions" => report.contradictions);
                        }
                        Err(e) => {
                            // LLM 不可用/解析失败：静默放弃本轮（审计可查），
                            // 仍推进 last_run_at 防下个 tick 立刻重试刷屏
                            crate::audit_event!(&app, crate::audit::AuditLevel::Warn,
                                "memory.consolidate_failed", "error" => e.message());
                        }
                    }
                    persist_last_run(&app);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests;
