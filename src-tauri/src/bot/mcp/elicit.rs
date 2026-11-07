//! MCP elicitation（服务器向用户反问）桥接（差距收编方案 D-7）。
//!
//! 设计（方案 §五）：
//! - per-server 开关 `allowElicitation` **默认 false**——默认行为与 rmcp 的
//!   默认自动 Decline 完全一致（零变化），打开才走桥接。
//! - v1 只支持**单字段** form elicitation（string/number/integer/boolean/enum
//!   单选）；多字段表单与 URL 模式登记不做（Decline 并说明）。
//! - 等待 60s（对齐 ConfirmMap 默认拒节奏；不等 ask 的 24h——等待发生在
//!   工具调用内，长等占住 per-server 串行锁）。主窗口不可见立即 Decline
//!   （与 ask_confirm_inner 同判定）。
//! - 安全：应答值不进审计/日志（可能含凭据）；来源 server 名拼进问题文本
//!   （不可信内容按纯文本渲染）；字段值钳 2KB。
//!
//! 与 ask 卡的关系：复用 `bot-ask` 事件与 AskCard 渲染（payload 带
//! `source: "mcp_elicit"`），应答走独立命令 `elicit_respond`（不落 wfq
//! 通知表、不进 ask_user 预算/假设兜底——那是主循环 ask_user 工具的语义）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction, ErrorData as McpError};
use serde_json::Value;

/// 等待应答超时（秒）：对齐 ConfirmMap 60s 默认拒节奏
const ELICIT_TIMEOUT_SECS: u64 = 60;
/// 单字段值长度上限（UTF-8 字节）
const ELICIT_VALUE_MAX_BYTES: usize = 2 * 1024;
/// 枚举选项数上限（AskCard 按钮排布的上限口径）
const ELICIT_OPTIONS_MAX: usize = 4;

/// McpManager 无 app 字段（spawn_service 只有 &self）——进程级注册一次，
/// 首个 MCP 入口（工具调用/发现）写入；未注册时 elicitation 走默认 Decline。
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// 首个 MCP 入口注册 app（幂等，后写不覆盖先写）
pub fn set_app(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// 在途工具调用登记：server_id → (session_id, server 显示名)。
/// `call_locks` 已保证同 server 串行——每个 server 至多一条在途，登记天然单条。
/// elicitation 到达时反查会话归属（发事件按会话盖章）。
fn active_calls() -> &'static Mutex<HashMap<String, (String, String)>> {
    static MAP: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// call_tool 入口登记（session_id = 会话归属；server_name = 卡面来源标识）
pub fn register_active_call(server_id: &str, session_id: &str, server_name: &str) {
    active_calls()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            server_id.to_string(),
            (session_id.to_string(), server_name.to_string()),
        );
}

/// call_tool 收尾注销（retire 前后皆可，幂等）
pub fn unregister_active_call(server_id: &str) {
    active_calls()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(server_id);
}

/// 应答 waiter：qid → oneshot 回传用户输入文本
fn waiters() -> &'static Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>> {
    static MAP: OnceLock<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>> =
        OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// rmcp ClientHandler 的 elicitation 入口（manager spawn_service 按开关注入）
pub async fn handle_elicitation(
    server_id: &str,
    request: ElicitRequestParams,
) -> Result<ElicitResult, McpError> {
    let (session_id, server_name) = active_calls()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(server_id)
        .cloned()
        .ok_or_else(|| decline("没有在途工具调用关联本请求（SEP-2260 关联校验兜底）"))?;
    let msg = match request {
        ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => (message, requested_schema),
        // ElicitRequestParams 标注 #[non_exhaustive]：未来新增模式一律 Decline
        _ => {
            return Err(decline("URL 模式 elicitation 暂不支持"));
        }
    };
    let (message, schema) = msg;
    // 序列化成 JSON 后按通用形状判型——绕开 rmcp 各 Primitive 类型 API 的琐碎
    let props: std::collections::BTreeMap<String, Value> = {
        let raw = serde_json::to_value(&schema)
            .map_err(|e| decline(&format!("schema 序列化失败：{e}")))?;
        raw.get("properties")
            .and_then(|p| p.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .ok_or_else(|| decline("schema 缺 properties"))?
    };
    if props.len() != 1 {
        return Err(decline(&format!(
            "暂只支持单字段表单（收到 {} 个字段）",
            props.len()
        )));
    }
    let (field, def) = props.into_iter().next().expect("len==1 已判");
    let ftype = def
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let enum_values: Vec<String> = def
        .get("enum")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let field_label = def
        .get("title")
        .or_else(|| def.get("description"))
        .and_then(|t| t.as_str())
        .unwrap_or(field.as_str())
        .to_string();

    // 类型白名单：string / number / integer / boolean / enum（string+enum 数组）
    let known = matches!(ftype.as_str(), "string" | "number" | "integer" | "boolean")
        || (ftype == "string" && !enum_values.is_empty());
    if !known {
        return Err(decline(&format!("不支持的字段类型：{ftype}")));
    }
    if !enum_values.is_empty() && enum_values.len() > ELICIT_OPTIONS_MAX {
        return Err(decline(&format!(
            "枚举选项超过 {ELICIT_OPTIONS_MAX} 个，暂不支持"
        )));
    }

    let app = APP
        .get()
        .ok_or_else(|| decline("app 未注册（早期发现阶段不桥接）"))?;
    use tauri::Manager as _;
    // 主窗口不可见 → 没人能答，白等只会占住 per-server 串行锁（与 ask_confirm_inner 同判定）
    let main_visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !main_visible {
        return Err(decline("主窗口不可见，无人应答"));
    }

    let options: Vec<String> = if ftype == "boolean" {
        vec!["允许".into(), "不允许".into()]
    } else if !enum_values.is_empty() {
        enum_values
    } else {
        Vec::new()
    };
    let question = format!(
        "[MCP 服务器「{server_name}」请求输入] {message}\n（字段：{field_label}；类型：{ftype}）"
    );
    let qid = format!("elicit:{}", uuid::Uuid::new_v4());
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    waiters()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(qid.clone(), tx);
    // 事件发前先抢 waiter 占位（上面已插），这里挂起保护：作用域结束必然注销
    crate::bot::stream_log::emit_recorded(
        app,
        Some(&session_id),
        "bot-ask",
        serde_json::json!({
            "questionId": qid,
            "question": question,
            "why": "",
            "options": options,
            "assumption": "",
            "source": "mcp_elicit",
        }),
    );

    let waited =
        tokio::time::timeout(std::time::Duration::from_secs(ELICIT_TIMEOUT_SECS), rx).await;
    waiters()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&qid);
    let text = match waited {
        Ok(Ok(t)) => t,
        _ => {
            // 超时 / 应答通道关闭 → Decline（60s 默认拒）
            crate::bot::stream_log::emit_recorded(
                app,
                Some(&session_id),
                "bot-ask-resolved",
                serde_json::json!({ "questionId": qid, "action": "expired", "answer": null }),
            );
            return Err(decline("60s 未获应答，按拒绝处理"));
        }
    };
    if text.trim().is_empty() {
        crate::bot::stream_log::emit_recorded(
            app,
            Some(&session_id),
            "bot-ask-resolved",
            serde_json::json!({ "questionId": qid, "action": "dismiss", "answer": null }),
        );
        return Err(decline("用户未提供内容"));
    }
    if text.len() > ELICIT_VALUE_MAX_BYTES {
        crate::bot::stream_log::emit_recorded(
            app,
            Some(&session_id),
            "bot-ask-resolved",
            serde_json::json!({ "questionId": qid, "action": "dismiss", "answer": null }),
        );
        return Err(decline(&format!(
            "输入超过 {ELICIT_VALUE_MAX_BYTES} 字节上限"
        )));
    }
    // 类型解析：boolean 按按钮语义映射；number/integer 严格解析；string 原样
    let content: Value = match ftype.as_str() {
        "boolean" => match text.trim() {
            "允许" | "true" => serde_json::json!({ field.clone(): true }),
            _ => serde_json::json!({ field.clone(): false }),
        },
        "integer" => match text.trim().parse::<i64>() {
            Ok(n) => serde_json::json!({ field.clone(): n }),
            Err(_) => {
                crate::bot::stream_log::emit_recorded(
                    app,
                    Some(&session_id),
                    "bot-ask-resolved",
                    serde_json::json!({ "questionId": qid, "action": "dismiss", "answer": null }),
                );
                return Err(decline(&format!("输入不是整数：{}", truncate(&text, 40))));
            }
        },
        "number" => match text.trim().parse::<f64>() {
            Ok(n) => serde_json::json!({ field.clone(): n }),
            Err(_) => {
                crate::bot::stream_log::emit_recorded(
                    app,
                    Some(&session_id),
                    "bot-ask-resolved",
                    serde_json::json!({ "questionId": qid, "action": "dismiss", "answer": null }),
                );
                return Err(decline(&format!("输入不是数字：{}", truncate(&text, 40))));
            }
        },
        _ => serde_json::json!({ field.clone(): text.trim() }),
    };
    crate::bot::stream_log::emit_recorded(
        app,
        Some(&session_id),
        "bot-ask-resolved",
        serde_json::json!({ "questionId": qid, "action": "answer", "answer": "[MCP 应答，值不落日志]" }),
    );
    Ok(ElicitResult::new(ElicitationAction::Accept).with_content(content))
}

fn decline(reason: &str) -> McpError {
    McpError::invalid_params(reason.to_string(), None)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 用户应答回程（AskCard source=mcp_elicit 时调这个）：按 qid 唤醒等待方。
/// 应答值只在内存中转一次，不进审计/日志。
#[tauri::command]
pub fn elicit_respond(qid: String, text: String) -> crate::error::CommandResult<()> {
    let tx = waiters()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&qid);
    if let Some(tx) = tx {
        let _ = tx.send(text);
        Ok(())
    } else {
        Err(crate::error::CommandError::InvalidArgument {
            field: "qid".into(),
            value: qid.chars().take(24).collect(),
            reason: "该输入请求不存在或已超时".into(),
        })
    }
}

/// 桥接 handler（spawn_service 按开关注入）：只覆写 create_elicitation，
/// 其余生命周期事件走默认空实现。
pub struct ElicitBridge {
    pub server_id: String,
}

impl rmcp::handler::client::ClientHandler for ElicitBridge {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleClient>,
    ) -> Result<ElicitResult, McpError> {
        handle_elicitation(&self.server_id, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rmcp 的 ElicitationSchema 标 non_exhaustive（无法手工构造），测试走
    /// builder；「未知字段类型」分支 rmcp 层进不来（schema 只收 primitive，
    /// wire 反序列化即拒）——类型白名单代码保留作纵深，不设测试
    fn form_request_two_fields() -> ElicitRequestParams {
        let schema = rmcp::model::ElicitationSchema::builder()
            .required_string("a")
            .required_string("b")
            .build()
            .expect("builder 两字段构造必成功");
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "m".into(),
            requested_schema: schema,
        }
    }

    #[tokio::test]
    async fn multi_field_decline_before_app_needed() {
        // 多字段：先查在途登记，再轮到字段判定（APP 未注册也在字段判定之后）
        register_active_call("srv-a", "s1", "测试服务器");
        match handle_elicitation("srv-a", form_request_two_fields()).await {
            Err(e) => assert!(e.message.contains("单字段"), "{}", e.message),
            Ok(_) => panic!("必须 Decline"),
        }
        unregister_active_call("srv-a");
    }

    #[tokio::test]
    async fn url_mode_and_no_active_call_decline() {
        let url_req: ElicitRequestParams = serde_json::from_str(
            r#"{"mode":"url","message":"m","url":"https://x","elicitationId":"e1"}"#,
        )
        .unwrap();
        match handle_elicitation("srv-none", url_req).await {
            Err(e) => assert!(e.message.contains("没有在途工具调用"), "{}", e.message),
            Ok(_) => panic!("必须 Decline"),
        }
        register_active_call("srv-b", "s1", "测试");
        match handle_elicitation(
            "srv-b",
            serde_json::from_str(
                r#"{"mode":"url","message":"m","url":"https://x","elicitationId":"e1"}"#,
            )
            .unwrap(),
        )
        .await
        {
            Err(e) => assert!(e.message.contains("URL 模式"), "{}", e.message),
            Ok(_) => panic!("必须 Decline"),
        }
        unregister_active_call("srv-b");
    }

    #[tokio::test]
    async fn respond_wakes_waiter() {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        waiters()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("elicit:t1".into(), tx);
        elicit_respond("elicit:t1".into(), "hello".into()).unwrap();
        assert_eq!(rx.await.unwrap(), "hello");
        // 二次应答同 qid → 已移除 → 报不存在
        assert!(elicit_respond("elicit:t1".into(), "x".into()).is_err());
        waiters().lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
