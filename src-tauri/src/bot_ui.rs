//! UI 自动化工具层（六件套）+ 安全闸。
//!
//! 六件套：`list_windows` / `activate_window` / `ui_inspect` / `ui_click` /
//! `ui_type` / `wait_for_element`。
//!
//! 四道闸（这活儿是替用户操作别的应用，权限面最大，必须层层收）：
//! 1. **总开关**：设置页没开「UI 自动化」时全部拒绝（flag 文件，`ui::ui_enabled`）；
//! 2. **窗口级授权**：每个动作都必须指定 window_id，且该窗口必须真实存在——
//!    不接受裸坐标、不接受「当前前台窗口」这种含糊目标；
//! 3. **危险动作确认**：点「发送 / 提交 / 支付 / 删除」这类按钮前弹窗请用户确认
//!    （60s 无响应默认拒绝）；
//! 4. **敏感控件禁打**：密码框一类控件不接受输入。
//!
//! 每次动作留痕：审计日志（工具 / 窗口 / 控件 / 动作）+ 前后各一张截图（落
//! AI_Gen_Files），截图路径写进工具结果，模型可据此核对动作是否真的生效。
//!
//! 定位（与 Playwright 的分工）：本模块管**页面之外**——原生应用（微信 / 飞书 /
//! Excel）、系统弹窗（下载框 / 支付确认 / 验证码）、接管已登录的窗口。为网站做
//! UI 自动化是降级（坐标点击不如元素定位），不做。

use std::path::PathBuf;

use tauri::AppHandle;

use crate::bot::registry::ToolResult;
use crate::ui::{UiNode, UiWindow};

/// 控件树最大深度（默认；超出即截断——深层树在原生应用里动辄几百节点）
const DEFAULT_MAX_DEPTH: usize = 6;
/// 控件树最多返回多少节点
const DEFAULT_MAX_NODES: usize = 200;
/// wait_for_element 默认等待上限
const DEFAULT_WAIT_MS: i64 = 5_000;
/// wait_for_element 等待上限（硬顶）
const MAX_WAIT_MS: i64 = 30_000;
/// 轮询间隔
const POLL_INTERVAL_MS: u64 = 300;

/// 危险动作关键词：命中即弹窗确认（中文界面为主，附英文常见词）
///
/// 匹配按小写做（英文界面按钮首字母大写），其中两字母以内的短词按整词匹配，
/// 见 `detail_is_dangerous`。
const DANGEROUS_KEYWORDS: &[&str] = &[
    "发送",
    "发表",
    "发布",
    "提交",
    "确定",
    "确认",
    "支付",
    "付款",
    "购买",
    "下单",
    "删除",
    "移除",
    "注销",
    "退出登录",
    "清空",
    "覆盖",
    "保存",
    "send",
    "submit",
    "confirm",
    "ok",
    "delete",
    "remove",
    "pay",
    "buy",
    "order",
];

/// 只按整词匹配的短词（子串匹配会误伤 Outlook / token 这类窗口名）
const DANGEROUS_WHOLE_WORDS: &[&str] = &["ok"];

/// 敏感控件关键词：命中即拒绝输入（密码 / 验证码 / 支付密码）
const SENSITIVE_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "密码",
    "验证码",
    "支付密码",
    "安全码",
    "cvv",
    "pin",
];

/// 开关闸：未开启直接拒绝
fn gate_enabled(app: &AppHandle) -> Option<ToolResult> {
    if crate::ui::ui_enabled(app) {
        return None;
    }
    Some(ToolResult::denied(
        "UI 自动化未开启：请到 设置页 打开「允许机器人操作桌面界面」后再用这些工具。",
    ))
}

/// 窗口闸：窗口必须真实存在（窗口级授权）
fn gate_window(window_id: &str) -> Result<UiWindow, ToolResult> {
    let windows = match crate::ui::list_windows() {
        Ok(w) => w,
        Err(e) => {
            return Err(ToolResult::error(e, Vec::new()));
        }
    };
    match windows.into_iter().find(|w| w.id == window_id) {
        Some(w) => Ok(w),
        None => Err(ToolResult::denied(format!(
            "窗口 {window_id} 不存在（可能已关闭）。先用 list_windows 重新取窗口列表。"
        ))),
    }
}

/// 危险动作闸：命中关键词就要用户确认
async fn gate_dangerous(
    app: &AppHandle,
    tool: &str,
    detail: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> Option<ToolResult> {
    if !detail_is_dangerous(detail) {
        return None;
    }
    let approved =
        crate::bot_slash::ask_user_confirm(app, tool, detail, interactive, session_id).await;
    if approved {
        return None;
    }
    Some(ToolResult::denied("用户拒绝了这次操作，未执行。"))
}

/// 危险动作闸的判定：detail 是否命中危险关键词
///
/// 英文 UI 的标准写法是首字母大写（"Send" / "Delete" / "OK"），关键词全是小写
/// 字面量时这些按钮一个都不拦——闸等于没设。所以匹配一律按小写做。
///
/// 但两字母以内的短词（"ok"）不能按子串找："Outlook" / "token" 这类窗口名里
/// 都含 "ok"，每次点击都要人确认，确认疲劳之后用户会闭眼点同意——闸又白设。
/// 短词因此只按整词匹配（词的边界按 ASCII 字母数字与下划线判定，中文不算词
/// 字符，「发送ok」这种照样能命中）。
fn detail_is_dangerous(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    DANGEROUS_KEYWORDS.iter().any(|k| {
        if DANGEROUS_WHOLE_WORDS.contains(k) {
            contains_word(&lower, k)
        } else {
            lower.contains(k)
        }
    })
}

/// 整词匹配：词元恰好等于该词（不做正则，避开 Unicode 词边界的坑）
fn contains_word(hay: &str, needle: &str) -> bool {
    hay.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w == needle)
}

/// ui_type 的文本闸：注入的换行等于按回车键
///
/// 微信 / 飞书 / Electron 这类应用收到回车就当场发送或提交——而本工具的承诺是
/// 「输入完不会自动发出去，发送是另一次 ui_click」。所以带换行的文本直接拒绝，
/// 不静默剥除：剥了模型会以为换行已经打进去，下一轮再发一次。
fn gate_no_line_break(text: &str) -> Option<ToolResult> {
    if text.contains('\n') || text.contains('\r') {
        return Some(ToolResult::denied(
            "ui_type 拒绝带换行符的文本：注入换行等于按回车键，目标应用会当场发送 / 提交，\
             而发送必须是另一次 ui_click。请去掉换行后重试（多行内容按行拆成多次 ui_type）。",
        ));
    }
    None
}

/// 敏感控件角色：这类角色本身就是密码框
///
/// macOS 的密码框角色是 AXSecureTextField，标题常是空的——只按「标题 / 标识里
/// 有没有『密码』这类词」判，等于把闸门交给应用作者的文案习惯。
const SENSITIVE_ROLES: &[&str] = &["AXSecureTextField"];

/// 敏感控件闸：密码 / 验证码一类控件不接受输入
fn gate_sensitive(node: &UiNode) -> Option<ToolResult> {
    // 角色与平台标志优先于文案：密码框的标题可能是空的
    let role = node.role.to_ascii_lowercase();
    if node.is_password || SENSITIVE_ROLES.iter().any(|r| role.eq_ignore_ascii_case(r)) {
        return Some(sensitive_denied(node));
    }
    let hay = format!(
        "{} {} {}",
        node.role,
        node.identifier.clone().unwrap_or_default(),
        node.title
    )
    .to_lowercase();
    if SENSITIVE_KEYWORDS.iter().any(|k| hay.contains(k)) {
        return Some(sensitive_denied(node));
    }
    None
}

/// 敏感控件拒绝文案（标题空时用角色，模型才知道拦的是哪个控件）
fn sensitive_denied(node: &UiNode) -> ToolResult {
    ToolResult::denied(format!(
        "拒绝操作：目标控件是敏感控件（{}），不代填密码 / 验证码类内容。",
        if node.title.is_empty() {
            &node.role
        } else {
            &node.title
        }
    ))
}

/// 目标解析：控件 id 直接用；否则按标题 / 角色 / 标识模糊匹配树里的控件
fn resolve_target(window_id: &str, target: &str, max_depth: usize) -> Result<UiNode, ToolResult> {
    let t = target.trim();
    if t.is_empty() {
        return Err(ToolResult::denied(
            "缺少 target 参数（控件 id 或控件标题关键词）",
        ));
    }
    // 形如 <pid>:<窗口序号>[.<路径>] 的控件 id：直接用
    if t.contains(':') && t.split(':').count() == 2 {
        let tree = match crate::ui::inspect_tree(window_id, max_depth, DEFAULT_MAX_NODES) {
            Ok(nodes) => nodes,
            Err(e) => return Err(ToolResult::error(e, Vec::new())),
        };
        if let Some(node) = tree.iter().find(|n| n.id == t) {
            return Ok(node.clone());
        }
        return Err(ToolResult::denied(format!(
            "控件 {t} 不在窗口 {window_id} 的当前界面里（界面可能已变，先 ui_inspect 重新看）"
        )));
    }
    // 否则按标题 / 角色 / 标识匹配
    let tree = match crate::ui::inspect_tree(window_id, max_depth, DEFAULT_MAX_NODES) {
        Ok(nodes) => nodes,
        Err(e) => return Err(ToolResult::error(e, Vec::new())),
    };
    let needle = t.to_lowercase();
    let hit = tree
        .iter()
        .find(|n| {
            n.title.to_lowercase().contains(&needle)
                || n.role.to_lowercase().contains(&needle)
                || n.identifier
                    .as_ref()
                    .map(|i| i.to_lowercase().contains(&needle))
                    .unwrap_or(false)
        })
        .cloned();
    match hit {
        Some(n) => Ok(n),
        None => Err(ToolResult::denied(format!(
            "窗口 {window_id} 里找不到匹配「{t}」的控件（先 ui_inspect 看看有哪些控件）"
        ))),
    }
}

/// 动作留痕截图（落 AI_Gen_Files；失败只记日志不阻断动作）
async fn trace_shot(app: &AppHandle, tag: &str) -> Option<PathBuf> {
    let dir = match crate::db::gen_dir(app) {
        Ok(d) => d,
        Err(_) => return None,
    };
    let ts = chrono::Utc::now().timestamp_millis();
    let path = dir.join(format!("wm-ui-{tag}-{ts}.png"));
    let p = path.clone();
    let ok = tauri::async_runtime::spawn_blocking(move || crate::bot_desktop::capture_to(&p))
        .await
        .unwrap_or(false);
    if ok {
        Some(path)
    } else {
        None
    }
}

// ── 工具 ───────────────────────────────────────────────────────────────────

/// list_windows：列出可接管的窗口
pub async fn tool_list_windows(app: &AppHandle, _args: &str) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    match crate::ui::list_windows() {
        Ok(windows) => {
            if windows.is_empty() {
                return ToolResult::ok(
                    "当前没有可接管的窗口（只列有标题的窗口；无标题面板 / 托盘窗不在清单里）"
                        .to_string(),
                    Vec::new(),
                );
            }
            let lines: Vec<String> = windows
                .iter()
                .map(|w| {
                    format!(
                        "- id: {} | 应用: {} | 标题: {}{}",
                        w.id,
                        w.app,
                        w.title,
                        if w.focused { "（前台）" } else { "" }
                    )
                })
                .collect();
            ToolResult::ok(
                format!("可接管窗口 {} 个：\n{}", windows.len(), lines.join("\n")),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::error(e, Vec::new()),
    }
}

/// activate_window：把窗口切到前台
pub async fn tool_activate_window(app: &AppHandle, args: &str) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    let v = crate::bot::parse_args(args);
    let window_id = v["windowId"].as_str().unwrap_or("").trim().to_string();
    if window_id.is_empty() {
        return ToolResult::denied(
            "activate_window 缺少 windowId 参数（先用 list_windows 拿窗口 id）",
        );
    }
    let w = match gate_window(&window_id) {
        Ok(w) => w,
        Err(t) => return t,
    };
    match crate::ui::activate_window(&window_id) {
        Ok(()) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "ui.activate | window: {window_id} | {}",
                    crate::bot::truncate_for_log(&w.title, 80)
                ),
            );
            ToolResult::ok(format!("已激活窗口：{}（{}）", w.title, w.app), Vec::new())
        }
        Err(e) => ToolResult::error(format!("激活窗口失败：{e}"), Vec::new()),
    }
}

/// ui_inspect：读窗口的控件树
pub async fn tool_ui_inspect(app: &AppHandle, args: &str) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    let v = crate::bot::parse_args(args);
    let window_id = v["windowId"].as_str().unwrap_or("").trim().to_string();
    if window_id.is_empty() {
        return ToolResult::denied("ui_inspect 缺少 windowId 参数（先用 list_windows 拿窗口 id）");
    }
    let w = match gate_window(&window_id) {
        Ok(w) => w,
        Err(t) => return t,
    };
    let max_depth = v["maxDepth"]
        .as_u64()
        .map(|d| (d as usize).min(DEFAULT_MAX_DEPTH))
        .unwrap_or(DEFAULT_MAX_DEPTH);
    let query = v["query"].as_str().unwrap_or("").trim().to_lowercase();
    let nodes = match crate::ui::inspect_tree(&window_id, max_depth, DEFAULT_MAX_NODES) {
        Ok(n) => n,
        Err(e) => return ToolResult::error(e, Vec::new()),
    };
    let filtered: Vec<&UiNode> = if query.is_empty() {
        nodes.iter().collect()
    } else {
        nodes
            .iter()
            .filter(|n| {
                n.title.to_lowercase().contains(&query)
                    || n.role.to_lowercase().contains(&query)
                    || n.identifier
                        .as_ref()
                        .map(|i| i.to_lowercase().contains(&query))
                        .unwrap_or(false)
                    || n.value
                        .as_ref()
                        .map(|v| v.to_lowercase().contains(&query))
                        .unwrap_or(false)
            })
            .collect()
    };
    if filtered.is_empty() {
        return ToolResult::ok(
            format!(
                "窗口 {}（{}）里没有匹配「{query}」的控件（共读到 {} 个控件）",
                w.title,
                w.app,
                nodes.len()
            ),
            Vec::new(),
        );
    }
    let lines: Vec<String> = filtered
        .iter()
        .take(80)
        .map(|n| {
            let mut s = format!(
                "- id: {} | 角色: {} | 标题: {}",
                n.id,
                if n.role.is_empty() { "?" } else { &n.role },
                if n.title.is_empty() { "-" } else { &n.title }
            );
            if let Some(v) = &n.value {
                if !v.is_empty() {
                    s.push_str(&format!(" | 值: {v}"));
                }
            }
            if n.enabled == Some(false) {
                s.push_str(" | 不可用");
            }
            if n.actionable {
                s.push_str(" | 可直接触发");
            }
            s
        })
        .collect();
    ToolResult::ok(
        format!(
            "窗口 {}（{}）里匹配到 {} 个控件：\n{}",
            w.title,
            w.app,
            filtered.len(),
            lines.join("\n")
        ),
        Vec::new(),
    )
}

/// ui_click：点击窗口里的控件
pub async fn tool_ui_click(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    let v = crate::bot::parse_args(args);
    let window_id = v["windowId"].as_str().unwrap_or("").trim().to_string();
    if window_id.is_empty() {
        return ToolResult::denied("ui_click 缺少 windowId 参数（先用 list_windows 拿窗口 id）");
    }
    let target = v["target"].as_str().unwrap_or("").trim().to_string();
    let w = match gate_window(&window_id) {
        Ok(w) => w,
        Err(t) => return t,
    };
    let node = match resolve_target(&window_id, &target, DEFAULT_MAX_DEPTH) {
        Ok(n) => n,
        Err(t) => return t,
    };
    // 危险动作闸：点「发送 / 提交 / 支付 / 删除」这类按钮前要用户确认
    let detail = format!(
        "在「{}」（{}）里点击「{}」",
        w.title,
        w.app,
        if node.title.is_empty() {
            &node.role
        } else {
            &node.title
        }
    );
    if let Some(t) = gate_dangerous(app, "ui_click", &detail, interactive, session_id).await {
        return t;
    }
    let before = trace_shot(app, "before").await;
    let click_res = tauri::async_runtime::spawn_blocking({
        let node_id = node.id.clone();
        move || crate::ui::press_node(&node_id)
    })
    .await
    .unwrap_or_else(|e| Err(format!("点击线程失败：{e}")));
    match click_res {
        Ok(()) => {
            let after = trace_shot(app, "after").await;
            crate::bot::audit_log(
                app,
                &format!(
                    "ui.click | window: {window_id} | node: {} | title: {}",
                    node.id,
                    crate::bot::truncate_for_log(&node.title, 60)
                ),
            );
            let mut text = format!(
                "已点击「{}」（控件 {}）",
                if node.title.is_empty() {
                    &node.role
                } else {
                    &node.title
                },
                node.id
            );
            if let Some(b) = &before {
                text.push_str(&format!("\n动作前截图：{}", b.display()));
            }
            if let Some(a) = &after {
                text.push_str(&format!("\n动作后截图：{}", a.display()));
            }
            ToolResult::ok(text, Vec::new())
        }
        Err(e) => ToolResult::error(format!("点击失败：{e}"), Vec::new()),
    }
}

/// ui_type：往窗口里的输入框输入文本
pub async fn tool_ui_type(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    let v = crate::bot::parse_args(args);
    let window_id = v["windowId"].as_str().unwrap_or("").trim().to_string();
    if window_id.is_empty() {
        return ToolResult::denied("ui_type 缺少 windowId 参数（先用 list_windows 拿窗口 id）");
    }
    let target = v["target"].as_str().unwrap_or("").trim().to_string();
    let text = v["text"].as_str().unwrap_or("").to_string();
    if text.is_empty() {
        return ToolResult::denied("ui_type 缺少 text 参数（要输入的内容）");
    }
    // 文本闸：注入换行等于按回车键，目标应用会当场发送 / 提交
    if let Some(t) = gate_no_line_break(&text) {
        return t;
    }
    let w = match gate_window(&window_id) {
        Ok(w) => w,
        Err(t) => return t,
    };
    let node = match resolve_target(&window_id, &target, DEFAULT_MAX_DEPTH) {
        Ok(n) => n,
        Err(t) => return t,
    };
    // 敏感控件闸
    if let Some(t) = gate_sensitive(&node) {
        return t;
    }
    // 危险动作闸：往「发送 / 提交」框里打字同样要确认（点了发送就是发出去）
    let detail = format!(
        "在「{}」（{}）的「{}」里输入：{}",
        w.title,
        w.app,
        if node.title.is_empty() {
            &node.role
        } else {
            &node.title
        },
        crate::bot::truncate_for_log(&text, 60)
    );
    if let Some(t) = gate_dangerous(app, "ui_type", &detail, interactive, session_id).await {
        return t;
    }
    let before = trace_shot(app, "before").await;
    let node_id = node.id.clone();
    let typed = text.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::ui::focus_node(&node_id)?;
        crate::ui::type_text(&typed)
    })
    .await
    .unwrap_or_else(|e| Err(format!("输入线程失败：{e}")));
    match res {
        Ok(()) => {
            let after = trace_shot(app, "after").await;
            crate::bot::audit_log(
                app,
                &format!(
                    "ui.type | window: {window_id} | node: {} | {} chars",
                    node.id,
                    text.chars().count()
                ),
            );
            let mut out = format!(
                "已输入 {} 个字符到「{}」（控件 {}）",
                text.chars().count(),
                if node.title.is_empty() {
                    &node.role
                } else {
                    &node.title
                },
                node.id
            );
            if let Some(b) = &before {
                out.push_str(&format!("\n动作前截图：{}", b.display()));
            }
            if let Some(a) = &after {
                out.push_str(&format!("\n动作后截图：{}", a.display()));
            }
            ToolResult::ok(out, Vec::new())
        }
        Err(e) => ToolResult::error(format!("输入失败：{e}"), Vec::new()),
    }
}

/// wait_for_element：等某个控件出现
pub async fn tool_wait_for_element(app: &AppHandle, args: &str) -> ToolResult {
    if let Some(t) = gate_enabled(app) {
        return t;
    }
    let v = crate::bot::parse_args(args);
    let window_id = v["windowId"].as_str().unwrap_or("").trim().to_string();
    if window_id.is_empty() {
        return ToolResult::denied(
            "wait_for_element 缺少 windowId 参数（先用 list_windows 拿窗口 id）",
        );
    }
    let query = v["query"].as_str().unwrap_or("").trim().to_lowercase();
    if query.is_empty() {
        return ToolResult::denied(
            "wait_for_element 缺少 query 参数（要找的控件标题 / 角色关键词）",
        );
    }
    let timeout_ms = v["timeoutMs"]
        .as_i64()
        .unwrap_or(DEFAULT_WAIT_MS)
        .clamp(0, MAX_WAIT_MS);
    let w = match gate_window(&window_id) {
        Ok(w) => w,
        Err(t) => return t,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    loop {
        let nodes = crate::ui::inspect_tree(&window_id, DEFAULT_MAX_DEPTH, DEFAULT_MAX_NODES)
            .unwrap_or_default();
        if let Some(node) = nodes.iter().find(|n| {
            n.title.to_lowercase().contains(&query)
                || n.role.to_lowercase().contains(&query)
                || n.identifier
                    .as_ref()
                    .map(|i| i.to_lowercase().contains(&query))
                    .unwrap_or(false)
        }) {
            return ToolResult::ok(
                format!(
                    "已找到控件「{}」（id: {}，角色 {}），出现在「{}」（{}）",
                    if node.title.is_empty() {
                        &node.role
                    } else {
                        &node.title
                    },
                    node.id,
                    node.role,
                    w.title,
                    w.app
                ),
                Vec::new(),
            );
        }
        if std::time::Instant::now() >= deadline {
            return ToolResult::error(
                format!(
                    "等待 {:?} 结束：窗口「{}」（{}）里始终没出现匹配「{}」的控件",
                    std::time::Duration::from_millis(timeout_ms as u64),
                    w.title,
                    w.app,
                    query
                ),
                Vec::new(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 危险关键词闸：发送 / 提交类要拦，普通的不要拦
    #[test]
    fn dangerous_keywords_gate_only_hits_risky_actions() {
        assert!(DANGEROUS_KEYWORDS.iter().any(|k| "发送".contains(k)));
        assert!(DANGEROUS_KEYWORDS.iter().any(|k| "提交订单".contains(k)));
        assert!(!DANGEROUS_KEYWORDS.iter().any(|k| "搜索".contains(k)));
        assert!(!DANGEROUS_KEYWORDS.iter().any(|k| "查看图片".contains(k)));
    }

    /// 英文界面的按钮标题同样要命中危险闸
    ///
    /// 英文 UI 的标准写法是首字母大写（"Send" / "OK" / "Delete"），
    /// 关键词全是小写字面量时这些按钮一个都不拦——闸等于没设。
    #[test]
    fn english_button_titles_must_hit_the_dangerous_gate() {
        assert!(
            detail_is_dangerous("在「Mail」（Mail）里点击「Send」"),
            "英文按钮标题 Send 必须触发确认闸"
        );
        assert!(
            detail_is_dangerous("在「GitHub」里点击「Delete」"),
            "英文按钮标题 Delete 必须触发确认闸"
        );
        assert!(
            detail_is_dangerous("在「设置」里点击「确定」"),
            "中文按钮标题仍要触发确认闸"
        );
        assert!(
            !detail_is_dangerous("在「搜索框」里输入：hello"),
            "普通输入不该触发确认闸"
        );
    }

    /// 短词 "ok" 只按整词匹配：Outlook 这类窗口名不该让每次点击都要人确认
    #[test]
    fn short_keyword_ok_must_not_trip_on_window_names() {
        assert!(
            !detail_is_dangerous("在「Outlook」（Microsoft Outlook）里点击「下一封」"),
            "Outlook 不该被当成 OK 按钮"
        );
        assert!(
            !detail_is_dangerous("在「Token 窗口」里点击「复制」"),
            "token 不该被当成 OK 按钮"
        );
        assert!(
            detail_is_dangerous("在「设置」里点击「OK」"),
            "OK 按钮本身仍要触发确认闸"
        );
    }

    /// ui_type 不接受带换行的文本：注入换行等于按回车键，会当场发出去
    #[test]
    fn ui_type_rejects_text_carrying_line_breaks() {
        assert!(
            gate_no_line_break("早上好\n").is_some(),
            "带换行的文本必须被拒绝（否则回车键把消息直接发出去）"
        );
        assert!(
            gate_no_line_break("早上好\r\n").is_some(),
            "CRLF 同样要拒绝"
        );
        assert!(
            gate_no_line_break("早上好\r").is_some(),
            "单独的 CR 也是回车键"
        );
        assert!(
            gate_no_line_break("早上好").is_none(),
            "普通单行文本不该被拦"
        );
    }

    /// 敏感控件闸：密码 / 验证码类角色要拦
    #[test]
    fn sensitive_gate_hits_password_fields() {
        let node = UiNode {
            id: "1:0.0".into(),
            role: "AXSecureTextField".into(),
            title: "密码".into(),
            description: String::new(),
            value: None,
            identifier: None,
            enabled: Some(true),
            position: None,
            size: None,
            depth: 0,
            actionable: false,
            is_password: false,
        };
        assert!(gate_sensitive(&node).is_some(), "密码框必须拒绝");
        let normal = UiNode {
            role: "AXTextField".into(),
            title: "搜索框".into(),
            ..node
        };
        assert!(gate_sensitive(&normal).is_none(), "普通输入框不该拦");
    }

    /// 敏感控件闸：角色是密码框、标题为空时也要拦
    ///
    /// 只按文案判的旧实现在这里会放行——macOS 的 AXSecureTextField 标题常是空的，
    /// 于是「密码框可被代填」。
    #[test]
    fn sensitive_gate_blocks_password_role_without_a_title() {
        let node = UiNode {
            id: "1:0.0".into(),
            role: "AXSecureTextField".into(),
            title: String::new(),
            description: String::new(),
            value: None,
            identifier: None,
            enabled: Some(true),
            position: None,
            size: None,
            depth: 0,
            actionable: false,
            is_password: false,
        };
        assert!(
            gate_sensitive(&node).is_some(),
            "AXSecureTextField 必须拒绝（标题为空时更不能放行）"
        );
    }

    /// 敏感控件闸：Windows UIA 报 IsPassword 的控件也要拦
    ///
    /// UIA 的密码框在本地化环境里角色只是普通「编辑」，Name 也可能是空的——
    /// 靠角色文案判会漏，只能看 IsPassword 属性。
    #[test]
    fn sensitive_gate_blocks_uia_password_flag() {
        let node = UiNode {
            id: "1:0.1".into(),
            role: "编辑".into(),
            title: String::new(),
            description: String::new(),
            value: None,
            identifier: None,
            enabled: Some(true),
            position: None,
            size: None,
            depth: 0,
            actionable: false,
            is_password: true,
        };
        assert!(
            gate_sensitive(&node).is_some(),
            "UIA 报 IsPassword 的控件必须拒绝"
        );
    }

    /// 接线守卫：六件套必须真的接进工具表
    #[test]
    fn ui_tools_are_wired_into_registry() {
        let src = include_str!("bot/registry.rs");
        for name in [
            "list_windows",
            "activate_window",
            "ui_inspect",
            "ui_click",
            "ui_type",
            "wait_for_element",
        ] {
            assert!(
                src.contains(&format!("name: \"{name}\"")),
                "{name} 必须登记进 TOOLS_TABLE"
            );
        }
    }
}
