//! 在途流式事件日志：按会话编号 + 有界缓冲 + 在途登记。
//!
//! 为什么要有它：后端广播是「即焚」的——事件只往窗口广播一次，客户端不在场
//! 时那段时间没有任何接收者，也没有游标与回放，在途回复的**流式过程**就永久
//! 没了（最终正文后端会落库，回来还看得到，丢的正是过程）。这个模块让后端
//! 当真相源，三件缺一不可：
//!
//! - **序号**：每个会话的事件带单调递增的 `seq`（进程内不重置，跨轮次连续）。
//!   没有序号，客户端不知道「补到哪」，补齐与实时的交接就无从判定。
//! - **回放缓冲**：在途会话保留最近一批事件，客户端带 `after_seq` 来补。
//!   只有序号没有缓冲，补不了；只有缓冲没有序号，不知道补到哪。
//! - **在途登记**：谁正在跑，客户端重挂载后能问出来。只有前两样没有登记，
//!   客户端根本不知道有在途回合，回放也就无从谈起（会话列表里没有这层信息）。
//!
//! 口径：
//! - **序号按会话**：跨轮次连续（不重置）——客户端的游标是「本会话已应用到的
//!   序号」，重置会让旧游标落到新一轮的头部之后，补齐直接变成空。
//! - **缓冲只收在途轮次**：回合外的事件（技能路由的失败行早于 `begin`、汇报轮
//!   的收尾行晚于 `finish`）不记也不占序号——记进去回放就会掺进不属于这一轮的
//!   内容。新一轮开始时缓冲清空（上一轮的事件属于上一轮）。
//! - **游标过老如实报 `truncated`**：中间有洞时客户端得从权威快照（历史）重建，
//!   不能把不完整的状态当实时展示。
//! - **收尾留一会儿**：回合结束后缓冲仍留 `RETAIN_AFTER_FINISH_MS`，让刚跑完
//!   就重连的客户端还能补齐最后一段；到期由惰性清扫回收（不另起后台线程）。
//!
//! 不收进缓冲的事件（各有各的理由，别顺手加进来）：
//! - `bot-usage-delta`：**累计型**（前端按会话累加 token），重放会重复计数；
//! - `bot-file-changed`：工落盘事件，发射点在工具执行侧（可能不在模型循环
//!   内），且前端有两个订阅者（聊天窗 + 右侧栏），重放要同时回灌两处才对得上。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

/// 单会话保留的事件条数上限（超了丢最旧的）。
const MAX_EVENTS_PER_SESSION: usize = 20_000;

/// 单会话保留的字节上限（粗估：正文/思考取文本长度，其余按常数计）。
const MAX_BYTES_PER_SESSION: usize = 8 * 1024 * 1024;

/// 非正文事件的字节粗估（工具行、状态这类载荷都很小）。
const EVENT_BYTES_ESTIMATE: usize = 256;

/// 收尾后缓冲保留时长（毫秒）：刚跑完就重连的客户端还能补齐最后一段。
const RETAIN_AFTER_FINISH_MS: i64 = 120_000;

/// 缓冲里的一条事件（回放时原样投递给客户端）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferedEvent {
    /// 本会话内单调递增的序号（跨轮次连续）。
    pub seq: u64,
    /// 事件名（与实时广播同名——回放走同一批前端处理器）。
    pub event: String,
    /// 事件载荷（与实时广播同构，另带 `seq`）。
    pub payload: serde_json::Value,
}

/// 在途会话摘要（客户端据此知道「有回合正在跑」）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InflightRun {
    pub session_id: String,
    /// 本轮开始时刻（毫秒时间戳）。
    pub started_at: i64,
    /// 已分配的最大序号。
    pub last_seq: u64,
    /// 本轮累计事件数。
    pub event_count: u64,
}

/// 回放结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamReplay {
    /// 序号大于 `after_seq` 的事件，按序排列。
    pub events: Vec<BufferedEvent>,
    /// 本次回放覆盖到的最大序号（无事件时等于传入的 `after_seq`）。
    pub last_seq: u64,
    /// 游标已老于缓冲窗口：中间有洞，客户端得从权威快照（历史）重建。
    pub truncated: bool,
    /// 该会话此刻是否仍在跑。false = 回合已收尾，正文以库为准。
    pub inflight: bool,
}

/// 单会话的日志状态。
struct SessionLog {
    /// 下一个要分配的序号（跨轮次连续，不重置）。
    next_seq: u64,
    /// 环形缓冲（超出上限丢最旧的）。
    events: VecDeque<BufferedEvent>,
    /// 缓冲字节粗估。
    bytes: usize,
    /// 本轮累计事件数（跨缓冲裁剪累计）。
    event_count: u64,
    /// 本轮开始时刻。
    started_at: i64,
    /// 是否正在跑。
    inflight: bool,
    /// 收尾时刻（None = 还在跑或从未跑过）。
    finished_at: Option<i64>,
}

impl SessionLog {
    fn new(now: i64) -> Self {
        Self {
            next_seq: 1,
            events: VecDeque::new(),
            bytes: 0,
            event_count: 0,
            started_at: now,
            inflight: false,
            finished_at: None,
        }
    }
}

/// 全局注册表：一个进程一份（与 tool_guard 的会话注册表同款原语）。
static SESSION_LOGS: OnceLock<Mutex<HashMap<String, SessionLog>>> = OnceLock::new();

fn logs() -> MutexGuard<'static, HashMap<String, SessionLog>> {
    SESSION_LOGS
        .get_or_init(|| Mutex::new(HashMap::new()))
        // 锁中毒（某个线程 panic 时持锁）不该把整条流式链路带走：
        // 取出内层数据继续用，宁可少记一条事件也不能让聊天彻底哑掉。
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    crate::db::now_ms()
}

/// 载荷字节粗估：正文/思考取文本长度（这两类是大头），其余按常数计。
fn estimate_bytes(payload: &serde_json::Value) -> usize {
    payload
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.len())
        .unwrap_or(EVENT_BYTES_ESTIMATE)
}

/// 清扫过期条目：收尾超过保留期的会话整条丢弃。
fn sweep_expired(m: &mut HashMap<String, SessionLog>, now: i64) {
    m.retain(|_, v| match v.finished_at {
        Some(t) if !v.inflight => now - t < RETAIN_AFTER_FINISH_MS,
        _ => true,
    });
}

/// 登记一轮开始：清空上一轮缓冲，标记在途。
///
/// 序号**不重置**（见模块头「口径」）——客户端的游标是本会话已应用到的序号，
/// 重置会让旧游标落到新一轮头部之后，补齐直接变空。
pub fn begin(session_id: &str) {
    let now = now_ms();
    let mut m = logs();
    sweep_expired(&mut m, now);
    let entry = m
        .entry(session_id.to_string())
        .or_insert_with(|| SessionLog::new(now));
    entry.inflight = true;
    entry.started_at = now;
    entry.finished_at = None;
    entry.events.clear();
    entry.bytes = 0;
}

/// 登记一轮收尾：不再是在途；缓冲留到保留期到期（惰性清扫）。
pub fn finish(session_id: &str) {
    let now = now_ms();
    let mut m = logs();
    if let Some(entry) = m.get_mut(session_id) {
        entry.inflight = false;
        entry.finished_at = Some(now);
    }
}

/// 记一条事件并返回它的序号。
///
/// **只有在途轮次的事件才进缓冲**（`inflight` 为 false 时不记、不占序号）：
/// 缓冲的语义是「当前这一轮的事件」，回合外的事件（技能路由的失败行、
/// 汇报轮的收尾行）不在其中——它们要么早于 `begin`、要么晚于 `finish`，
/// 记进去会让回放掺进不属于这一轮的内容。
fn record(session_id: &str, event: &str, payload: &serde_json::Value) -> Option<u64> {
    let now = now_ms();
    let mut m = logs();
    sweep_expired(&mut m, now);
    let entry = m
        .entry(session_id.to_string())
        .or_insert_with(|| SessionLog::new(now));
    if !entry.inflight {
        return None;
    }
    let seq = entry.next_seq;
    entry.next_seq += 1;
    entry.event_count += 1;
    entry.bytes += estimate_bytes(payload);
    entry.events.push_back(BufferedEvent {
        seq,
        event: event.to_string(),
        payload: payload.clone(),
    });
    while entry.events.len() > MAX_EVENTS_PER_SESSION || entry.bytes > MAX_BYTES_PER_SESSION {
        let Some(old) = entry.events.pop_front() else {
            break;
        };
        entry.bytes = entry.bytes.saturating_sub(estimate_bytes(&old.payload));
    }
    Some(seq)
}

/// 会话级流式事件的统一出口：盖信封 → 分配序号 → 写缓冲 → 广播。
///
/// **信封字段在这里统一盖章**：载荷顶层的 `sessionId` 由本函数注入，调用方
/// 不必自带。盖章权分散到各发射点，漏一处就静默丢事件——`bot-plan` 就漏过：
/// 载荷里没有会话号，前端按会话过滤的闸门把每条实时更新都丢了，计划浮层
/// 只在重新挂载（重开面板拉快照）时才更新。
///
/// 口径对齐业界事件信封（CloudEvents 的属性层 / Kafka 的 headers / ACP 的
/// `session/update` params）：**会话号是信封，不进业务载荷**。信封只此一处
/// 盖章，业务字段不许自带同名键。
///
/// 序号写进载荷的 `seq` 字段（前端据此去重：补齐与实时可能重叠）。
/// `session_id` 为 None 的事件不进缓冲（后台任务不推流，没有会话可归属），
/// 也就没有会话号可盖。
pub fn emit_recorded<R: Runtime>(
    app: &AppHandle<R>,
    session_id: Option<&str>,
    event: &str,
    mut payload: serde_json::Value,
) {
    if let Some(sid) = session_id {
        // 信封：会话号由出口统一盖章（前端按会话过滤靠它）
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("sessionId".into(), serde_json::json!(sid));
        }
        if let Some(seq) = record(sid, event, &payload) {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("seq".into(), serde_json::json!(seq));
            }
        }
    }
    let _ = app.emit_to("main", event, payload);
}

/// 某会话本轮的起始时刻（毫秒）。
///
/// 前端的回合秒表要按真实起点算：回灌接回时「本地此刻」是补齐的那一刻，
/// 拿它当起点会让秒表从零开始——回复其实已经跑了一会儿了。
pub fn run_started_at(session_id: &str) -> Option<i64> {
    let m = logs();
    m.get(session_id).map(|v| v.started_at)
}

/// 在途会话列表（谁正在跑）。
pub fn inflight_list() -> Vec<InflightRun> {
    let now = now_ms();
    let mut m = logs();
    sweep_expired(&mut m, now);
    m.iter()
        .filter(|(_, v)| v.inflight)
        .map(|(k, v)| InflightRun {
            session_id: k.clone(),
            started_at: v.started_at,
            last_seq: v.next_seq.saturating_sub(1),
            event_count: v.event_count,
        })
        .collect()
}

/// 回放：取序号大于 `after_seq` 的事件。
///
/// `after_seq` 传 0 = 要本轮全部事件（客户端本地没有状态时走这条）。
/// 游标老于缓冲窗口时如实报 `truncated`——中间有洞，客户端得从历史重建，
/// 不能把不完整的状态当实时展示。
pub fn replay(session_id: &str, after_seq: u64) -> StreamReplay {
    let now = now_ms();
    let mut m = logs();
    sweep_expired(&mut m, now);
    let Some(entry) = m.get(session_id) else {
        return StreamReplay {
            events: Vec::new(),
            last_seq: after_seq,
            truncated: false,
            inflight: false,
        };
    };
    let oldest = entry.events.front().map(|e| e.seq);
    let truncated = matches!(oldest, Some(first) if first > after_seq + 1);
    let events: Vec<BufferedEvent> = entry
        .events
        .iter()
        .filter(|e| e.seq > after_seq)
        .cloned()
        .collect();
    let last_seq = events.last().map(|e| e.seq).unwrap_or(after_seq);
    StreamReplay {
        events,
        last_seq,
        truncated,
        inflight: entry.inflight,
    }
}

/// 在途会话列表（客户端命令）。
#[tauri::command]
pub fn bot_inflight_list() -> Vec<InflightRun> {
    inflight_list()
}

/// 回放某会话的流式事件（客户端命令）。
#[tauri::command]
pub fn bot_stream_replay(session_id: String, after_seq: u64) -> StreamReplay {
    replay(&session_id, after_seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测例之间共享全局注册表：用唯一会话 id 隔开，避免互相串味。
    fn sid(tag: &str) -> String {
        format!("test-stream-log-{tag}-{}", uuid::Uuid::new_v4().simple())
    }

    /// 信封：会话号由统一出口盖章，调用方不自带也不会漏。
    ///
    /// 断言的是**缓冲里**的那份载荷——它与广播出去的是同一个值（盖章发生在
    /// 写缓冲之前），所以这份断言同时罩住广播路径。
    #[test]
    fn emit_recorded_stamps_session_id_onto_payload() {
        let app = tauri::test::mock_app();
        let s = sid("stamp");
        begin(&s);
        emit_recorded(
            app.handle(),
            Some(&s),
            "bot-plan",
            serde_json::json!({ "steps": [] }),
        );
        let r = replay(&s, 0);
        assert_eq!(r.events.len(), 1);
        assert_eq!(
            r.events[0].payload["sessionId"],
            serde_json::json!(s),
            "事件信封必须带会话号：前端按会话过滤，缺了就静默丢事件"
        );
        // 盖章不挤掉业务载荷
        assert!(r.events[0].payload["steps"].is_array());
    }

    #[test]
    fn seq_is_monotonic_and_carried_in_payload() {
        let s = sid("mono");
        // 只有在途轮次的事件才进缓冲：不在途时既不记也不占序号
        assert!(record(&s, "bot-chat-delta", &serde_json::json!({ "text": "a" })).is_none());
        begin(&s);
        let a = record(&s, "bot-chat-delta", &serde_json::json!({ "text": "a" }))
            .expect("在途轮次的事件要有序号");
        let b = record(&s, "bot-chat-delta", &serde_json::json!({ "text": "b" }))
            .expect("在途轮次的事件要有序号");
        assert!(b > a, "序号必须单调递增：{a} -> {b}");
        let r = replay(&s, 0);
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.events[0].seq, a);
        assert_eq!(r.events[1].seq, b);
        assert!(r.inflight, "登记在途的回放结果要认在途");
    }

    #[test]
    fn replay_returns_only_events_after_cursor() {
        let s = sid("cursor");
        begin(&s);
        record(&s, "e", &serde_json::json!({}));
        record(&s, "e", &serde_json::json!({}));
        let last = record(&s, "e", &serde_json::json!({})).expect("在途事件有序号");
        let r = replay(&s, last - 1);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].seq, last);
        assert_eq!(r.last_seq, last);
        assert!(!r.truncated, "游标就在窗口内，不该报截断");
    }

    #[test]
    fn begin_clears_previous_round_but_keeps_seq_continuous() {
        let s = sid("rounds");
        begin(&s);
        record(&s, "e", &serde_json::json!({ "text": "上一轮" }));
        let first = replay(&s, 0).events[0].seq;
        finish(&s);
        begin(&s);
        let second =
            record(&s, "e", &serde_json::json!({ "text": "这一轮" })).expect("在途事件有序号");
        assert!(
            second > first,
            "序号跨轮次连续：{first} -> {second}（重置会让旧游标失效）"
        );
        let r = replay(&s, 0);
        assert_eq!(r.events.len(), 1, "新一轮的缓冲里不该混着上一轮的事件");
        assert_eq!(r.events[0].payload["text"], "这一轮");
    }

    #[test]
    fn inflight_registry_tracks_begin_and_finish() {
        let s = sid("inflight");
        assert!(
            !inflight_list().iter().any(|r| r.session_id == s),
            "没登记过的会话不该出现在在途列表里"
        );
        begin(&s);
        let run = inflight_list().into_iter().find(|r| r.session_id == s);
        assert!(run.is_some(), "begin 之后该出现在在途列表里");
        assert_eq!(run.expect("在途项").last_seq, 0);
        record(&s, "e", &serde_json::json!({}));
        assert_eq!(
            inflight_list()
                .into_iter()
                .find(|r| r.session_id == s)
                .expect("在途项")
                .last_seq,
            1
        );
        finish(&s);
        assert!(
            !inflight_list().iter().any(|r| r.session_id == s),
            "finish 之后不该还在途"
        );
        // 收尾后缓冲仍在（迟到的客户端还能补齐最后一段）
        assert_eq!(replay(&s, 0).events.len(), 1);
        assert!(!replay(&s, 0).inflight);
    }

    #[test]
    fn stale_cursor_is_reported_as_truncated() {
        let s = sid("truncated");
        begin(&s);
        // 记一批，再把窗口推到只剩最后一条（模拟缓冲被裁）
        for i in 0..5 {
            record(&s, "e", &serde_json::json!({ "i": i }));
        }
        {
            let mut m = logs();
            let entry = m
                .entry(s.clone())
                .or_insert_with(|| SessionLog::new(now_ms()));
            entry.events.pop_front();
        }
        // 游标 0 = 客户端一条都没收到，窗口里最早的是第 2 条 → 中间有洞
        let r = replay(&s, 0);
        assert!(r.truncated, "游标老于窗口就该报截断");
        assert_eq!(r.events.len(), 4);
        // 游标就在窗口内 → 不报截断
        assert!(!replay(&s, 1).truncated);
    }

    #[test]
    fn buffer_is_bounded() {
        let s = sid("bounded");
        begin(&s);
        for _ in 0..(MAX_EVENTS_PER_SESSION + 10) {
            record(&s, "e", &serde_json::json!({}));
        }
        let r = replay(&s, 0);
        assert!(
            r.events.len() <= MAX_EVENTS_PER_SESSION,
            "缓冲必须有界：{} > {}",
            r.events.len(),
            MAX_EVENTS_PER_SESSION
        );
    }

    #[test]
    fn unknown_session_replays_empty_without_truncation() {
        let s = sid("unknown");
        let r = replay(&s, 7);
        assert!(r.events.is_empty());
        assert_eq!(r.last_seq, 7);
        assert!(!r.truncated);
        assert!(!r.inflight);
    }
}
