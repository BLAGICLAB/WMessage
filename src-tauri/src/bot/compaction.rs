//! 上下文预算压缩：长会话工具结果累积时分级降密度，不动 user/assistant/system。
//!
//! 三档阈值（占 MSGS_BUDGET_CHARS 的比例）：
//! - 软压缩（>70%）：tool 消息保留首 200 + 尾 100 字符，中间替换为 sidecar 指针
//! - 硬压缩（>85%）：tool 消息只保留首 30 + 尾 30 + 指针
//! - 熔断（>100%）：调用方应拒收新工具调用（不在本模块内做，靠 soft_warn_queued 触发）
//!
//! sidecar 写盘可选：None = 不写盘，压缩文本里留"原文未保留"标记（信息丢失，
//! 但接口仍可用；侧车路径稳定存在时效果最好，模型可通过 read_text_file 回看）。

use std::path::Path;

use serde_json::Value;

const SOFT_KEEP_HEAD: usize = 200;
const SOFT_KEEP_TAIL: usize = 100;
const HARD_KEEP_HEAD: usize = 30;
const HARD_KEEP_TAIL: usize = 30;
/// 软/硬压缩的"最小可节省"门槛：原始文本 ≤ 此值不压缩（替身文本本身就比原文长，省不下）
const MIN_SAVING_CHARS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionLevel {
    None,
    Soft,
    Hard,
    Fuse,
}

#[derive(Debug, Default)]
pub struct CompactionReport {
    pub before_chars: usize,
    pub after_chars: usize,
    pub tool_msgs_compacted: usize,
    /// 实际写入磁盘的 sidecar 绝对路径；sidecar_dir=None 时空
    pub sidecar_paths: Vec<String>,
}

/// 判定当前字符数对应的压缩档位（边界与 bot_model_loop.rs 的 MSGS_BUDGET_CHARS 对齐）。
pub fn classify_level(chars: usize, budget: usize) -> CompactionLevel {
    if chars > budget {
        CompactionLevel::Fuse
    } else if chars > budget * 17 / 20 {
        CompactionLevel::Hard
    } else if chars > budget * 7 / 10 {
        CompactionLevel::Soft
    } else {
        CompactionLevel::None
    }
}

pub fn soft_compact(
    msgs: &mut [Value],
    sidecar_dir: Option<&Path>,
    session_id: Option<&str>,
) -> Result<CompactionReport, String> {
    compact_internal(
        msgs,
        sidecar_dir,
        session_id,
        SOFT_KEEP_HEAD,
        SOFT_KEEP_TAIL,
    )
}

pub fn hard_compact(
    msgs: &mut [Value],
    sidecar_dir: Option<&Path>,
    session_id: Option<&str>,
) -> Result<CompactionReport, String> {
    compact_internal(
        msgs,
        sidecar_dir,
        session_id,
        HARD_KEEP_HEAD,
        HARD_KEEP_TAIL,
    )
}

fn compact_internal(
    msgs: &mut [Value],
    sidecar_dir: Option<&Path>,
    session_id: Option<&str>,
    head_keep: usize,
    tail_keep: usize,
) -> Result<CompactionReport, String> {
    let mut report = CompactionReport {
        before_chars: count_chars(msgs),
        ..Default::default()
    };

    for m in msgs.iter_mut() {
        if m.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        let Some(text) = m.get("content").and_then(|c| c.as_str()) else {
            continue;
        };
        let total = text.chars().count();
        if total <= head_keep + tail_keep + MIN_SAVING_CHARS {
            continue;
        }
        let original = text.to_string();
        let head: String = original.chars().take(head_keep).collect();
        let tail: String = original
            .chars()
            .rev()
            .take(tail_keep)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let middle = total - head_keep - tail_keep;
        let sidecar_path = write_sidecar(sidecar_dir, session_id, &original)?;
        let placeholder = match &sidecar_path {
            Some(p) => format!("\n…（已压缩 {middle} 字符 · 原文见 {p}）…"),
            None => format!("\n…（已压缩 {middle} 字符 · 原文未保留）…"),
        };
        let new_text = format!("{head}{placeholder}{tail}");
        m["content"] = Value::String(new_text);
        if let Some(p) = sidecar_path {
            report.sidecar_paths.push(p);
        }
        report.tool_msgs_compacted += 1;
    }

    report.after_chars = count_chars(msgs);
    Ok(report)
}

fn write_sidecar(
    dir: Option<&Path>,
    session_id: Option<&str>,
    original: &str,
) -> Result<Option<String>, String> {
    let Some(dir) = dir else { return Ok(None) };
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 sidecar 目录失败：{e}"))?;
    let ts = chrono::Utc::now().timestamp_millis();
    let sid_raw = session_id.unwrap_or("anon");
    let safe_sid: String = sid_raw
        .chars()
        .take(32)
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let path = dir.join(format!("conv-{safe_sid}-{ts}.md"));
    let mut content = String::with_capacity(original.len() + 64);
    content.push_str("<!-- compressed tool output, see bot::compaction -->\n\n");
    content.push_str(original);
    std::fs::write(&path, content).map_err(|e| format!("写入 sidecar 失败：{e}"))?;
    Ok(Some(path.display().to_string()))
}

fn count_chars(msgs: &[Value]) -> usize {
    msgs.iter()
        .filter_map(|m| m["content"].as_str())
        .map(|s| s.chars().count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mk_tool_msg(text: &str) -> Value {
        json!({ "role": "tool", "content": text, "tool_call_id": "x" })
    }

    fn mk_user_msg(text: &str) -> Value {
        json!({ "role": "user", "content": text })
    }

    fn mk_assistant_msg(text: &str) -> Value {
        json!({ "role": "assistant", "content": text })
    }

    #[test]
    fn classify_level_thresholds() {
        let b = 1000;
        assert_eq!(classify_level(600, b), CompactionLevel::None);
        assert_eq!(classify_level(701, b), CompactionLevel::Soft);
        assert_eq!(classify_level(851, b), CompactionLevel::Hard);
        assert_eq!(classify_level(1001, b), CompactionLevel::Fuse);
        // 边界精确值：70%/85% 不算软/硬（用 >）
        assert_eq!(classify_level(700, b), CompactionLevel::None);
        assert_eq!(classify_level(850, b), CompactionLevel::Soft);
        assert_eq!(classify_level(1000, b), CompactionLevel::Hard);
    }

    #[test]
    fn soft_compact_preserves_user_assistant_system_intact() {
        let user_text = "用户输入——重要语义，模型必须记住";
        let assistant_text = "模型上一轮的回复";
        let tool_text = "x".repeat(500);
        let mut msgs = vec![
            mk_user_msg(user_text),
            mk_assistant_msg(assistant_text),
            mk_tool_msg(&tool_text),
        ];
        let report = soft_compact(&mut msgs, None, None).unwrap();
        assert_eq!(msgs[0]["content"].as_str().unwrap(), user_text);
        assert_eq!(msgs[1]["content"].as_str().unwrap(), assistant_text);
        let compressed = msgs[2]["content"].as_str().unwrap();
        assert!(compressed.starts_with(&"x".repeat(SOFT_KEEP_HEAD)));
        assert!(compressed.contains("已压缩"));
        assert!(compressed.ends_with(&"x".repeat(SOFT_KEEP_TAIL)));
        assert_eq!(report.tool_msgs_compacted, 1);
    }

    #[test]
    fn soft_compact_short_tool_text_unchanged() {
        let short = "abc";
        let mut msgs = vec![mk_tool_msg(short)];
        let report = soft_compact(&mut msgs, None, None).unwrap();
        assert_eq!(msgs[0]["content"].as_str().unwrap(), short);
        assert_eq!(report.tool_msgs_compacted, 0);
    }

    #[test]
    fn hard_compact_keeps_head_30_tail_30() {
        let head: String = "H".repeat(40);
        let mid: String = "M".repeat(400);
        let tail: String = "T".repeat(40);
        let tool_text = head + &mid + &tail;
        let mut msgs = vec![mk_tool_msg(&tool_text)];
        let report = hard_compact(&mut msgs, None, None).unwrap();
        let compressed = msgs[0]["content"].as_str().unwrap();
        assert!(compressed.starts_with(&"H".repeat(HARD_KEEP_HEAD)));
        assert!(compressed.ends_with(&"T".repeat(HARD_KEEP_HEAD)));
        assert_eq!(report.tool_msgs_compacted, 1);
        // 中间被替身，不应有大段 M
        assert!(!compressed.contains(&"M".repeat(100)));
    }

    #[test]
    fn sidecar_writes_file_and_is_referenced() {
        let dir = std::env::temp_dir().join(format!("wm-compact-{}", uuid::Uuid::new_v4()));
        let original: String = "X".repeat(1000);
        let mut msgs = vec![mk_tool_msg(&original)];
        let report = soft_compact(&mut msgs, Some(&dir), Some("sess-42")).unwrap();
        assert_eq!(report.sidecar_paths.len(), 1);
        let path_str = &report.sidecar_paths[0];
        let path = std::path::Path::new(path_str);
        assert!(path.exists(), "sidecar 文件应存在：{path_str}");
        let written = std::fs::read_to_string(path).unwrap();
        assert!(written.contains(&original));
        assert!(msgs[0]["content"].as_str().unwrap().contains(path_str));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sidecar_dir_none_skips_write_but_compresses() {
        let mut msgs = vec![mk_tool_msg(&"Y".repeat(1000))];
        let report = soft_compact(&mut msgs, None, None).unwrap();
        assert_eq!(report.sidecar_paths.len(), 0);
        assert_eq!(report.tool_msgs_compacted, 1);
        assert!(msgs[0]["content"].as_str().unwrap().contains("原文未保留"));
    }

    #[test]
    fn report_chars_before_after() {
        let tool_text: String = "Z".repeat(2000);
        let mut msgs = vec![mk_tool_msg(&tool_text)];
        let report = soft_compact(&mut msgs, None, None).unwrap();
        assert!(report.before_chars > report.after_chars);
        assert_eq!(report.before_chars, 2000);
        assert!(report.after_chars < 1000);
    }
}
