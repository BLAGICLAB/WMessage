//! 用户反馈信号（feedback.jsonl）
//!
//! spec 格式：`{session_id, signal_type, value, timestamp}`
//! 实际扩展 `case_id` / `note` 两个可选项（不进 bot.log，独立 jsonl）。
//! `signal_type` ∈ {thumbs_up, thumbs_down, rollback, task_complete, task_failed, tool_error}。

use serde::{Deserialize, Serialize};

/// 一条用户反馈信号
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackEntry {
    /// 关联 session
    pub session_id: String,
    /// 关联 eval case（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
    /// 信号类型
    pub signal_type: SignalType,
    /// 信号值（不同类型语义不同，见 SignalType 注释）
    pub value: f64,
    /// 时间戳（epoch ms）
    pub timestamp_ms: i64,
    /// 备注（可选；不进 bot.log）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 反馈信号类型（字符串值锁死，便于 grep）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    /// 赞（value ∈ {0.0, 1.0}）
    ThumbsUp,
    /// 踩（value ∈ {0.0, 1.0}）
    ThumbsDown,
    /// 用户主动回滚（value = 1.0 触发、0.0 取消）
    Rollback,
    /// 任务完成（value = 1.0）
    TaskComplete,
    /// 任务失败（value = 1.0）
    TaskFailed,
    /// 工具调用错误（value = 错误码或 1.0）
    ToolError,
}

impl SignalType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThumbsUp => "thumbs_up",
            Self::ThumbsDown => "thumbs_down",
            Self::Rollback => "rollback",
            Self::TaskComplete => "task_complete",
            Self::TaskFailed => "task_failed",
            Self::ToolError => "tool_error",
        }
    }
}

/// 追加一条反馈到 jsonl；保证父目录存在
pub fn append(path: &std::path::Path, entry: &FeedbackEntry) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let line = serde_json::to_string(entry).map_err(|e| format!("序列化失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

/// 读取全部反馈（按时间升序）
pub fn read_all(path: &std::path::Path) -> Result<Vec<FeedbackEntry>, String> {
    use std::io::BufRead;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(path).map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("读取第 {} 行失败：{e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let e: FeedbackEntry = serde_json::from_str(&line)
            .map_err(|e| format!("第 {} 行 JSON 错误：{e}", i + 1))?;
        out.push(e);
    }
    out.sort_by_key(|e| e.timestamp_ms);
    Ok(out)
}

/// 过滤某 session 的反馈
pub fn filter_session(entries: &[FeedbackEntry], session_id: &str) -> Vec<FeedbackEntry> {
    entries.iter().filter(|e| e.session_id == session_id).cloned().collect()
}

/// 过滤某信号类型
pub fn filter_type(entries: &[FeedbackEntry], signal_type: SignalType) -> Vec<FeedbackEntry> {
    entries.iter().filter(|e| e.signal_type == signal_type).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(signal_type: SignalType, value: f64, session_id: &str) -> FeedbackEntry {
        FeedbackEntry {
            session_id: session_id.into(),
            case_id: None,
            signal_type,
            value,
            timestamp_ms: 1_700_000_000_000,
            note: None,
        }
    }

    #[test]
    fn signal_type_strings_locked() {
        // 锁死 grep 字符串
        assert_eq!(SignalType::ThumbsUp.as_str(), "thumbs_up");
        assert_eq!(SignalType::ThumbsDown.as_str(), "thumbs_down");
        assert_eq!(SignalType::Rollback.as_str(), "rollback");
        assert_eq!(SignalType::TaskComplete.as_str(), "task_complete");
        assert_eq!(SignalType::TaskFailed.as_str(), "task_failed");
        assert_eq!(SignalType::ToolError.as_str(), "tool_error");
    }

    #[test]
    fn append_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("fb-rt-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("feedback.jsonl");
        let e1 = mk(SignalType::ThumbsUp, 1.0, "s1");
        let e2 = mk(SignalType::TaskComplete, 1.0, "s1");
        append(&p, &e1).unwrap();
        append(&p, &e2).unwrap();
        let read = read_all(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].signal_type, SignalType::ThumbsUp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let p = std::path::Path::new("/tmp/definitely-does-not-exist-xyz-12345.jsonl");
        let read = read_all(p).unwrap();
        assert!(read.is_empty());
    }

    #[test]
    fn filter_session_and_type() {
        let entries = vec![
            mk(SignalType::ThumbsUp, 1.0, "s1"),
            mk(SignalType::ThumbsUp, 1.0, "s2"),
            mk(SignalType::TaskFailed, 1.0, "s1"),
        ];
        let s1 = filter_session(&entries, "s1");
        assert_eq!(s1.len(), 2);
        let up = filter_type(&entries, SignalType::ThumbsUp);
        assert_eq!(up.len(), 2);
    }
}