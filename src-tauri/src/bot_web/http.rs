//! http_request 的请求执行层：通用 HTTP 客户端（方法 / 请求体 / 自定义头）。
//!
//! 与 `fetch.rs` 的分工：fetch 是「读网页正文」（内容类型闸 + 主块提取 + html2text），
//! 这里是「原样字节回灌或落盘」。四道既有防线全部沿用：公网闸（环回/私网/链路本地
//! 全拒）、钉 IP（DNS TOCTOU）、手动逐跳重定向校验、流式封顶（不信 Content-Length）。
//! 目的地闸在本层**每跳都判**——首跳过闸后 302 到白名单外域名不能绕过。

use std::time::Duration;

use tauri::AppHandle;
use url::Url;

use super::check_public_url;
use super::destination::{check_destination, DestOutcome};

const MAX_REDIRECTS: usize = 5;

/// 一次请求的全部参数。body 已编码为字节并带最终 Content-Type（编码/校验在
/// 工具层完成——执行层只认成品，错误文案对模型才有上下文）。
pub(crate) struct RequestSpec {
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<(Vec<u8>, String)>,
    pub timeout_secs: u64,
    pub max_bytes: usize,
}

/// 执行结果。`dest` 是最终请求那一跳的目的地判定来源（审计用）；
/// 非 2xx 也算执行成功——状态码是业务答案不是执行错误。
pub(crate) struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
    /// 响应头脱敏行：set-cookie / www-authenticate / proxy-authenticate 只回名
    /// （服务端下发的 session token 回显给模型 = 进上下文）；其余名+值截断。
    pub headers_brief: Vec<String>,
    pub body: Vec<u8>,
    pub truncated: bool,
    pub final_url: Url,
    pub dest: &'static str,
}

/// 失败带 dest：目的地拒（denied 细分值）与网络失败（None）都从这走，
/// 工具层统一记审计事件。
pub(crate) struct HttpError {
    pub dest: Option<&'static str>,
    pub message: String,
}

/// 目的地闸的上下文打包：执行层每跳判定都要弹窗可能（ask 档白名单外），
/// app / interactive / session_id 由工具层原样传入。
pub(crate) struct GateCtx<'a> {
    pub app: &'a AppHandle,
    pub tool: &'a str,
    pub interactive: bool,
    pub session_id: Option<&'a str>,
}

fn client_pinned(
    url: &Url,
    addrs: &[std::net::SocketAddr],
    timeout: Duration,
) -> Result<reqwest::Client, String> {
    let host = url.host_str().unwrap_or_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))
}

/// 响应头脱敏行：鉴权/会话类头只回名——它们是服务端发给客户端的凭据，
/// 回显值等于把凭据送进模型上下文；其余名+值截断。
fn brief_headers(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    const NAME_ONLY_HEADERS: &[&str] = &["set-cookie", "www-authenticate", "proxy-authenticate"];
    headers
        .iter()
        .map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            if NAME_ONLY_HEADERS.contains(&n.as_str()) {
                format!("{n}: ⟨值不回显⟩")
            } else {
                let v = value.to_str().unwrap_or("⟨非文本⟩");
                let cut: String = v.chars().take(120).collect();
                format!("{n}: {cut}")
            }
        })
        .collect()
}

/// 执行请求：每跳「公网闸 → 目的地闸 → 钉 IP → 发送」，重定向手动跟随。
pub(crate) async fn execute(spec: &RequestSpec, gate: &GateCtx<'_>) -> Result<Reply, HttpError> {
    let method = reqwest::Method::from_bytes(spec.method.as_bytes()).map_err(|e| HttpError {
        dest: None,
        message: format!("非法 HTTP 方法 {}：{e}", spec.method),
    })?;
    let mut url_cursor = spec.url.clone();
    let mut hops = 0usize;
    // 每轮先过目的地闸（Allowed 必赋值 / Denied 直接返回），循环后读是安全的
    let mut final_dest: &'static str;
    let mut resp;
    loop {
        let addrs = check_public_url(&url_cursor).await.map_err(|e| HttpError {
            dest: None,
            message: format!("公网闸拒绝：{}", e.reason()),
        })?;
        let host = url_cursor
            .host_str()
            .ok_or_else(|| HttpError {
                dest: None,
                message: "URL 无主机名".to_string(),
            })?
            .to_ascii_lowercase();
        // 目的地闸每跳都判：白名单内域名 302 到任意公网不能成为绕过路径
        match check_destination(
            gate.app,
            gate.tool,
            &host,
            gate.interactive,
            gate.session_id,
        )
        .await
        {
            DestOutcome::Allowed { dest } => final_dest = dest,
            DestOutcome::Denied { dest, reason } => {
                return Err(HttpError {
                    dest: Some(dest),
                    message: reason,
                });
            }
        }
        let client = client_pinned(&url_cursor, &addrs, Duration::from_secs(spec.timeout_secs))
            .map_err(|e| HttpError {
                dest: None,
                message: e,
            })?;
        let mut req = client.request(method.clone(), url_cursor.clone());
        for (k, v) in &spec.headers {
            // HeaderName/HeaderValue 的类型解析拒非法字符（含 \r\n）——
            // CRLF 注入在这里天然死掉，不需要自己写正则
            let name =
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).map_err(|e| HttpError {
                    dest: None,
                    message: format!("非法请求头名 {k:?}：{e}"),
                })?;
            let value = reqwest::header::HeaderValue::from_str(v).map_err(|e| HttpError {
                dest: None,
                message: format!("请求头 {k} 的值含非法字符：{e}"),
            })?;
            req = req.header(name, value);
        }
        if let Some((bytes, content_type)) = &spec.body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(bytes.clone());
        }
        resp = req.send().await.map_err(|e| HttpError {
            dest: Some(final_dest),
            message: format!("请求失败：{}", super::err_chain(&e)),
        })?;
        let status = resp.status();
        if status.is_redirection() {
            if hops >= MAX_REDIRECTS {
                return Err(HttpError {
                    dest: Some(final_dest),
                    message: format!("重定向超过 {MAX_REDIRECTS} 次，已停止"),
                });
            }
            let Some(loc) = resp.headers().get(reqwest::header::LOCATION).cloned() else {
                return Err(HttpError {
                    dest: Some(final_dest),
                    message: format!("返回重定向 {status} 但缺少 Location"),
                });
            };
            let loc = loc
                .to_str()
                .map_err(|_| HttpError {
                    dest: Some(final_dest),
                    message: "Location 头编码无效".to_string(),
                })?
                .to_string();
            let prev_scheme = url_cursor.scheme().to_string();
            url_cursor = url_cursor.join(&loc).map_err(|_| HttpError {
                dest: Some(final_dest),
                message: format!("重定向目标无效：{loc}"),
            })?;
            // https→http 降级拒：与 fetch_direct 同一理由（首跳 TLS 不保护后续明文跳）
            if prev_scheme == "https" && url_cursor.scheme() == "http" {
                return Err(HttpError {
                    dest: Some(final_dest),
                    message: format!("拒绝 https→http 降级重定向：{loc}"),
                });
            }
            hops += 1;
            continue;
        }
        break;
    }
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    // 先取头再读体（read_body_capped 拿走 resp 所有权）
    let headers_brief = brief_headers(resp.headers());
    let body = super::read_body_capped(resp, spec.max_bytes)
        .await
        .map_err(|e| HttpError {
            dest: Some(final_dest),
            message: e,
        })?;
    let truncated = body.len() >= spec.max_bytes;
    Ok(Reply {
        status,
        content_type,
        headers_brief,
        body,
        truncated,
        final_url: url_cursor,
        dest: final_dest,
    })
}

#[cfg(test)]
mod tests {
    use super::brief_headers;

    /// 锚点：服务端下发的凭据类响应头只回名，值不进任何回显。
    #[test]
    fn credential_response_headers_are_name_only() {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_static("session=secret-token-123; Path=/"),
        );
        map.insert(
            reqwest::header::HeaderName::from_static("x-custom"),
            reqwest::header::HeaderValue::from_static("visible-value"),
        );
        let brief = brief_headers(&map);
        assert!(
            brief
                .iter()
                .any(|l| l.starts_with("set-cookie") && !l.contains("secret-token")),
            "set-cookie 只能回名：{brief:?}"
        );
        assert!(
            brief
                .iter()
                .any(|l| l.contains("x-custom") && l.contains("visible-value")),
            "普通头名值都回显：{brief:?}"
        );
    }

    /// 锚点：请求头 CRLF 注入依赖 reqwest 类型解析拒收——把这一依赖钉住，
    /// reqwest 升级若放宽（HeaderValue 接受 \r\n）此测试当场红。
    #[test]
    fn reqwest_header_types_reject_crlf() {
        assert!(reqwest::header::HeaderValue::from_str("ok value").is_ok());
        assert!(
            reqwest::header::HeaderValue::from_str("a\r\nSet-Cookie: x=1").is_err(),
            "含 CRLF 的头值必须被类型解析拒绝"
        );
        assert!(
            reqwest::header::HeaderName::from_bytes(b"Bad\nName").is_err(),
            "含换行的头名必须被类型解析拒绝"
        );
    }
}
