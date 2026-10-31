//! http_request 工具层：参数解析 → 凭据引用解析 → 执行（含目的地闸）→ 回灌整形。
//!
//! 凭据引用（值不过模型上下文的唯一手段）：
//! - `"keyring:<厂商名>"` → 注入该厂商在设置页配置过的 key
//! - `"env:<变量名>"` → 注入宿主进程环境变量
//! 明文值仍可用（向后兼容），但无论哪条路，值都不回显、不进审计、不进 preview。

use serde_json::Value;
use tauri::AppHandle;

use crate::bot::registry::ToolResult;

use super::doc::slice_fetch_output;
use crate::bot_web::{execute_http, GateCtx, HttpError, RequestSpec, MAX_TIMEOUT_SECS};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_BODY_BYTES: usize = 10 * 1024 * 1024;
const KEYRING_PREFIX: &str = "keyring:";
const ENV_PREFIX: &str = "env:";
/// saveAs（下载落盘）模式的超时上限：慢网大文件下载与交互请求的时长语义不同，
/// 放宽到 300s（普通请求仍 120s）。
const SAVE_MAX_TIMEOUT_SECS: u64 = 300;

/// saveAs 文件名清洗：只留文件名语义，拒一切目录语义——路径分隔符（/ \\）、
/// 父目录引用（..）、控制字符都拒；落点固定 AI_Gen_Files 根，不跟随子目录。
/// 通过的返回清洗后的名字（trim），拒的给原因。
fn sanitize_save_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("saveAs 文件名不能为空".into());
    }
    if name.len() > 200 {
        return Err("saveAs 文件名过长（上限 200 字符）".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("saveAs 文件名不能含路径分隔符（只能存到 AI_Gen_Files 根目录）".into());
    }
    if name == "." || name == ".." || name.contains("..") {
        return Err("saveAs 文件名不能含 ..".into());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("saveAs 文件名含控制字符".into());
    }
    Ok(name.to_string())
}

/// 解析凭据引用前缀：真值在宿主侧解开，模型上下文从头到尾只见条目名。
/// 读不到明确报错——静默去掉头会让「以为带了鉴权」的请求裸奔。
fn resolve_secret(raw: &str) -> Result<String, String> {
    if let Some(vendor) = raw.strip_prefix(KEYRING_PREFIX) {
        let vendor = vendor.trim();
        if vendor.is_empty() {
            return Err(format!("{KEYRING_PREFIX} 后面要跟厂商名"));
        }
        return crate::bot::config::keyring::read_vendor_key(vendor)
            .map_err(|e| format!("读厂商 {vendor} 的 key 失败：{e}"));
    }
    if let Some(name) = raw.strip_prefix(ENV_PREFIX) {
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("{ENV_PREFIX} 后面要跟环境变量名"));
        }
        return std::env::var(name)
            .map_err(|_| format!("环境变量 {name} 不存在（须在启动应用前设好）"));
    }
    Ok(raw.to_string())
}

/// 请求体编码：text 原样 / json 先校验再原样发 / form 把 JSON 对象编成
/// x-www-form-urlencoded（url crate 的 form_urlencoded，零新依赖）。
fn encode_body(body: &str, body_type: &str) -> Result<(Vec<u8>, String), String> {
    match body_type {
        "json" => {
            let v: Value = serde_json::from_str(body)
                .map_err(|e| format!("bodyType=json 但 body 不是合法 JSON：{e}"))?;
            let _ = v;
            Ok((body.as_bytes().to_vec(), "application/json".into()))
        }
        "form" => {
            let v: Value = serde_json::from_str(body)
                .map_err(|e| format!("bodyType=form 的 body 须是 JSON 对象：{e}"))?;
            let Some(obj) = v.as_object() else {
                return Err("bodyType=form 的 body 须是 JSON 对象（键值对）".into());
            };
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, val) in obj {
                let s = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                ser.append_pair(k, &s);
            }
            Ok((
                ser.finish().into_bytes(),
                "application/x-www-form-urlencoded".into(),
            ))
        }
        _ => Ok((body.as_bytes().to_vec(), "text/plain; charset=utf-8".into())),
    }
}

/// 审计 kv 统一组装：host / method / status / bytes / dest——凭据、body、
/// 完整 URL（查询串可夹带数据）一律不记。
fn audit_event_kv(
    host: &str,
    method: &str,
    status: Option<u16>,
    bytes: usize,
    dest: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("host", host.to_string()),
        ("method", method.to_string()),
        (
            "status",
            status.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
        ),
        ("bytes", bytes.to_string()),
        ("dest", dest.to_string()),
    ]
}

pub async fn tool_http_request(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(u) = v["url"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("http_request 缺少 url 参数");
    };
    let url = match url::Url::parse(&u) {
        Ok(u) if u.scheme() == "http" || u.scheme() == "https" => u,
        _ => return ToolResult::denied("url 必须是 http/https 地址"),
    };
    let host = url.host_str().unwrap_or_default().to_string();
    let method = v["method"]
        .as_str()
        .unwrap_or("GET")
        .trim()
        .to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return ToolResult::denied(format!(
            "不支持的方法 {method}（可用 GET/POST/PUT/PATCH/DELETE/HEAD）"
        ));
    }
    // 自定义头：键做基本清洗，值走凭据引用解析
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(map) = v["headers"].as_object() {
        for (k, val) in map {
            let Some(raw) = val.as_str() else {
                return ToolResult::denied(format!("请求头 {k} 的值必须是字符串"));
            };
            let value = match resolve_secret(raw) {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult::error(format!("解析请求头 {k} 失败：{e}"), Vec::new())
                }
            };
            headers.push((k.clone(), value));
        }
    }
    let body = match v["body"].as_str() {
        Some(b) if !b.is_empty() => {
            let body_type = v["bodyType"].as_str().unwrap_or("text");
            match encode_body(b, body_type) {
                Ok(x) => Some(x),
                Err(e) => return ToolResult::denied(e),
            }
        }
        _ => None,
    };
    let save_as = match v["saveAs"].as_str() {
        Some(s) => match sanitize_save_name(s) {
            Ok(n) => Some(n),
            Err(e) => return ToolResult::denied(e),
        },
        None => None,
    };
    // 下载落盘放宽超时上限（慢网大文件），交互请求维持 120s
    let timeout_secs = v["timeoutSecs"]
        .as_u64()
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(
            1,
            if save_as.is_some() {
                SAVE_MAX_TIMEOUT_SECS
            } else {
                MAX_TIMEOUT_SECS
            },
        );
    let max_bytes = v["maxBytes"]
        .as_u64()
        .unwrap_or(DEFAULT_BODY_BYTES as u64)
        .clamp(1024, MAX_BODY_BYTES as u64) as usize;
    let offset = v["offset"].as_u64().unwrap_or(0) as usize;

    let spec = RequestSpec {
        method: method.clone(),
        url: url.clone(),
        headers,
        body,
        timeout_secs,
        max_bytes,
    };
    let gate = GateCtx {
        app,
        tool: "http_request",
        interactive,
        session_id,
    };
    match execute_http(&spec, &gate).await {
        Ok(reply) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "bot.http_request",
                &audit_event_kv(
                    &host,
                    &method,
                    Some(reply.status),
                    reply.body.len(),
                    reply.dest,
                ),
            );
            let mut out = format!(
                "HTTP {} · {}\n来源：{}\n",
                reply.status,
                reply.content_type.as_deref().unwrap_or("未知类型"),
                reply.final_url
            );
            if !reply.headers_brief.is_empty() {
                out.push_str("[响应头]\n");
                for h in &reply.headers_brief {
                    out.push_str(h);
                    out.push('\n');
                }
            }
            // 下载落盘分支：给了 saveAs 就存文件不回正文。
            // truncated（传输达上限）不落盘——落一个可能不完整的文件再让脚本去
            // 算，错数据比没有数据更糟；报错让模型调大 maxBytes 重试。
            if let Some(name) = &save_as {
                if reply.truncated {
                    return ToolResult::error(
                        format!(
                            "响应体达到传输上限 {}MB，可能不完整，没有落盘；调大 maxBytes（上限 50MB）重试",
                            max_bytes / (1024 * 1024)
                        ),
                        Vec::new(),
                    );
                }
                if method == "HEAD" {
                    return ToolResult::error("HEAD 请求没有响应体，saveAs 无意义", Vec::new());
                }
                let dir = match crate::db::gen_dir(app) {
                    Ok(d) => d,
                    Err(e) => return ToolResult::error(format!("产物目录不可用：{e}"), Vec::new()),
                };
                let mut path = dir.join(name);
                // 重名不覆盖（与 manage_file 同口径）：追加毫秒时间戳到 stem
                if path.exists() {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("download")
                        .to_string();
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|e| format!(".{e}"))
                        .unwrap_or_default();
                    path = dir.join(format!("{stem}_{}{ext}", crate::db::now_ms()));
                }
                if let Err(e) = std::fs::write(&path, &reply.body) {
                    return ToolResult::error(format!("写入产物文件失败：{e}"), Vec::new());
                }
                out.push_str(&format!(
                    "\n[已保存] {}（{} 字节）\n数据文件交给 run_python / extract_document 按路径直读，不必经过对话上下文",
                    path.display(),
                    reply.body.len()
                ));
                return ToolResult::ok(out, Vec::new());
            }
            if method == "HEAD" || reply.body.is_empty() {
                return ToolResult::ok(out, Vec::new());
            }
            let text = String::from_utf8_lossy(&reply.body);
            out.push_str("[正文]\n");
            out.push_str(&slice_fetch_output(&text, offset));
            if reply.truncated {
                out.push_str(&format!(
                    "\n（传输已按 maxBytes={} 截断；要完整内容用 saveAs 落盘后本地处理）",
                    max_bytes
                ));
            }
            ToolResult::ok(out, Vec::new())
        }
        Err(HttpError { dest, message }) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "bot.http_request",
                &audit_event_kv(&host, &method, None, 0, dest.unwrap_or("network_error")),
            );
            // 目的地拒是预期内控制流（Denied），网络失败才是执行错误（Error）——
            // 两者的审计级别不同，调度侧「这一步算不算成功」的判定同源
            match dest {
                Some(_) => ToolResult::denied(message),
                None => ToolResult::error(message, Vec::new()),
            }
        }
    }
}

/// dispatch 记账的 args_preview 特供：http_request 的入参前 80 字符必含完整 URL
/// （含查询串），与「审计不记完整 URL」冲突——这里裁成只留方法与目标 host。
/// 解析失败回落通用 80 字符截断（保守：宁可少裁不可崩）。
pub fn preview_args(args: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(args) {
        let method = v["method"]
            .as_str()
            .unwrap_or("GET")
            .trim()
            .to_ascii_uppercase();
        if let Some(u) = v["url"]
            .as_str()
            .and_then(|s| url::Url::parse(s.trim()).ok())
        {
            let host = u.host_str().unwrap_or("?");
            let path = u.path();
            return format!("{method} {host}{path}");
        }
    }
    args.chars().take(80).collect()
}

/// 结果文本进任何审计/可见性 sink（tool.return preview、子 agent 动作流、
/// 执行痕迹 span）前的脱敏：`来源：` 行的 final_url 去掉查询串与 fragment——
/// 那是模型塞进请求 URL 的数据（可带 token），与 tool.call 入参「不记完整
/// URL」同一口径；正文节选里的第三方链接是页面内容，不在此列。
/// 模型可见的 tool result 不经此函数：模型需要知道重定向后的最终落点。
pub fn redact_result_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, seg) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if seg.starts_with("来源：") {
            out.push_str(&strip_url_query(seg));
        } else {
            out.push_str(seg);
        }
    }
    out
}

/// 「来源：{url}」行的 URL 裁剪：保 scheme://host/path，自 `?` 或 `#` 起整段
/// 去掉并留「…」标记；无 URL 或无查询串原样返回。
fn strip_url_query(line: &str) -> String {
    let Some(pos) = line.find("://") else {
        return line.to_string();
    };
    let (prefix, rest) = line.split_at(pos + 3);
    let Some(cut) = rest.find(['?', '#']) else {
        return line.to_string();
    };
    let mut out = String::with_capacity(line.len() + 4);
    out.push_str(prefix);
    out.push_str(&rest[..cut]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锚点：来源行的查询串/fragment 不得进任何审计 sink。
    #[test]
    fn redact_result_text_strips_query_on_source_line() {
        let t = "HTTP 200 · text/html\n来源：https://api.example.com/v1/data?token=secret123&page=2\n正文里的链接 https://cdn.example.com/x.js?ver=1 是页面内容";
        let r = redact_result_text(t);
        assert!(!r.contains("secret123"), "来源行查询串必须去掉：{r}");
        assert!(!r.contains("page=2"), "查询参数也不得残留：{r}");
        assert!(
            r.contains("来源：https://api.example.com/v1/data…"),
            "来源行只留 scheme://host/path：{r}"
        );
        assert!(
            r.contains("cdn.example.com/x.js?ver=1"),
            "正文里的第三方链接是页面内容，不脱敏：{r}"
        );
    }

    /// 锚点：无查询串的结果文本原样往返（脱敏不得顺手改写正文）。
    #[test]
    fn redact_result_text_keeps_clean_text_intact() {
        let t = "HTTP 200 · application/json\n来源：https://api.example.com/v1/data\n{\"ok\":true}";
        assert_eq!(redact_result_text(t), t);
    }

    /// 锚点：查询串（可夹带数据/凭据）不得进 preview。
    #[test]
    fn preview_strips_query_string() {
        let args =
            r#"{"url":"https://api.example.com/v1/data?token=secret123&page=2","method":"POST"}"#;
        let p = preview_args(args);
        assert!(
            p.contains("POST api.example.com"),
            "preview 含方法与 host：{p}"
        );
        assert!(!p.contains("secret123"), "查询串不得进 preview：{p}");
        assert!(!p.contains("token="), "查询参数名也不进：{p}");
    }

    /// 锚点：解析失败回落通用截断（不崩、不吞）。
    #[test]
    fn preview_falls_back_on_bad_json() {
        assert_eq!(preview_args("not json"), "not json");
    }

    /// 锚点：env: 引用读不到明确报错（不静默空值——裸奔比失败更糟）；
    /// 读得到时返回真值（值只在宿主内存，不进任何返回文本由上层保证）。
    #[test]
    fn env_secret_reference_resolves_or_errors() {
        std::env::set_var("WM_TEST_HTTP_SECRET", "the-real-value");
        assert_eq!(
            resolve_secret("env:WM_TEST_HTTP_SECRET").unwrap(),
            "the-real-value"
        );
        let err = resolve_secret("env:WM_TEST_HTTP_NO_SUCH_VAR").unwrap_err();
        assert!(err.contains("不存在"), "读不到要说明：{err}");
        std::env::remove_var("WM_TEST_HTTP_SECRET");
        assert!(resolve_secret("keyring:").is_err(), "空前缀要报错");
    }

    /// 锚点：body 编码三形态 + 非法 JSON 拒绝。
    #[test]
    fn body_encoding_three_shapes() {
        let (b, ct) = encode_body("hello", "text").unwrap();
        assert_eq!(b, b"hello");
        assert!(ct.starts_with("text/plain"));
        let (b, ct) = encode_body("{\"a\":1}", "json").unwrap();
        assert_eq!(b, b"{\"a\":1}");
        assert_eq!(ct, "application/json");
        assert!(
            encode_body("{not json", "json").is_err(),
            "非法 JSON 必须拒"
        );
        let (b, ct) = encode_body("{\"q\":\"你好 w\"}", "form").unwrap();
        assert_eq!(b, b"q=%E4%BD%A0%E5%A5%BD+w");
        assert_eq!(ct, "application/x-www-form-urlencoded");
        assert!(
            encode_body("[1,2]", "form").is_err(),
            "form 的 body 必须是对象"
        );
    }

    /// 锚点：saveAs 拒绝一切目录语义（路径分隔符 / .. / 控制字符），防目录穿越。
    #[test]
    fn save_name_rejects_path_semantics() {
        assert!(sanitize_save_name("data.csv").is_ok());
        assert!(
            sanitize_save_name("  数据集 2026.csv ").is_ok(),
            "空白与中文合法"
        );
        assert!(sanitize_save_name("").is_err());
        assert!(sanitize_save_name("a/b.csv").is_err(), "正斜杠拒");
        assert!(sanitize_save_name("a\\b.csv").is_err(), "反斜杠拒");
        assert!(sanitize_save_name("..").is_err());
        assert!(sanitize_save_name("a..b.csv").is_err(), "内嵌 .. 也拒");
        assert!(sanitize_save_name("../etc/passwd").is_err());
        assert!(sanitize_save_name("\u{7f}x").is_err(), "控制字符拒");
        assert!(sanitize_save_name(&"x".repeat(201)).is_err(), "超长拒");
    }
}
