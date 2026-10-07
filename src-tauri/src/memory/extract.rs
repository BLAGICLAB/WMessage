//! 自动记忆抽取（U16）：交互式会话收尾，异步抽取「值得跨会话记住」的用户
//! 画像/偏好/稳定事实，入库或进待确认队列——记忆生态从「被动等模型调工具」
//! 走向「主动从对话中学习」。
//!
//! 触发链：bot_chat 收尾（trace 挂点旁）→ [`maybe_extract_from_session`]
//! （同步快速门禁 + 限频 check-and-set）→ spawn 异步管线（LLM 抽取 → 解析 →
//! 入库/入队）。fire-and-forget：任何一步失败只记 WARN 审计，绝不影响主对话。
//!
//! 门禁（拍板口径）：
//! - 仅交互式聊天（任务执行/定时/Skill 会话语料是任务指令，不是用户画像）；
//! - `memoryControl.autoExtract`：off（默认）/ auto（直接入库）/ confirm（进
//!   待确认队列 mem_pending，用户过目后入库）；
//! - `memoryControl.autoWriteEnabled` = false 时抽取整体不跑（U15 总闸优先）。
//!
//! 限频：每会话 [`EXTRACT_MIN_INTERVAL_SECS`] 内最多抽取一次（进程内
//! check-and-set，gate 通过即占窗口——抽取失败也等下一窗口，防失败风暴）。
//!
//! 写入时冲突裁决（U19 两段式，仅 Auto 档）：抽取解析后逐条与既有记忆比对
//! 语义相似度，top-1 且 cos ≥ dedupHint 才算冲突候选；有候选才发第二次 LLM
//! 逐条裁决 new（新信息照插入）/ update（改口 → update_by_id 更新原条目，
//! 不堆积）/ skip（重复无增量丢弃）；坏输出整体回退全 new（照插入，语义去重
//! 兜底）。锁纪律：嵌入/LLM/解析全在锁外，DB 只在快照与应用两段短临界区。
//!
//! 入库复用：auto 档走 `store::insert_item`（语义去重/容量/淘汰全现成，
//! source=model_inferred → 注入块 [推断] 徽标天然可辨）；confirm 档进
//! mem_pending（上限 [`PENDING_CAP`]，满则丢最旧），approve 时才真正入库。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::panel::MemImportReport;
use super::store::{self, NewItem};
use super::{auto_extract_mode, auto_write_enabled, AutoExtract};
use crate::error::{CommandError, CommandResult};
use crate::prompts::{ADJUDICATE_PROMPT, EXTRACT_PROMPT};

/// 抽取限频：每会话两次抽取的最小间隔（30 分钟；节流是 LLM 成本第一道闸）
pub const EXTRACT_MIN_INTERVAL_SECS: i64 = 30 * 60;

/// 参与抽取的最近消息条数（6 轮对话）
const EXTRACT_MAX_MSGS: usize = 12;

/// 每条参与抽取的消息内容截断（防超长消息撑爆抽取请求）
const EXTRACT_MSG_CHARS: usize = 400;

/// 单条抽取结果的 content 上限（入库契约 ≤800；抽取从严 500，同 remember 工具）
const MAX_EXTRACT_CONTENT_CHARS: usize = 500;

/// 待确认队列上限：满则丢最旧（抽取静默放弃，不做无限堆积）
pub const PENDING_CAP: i64 = 50;

/// kind 白名单：抽取只产出这三类用户信息（summary/reflection/lesson 等是
/// 系统流水线的 kind，不由抽取产生）
const EXTRACT_KINDS: [&str; 3] = ["profile", "preference", "fact"];

// ───────────────────────── mem_pending 表 ─────────────────────────

/// 待确认条目（对外视图）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemPendingView {
    pub id: i64,
    pub content: String,
    pub kind: String,
    pub importance: i64,
    pub session_id: String,
    pub created_at: i64,
}

fn ensure_pending_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mem_pending(
           id         INTEGER PRIMARY KEY AUTOINCREMENT,
           content    TEXT NOT NULL,
           kind       TEXT NOT NULL,
           importance INTEGER NOT NULL DEFAULT 2,
           session_id TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )
    .map_err(|e| e.to_string())
}

/// 入队（容量满丢最旧）：now_ms 供 created_at
fn pending_insert(
    conn: &rusqlite::Connection,
    content: &str,
    kind: &str,
    importance: i64,
    session_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    ensure_pending_table(conn)?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_pending", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count >= PENDING_CAP {
        conn.execute(
            "DELETE FROM mem_pending WHERE id = (SELECT id FROM mem_pending ORDER BY id LIMIT 1)",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "INSERT INTO mem_pending (content, kind, importance, session_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            content,
            kind,
            importance.clamp(1, 5),
            session_id,
            super::store::ts_to_text(now_ms),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn pending_list(conn: &rusqlite::Connection) -> Result<Vec<MemPendingView>, String> {
    ensure_pending_table(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, content, kind, importance, session_id, created_at
             FROM mem_pending ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(MemPendingView {
                id: r.get(0)?,
                content: r.get(1)?,
                kind: r.get(2)?,
                importance: r.get(3)?,
                session_id: r.get(4)?,
                created_at: super::store::ts_to_ms(&r.get::<_, String>(5)?),
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 按 id 集合删除，返回删到的条数
fn pending_delete(conn: &rusqlite::Connection, ids: &[i64]) -> Result<usize, String> {
    ensure_pending_table(conn)?;
    let mut n = 0;
    for id in ids {
        n += conn
            .execute("DELETE FROM mem_pending WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// 取待确认行（approve 用）
struct PendingRow {
    content: String,
    kind: String,
    importance: i64,
}

fn pending_get(conn: &rusqlite::Connection, id: i64) -> Result<Option<PendingRow>, String> {
    ensure_pending_table(conn)?;
    conn.query_row(
        "SELECT content, kind, importance FROM mem_pending WHERE id = ?1",
        [id],
        |r| {
            Ok(PendingRow {
                content: r.get(0)?,
                kind: r.get(1)?,
                importance: r.get(2)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })
}

// ───────────────────────── 限频（进程内） ─────────────────────────

fn last_extract_map() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, i64>> {
    static LAST_EXTRACT_MS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, i64>>,
    > = std::sync::OnceLock::new();
    LAST_EXTRACT_MS
        .get_or_init(std::sync::Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 限频判定（纯函数，单测直打）：None = 从未抽过 → 到期
fn throttle_due(last: Option<i64>, now_ms: i64, min_interval_secs: i64) -> bool {
    match last {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= min_interval_secs * 1000,
    }
}

/// check-and-set：到期则写入 now 并返回 true（占住窗口——抽取失败也等下一窗口，
/// 防失败风暴刷 LLM）；未到期返回 false
fn throttle_check_set(session_id: &str, now_ms: i64) -> bool {
    let mut map = last_extract_map();
    let due = throttle_due(
        map.get(session_id).copied(),
        now_ms,
        EXTRACT_MIN_INTERVAL_SECS,
    );
    if due {
        map.insert(session_id.to_string(), now_ms);
    }
    due
}

// ───────────────────────── 抽取解析 ─────────────────────────

/// 单条抽取结果
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExtractedFact {
    pub content: String,
    pub kind: String,
    pub importance: i64,
}

/// 解析抽取输出（容错同 parse_ops 风格：剥围栏 → 截取首尾方括号 →
/// 逐条校验：content 非空（超长截断）、kind 白名单、importance 钳 1..=5）
pub(crate) fn parse_extract(text: &str) -> Vec<ExtractedFact> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let Some(start) = t.find('[') else {
        return Vec::new();
    };
    let Some(end) = t.rfind(']').filter(|e| *e > start) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&t[start..=end]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(content) = item["content"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(kind) = item["kind"].as_str().map(str::trim) else {
            continue;
        };
        if !EXTRACT_KINDS.contains(&kind) {
            continue;
        }
        let importance = item["importance"].as_i64().unwrap_or(2).clamp(1, 5);
        out.push(ExtractedFact {
            content: super::truncate_chars(content, MAX_EXTRACT_CONTENT_CHARS),
            kind: kind.to_string(),
            importance,
        });
    }
    out
}

// ───────────────────────── 触发与管线 ─────────────────────────

/// 会话收尾触发入口（fire-and-forget；同步快速门禁 + 限频，过了才 spawn 管线）。
/// 挂在 bot_chat 最终 return 处；非交互 / 未开档 / 总闸关 / 限频未到 → 直接返回。
pub fn maybe_extract_from_session(app: AppHandle, session_id: Option<String>, interactive: bool) {
    if !interactive {
        return;
    }
    let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) else {
        return;
    };
    let ctrl = crate::bot::read_memory_control(&app);
    if !auto_write_enabled(ctrl.as_ref()) {
        return; // U15 总闸优先：模型主动记忆关 = 抽取也不跑
    }
    let mode = auto_extract_mode(ctrl.as_ref());
    if mode == AutoExtract::Off {
        return;
    }
    if !throttle_check_set(&session_id, super::now_ms()) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let app_for_audit = app.clone();
        if let Err(e) = run_extract(app, session_id, mode).await {
            eprintln!("[memory] 自动抽取失败（静默降级）：{e}");
            crate::audit_event!(
                &app_for_audit,
                crate::audit::AuditLevel::Warn,
                "memory.extract_failed",
                "error" => e
            );
        }
    });
}

/// 抽取管线：读会话最近消息 → [`run_extract_with`]（LLM 调用方 = 生产配置薄壳）。
/// 全程失败只 Err（调用方 eprintln + WARN 审计），不重试。
async fn run_extract(app: AppHandle, session_id: String, mode: AutoExtract) -> Result<(), String> {
    let msgs = load_recent_messages(app.clone(), &session_id).await?;
    run_extract_with(&app, &session_id, msgs, mode, |prompt, msgs| {
        let app = app.clone();
        async move { crate::bot_chat::summarize_messages(&app, prompt, &msgs).await }
    })
    .await
}

// ───────────────────────── 写入时冲突裁决（U19） ─────────────────────────

/// 单条裁决结果（解析后的中间态；应用层据此分流）
#[derive(Clone, Debug, PartialEq, Eq)]
enum Adjudication {
    /// 新信息 → 走原入库路径（insert_item_with 语义去重兜底）
    New,
    /// 改口 → 更新原条目（update_by_id，不堆积）；值 = 原条目 id
    Update(String),
    /// 重复无增量 → 丢弃
    Skip,
}

/// 应用计数（审计与单测用）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AdjudicationCounts {
    /// New 落库（含语义去重合并——都算「进来了」）
    inserted: usize,
    /// Update 改写原条目
    updated: usize,
    /// Skip 丢弃（含容量满拒写/防劫持拒写——数据性拒收）
    skipped: usize,
    /// 存储故障条数（聚合计数，U16 口径）
    failed: usize,
}

/// 逐条找 top-1 相似既有记忆（纯函数，锁外跑）：只在抽取同域 kind
///（profile/preference/fact）里找，cos ≥ hint 阈值才算候选——summary/
/// reflection/lesson 不在抽取域，不该被「改口」。None = 无候选（跳过裁决，
/// 直接按新条目处理）。
fn plan_adjudication(
    items: &[store::MemItem],
    fact_embs: &[Option<Vec<f32>>],
    hint_cosine: f64,
) -> Vec<Option<store::MemItem>> {
    fact_embs
        .iter()
        .map(|emb| {
            let mut best: Option<(f64, &store::MemItem)> = None;
            if let Some(e) = emb {
                for m in items
                    .iter()
                    .filter(|m| matches!(m.kind.as_str(), "profile" | "preference" | "fact"))
                {
                    if let Some(c) = store::cosine(Some(e), m.embedding.as_deref()) {
                        if c >= hint_cosine && best.map_or(true, |(s, _)| c > s) {
                            best = Some((c, m));
                        }
                    }
                }
            }
            best.map(|(_, m)| m.clone())
        })
        .collect()
}

/// 裁决消息拼装（纯函数）：只发有候选的条目（无候选 = 没什么可裁决，
/// 返回空让调用方短路不发 LLM）；编号与 facts 下标一致，供解析对位。
fn build_adjudication_msgs(
    facts: &[ExtractedFact],
    candidates: &[Option<store::MemItem>],
) -> Vec<crate::bot_chat::ChatMsg> {
    let mut body = String::from("待裁决的新事实与各自对应的已有记忆：");
    let mut any = false;
    for (i, (f, c)) in facts.iter().zip(candidates).enumerate() {
        let Some(m) = c else { continue };
        any = true;
        body.push_str(&format!(
            "\n\n[{}]（kind={}）{}\n已有记忆 id={}（kind={}）：{}",
            i, f.kind, f.content, m.id, m.kind, m.content
        ));
    }
    if !any {
        return Vec::new();
    }
    vec![crate::bot_chat::ChatMsg {
        role: "user".to_string(),
        content: body,
    }]
}

/// 解析裁决输出（容错同 parse_extract 风格：剥围栏 → 截方括号 → 逐项对位）。
/// 坏输出 / 缺项 / 未知 action / 幻觉 existing_id 一律回退 New——裁决失败只
/// 降级为 v1 行为（照插入，insert_item_with 语义去重兜底），绝不丢数据。
fn parse_adjudication(text: &str, candidates: &[Option<store::MemItem>]) -> Vec<Adjudication> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let parsed = t.find('[').and_then(|start| {
        t.rfind(']')
            .filter(|end| *end > start)
            .and_then(|end| serde_json::from_str::<Vec<serde_json::Value>>(&t[start..=end]).ok())
    });
    let Some(items) = parsed else {
        return vec![Adjudication::New; candidates.len()]; // 坏输出整体回退全 new
    };
    candidates
        .iter()
        .enumerate()
        .map(|(i, cand)| {
            let Some(m) = cand else {
                return Adjudication::New; // 无候选条目不参与裁决（本就不发它）
            };
            let Some(entry) = items.iter().find(|e| e["index"].as_u64() == Some(i as u64)) else {
                return Adjudication::New; // 缺该项 → 回退 new
            };
            match entry["action"].as_str().map(str::trim) {
                Some("skip") => Adjudication::Skip,
                Some("update") => match entry["existing_id"].as_str() {
                    // id 必须精确等于该条候选（防幻觉 id 误改别条记忆）
                    Some(id) if id == m.id => Adjudication::Update(m.id.clone()),
                    _ => Adjudication::New,
                },
                _ => Adjudication::New, // new / 未知 / 缺 action → 回退 new
            }
        })
        .collect()
}

/// 应用裁决（&Connection 内核，锁内调用；内存库可单测）：
/// New → insert_item_with（语义去重/容量淘汰照走）；Update → 存在性复查后
/// update_by_id——只换 content+向量+updated_at，kind/importance/source/tags
/// 保持原口径（改口不改档：key 覆盖语义、pinned 判定、[推断] 徽标都不漂移），
/// 目标已消失（裁决在锁外、落库前被删）→ 回退 New；改写存储故障 → 计 failed
/// 原条目未动（不做插入兜底：新条目带空 tags，语义去重可能 merge 到原行把
/// key 覆盖掉）；Skip → 丢弃。
/// 单条存储故障计 failed 聚合（U16 口径），ensure_table 故障才 Err 上抛。
fn apply_adjudications(
    conn: &rusqlite::Connection,
    facts: &[ExtractedFact],
    fact_embs: &[Option<Vec<f32>>],
    adjs: &[Adjudication],
    now_ms: i64,
    sp: &store::StoreParams,
) -> Result<AdjudicationCounts, String> {
    let mut counts = AdjudicationCounts::default();
    // Update 目标存在性复查：裁决在锁外做，落库前按当前表定向点查（防间隙删除；
    // 不再 load_all 反序列化整表 ~1MB 向量，同 update_core/stats_core 注释口径）。
    // 表 existence 由调用方（快照段 ensure_table）保证；insert_item_with 内部
    // 还会再 ensure 一次（幂等）。
    for ((f, emb), adj) in facts.iter().zip(fact_embs).zip(adjs) {
        match adj {
            Adjudication::Skip => counts.skipped += 1,
            Adjudication::New => apply_insert(conn, f, emb.as_deref(), now_ms, sp, &mut counts),
            Adjudication::Update(id) => match store::find_by_id(conn, id)? {
                Some(orig) => {
                    match store::update_by_id(
                        conn,
                        id,
                        &f.content,
                        orig.importance,
                        &orig.source,
                        &orig.kind,
                        emb.as_deref(),
                        now_ms,
                    ) {
                        Ok(()) => counts.updated += 1,
                        Err(e) => {
                            // 改写存储故障：原条目未动，计 failed 留待下轮抽取重试
                            //（不插入兜底——空 tags 新条目若 merge 到原行会覆盖掉 key）
                            eprintln!("[memory] 裁决改写失败（跳过）：{e}");
                            counts.failed += 1;
                        }
                    }
                }
                None => apply_insert(conn, f, emb.as_deref(), now_ms, sp, &mut counts),
            },
        }
    }
    Ok(counts)
}

/// 插入单条抽取事实并计数（New/消失目标兜底共用）
fn apply_insert(
    conn: &rusqlite::Connection,
    f: &ExtractedFact,
    emb: Option<&[f32]>,
    now_ms: i64,
    sp: &store::StoreParams,
    counts: &mut AdjudicationCounts,
) {
    let item = store::NewItem {
        kind: f.kind.clone(),
        content: f.content.clone(),
        tags: Vec::new(),
        importance: f.importance,
        source: "model_inferred".to_string(),
    };
    match store::insert_item_with(conn, &item, emb, now_ms, sp) {
        Ok((store::InsertOutcome::Inserted(_), _))
        | Ok((store::InsertOutcome::Merged { .. }, _)) => counts.inserted += 1,
        Ok((store::InsertOutcome::RejectedFull(_), _))
        | Ok((store::InsertOutcome::RefusedForeignMerge { .. }, _)) => counts.skipped += 1,
        Err(e) => {
            eprintln!("[memory] 抽取条目入库失败（跳过）：{e}");
            counts.failed += 1;
        }
    }
}

/// 抽取管线主体（消息已就位）：LLM 抽取 → 解析 →〔Auto 档〕预嵌入 → 既有记忆
/// 快照 → 相似候选 →（有候选才）LLM 逐条裁决 new/update/skip → 应用；
/// 〔Confirm 档〕进待确认队列（不经裁决，语义与 U16 一致）。
/// LLM 调用方注入（pub 供集成测试直连 mock，同 run_model_loop_core /
/// summarize_http 先例）：生产闭包 = summarize_messages 配置薄壳，测试闭包 =
/// summarize_http 直连 mock。锁纪律：嵌入/LLM/解析全在锁外，DB 只在快照与
/// 应用两段短临界区里碰（三段式锁纪律保持）。
pub async fn run_extract_with<R, L, Fut>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
    msgs: Vec<crate::bot_chat::ChatMsg>,
    mode: AutoExtract,
    llm: L,
) -> Result<(), String>
where
    R: tauri::Runtime,
    L: Fn(&'static str, Vec<crate::bot_chat::ChatMsg>) -> Fut,
    Fut: std::future::Future<Output = CommandResult<String>>,
{
    if msgs.len() < 2 {
        return Ok(()); // 单条消息（用户刚开聊）无可抽取
    }
    let text = llm(EXTRACT_PROMPT, msgs)
        .await
        .map_err(|e| format!("抽取 LLM 调用失败：{}", e.message()))?;
    let facts = parse_extract(&text);
    if facts.is_empty() {
        return Ok(());
    }
    match mode {
        AutoExtract::Auto => {
            // 第一段（锁外 + 阻塞线程池）：预嵌入（ONNX 推理不占 async worker，
            // 同注入路径纪律）+ 参数读取（1KB 级文件读，同热路径先例）
            let facts_for_emb = facts.clone();
            let embs: Vec<Option<Vec<f32>>> = tauri::async_runtime::spawn_blocking(move || {
                facts_for_emb
                    .iter()
                    .map(|f| super::embed::embed_text(&f.content))
                    .collect()
            })
            .await
            .map_err(|e| format!("抽取预嵌入线程 join 失败：{e}"))?;
            let sp = store::StoreParams::of(&crate::bot::read_memory_tuning(app));
            // 第二段（锁内）：既有记忆快照（纯读；ensure 一次供整条管线用）
            let items: Vec<store::MemItem> = {
                let _g = super::store_lock();
                let conn = crate::db::open_db(app).map_err(|e| e.to_string())?;
                store::ensure_table(&conn)?;
                store::load_all(&conn)?
            };
            // 第三段（锁外）：相似候选 + LLM 逐条裁决（无候选不发 LLM，坏输出回退全 new）
            let candidates = plan_adjudication(&items, &embs, sp.dedup_hint);
            let adjs: Vec<Adjudication> = if candidates.iter().any(|c| c.is_some()) {
                let reply = llm(
                    ADJUDICATE_PROMPT,
                    build_adjudication_msgs(&facts, &candidates),
                )
                .await
                .map_err(|e| format!("裁决 LLM 调用失败：{}", e.message()))?;
                parse_adjudication(&reply, &candidates)
            } else {
                vec![Adjudication::New; facts.len()]
            };
            // 第四段（锁内）：应用（new 入库 / update 改写 / skip 丢弃）
            let counts = {
                let _g = super::store_lock();
                let conn = crate::db::open_db(app).map_err(|e| e.to_string())?;
                apply_adjudications(&conn, &facts, &embs, &adjs, super::now_ms(), &sp)?
            };
            if counts.failed > 0 {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "memory.extract_insert_failed",
                    "count" => counts.failed
                );
            }
            if counts.updated > 0 || counts.skipped > 0 {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Info,
                    "memory.extract_adjudicated",
                    "new" => counts.inserted,
                    "update" => counts.updated,
                    "skip" => counts.skipped
                );
            }
        }
        AutoExtract::Confirm => {
            // 配置读取在锁外；队列写入事务化（半批不留中间态）；同批时间基准统一
            let _ = crate::bot::read_memory_tuning(app);
            let now = super::now_ms();
            let mut notif_ids: Vec<i64> = Vec::new();
            {
                let _g = super::store_lock();
                let mut conn = crate::db::open_db(app).map_err(|e| e.to_string())?;
                let tx = conn.transaction().map_err(|e| e.to_string())?;
                for f in &facts {
                    pending_insert(&tx, &f.content, &f.kind, f.importance, session_id, now)?;
                    notif_ids.push(tx.last_insert_rowid());
                }
                tx.commit().map_err(|e| e.to_string())?;
            }
            // 通知中心落一条消息（事务已提交，id 集合已定）；失败只影响提醒不影响队列
            if let Err(e) = notify_memory_proposals(app, session_id, now, &notif_ids, &facts) {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "memory.proposal_notify_failed",
                    "error" => e
                );
            }
        }
        AutoExtract::Off => return Ok(()), // 入口已挡；防御臂显式返回防门禁被删后静默吞管线
    }
    Ok(())
}

/// 读会话最近 EXTRACT_MAX_MSGS 条 user/assistant 消息（每条截断）
async fn load_recent_messages(
    app: AppHandle,
    session_id: &str,
) -> Result<Vec<crate::bot_chat::ChatMsg>, String> {
    let sid = session_id.to_string();
    let rows =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(String, String)>, String> {
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM bot_messages
                 WHERE session_id = ?1 AND role IN ('user','assistant')
                 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![sid, EXTRACT_MAX_MSGS], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("历史读取线程 join 失败：{e}"))??;
    // 反转回时间正序
    Ok(rows
        .into_iter()
        .rev()
        .map(|(role, content)| crate::bot_chat::ChatMsg {
            role,
            content: super::truncate_chars(&content, EXTRACT_MSG_CHARS),
        })
        .collect())
}

// ───────────────────────── 通知中心接入 ─────────────────────────

/// confirm 档入队后向通知中心落一条持久化消息（幂等 id：memory:{session}:{ts}）。
/// 失败只影响提醒，不影响队列本身——调用方 audit 留痕。
fn notify_memory_proposals<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    batch_ts_ms: i64,
    ids: &[i64],
    facts: &[ExtractedFact],
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let conn = crate::db::open_db(app).map_err(|e| e.to_string())?;
    let title = format!("新记忆提案 · {} 条", ids.len());
    let body: String = facts
        .first()
        .map(|f| f.content.chars().take(80).collect())
        .unwrap_or_default();
    let payload = serde_json::json!({ "ids": ids });
    let inserted = crate::notifications::notif_insert(
        &conn,
        &format!("memory:{session_id}:{batch_ts_ms}"),
        crate::notifications::KIND_MEMORY,
        &title,
        &body,
        &payload,
    )?;
    if inserted {
        crate::notifications::emit_changed(app);
    }
    Ok(())
}

// ───────────────────────── tauri 命令（待确认队列） ─────────────────────────

/// 待确认队列列表（新→旧）
#[tauri::command]
pub async fn mem_pending_list(app: AppHandle) -> CommandResult<Vec<MemPendingView>> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<MemPendingView>, String> {
        let _g = super::store_lock();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        pending_list(&conn)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("待确认列表线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 收下待确认条目：逐条走 insert_item 既有语义去重入库。
/// 三段式锁纪律：锁内取行 → 锁外预嵌入（ONNX 推理不持写锁）→ 锁内入库并删除。
#[tauri::command]
pub async fn mem_pending_approve(app: AppHandle, ids: Vec<i64>) -> CommandResult<MemImportReport> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemImportReport, String> {
        // 去重（保序）：防前端重发导致同条目双嵌入/双入库
        let mut unique_ids: Vec<i64> = Vec::new();
        for id in ids {
            if !unique_ids.contains(&id) {
                unique_ids.push(id);
            }
        }
        // 第一段（锁内）：取待确认行
        let rows: Vec<(i64, PendingRow)> = {
            let _g = super::store_lock();
            let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for id in &unique_ids {
                if let Some(row) = pending_get(&conn, *id)? {
                    out.push((*id, row));
                }
            }
            out
        };
        if rows.is_empty() {
            return Ok(MemImportReport::default());
        }
        // 第二段（锁外）：预嵌入 + 参数读取
        let prepared: Vec<(i64, PendingRow, Option<Vec<f32>>)> = rows
            .into_iter()
            .map(|(id, row)| {
                let emb = super::embed::embed_text(&row.content);
                (id, row, emb)
            })
            .collect();
        let sp = store::StoreParams::of(&crate::bot::read_memory_tuning(&app2));
        // 第三段（锁内）：入库 + 删队列。
        // 入库 Err → 中止上抛且**不删队列行**（用户可重试，条目不丢）；
        // RejectedFull/RefusedForeignMerge → skipped 并删行（数据性拒收不可重试）
        let mut report = MemImportReport::default();
        let _g = super::store_lock();
        let mut conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        // 整批包同一事务：入库与删队列行必须同成败——半途失败整体回滚，
        // 不会出现「已入库但队列行还在」的重影（重收下会永远 Merging 清不掉）
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (id, row, emb) in &prepared {
            let item = NewItem {
                kind: row.kind.clone(),
                content: row.content.clone(),
                tags: Vec::new(),
                importance: row.importance,
                source: "model_inferred".to_string(),
            };
            match store::insert_item_with(&tx, &item, emb.as_deref(), super::now_ms(), &sp) {
                Ok((store::InsertOutcome::Inserted(_), _)) => report.inserted += 1,
                Ok((store::InsertOutcome::Merged { .. }, _)) => report.merged += 1,
                Ok((store::InsertOutcome::RejectedFull(_), _))
                | Ok((store::InsertOutcome::RefusedForeignMerge { .. }, _)) => report.skipped += 1,
                Err(e) => {
                    return Err(format!(
                        "收下中止（存储故障，本批未提交任何变更，全部条目仍在列表）：{e}"
                    ))
                }
            }
            pending_delete(&tx, &[*id])?;
        }
        // 通知中心回写：队列已处理的提案从对应消息中剔除/整条解决（收下→done）
        crate::notifications::notif_sync_memory(&tx, crate::notifications::STATUS_DONE)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(report)
    })
    .await;
    let report = r
        .map_err(|e| CommandError::from(format!("收下线程 join 失败：{e}")))?
        .map_err(CommandError::from)?;
    crate::notifications::emit_changed(&app);
    Ok(report)
}

/// 忽略待确认条目（直接删除），返回删到的条数
#[tauri::command]
pub async fn mem_pending_reject(app: AppHandle, ids: Vec<i64>) -> CommandResult<usize> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<(usize, ()), String> {
        let _g = super::store_lock();
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        let n = pending_delete(&conn, &ids)?;
        // 通知中心回写：被忽略提案从对应消息中剔除/整条解决（忽略→dismissed）
        crate::notifications::notif_sync_memory(&conn, crate::notifications::STATUS_DISMISSED)?;
        Ok((n, ()))
    })
    .await;
    let (n, ()) = r
        .map_err(|e| CommandError::from(format!("忽略线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    crate::notifications::emit_changed(&app);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::super::MemoryControl;
    use super::*;

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_pending_table(&conn).unwrap();
        conn
    }

    // ── 档位解析 ──

    #[test]
    fn auto_extract_mode_parses_and_defaults_off() {
        assert_eq!(auto_extract_mode(None), AutoExtract::Off);
        let off = Some(MemoryControl::default());
        assert_eq!(auto_extract_mode(off.as_ref()), AutoExtract::Off);
        for (s, want) in [
            ("auto", AutoExtract::Auto),
            ("confirm", AutoExtract::Confirm),
            ("off", AutoExtract::Off),
            ("垃圾值", AutoExtract::Off),
            ("", AutoExtract::Off),
        ] {
            let ctrl = Some(MemoryControl {
                auto_extract: s.into(),
                ..MemoryControl::default()
            });
            assert_eq!(auto_extract_mode(ctrl.as_ref()), want, "mode({s})");
        }
    }

    #[test]
    fn serde_default_fills_auto_extract_off_for_old_blocks() {
        // U15 时期落盘的 memoryControl 块无 autoExtract 字段 → 反序列化补 off
        let ctrl: MemoryControl =
            serde_json::from_str(r#"{"injectionEnabled":true,"autoWriteEnabled":false}"#).unwrap();
        assert_eq!(ctrl.auto_extract, "off");
        assert_eq!(auto_extract_mode(Some(&ctrl)), AutoExtract::Off);
    }

    // ── 限频 ──

    #[test]
    fn throttle_due_first_and_interval() {
        const MIN: i64 = EXTRACT_MIN_INTERVAL_SECS;
        assert!(throttle_due(None, 1_000_000, MIN), "从未抽取 → 到期");
        assert!(!throttle_due(
            Some(1_000_000),
            1_000_000 + MIN * 1000 - 1,
            MIN
        ));
        assert!(throttle_due(Some(1_000_000), 1_000_000 + MIN * 1000, MIN));
    }

    #[test]
    fn throttle_check_set_occupies_window() {
        let now = 10_000_000;
        assert!(throttle_check_set("sess-throttle-test", now), "首次应到期");
        assert!(
            !throttle_check_set("sess-throttle-test", now + 1),
            "窗口内不再触发"
        );
        assert!(
            throttle_check_set("sess-throttle-test", now + EXTRACT_MIN_INTERVAL_SECS * 1000),
            "跨过间隔重新到期"
        );
    }

    // ── 解析矩阵 ──

    #[test]
    fn parse_extract_accepts_array_and_clamps() {
        let text = r#"[{"content":"用户偏好简洁回复","kind":"preference","importance":3},
                       {"content":"用户是后端工程师","kind":"profile","importance":9}]"#;
        let facts = parse_extract(text);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].kind, "preference");
        assert_eq!(facts[1].importance, 5, "importance 钳到 5");
    }

    #[test]
    fn parse_extract_tolerates_fences_and_prose() {
        let fenced = "好的，以下是抽取结果：\n```json\n[{\"content\":\"喜欢中文回复\",\"kind\":\"preference\",\"importance\":2}]\n```\n以上。";
        assert_eq!(parse_extract(fenced).len(), 1);
        assert!(parse_extract("我觉得这段对话没什么可记的。").is_empty());
        assert!(parse_extract("[]").is_empty());
        assert!(parse_extract("").is_empty());
    }

    #[test]
    fn parse_extract_skips_contract_violations() {
        // 空 content / 未知 kind / 缺 kind → 跳过；超长 content 截断
        let long = "长".repeat(MAX_EXTRACT_CONTENT_CHARS + 50);
        let text = format!(
            r#"[{{"content":"","kind":"fact","importance":2}},
               {{"content":"未知类型","kind":"magic","importance":2}},
               {{"content":"缺 kind","importance":2}},
               {{"content":"{long}","kind":"fact","importance":2}}]"#
        );
        let facts = parse_extract(&text);
        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].content.chars().count(),
            MAX_EXTRACT_CONTENT_CHARS,
            "超长内容截断到上限"
        );
    }

    // ── 待确认队列 ──

    #[test]
    fn pending_insert_list_delete_roundtrip() {
        let conn = mem_db();
        pending_insert(&conn, "待确认一", "preference", 3, "s1", 1_000).unwrap();
        pending_insert(&conn, "待确认二", "fact", 2, "s1", 2_000).unwrap();
        let list = pending_list(&conn).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].content, "待确认二", "新→旧排序");
        assert_eq!(list[0].created_at, 2_000);
        assert_eq!(pending_delete(&conn, &[list[0].id]).unwrap(), 1);
        assert_eq!(pending_list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn pending_cap_evicts_oldest() {
        let conn = mem_db();
        for i in 0..PENDING_CAP + 5 {
            pending_insert(&conn, &format!("条目{i}"), "fact", 2, "s", 1_000 + i).unwrap();
        }
        let list = pending_list(&conn).unwrap();
        assert_eq!(list.len() as i64, PENDING_CAP, "队列钉在上限");
        // 最旧的 5 条（条目0..条目4）被挤掉
        assert!(list.iter().all(|v| !v.content.contains("条目0")));
        assert!(list.iter().any(|v| v.content.contains("条目5")));
    }

    #[test]
    fn pending_get_returns_row_or_none() {
        let conn = mem_db();
        pending_insert(&conn, "某条", "fact", 2, "s", 1_000).unwrap();
        let id = pending_list(&conn).unwrap()[0].id;
        assert_eq!(pending_get(&conn, id).unwrap().unwrap().content, "某条");
        assert!(pending_get(&conn, 999).unwrap().is_none());
    }

    // ── 写入时冲突裁决（U19）──

    /// 512 维单热向量（与 memory/tests.rs 同款假向量）
    fn onehot(i: usize) -> Vec<f32> {
        let mut v = vec![0f32; 512];
        v[i % 512] = 1.0;
        v
    }

    /// 与 onehot(0) 余弦 = w 的向量
    fn tilted(w: f32) -> Vec<f32> {
        let mut v = vec![0f32; 512];
        v[0] = w;
        v[1] = (1.0 - w * w).max(0.0).sqrt();
        v
    }

    fn fact_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        store::ensure_table(&conn).unwrap();
        conn
    }

    fn seeded_item(
        conn: &rusqlite::Connection,
        kind: &str,
        content: &str,
        tags: &[&str],
        emb: Option<&[f32]>,
        ms: i64,
    ) -> String {
        let item = store::NewItem {
            kind: kind.to_string(),
            content: content.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            importance: 3,
            source: "user_stated".to_string(),
        };
        match store::insert_item(conn, &item, emb, ms).unwrap() {
            (store::InsertOutcome::Inserted(m), _) => m.id,
            other => panic!("应直接插入：{other:?}"),
        }
    }

    fn mkfact(content: &str, kind: &str, importance: i64) -> ExtractedFact {
        ExtractedFact {
            content: content.to_string(),
            kind: kind.to_string(),
            importance,
        }
    }

    fn cand(id: &str) -> Option<store::MemItem> {
        Some(store::MemItem {
            id: id.to_string(),
            kind: "fact".into(),
            content: "已有记忆内容".into(),
            tags: vec![],
            importance: 3,
            source: "user_stated".into(),
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
            access_count: 0,
            last_accessed_at_ms: None,
            embedding: None,
        })
    }

    #[test]
    fn adjudication_parse_matrix() {
        let cands = vec![cand("id-a"), cand("id-b"), None];
        let text = r#"裁决如下：```json
            [{"index":0,"action":"new"},
             {"index":1,"action":"update","existing_id":"id-b"},
             {"index":2,"action":"skip"}]
            ```"#;
        assert_eq!(
            parse_adjudication(text, &cands),
            vec![
                Adjudication::New,
                Adjudication::Update("id-b".into()),
                Adjudication::New, // 无候选条目恒 New（本就不该被裁决）
            ]
        );
        // skip / 幻觉 id / 无候选条目被误裁决
        let text2 = r#"[{"index":0,"action":"skip"},
                        {"index":1,"action":"update","existing_id":"id-幻觉"},
                        {"index":2,"action":"update","existing_id":"id-a"}]"#;
        assert_eq!(
            parse_adjudication(text2, &cands),
            vec![
                Adjudication::Skip,
                Adjudication::New, // 幻觉 id ≠ 候选 id → 回退 new
                Adjudication::New, // 无候选恒 New（幻觉 id 指到有候选的别条也不放行）
            ],
            "输出长度与 candidates 对齐，越界 index 忽略"
        );
    }

    #[test]
    fn adjudication_parse_bad_output_falls_back_all_new() {
        let cands = vec![cand("id-a"), cand("id-b")];
        for bad in [
            "",
            "我觉得这几条都没问题。",
            "[{broken json",
            "[]", // 合法空数组 = 没给任何裁决 → 全回退 new
            r#"[{"index":0,"action":"炸"}]"#,
        ] {
            assert_eq!(
                parse_adjudication(bad, &cands),
                vec![Adjudication::New, Adjudication::New],
                "坏输出应整体回退全 new：{bad}"
            );
        }
    }

    #[test]
    fn plan_adjudication_top1_threshold_and_kind_scope() {
        let conn = fact_db();
        let id_hit = seeded_item(
            &conn,
            "fact",
            "用户住在上海",
            &["居住城市"],
            Some(&onehot(0)),
            1_000,
        );
        // lesson 同向量但不在抽取域 → 不算候选（关语义合并让两行共存）
        let lesson = store::NewItem {
            kind: "lesson".to_string(),
            content: "住在上海的教训".to_string(),
            tags: vec!["lesson".to_string()],
            importance: 3,
            source: "user_stated".to_string(),
        };
        let no_merge = store::StoreParams {
            dedup_merge: 1.0,
            ..store::StoreParams::default()
        };
        store::insert_item_with(&conn, &lesson, Some(&onehot(0)), 1_001, &no_merge).unwrap();
        let items = store::load_all(&conn).unwrap();
        // cos=0.8 ≥ hint(0.75) → 命中 fact 候选（而非同向 lesson）
        let r = plan_adjudication(&items, &[Some(tilted(0.8))], 0.75);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_ref().map(|m| m.id.as_str()), Some(id_hit.as_str()));
        // cos=0.5 < hint → 无候选
        assert!(plan_adjudication(&items, &[Some(tilted(0.5))], 0.75)[0].is_none());
        // 事实无向量（降级模式）→ 无候选
        assert!(plan_adjudication(&items, &[None], 0.75)[0].is_none());
        // top-1：两条候选取相似度更高者
        let conn2 = fact_db();
        seeded_item(&conn2, "fact", "弱相似", &[], Some(&onehot(1)), 1_000);
        let id_strong = seeded_item(&conn2, "fact", "强相似", &[], Some(&onehot(0)), 1_001);
        let items2 = store::load_all(&conn2).unwrap();
        let embs = vec![Some(onehot(0))]; // 与「强相似」同向（cos=1），与「弱相似」正交
        assert_eq!(
            plan_adjudication(&items2, &embs, 0.75)[0]
                .as_ref()
                .map(|m| m.id.as_str()),
            Some(id_strong.as_str())
        );
    }

    #[test]
    fn adjudication_msgs_empty_without_candidates() {
        let facts = vec![mkfact("事实一", "fact", 2), mkfact("事实二", "fact", 2)];
        // 全无候选 → 空（调用方短路不发 LLM）
        assert!(build_adjudication_msgs(&facts, &[None, None]).is_empty());
        // 有候选 → 只列有候选的条目，编号与 facts 下标一致
        let cands = vec![cand("id-x"), None];
        let msgs = build_adjudication_msgs(&facts, &cands);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert!(
            msgs[0].content.contains("[0]"),
            "编号对位：{}",
            msgs[0].content
        );
        assert!(msgs[0].content.contains("id=id-x"));
        assert!(
            !msgs[0].content.contains("事实二"),
            "无候选条目不进裁决消息"
        );
    }

    #[test]
    fn adjudication_apply_update_keeps_original_columns() {
        let conn = fact_db();
        let now = 10_000;
        let orig_id = seeded_item(
            &conn,
            "preference",
            "用户住在上海",
            &["居住城市"],
            Some(&onehot(0)),
            1_000,
        );
        let facts = vec![mkfact("用户已经搬到北京海淀", "fact", 2)];
        let embs = vec![Some(onehot(2))];
        let adjs = vec![Adjudication::Update(orig_id.clone())];
        let counts = apply_adjudications(
            &conn,
            &facts,
            &embs,
            &adjs,
            now,
            &store::StoreParams::default(),
        )
        .unwrap();
        assert_eq!(
            counts,
            AdjudicationCounts {
                updated: 1,
                ..Default::default()
            }
        );
        let rows = store::load_all(&conn).unwrap();
        assert_eq!(rows.len(), 1, "改口不堆积");
        let m = &rows[0];
        assert_eq!(m.id, orig_id, "原条目被更新而非新增");
        assert_eq!(m.content, "用户已经搬到北京海淀");
        assert_eq!(
            m.tags,
            vec!["居住城市"],
            "tags 保持原口径（key 覆盖语义不破）"
        );
        assert_eq!(m.kind, "preference", "kind 保持原口径");
        assert_eq!(m.source, "user_stated", "source 保持原口径");
        assert_eq!(m.importance, 3, "importance 保持原口径");
        assert_eq!(
            m.embedding.as_deref(),
            Some(&onehot(2)[..]),
            "向量随新内容刷新"
        );
        assert_eq!(m.updated_at_ms, now);
    }

    #[test]
    fn adjudication_apply_new_skip_and_vanished_target_fallback() {
        let conn = fact_db();
        let now = 10_000;
        let facts = vec![
            mkfact("全新事实一条", "fact", 3),
            mkfact("重复没增量", "fact", 2),
            mkfact("目标已消失的改口", "fact", 2),
        ];
        let embs = vec![Some(onehot(0)), Some(onehot(1)), Some(onehot(2))];
        let adjs = vec![
            Adjudication::New,
            Adjudication::Skip,
            Adjudication::Update("不存在的id".into()),
        ];
        let counts = apply_adjudications(
            &conn,
            &facts,
            &embs,
            &adjs,
            now,
            &store::StoreParams::default(),
        )
        .unwrap();
        assert_eq!(counts.inserted, 2, "New 落库 + 消失目标回退插入");
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.updated, 0);
        let rows = store::load_all(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        let fresh = rows.iter().find(|m| m.content == "全新事实一条").unwrap();
        assert_eq!(fresh.source, "model_inferred", "抽取产物带推断来源");
        assert_eq!(fresh.kind, "fact");
        assert_eq!(fresh.importance, 3);
        assert!(
            rows.iter().all(|m| m.content != "重复没增量"),
            "skip 丢弃不落库"
        );
    }

    #[test]
    fn adjudication_apply_capacity_reject_counts_skipped() {
        let conn = fact_db();
        let now = 10_000;
        // 灌一条受保护条目（importance=5 + user_stated）：容量=1 时无可淘汰 → 拒写
        let protected = seeded_item(&conn, "fact", "受保护条目", &[], Some(&onehot(9)), 1_000);
        conn.execute(
            "UPDATE mem_items SET importance = 5 WHERE id = ?1",
            rusqlite::params![protected],
        )
        .unwrap();
        let sp = store::StoreParams {
            capacity: 1,
            ..store::StoreParams::default()
        };
        let facts = vec![mkfact("第一条", "fact", 2), mkfact("第二条", "fact", 2)];
        let embs = vec![None, None];
        let adjs = vec![Adjudication::New, Adjudication::New];
        let counts = apply_adjudications(&conn, &facts, &embs, &adjs, now, &sp).unwrap();
        assert_eq!(counts.inserted, 0);
        assert_eq!(counts.skipped, 2, "容量满拒写计 skipped（数据性拒收）");
        assert_eq!(counts.failed, 0);
        assert_eq!(store::load_all(&conn).unwrap().len(), 1, "只剩受保护条目");
    }
}
