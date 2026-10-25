//! 文件变更凭证面：unified diff / sha256 / before 快照 / 回执构建。

use super::*;

// 文件变更证据（Agent 透明化设计 §4.2）

/// 文件变更证据：文件类工具成功落盘后随 `ToolResult::file_changes` 回传，
/// dispatch据此落 `file_changes` 表 + emit `bot-file-changed`。
/// 纯数据结构——trace 归属字段（trace_id/span_id/created_at）由 dispatch 侧补。
#[derive(Debug, Clone)]
pub struct FileChangeReceipt {
    pub path: String,
    /// "create" | "modify"（现有文件工具不产 delete）
    pub kind: &'static str,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<String>,
    pub truncated: bool,
    /// before 全文快照文件名（`data_dir/checkpoints/<before_ref>`，uuid 发号）。
    /// None = 新建（无旧文可存）或快照写盘失败（降级，`checkpoint.write_fail` 审计）。
    pub before_ref: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    /// edit_file 的实际替换次数：Exact/WhitespaceTolerant = 1；regex = find_iter
    /// 计数（也是 replace_all 实际次数）。0 用于「试过但 0 处匹配」/「write_file 全
    /// 量写入」等场景——把「替换 1 处」与「替换 1 万处」/「新建 0 行」区分开
    /// （审计/receipt 原只记行数 delta，不区分
    /// 单点替换与全清替换）。
    pub replacements: usize,
}

/// unified diff 生成（纯函数可测）：similar 行级 diff → unified 文本 + ±行计数。
/// ±行计数是事实（不随截断丢失），diff 文本超 `MAX_DIFF_LINES` 行截断置位。
pub(crate) fn build_unified_diff(
    before: &str,
    after: &str,
    path: &str,
) -> (Option<String>, i64, i64, bool) {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut added = 0i64;
    let mut deleted = 0i64;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => deleted += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    if added == 0 && deleted == 0 {
        return (None, 0, 0, false);
    }
    let full = diff
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string();
    let lines: Vec<&str> = full.lines().collect();
    let truncated = lines.len() > crate::db::MAX_DIFF_LINES;
    let mut text = lines
        .iter()
        .take(crate::db::MAX_DIFF_LINES)
        .copied()
        .collect::<Vec<&str>>()
        .join("\n");
    if truncated {
        text.push_str(&format!(
            "\n…（diff 超过 {} 行已截断）",
            crate::db::MAX_DIFF_LINES
        ));
    }
    (Some(text), added, deleted, truncated)
}

/// sha256 十六进制（回滚证据链： 回滚前比对 after_sha 防文件漂移）
pub(crate) fn sha256_hex(s: &str) -> String {
    sha256_hex_bytes(s.as_bytes())
}

/// sha256 十六进制（字节入参）：二进制产物（docx/xlsx/pptx 是 zip 容器，不是
/// UTF-8 文本）只能按字节算哈希——按字符串算会先经过一次有损转换，哈希出来的
/// 值与文件实际内容对不上。
pub(crate) fn sha256_hex_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// before 全文快照（`data_dir/checkpoints/<uuid>`，db::atomic_write 原子写）。
/// 快照是回滚的 nice-to-have：失败降级 None + Error 审计，不阻断已成功的业务写盘
/// ——工具结果不能因观测面失败而翻转。
pub(super) fn write_before_snapshot(app: &AppHandle, before: &str) -> Option<String> {
    let dir = crate::db::paths::data_dir(app).join("checkpoints");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Error,
            "checkpoint.write_fail",
            &[("err", e.to_string())],
        );
        return None;
    }
    let name = uuid::Uuid::new_v4().to_string();
    match crate::db::atomic_write(&dir.join(&name), before) {
        Ok(()) => Some(name),
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Error,
                "checkpoint.write_fail",
                &[("err", e)],
            );
            None
        }
    }
}

/// 变更证据构建（纯函数可测；快照写盘由调用方另接 write_before_snapshot 回填 before_ref）：
/// create（before=None）→ 全 + 行 diff、无 before_sha；modify → before/after diff + before_sha。
/// `replacements` 0 用于「试过但 0 处匹配」/「全量写入」等场景——把「替换 1 处」
/// 与「替换 1 万处」/「新建 0 行」区分开。
pub(crate) fn build_file_change_receipt(
    path: &str,
    kind: &'static str,
    before: Option<&str>,
    after: &str,
    replacements: usize,
) -> FileChangeReceipt {
    let (diff, added, deleted, truncated) = match before {
        Some(b) => build_unified_diff(b, after, path),
        None => build_unified_diff("", after, path),
    };
    FileChangeReceipt {
        path: path.to_string(),
        kind,
        added,
        deleted,
        diff,
        truncated,
        before_ref: None,
        before_sha: before.map(sha256_hex),
        after_sha: Some(sha256_hex(after)),
        replacements,
    }
}

/// 二进制产物（docx/xlsx/pptx/pdf）的落盘凭证：路径 + 内容哈希，无行级 diff。
///
/// 这类文件是 zip 容器、不是 UTF-8 文本——按行 diff 只会产出乱码，所以这档只记
/// 「落在哪、是新建、内容哈希是多少」：执行痕迹面板靠它显示「新增了一个文档」，
/// 回滚的漂移闸靠 after_sha 核内容。行数两档恒 0，因为二进制没有行。
///
/// 读不出内容时**仍然给凭证**（after_sha=None）——凭证的**存在**才是「这个文件
/// 是工具产物」的凭据；缺了它，幻觉守卫会把模型真写出来的文档判成凭空声称。
pub(crate) fn build_binary_file_receipt(path: &str) -> FileChangeReceipt {
    let after_sha = std::fs::read(path).ok().map(|b| sha256_hex_bytes(&b));
    FileChangeReceipt {
        path: path.to_string(),
        kind: "create",
        added: 0,
        deleted: 0,
        diff: None,
        truncated: false,
        before_ref: None,
        before_sha: None,
        after_sha,
        replacements: 0,
    }
}
