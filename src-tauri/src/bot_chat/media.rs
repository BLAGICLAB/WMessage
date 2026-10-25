//! 图片附件面：base64 入模、扩展名白名单、附件索引。

use super::*;

/// 需要内联图片的消息下标：原先「最近两条 user 消息」
/// 永不失效，一张图每轮对话都重复 base64 重发。改为最后 3 条消息内的 user 消息——
/// 覆盖「发图 → 追问一轮」场景，更早的历史保持纯文本。
pub(super) fn image_attach_indices(messages: &[ChatMsg]) -> Vec<usize> {
    let from = messages.len().saturating_sub(3);
    (from..messages.len())
        .filter(|&i| messages[i].role == "user")
        .collect()
}

/// 解析 [附件文件] 块里的图片路径，读文件转 base64 data URL，附加为多模态消息内容。
/// 无图片附件时返回纯文本字符串（保持原格式）；非图片附件保持路径文本（模型用 extract_document 直读）。
///
/// 图片路径过白名单（桌面/下载/文档/图片 + AI_Gen_Files）——
/// 原先任意路径的图片都被读取并外发给 LLM API，[附件文件] 块若被污染（历史注入）即成外泄通道。
/// 校验失败的附件跳过并记审计（不打断聊天）。
pub(super) fn attach_images(app: &AppHandle, content: &str) -> serde_json::Value {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = crate::bot_fs::home_dir() {
        for d in ["Desktop", "Downloads", "Documents", "Pictures"] {
            roots.push(home.join(d));
        }
    }
    if let Ok(gen) = crate::db::gen_dir(app) {
        roots.push(gen);
    }
    let (v, skipped) = attach_images_in(&roots, content);
    if skipped > 0 {
        crate::bot::audit_log(
            app,
            &format!("attach_images.denied | {skipped} 张图片附件不在白名单目录，已跳过"),
        );
    }
    v
}

/// 白名单根目录集合内的判定内核（纯函数便于单测）：返回 (消息内容, 跳过数)
pub(super) fn attach_images_in(
    roots: &[std::path::PathBuf],
    content: &str,
) -> (serde_json::Value, usize) {
    let mut parts: Vec<serde_json::Value> = Vec::new();
    parts.push(serde_json::json!({"type": "text", "text": content}));
    let mut added = 0usize;
    let mut skipped = 0usize;
    for line in content.lines() {
        let Some(path) = line.strip_prefix("- ").map(str::trim) else {
            continue;
        };
        if added >= MAX_IMAGES_PER_MSG {
            break;
        }
        let p = std::path::Path::new(path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        // 白名单判定：canonical 双向比较（软链解析后落点必须在白名单根内）。
        // 校验与读取必须同一对象——读 canonical 落点而非原路径 p，
        // 消 check-then-read 之间 symlink 掉包窗（掉包则任意文件进 LLM prompt）
        let canon = std::fs::canonicalize(p).ok().filter(|c| {
            roots.iter().any(|r| {
                std::fs::canonicalize(r)
                    .map(|rc| c.starts_with(&rc))
                    .unwrap_or(false)
            })
        });
        let Some(canon) = canon else {
            skipped += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(&canon) else {
            continue;
        };
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            continue;
        }
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        };
        let b64 = B64.encode(&bytes);
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:{mime};base64,{b64}")}
        }));
        added += 1;
    }
    let v = if parts.len() == 1 {
        serde_json::json!(content)
    } else {
        serde_json::Value::Array(parts)
    };
    (v, skipped)
}

/// 单个图片文件 → OpenAI image_url content part（data URL）。
/// 工具图片回传（screenshot）与附件链路共用同上限/同 mime 表；
/// 不做白名单校验（调用方是工具自身产物，非用户不可信输入）。
/// 文件缺失/超限/非图片扩展名返回 None（调用方跳过，不阻断）。
pub(crate) fn image_part_from_file(p: &std::path::Path) -> Option<serde_json::Value> {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    let b64 = B64.encode(&bytes);
    Some(serde_json::json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime};base64,{b64}")}
    }))
}
