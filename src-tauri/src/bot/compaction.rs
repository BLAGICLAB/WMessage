//! 上下文预算压缩：长会话工具结果累积时分级降密度，不动 user/assistant/system。
//!
//! 三档阈值（占 MSGS_BUDGET_CHARS 的比例）：
//! - 软压缩（>70%）：tool 消息保留首 200 + 尾 100 字符，中间替换为 sidecar 指针
//! - 硬压缩（>85%）：tool 消息只保留首 30 + 尾 30 + 指针
//! - 熔断（>100%）：调用方应拒收新工具调用（不在本模块内做，靠 soft_warn_queued 触发）
//!
//! sidecar 写盘可选：None = 不写盘，压缩文本里留"原文未保留"标记（信息丢失，
//! 但接口仍可用；侧车路径稳定存在时效果最好，模型可通过 read_text_file 回看）。
//!
//! 已压缩消息带 ⟦c:…⟧ 标记：软压缩跳过（避免对替身文本重复压缩）；硬压缩
//! 允许再压，但沿用原指针（盘上原文还在，不写第二个 sidecar）。

use std::path::{Path, PathBuf};

use serde_json::Value;

const SOFT_KEEP_HEAD: usize = 200;
const SOFT_KEEP_TAIL: usize = 100;
const HARD_KEEP_HEAD: usize = 30;
const HARD_KEEP_TAIL: usize = 30;

/// 压缩标记前缀：替身文本尾部带 ⟦{uid8}⟧，识别"这条消息已压过"。
/// uid8 每次随机，普通工具输出不可能撞上；content 里出现它 = 我们自己写的。
const COMPACT_MARK: &str = "⟦c:";

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
        false,
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
        true,
    )
}

/// 替身标记 uid（8 位随机）：同毫秒批量压缩也不撞、跨轮可识别已压缩消息。
fn mark_uid() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    u[..8].to_string()
}

/// 从已压缩文本里提取原 sidecar 指针路径（"原文见 " 到 "）" 之间）。
fn extract_pointer(text: &str) -> Option<String> {
    let start = text.find("原文见 ")? + "原文见 ".len();
    let rest = &text[start..];
    let end = rest.find('）')?;
    let p = &rest[..end];
    (!p.is_empty()).then(|| p.to_string())
}

/// sidecar 文件路径：毫秒时间戳 + 8 位随机后缀——同一轮批量压缩落在同一毫秒
/// 也不会同名互覆（旧版只有时间戳，前一条的指针会被后一条的原文覆盖）。
fn sidecar_file_path(dir: &Path, session_id: Option<&str>) -> PathBuf {
    let ts = chrono::Utc::now().timestamp_millis();
    let sid_raw = session_id.unwrap_or("anon");
    let safe_sid: String = sid_raw
        .chars()
        .take(32)
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    dir.join(format!("conv-{safe_sid}-{ts}-{}.md", mark_uid()))
}

/// 对单条 tool 消息按档位压缩，返回 (是否实际压缩, 新落盘的 sidecar 路径)。
/// - hard=false（软档）：未压过的压软档；已压过（带 ⟦c:⟧ 标记）的跳过——
///   对替身文本再压省不了多少、还叠标记。
/// - hard=true（硬档）：未压过的压硬档；已压过的再压但沿用原指针、不写
///   第二个 sidecar（盘上原文还在，二次落盘只是复制品）。
/// 非 tool 消息 / content 非字符串：原样返回 false。
pub(crate) fn compact_one(
    m: &mut Value,
    hard: bool,
    sidecar_dir: Option<&Path>,
    session_id: Option<&str>,
) -> Result<(bool, Vec<String>), String> {
    if m.get("role").and_then(|r| r.as_str()) != Some("tool") {
        return Ok((false, Vec::new()));
    }
    let Some(text) = m.get("content").and_then(|c| c.as_str()) else {
        return Ok((false, Vec::new()));
    };
    let (head_keep, tail_keep) = if hard {
        (HARD_KEEP_HEAD, HARD_KEEP_TAIL)
    } else {
        (SOFT_KEEP_HEAD, SOFT_KEEP_TAIL)
    };
    let total = text.chars().count();
    let mut pointer: Option<String> = None;
    // 长度守卫对**所有**分支生效，不能只挂在 `else if` 上。
    //
    // 短于 head+tail 的文本本就没得压：硬档下 head/tail 还会重叠，替身把
    // 原文首尾重复一遍；而它同时是下面 `total - head_keep - tail_keep`
    // 不下溢的前提——原来带标记的短文本绕过这道守卫，直接落到无符号减法，
    // debug 构建 panic、release 静默回绕成天文数字。
    if total <= head_keep + tail_keep {
        return Ok((false, Vec::new()));
    }
    if text.contains(COMPACT_MARK) {
        // 软档跳过已压过的（对替身再压省不了多少、还叠标记）；
        // 硬档再压但沿用原指针、不写第二个 sidecar（盘上原文还在）。
        if !hard {
            return Ok((false, Vec::new()));
        }
        pointer = extract_pointer(text);
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
    // 守卫已保证 total > head+tail（故 ≥1）；saturating 只是兜住将来守卫
    // 被挪动时的静默回绕——回绕出来的天文数字会写进正文当成"压缩了多少"。
    let middle = total.saturating_sub(head_keep + tail_keep);
    // 先定路径、后写盘：替身不比原文短就整条跳过（sidecar 也不落）——
    // 指针里的绝对路径可能比省下的字符还长，压了白压还孤儿一个文件
    let sidecar_path: Option<PathBuf> = match (sidecar_dir, &pointer) {
        (Some(dir), None) => Some(sidecar_file_path(dir, session_id)),
        _ => None,
    };
    let placeholder = match (&sidecar_path, &pointer) {
        (Some(p), _) => format!(
            "\n…（已压缩 {middle} 字符 · 原文见 {}）…{COMPACT_MARK}{}⟧",
            p.display(),
            mark_uid()
        ),
        (None, Some(path)) => format!(
            "\n…（已压缩 {middle} 字符 · 原文见 {path}）…{COMPACT_MARK}{}⟧",
            mark_uid()
        ),
        (None, None) => format!(
            "\n…（已压缩 {middle} 字符 · 原文未保留）…{COMPACT_MARK}{}⟧",
            mark_uid()
        ),
    };
    if head_keep + tail_keep + placeholder.chars().count() >= total {
        return Ok((false, Vec::new()));
    }
    let mut written: Vec<String> = Vec::new();
    if let (Some(p), Some(dir)) = (&sidecar_path, sidecar_dir) {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 sidecar 目录失败：{e}"))?;
        let mut content = String::with_capacity(original.len() + 64);
        content.push_str("<!-- compressed tool output, see bot::compaction -->\n\n");
        content.push_str(&original);
        std::fs::write(p, content).map_err(|e| format!("写入 sidecar 失败：{e}"))?;
        written.push(p.display().to_string());
    }
    m["content"] = Value::String(format!("{head}{placeholder}{tail}"));
    Ok((true, written))
}

fn compact_internal(
    msgs: &mut [Value],
    sidecar_dir: Option<&Path>,
    session_id: Option<&str>,
    _head_keep: usize,
    _tail_keep: usize,
    recompact_marked: bool,
) -> Result<CompactionReport, String> {
    let mut report = CompactionReport {
        before_chars: count_chars(msgs),
        ..Default::default()
    };
    let hard = recompact_marked;

    for m in msgs.iter_mut() {
        let (compacted, paths) = compact_one(m, hard, sidecar_dir, session_id)?;
        if compacted {
            report.sidecar_paths.extend(paths);
            report.tool_msgs_compacted += 1;
        }
    }

    report.after_chars = count_chars(msgs);
    Ok(report)
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

    fn tmp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("wm-compact-{}", uuid::Uuid::new_v4()))
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
    fn compact_one_soft_skips_marked_hard_recompacts_with_pointer() {
        let dir = tmp_dir();
        let mut m = mk_tool_msg(&"V".repeat(1000));
        // 软档首压：落一个 sidecar
        let (ok, paths) = compact_one(&mut m, false, Some(&dir), Some("s")).unwrap();
        assert!(ok && paths.len() == 1);
        let soft_text = m["content"].as_str().unwrap().to_string();
        // 软档再压：跳过（不叠标记、不再落盘）
        let (ok2, p2) = compact_one(&mut m, false, Some(&dir), Some("s")).unwrap();
        assert!(!ok2 && p2.is_empty());
        assert_eq!(m["content"].as_str().unwrap(), soft_text);
        // 硬档再压：沿用原指针、不新增 sidecar、文本更短
        let (ok3, p3) = compact_one(&mut m, true, Some(&dir), Some("s")).unwrap();
        assert!(ok3 && p3.is_empty());
        let hard_text = m["content"].as_str().unwrap();
        assert_eq!(extract_pointer(hard_text), extract_pointer(&soft_text));
        assert!(hard_text.chars().count() < soft_text.chars().count());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compact_one_ignores_non_tool_and_short_text() {
        let mut u = mk_user_msg("用户消息不压");
        assert!(!compact_one(&mut u, true, None, None).unwrap().0);
        let mut t = mk_tool_msg("短结果");
        assert!(!compact_one(&mut t, true, None, None).unwrap().0);
    }

    #[test]
    fn sidecar_writes_file_and_is_referenced() {
        let dir = tmp_dir();
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
    fn sidecar_paths_unique_across_messages() {
        let dir = tmp_dir();
        let mut msgs = vec![mk_tool_msg(&"X".repeat(800)), mk_tool_msg(&"Y".repeat(800))];
        let report = soft_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        assert_eq!(report.sidecar_paths.len(), 2, "两条都应落盘");
        assert_ne!(
            report.sidecar_paths[0], report.sidecar_paths[1],
            "同毫秒批量压缩也不允许同名互覆"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 短于保留窗口的**已带标记**文本，硬档不得下溢。
    ///
    /// 触发条件：工具输出自带压缩标记前缀（`⟦c:`）。标记后缀是 8 位随机
    /// 十六进制，正常工具输出撞不上；但模型把占位符原样拷回工具参数、
    /// 或 grep 命中含标记的行时，就会出现这种短文本。
    ///
    /// 原实现里长度守卫挂在 `else if` 上，只挡未压过的文本——已带标记的
    /// 短文本直接落到 `total - head_keep - tail_keep`，无符号减法下溢：
    /// debug 构建 panic，release 静默回绕成天文数字（"已压缩 18446… 字符"）。
    #[test]
    fn compact_one_never_underflows_on_short_marked_text() {
        for text in ["⟦c:1f3a9b2c⟧", "⟦c:", "x⟦c:"] {
            let mut m = mk_tool_msg(text);
            let (ok, paths) = compact_one(&mut m, true, None, None).unwrap();
            assert!(!ok, "短于保留窗口的文本压不动：{text}");
            assert!(paths.is_empty(), "压不动就不该落 sidecar：{text}");
            assert_eq!(m["content"].as_str().unwrap(), text, "原样保留：{text}");
        }
    }

    /// 已压过但**长于**保留窗口的文本，硬档仍要能再压（沿用原指针）。
    ///
    /// 这是上面那条的对照：不能因为「带标记」就整条跳过。
    #[test]
    fn hard_pass_still_recompacts_long_marked_text() {
        let mut m = mk_tool_msg(&format!(
            "…（已压缩 700 字符 · 原文未保留）…⟦c:1f3a9b2c⟧{}",
            "M".repeat(80)
        ));
        let (ok, _) = compact_one(&mut m, true, None, None).unwrap();
        assert!(ok, "长于保留窗口的已压文本仍应再压");
        let out = m["content"].as_str().unwrap();
        assert!(
            out.chars().count() < 160,
            "再压后应变短：{}",
            out.chars().count()
        );
    }

    #[test]
    fn compaction_skipped_when_placeholder_would_grow_text() {
        let dir = tmp_dir();
        // 330 字符：进得了候选（>300），但替身含真实 sidecar 路径后必比原文长
        let mut msgs = vec![mk_tool_msg(&"Z".repeat(330))];
        let report = soft_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        assert_eq!(report.tool_msgs_compacted, 0, "替身比原文长时应跳过");
        assert_eq!(report.sidecar_paths.len(), 0, "跳过时不应落盘孤儿文件");
        assert_eq!(msgs[0]["content"].as_str().unwrap(), "Z".repeat(330));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn soft_pass_skips_already_compacted() {
        let dir = tmp_dir();
        let mut msgs = vec![mk_tool_msg(&"W".repeat(800))];
        let first = soft_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        assert_eq!(first.tool_msgs_compacted, 1);
        let after_first = msgs[0]["content"].as_str().unwrap().to_string();
        // 第二轮软压缩：已压缩消息跳过——不二次落盘、不叠标记
        let second = soft_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        assert_eq!(second.tool_msgs_compacted, 0);
        assert_eq!(msgs[0]["content"].as_str().unwrap(), after_first);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "不写第二个 sidecar"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hard_recompact_preserves_sidecar_pointer() {
        let dir = tmp_dir();
        let mut msgs = vec![mk_tool_msg(&"V".repeat(1000))];
        soft_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        let soft_text = msgs[0]["content"].as_str().unwrap().to_string();
        let soft_pointer = extract_pointer(&soft_text).expect("软压缩替身应带指针");
        // 硬压缩再压：指针必须保留（原文还在盘上），且不新增 sidecar 文件
        let report = hard_compact(&mut msgs, Some(&dir), Some("s")).unwrap();
        assert_eq!(report.tool_msgs_compacted, 1);
        assert_eq!(report.sidecar_paths.len(), 0, "再压不写第二个 sidecar");
        let hard_text = msgs[0]["content"].as_str().unwrap();
        assert_eq!(
            extract_pointer(hard_text).as_deref(),
            Some(soft_pointer.as_str()),
            "硬压缩后指针应与软压缩一致"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
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
