//! 回合终稿聚合器（真源化 §5.3）：流式发射点同步记账，回合终结处取走落库。
//!
//! 与 stream_log 回放缓冲的分工：缓冲是「给前端补看」的（有攒批上限、可裁头），
//! 这里是「落库事实」的——push 即记账、begin 清场、take 收走，不依赖缓冲存活。
//!
//! 记账点收口在两处（发射点全集）：
//! - 模型循环的 `emit` 收口（bot_model_loop/shell.rs）：buffered 事件全部经它，
//!   覆盖 bot-think-delta / bot-tool-running / bot-tool-done / bot-turn-status；
//! - bot-file-changed 发射点（bot/dispatch.rs）：自盖章不进缓冲，单独 push。
//!
//! 生命周期：`begin` 在循环开始处与 stream_log::begin 并肩清场（上一轮崩溃
//! 残留不得污染本轮）；`take` 在回合终结处收走（用户轮命令层落终稿行）。
//! 非主聊轮（汇报轮 / 非 stream_to_main 的子 agent 循环）push 进来的记录
//! 由下一次 begin 清掉，不落任何行。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::db::lock::lock_or_recover;

/// 一轮的落库事实。tools / file_changes 保持发射序（前端按序渲染工具行）。
#[derive(Default)]
pub(crate) struct TurnRecord {
    pub(crate) thinking: Option<String>,
    pub(crate) tools_json: Option<String>,
    pub(crate) file_changes_json: Option<String>,
    /// 最后一次 bot-turn-status 的 status（循环唯一出口必发终态）
    pub(crate) turn_status: Option<String>,
}

static RECORDS: Mutex<Option<HashMap<String, serde_json::Value>>> = Mutex::new(None);

fn with_map<T>(f: impl FnOnce(&mut HashMap<String, serde_json::Value>) -> T) -> T {
    let mut guard = lock_or_recover(&RECORDS, "turn_record::RECORDS");
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// 循环开始处清场（与 stream_log::begin 并肩调用）
pub(crate) fn begin(session_id: &str) {
    with_map(|map| {
        map.remove(session_id);
    });
}

/// 循环 emit 收口的统一记账：按事件名分派。非记账事件静默。
pub(crate) fn push_event(session_id: &str, event: &str, payload: &serde_json::Value) {
    with_map(|map| {
        let rec = map
            .entry(session_id.to_string())
            .or_insert_with(|| serde_json::json!({ "thinking": "", "tools": [], "files": [] }));
        match event {
            "bot-think-delta" => {
                if let Some(t) = payload["text"].as_str() {
                    rec["thinking"] = serde_json::Value::String(format!(
                        "{}{}",
                        rec["thinking"].as_str().unwrap_or(""),
                        t
                    ));
                }
            }
            "bot-tool-running" => {
                rec["tools"]
                    .as_array_mut()
                    .expect("tools 数组")
                    .push(serde_json::json!({
                        "id": payload["id"],
                        "name": payload["name"],
                        "argsPreview": payload["args_preview"],
                        "done": false,
                    }));
            }
            "bot-tool-done" => {
                // 回填最后一个同 id 的 running 行（工具调用 id 回合内唯一）
                let id = payload["id"].as_str();
                let tools = rec["tools"].as_array_mut().expect("tools 数组");
                if let Some(entry) = tools
                    .iter_mut()
                    .rev()
                    .find(|t| t["id"].as_str() == id && t["done"] == false)
                {
                    entry["args"] = payload["args"].clone();
                    entry["result"] = payload["result"].clone();
                    entry["ms"] = payload["ms"].clone();
                    entry["ok"] = payload["ok"].clone();
                    entry["done"] = serde_json::Value::Bool(true);
                }
            }
            "bot-turn-status" => {
                if let Some(s) = payload["status"].as_str() {
                    rec["status"] = serde_json::Value::String(s.to_string());
                }
            }
            _ => {}
        }
    });
}

/// bot-file-changed 发射点单独记账（自盖章事件不经 emit 收口）
pub(crate) fn push_file_change(session_id: &str, path: &str, kind: &str, added: i64, deleted: i64) {
    with_map(|map| {
        let rec = map
            .entry(session_id.to_string())
            .or_insert_with(|| serde_json::json!({ "thinking": "", "tools": [], "files": [] }));
        rec["files"]
            .as_array_mut()
            .expect("files 数组")
            .push(serde_json::json!({
                "path": path,
                "kind": kind,
                "added": added,
                "deleted": deleted,
            }));
    });
}

/// 回合终结处收走（移除），空字段折 None。
/// thinking 是纯文本值，直接取 str——过 serde_json::to_string 会变成带引号
/// 的 JSON 串（锚点⑫钉住的原始回归）。
pub(crate) fn take(session_id: &str) -> TurnRecord {
    let raw = with_map(|map| map.remove(session_id));
    let array_to_string = |v: Option<serde_json::Value>| -> Option<String> {
        match v {
            Some(serde_json::Value::Array(a)) if a.is_empty() => None,
            Some(other) => serde_json::to_string(&other).ok(),
            None => None,
        }
    };
    // 无记录（空回合 / 非主聊轮）：全 None，不凭空造 null 字段
    let Some(rec) = raw else {
        return TurnRecord::default();
    };
    TurnRecord {
        thinking: rec["thinking"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        tools_json: array_to_string(Some(rec["tools"].clone())),
        file_changes_json: array_to_string(Some(rec["files"].clone())),
        turn_status: rec["status"].as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests;
