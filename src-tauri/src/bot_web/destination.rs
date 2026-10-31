//! 目的地闸：模型选定的出网目的地在放行前必须过这里。
//!
//! 与公网闸（fetch.rs 的 check_public_url，挡环回/私网/链路本地）是两道独立的闸：
//! 公网闸管「这个地址是不是公网」，目的地闸管「这个公网是不是用户知情过的」。
//!
//! 免弹窗的放行判定源只有三类：
//! 1. 产品自有出口——搜索 API、渲染代理、技能安装源（固定清单；用户装应用、
//!    装 skill 时已知情这批域名，再弹窗是噪声）
//! 2. 用户配置——allowed_domains 白名单、provider base_url、MCP http 端点
//! 3. 会话内授权——host × 会话粒度，不落盘；任何配置写入即整体失效
//!
//! 弹窗只发生在 ask 档且三类都未命中时，批准记入会话授权（同会话同 host 不再问，
//! 不做「始终允许」——持久信任只有设置页白名单一条路，授权面不积累第二套账）。
//! 子 agent 会话（后台会话，弹窗无人认领）与后台执行（interactive=false，无人值守）
//! 按 strict 处理：白名单外且无会话授权即拒——会话授权是「用户点过头的凭据」，
//! 命中即放行（含无人值守），其余白名单外一律拒。这是 fail-closed，不是可用性缺陷。

use url::Url;

use crate::bot::PermMode;

/// 产品自有出口固定清单。
const PRODUCT_EGRESS_HOSTS: &[&str] = &[
    "api.tavily.com",
    "api.search.brave.com",
    "r.jina.ai",
    "github.com",
    "codeload.github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "skillhub.cn",
];

/// 判定中间态：`classify` 的产物，弹窗由 `check_destination` 在外层完成——
/// 判定链是纯函数可离线钉住，弹窗是 IO 不进判定主体。
#[derive(Debug, PartialEq, Eq)]
enum Classified {
    Allowed(&'static str),
    Denied { dest: &'static str, reason: String },
    NeedConfirm(String),
}

/// 最终判定结果。dest 是审计用的放行/拒绝来源，事件由调用方统一记
/// （destination 不重复记审计，避免同一动作两份事件）。
#[derive(Debug, PartialEq, Eq)]
pub enum DestOutcome {
    /// dest ∈ product / whitelist / session / asked / yolo
    Allowed { dest: &'static str },
    /// dest ∈ strict / subagent / unattended / confirm_denied
    Denied { dest: &'static str, reason: String },
}

/// 会话授权缓存：host × 会话。gen 是配置世代——任何配置写入都 bump 它并清空
/// 授权（授权不能活得比它的语境久：用户删掉白名单里的域名后，靠弹窗拿到的
/// 会话授权也不该继续有效）。条目量级 = 会话数 × 授权域数，线性扫足够。
struct Grants {
    gen: u64,
    entries: Vec<(String, String)>,
}

static GRANTS: std::sync::Mutex<Grants> = std::sync::Mutex::new(Grants {
    gen: 0,
    entries: Vec::new(),
});

/// 配置写入侧（白名单 / provider / MCP 任一变更）调用。
pub fn invalidate_grants() {
    if let Ok(mut g) = GRANTS.lock() {
        g.gen += 1;
        g.entries.clear();
    }
}

fn grant_session(session_id: &str, host: &str) {
    if let Ok(mut g) = GRANTS.lock() {
        g.entries.push((session_id.to_string(), host.to_string()));
    }
}

fn grants_contain(session_id: &str, host: &str) -> bool {
    GRANTS
        .lock()
        .map(|g| g.entries.iter().any(|e| e.0 == session_id && e.1 == host))
        .unwrap_or(false)
}

/// 条目归一：转小写、IDN 转 punycode（与 url::Url 的 host 形态一致），
/// `*.suffix` 保留通配前缀。parse 不出来的条目不参与匹配（配置侧不拦写入，
/// 无效条目在这里静默失效——写入侧拦会把「编辑到一半」变成报错噪声）。
fn normalize_entry(entry: &str) -> Option<String> {
    let e = entry.trim().to_ascii_lowercase();
    let (wild, rest) = match e.strip_prefix("*.") {
        Some(r) => (true, r.to_string()),
        None => (false, e),
    };
    if rest.is_empty() {
        return None;
    }
    let host = Url::parse(&format!("https://{rest}/"))
        .ok()?
        .host_str()?
        .to_string();
    Some(if wild { format!("*.{host}") } else { host })
}

/// 条目匹配：精确，或 `*.suffix`（host 以 `.suffix` 结尾且不等于 suffix 本身——
/// 裸 suffix 不匹配防止 `*.example.com` 意外放行 apex 域）。
fn host_matches(entry: &str, host: &str) -> bool {
    let Some(entry) = normalize_entry(entry) else {
        return false;
    };
    match entry.strip_prefix("*.") {
        Some(suffix) => host != suffix && host.ends_with(&format!(".{suffix}")),
        None => host == entry,
    }
}

/// 用户配置过的信任域：白名单 + provider base_url + MCP http 端点。
fn user_configured_hosts(app: &tauri::AppHandle) -> Vec<String> {
    let cfg = crate::bot::config::io::load_config(app);
    let mut hosts: Vec<String> = cfg
        .allowed_domains
        .unwrap_or_default()
        .iter()
        .filter_map(|e| normalize_entry(e))
        .collect();
    if let Some(h) = Url::parse(&cfg.base_url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
    {
        hosts.push(h.to_ascii_lowercase());
    }
    for s in cfg.mcp_servers.unwrap_or_default() {
        if let Some(h) = s
            .url
            .as_deref()
            .and_then(|u| Url::parse(u).ok())
            .and_then(|u| u.host_str().map(|s| s.to_string()))
        {
            hosts.push(h.to_ascii_lowercase());
        }
    }
    hosts
}

/// 判定主体（纯函数）：参数由调用方收集，弹窗不在其中。
/// 判定顺序即信任顺序：产品出口 → 用户配置 → 会话授权 → 上下文硬分支 → perm 三档。
fn classify(
    host: &str,
    user_hosts: &[String],
    granted: bool,
    subagent: bool,
    interactive: bool,
    perm: PermMode,
) -> Classified {
    if PRODUCT_EGRESS_HOSTS.contains(&host) {
        return Classified::Allowed("product");
    }
    if user_hosts.iter().any(|e| host_matches(e, host)) {
        return Classified::Allowed("whitelist");
    }
    if granted {
        return Classified::Allowed("session");
    }
    // 子 agent 是后台会话：弹窗无人认领（停滞 600s 才被接替），一律 strict。
    if subagent {
        return Classified::Denied {
            dest: "subagent",
            reason: format!(
                "子 agent 会话不放行白名单外目的地（{host}）；请主 agent 请求用户在设置页加入白名单"
            ),
        };
    }
    // 后台执行（无人值守）不弹窗：等一个没人点的确认窗只是浪费 60s。
    if !interactive {
        return Classified::Denied {
            dest: "unattended",
            reason: format!(
                "后台执行不放行白名单外目的地（{host}）；请用户在设置页加入白名单后重试"
            ),
        };
    }
    match perm {
        PermMode::Yolo => Classified::Allowed("yolo"),
        PermMode::Strict => Classified::Denied {
            dest: "strict",
            reason: format!("strict 模式不放行白名单外目的地（{host}）；可在设置页加入白名单"),
        },
        // auto 档在目的地维度没有「白名单内自动」可言（上面已判过白名单），
        // 与 ask 同分支——和 bot_fs 的降级口径一致。
        PermMode::Ask | PermMode::Auto => Classified::NeedConfirm(host.to_string()),
    }
}

/// 目的地闸主入口。`host` 取自 `url::Url::host_str()`——它已是 punycode ascii
/// （WHATWG 规范行为），unicode 同形域在这里天然无法伪装成白名单条目。
pub async fn check_destination(
    app: &tauri::AppHandle,
    tool: &str,
    host: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> DestOutcome {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return DestOutcome::Denied {
            dest: "strict",
            reason: "URL 无主机名".to_string(),
        };
    }
    let granted = session_id
        .map(|sid| grants_contain(sid, &host))
        .unwrap_or(false);
    let classified = classify(
        &host,
        &user_configured_hosts(app),
        granted,
        crate::tool_guard::subagent_ctx(session_id).is_some(),
        interactive,
        crate::bot::perm_mode(app),
    );
    match classified {
        Classified::Allowed(dest) => DestOutcome::Allowed { dest },
        Classified::Denied { dest, reason } => DestOutcome::Denied { dest, reason },
        Classified::NeedConfirm(h) => {
            let detail =
                format!("访问域名 {h}（批准仅本会话内有效；长期信任请在设置页加入白名单）");
            if crate::bot_slash::ask_user_confirm(app, tool, &detail, interactive, session_id).await
            {
                if let Some(sid) = session_id {
                    grant_session(sid, &h);
                }
                DestOutcome::Allowed { dest: "asked" }
            } else {
                DestOutcome::Denied {
                    dest: "confirm_denied",
                    reason: format!("用户未批准访问 {h}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锚点：白名单匹配三形态 + 无效条目静默失效。
    #[test]
    fn host_entry_matching() {
        assert!(
            host_matches("api.example.com", "api.example.com"),
            "精确命中"
        );
        assert!(
            host_matches("*.example.com", "a.example.com"),
            "通配命中子域"
        );
        assert!(
            host_matches("*.example.com", "a.b.example.com"),
            "通配命中深层子域"
        );
        assert!(
            !host_matches("*.example.com", "example.com"),
            "裸 apex 不放行"
        );
        assert!(
            !host_matches("*.example.com", "notexample.com"),
            "后缀伪装不命中"
        );
        assert!(
            !host_matches("bad entry!", "x.com"),
            "parse 不出的条目不参与匹配"
        );
        assert!(!host_matches("", "x.com"), "空条目不参与匹配");
        // 大小写与空白是输入噪声，不是语义
        assert!(host_matches("  API.Example.COM  ", "api.example.com"));
    }

    /// 锚点：IDN 条目归一成 punycode 后与 url crate 的 host 形态可比。
    #[test]
    fn idn_entry_normalizes_to_punycode() {
        let entry = "中文.example.com";
        let normalized = normalize_entry(entry).expect("IDN 条目应能归一");
        let host = Url::parse("https://中文.example.com/")
            .expect("url crate 可解析 IDN")
            .host_str()
            .expect("有 host")
            .to_string();
        assert!(
            host_matches(&normalized, &host),
            "归一后的条目必须匹配 url crate 给出的 punycode host"
        );
    }

    /// 锚点：产品自有出口不经弹窗直接放行。
    #[test]
    fn product_egress_bypasses_user_gate() {
        let out = classify("r.jina.ai", &[], false, false, true, PermMode::Strict);
        assert_eq!(out, Classified::Allowed("product"));
    }

    /// 锚点：白名单命中放行；未命中 strict 拒；未命中 ask 走确认。
    #[test]
    fn whitelist_hit_and_strict_deny_and_ask() {
        let wl = vec!["api.example.com".to_string()];
        assert_eq!(
            classify("api.example.com", &wl, false, false, true, PermMode::Strict),
            Classified::Allowed("whitelist")
        );
        assert_eq!(
            classify("other.com", &wl, false, false, true, PermMode::Strict),
            Classified::Denied {
                dest: "strict",
                reason: "strict 模式不放行白名单外目的地（other.com）；可在设置页加入白名单"
                    .to_string()
            }
        );
        assert_eq!(
            classify("other.com", &wl, false, false, true, PermMode::Ask),
            Classified::NeedConfirm("other.com".to_string())
        );
        // auto 档白名单外与 ask 同分支
        assert_eq!(
            classify("other.com", &wl, false, false, true, PermMode::Auto),
            Classified::NeedConfirm("other.com".to_string())
        );
    }

    /// 锚点：子 agent 上下文白名单外直接拒，无弹窗等待。
    #[test]
    fn subagent_context_is_strict() {
        let out = classify("other.com", &[], false, true, true, PermMode::Yolo);
        match out {
            Classified::Denied { dest, .. } => assert_eq!(dest, "subagent"),
            other => panic!("子 agent 白名单外必须拒，得到 {other:?}"),
        }
        // 白名单内照常放行（子 agent 档的一站式取数靠白名单）
        assert_eq!(
            classify(
                "api.example.com",
                &["api.example.com".to_string()],
                false,
                true,
                true,
                PermMode::Strict
            ),
            Classified::Allowed("whitelist")
        );
    }

    /// 锚点：无人值守（interactive=false）白名单外直接拒，不等 60s 弹窗。
    #[test]
    fn unattended_denies_without_confirm_wait() {
        let out = classify("other.com", &[], false, false, false, PermMode::Ask);
        match out {
            Classified::Denied { dest, .. } => assert_eq!(dest, "unattended"),
            other => panic!("无人值守白名单外必须直接拒，得到 {other:?}"),
        }
        // 会话授权命中时无人值守也放行（授权就是「用户点过头的凭据」）
        assert_eq!(
            classify("other.com", &[], true, false, false, PermMode::Ask),
            Classified::Allowed("session")
        );
    }

    /// 锚点：yolo 放行；会话授权命中放行；invalidate 后失效。
    #[test]
    fn yolo_and_session_grant_lifecycle() {
        assert_eq!(
            classify("other.com", &[], false, false, true, PermMode::Yolo),
            Classified::Allowed("yolo")
        );
        grant_session("sess-1", "other.com");
        assert!(
            grants_contain("sess-1", "other.com"),
            "授权后同会话同 host 命中"
        );
        assert!(!grants_contain("sess-2", "other.com"), "跨会话不共享授权");
        assert_eq!(
            classify("other.com", &[], true, false, true, PermMode::Ask),
            Classified::Allowed("session")
        );
        invalidate_grants();
        assert!(
            !grants_contain("sess-1", "other.com"),
            "配置写入后授权整体失效"
        );
    }
}
