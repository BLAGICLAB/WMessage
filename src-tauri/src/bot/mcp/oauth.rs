//! MCP OAuth 2.1 授权（差距收编方案 D-6）：HTTP 传输服务器的授权码登录。
//!
//! 流程（方案 §七）：发现（RFC 9728 保护资源元数据 → RFC 8414 授权服务器
//! 元数据；404 时对 server 根直接取授权服务器元数据）→ 客户端（DCR RFC 7591，
//! 不支持则用配置的手动 `oauthClientId` 档）→ 授权码 + PKCE S256（RFC 7636）
//! + state + `resource` 参数（RFC 8707）→ loopback 回调（RFC 8252，tiny_http
//! 127.0.0.1:0 随机端口、只收一个请求、5 分钟超时）→ 换 token → keyring。
//!
//! 安全边界：token 只存 keyring（`mcp-oauth:{id}` 独立条目），不进审计/日志/
//! 配置文件；state 与 PKCE verifier 只在内存；回调服务器只绑 loopback、只读
//! 一个请求。refresh 主动过期前 60s 刷新（401 文本兜底一层）。
//!
//! 范围（不做清单）：authorization_code + refresh_token 之外不做（device
//! flow / client_credentials / token 交换）；stdio 传输不接；client_secret
//! 手动档不做（PKCE public client 覆盖 MCP 主流场景，方案 §十一偏离 12）。

use sha2::Digest;

use serde::{Deserialize, Serialize};

use super::config::McpServerConfig;

/// 随机字节（uuid v4 内部走 OS CSPRNG：除版本/变体 6 位外全随机；
/// 两枚 UUID 拼 32 字节给 PKCE verifier 足够 240+ 位熵）
fn random_bytes(n: usize) -> Vec<u8> {
    (0..n.div_ceil(16))
        .flat_map(|_| *uuid::Uuid::new_v4().as_bytes())
        .take(n)
        .collect()
}

/// token blob 体积上限（与 MCP_SECRET_BLOB_MAX_BYTES 同量级口径）
const OAUTH_BLOB_MAX_BYTES: usize = 2048;
/// loopback 回调等待上限
const LOGIN_TIMEOUT_SECS: u64 = 300;
/// token 将过期的提前刷新窗（秒）
const REFRESH_AHEAD_SECS: i64 = 60;
/// state / PKCE verifier 字节长度
const STATE_BYTES: usize = 16;
const VERIFIER_BYTES: usize = 32;

/// 令牌束（keyring blob 原文即本结构 JSON）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBundle {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// 过期时刻（epoch ms；无 expires_in → 远期，靠 401 兜底）
    pub expires_at_ms: i64,
    /// 本次授权使用的 client_id（refresh 请求要带）
    pub client_id: String,
}

impl TokenBundle {
    fn to_blob(&self) -> Result<String, String> {
        let s = serde_json::to_string(self).map_err(|e| format!("token 序列化失败：{e}"))?;
        if s.len() > OAUTH_BLOB_MAX_BYTES {
            return Err(format!(
                "token blob 过大（{} 字节 > 上限 {OAUTH_BLOB_MAX_BYTES}）",
                s.len()
            ));
        }
        Ok(s)
    }

    fn from_blob(s: &str) -> Result<Self, String> {
        if s.len() > OAUTH_BLOB_MAX_BYTES {
            return Err(format!("token blob 过大（> 上限 {OAUTH_BLOB_MAX_BYTES}）"));
        }
        serde_json::from_str(s).map_err(|e| format!("token blob 解析失败：{e}"))
    }

    /// 是否临近过期（提前窗内视为过期）
    fn expiring(&self, now_ms: i64) -> bool {
        self.expires_at_ms - now_ms < REFRESH_AHEAD_SECS * 1000
    }
}

// ── 纯函数（可测）───────────────────────────────────────────────

/// PKCE 对：随机 verifier + S256 challenge（uuid v4 → OS CSPRNG，PKCE
/// verifier 是加密材料不得用可预测伪随机源）
fn pkce_pair() -> (String, String) {
    let verifier = base64url(&random_bytes(VERIFIER_BYTES));
    let challenge = base64url(&sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_state() -> String {
    base64url(&random_bytes(STATE_BYTES))
}

fn base64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 授权服务器元数据（RFC 8414 子集：本流程用到的字段）
#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMeta {
    #[serde(rename = "authorization_endpoint")]
    pub authorization_endpoint: String,
    #[serde(rename = "token_endpoint")]
    pub token_endpoint: String,
    #[serde(
        rename = "registration_endpoint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub registration_endpoint: Option<String>,
}

/// 从保护资源元数据（RFC 9728）取授权服务器根；无该字段 → server 根本身
fn authorization_server_root(
    resource_meta: &serde_json::Value,
    server_origin: &str,
) -> Option<String> {
    resource_meta
        .get("authorization_servers")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.as_str())
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| Some(server_origin.trim_end_matches('/').to_string()))
}

/// token 响应解析（纯函数）：取 access/refresh/expires_in
fn parse_token_response(
    v: &serde_json::Value,
) -> Result<(String, Option<String>, Option<i64>), String> {
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "token 响应缺 access_token".to_string())?
        .to_string();
    let refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let expires_in = v.get("expires_in").and_then(|x| x.as_i64());
    Ok((access, refresh, expires_in))
}

// ── keyring 存取（独立条目，raw blob 走 secrets 的后端分发）──────────

fn oauth_read(app: &tauri::AppHandle, id: &str) -> Result<Option<TokenBundle>, String> {
    match super::secrets::raw_read(app, &format!("mcp-oauth:{id}"))? {
        Some(s) => Ok(Some(TokenBundle::from_blob(&s)?)),
        None => Ok(None),
    }
}

fn oauth_write(app: &tauri::AppHandle, id: &str, t: &TokenBundle) -> Result<(), String> {
    super::secrets::raw_write(app, &format!("mcp-oauth:{id}"), &t.to_blob()?)
}

fn oauth_purge(app: &tauri::AppHandle, id: &str) {
    let _ = super::secrets::raw_delete(app, &format!("mcp-oauth:{id}"));
}

pub(crate) fn load_token(
    app: &tauri::AppHandle,
    server_id: &str,
) -> Result<Option<TokenBundle>, String> {
    oauth_read(app, server_id)
}

// ── 登录命令 ────────────────────────────────────────────────────

/// 查配置里的一台服务器（同步读配置）
fn find_server(app: &tauri::AppHandle, server_id: &str) -> Result<McpServerConfig, String> {
    let cfg = crate::bot::config::io::load_config(app);
    cfg.mcp_servers
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("MCP 服务器不存在：{server_id}"))
}

/// 令牌请求客户端（token/refresh/DCR/发现请求共用）：与 MCP HTTP 同闸
///（仅公网 + 钉 IP + 禁重定向），不带 MCP 服务器的静态头
async fn oauth_http_client(url: &str) -> Result<reqwest::Client, String> {
    let parsed =
        super::config::http_url_is_public(url).map_err(|reason| format!("URL 安全闸：{reason}"))?;
    let addrs = crate::bot_web::check_public_url(&parsed)
        .await
        .map_err(|e| format!("URL 安全闸：{}", e.gate_message()))?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    // 复用 manager 的钉 IP 客户端构建（同一禁重定向策略）
    super::manager::build_pinned_client(&host, &addrs, &std::collections::BTreeMap::new())
}

/// 发现：保护资源元数据 → 授权服务器元数据；两跳都走 URL 安全闸
async fn discover_auth_server(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<AuthServerMeta, String> {
    let origin = server_origin(server_url)?;
    // 第一跳：RFC 9728 保护资源元数据（拿 authorization_servers 根）
    let root = match client
        .get(format!("{origin}/.well-known/oauth-protected-resource"))
        .send()
        .await
        .ok()
        .map(|r| r.status())
    {
        Some(s) if s.is_success() => {
            let v: serde_json::Value = client
                .get(format!("{origin}/.well-known/oauth-protected-resource"))
                .send()
                .await
                .map_err(|e| format!("保护资源元数据请求失败：{e}"))?
                .json()
                .await
                .map_err(|e| format!("保护资源元数据解析失败：{e}"))?;
            authorization_server_root(&v, &origin)
                .ok_or_else(|| "元数据缺授权服务器".to_string())?
        }
        _ => origin.clone(),
    };
    // 第二跳：RFC 8414 授权服务器元数据
    let resp = client
        .get(format!("{root}/.well-known/oauth-authorization-server"))
        .send()
        .await
        .map_err(|e| format!("授权服务器元数据请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "授权服务器元数据不可达（{}）：该服务器可能不支持 OAuth，或未发布 /.well-known 端点",
            resp.status()
        ));
    }
    let meta: AuthServerMeta = resp
        .json()
        .await
        .map_err(|e| format!("授权服务器元数据解析失败：{e}"))?;
    Ok(meta)
}

fn server_origin(url: &str) -> Result<String, String> {
    let parsed: url::Url = url::Url::parse(url).map_err(|e| format!("URL 解析失败：{e}"))?;
    let host = parsed.host_str().ok_or_else(|| "URL 缺 host".to_string())?;
    let port = parsed
        .port_or_known_default()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    Ok(format!("{}://{host}{port}", parsed.scheme()))
}

/// DCR（RFC 7591）：拿 client_id；服务器不支持 → Err（提示手动档）
async fn register_client(
    client: &reqwest::Client,
    meta: &AuthServerMeta,
) -> Result<String, String> {
    let reg = meta.registration_endpoint.as_deref().ok_or_else(|| {
        "授权服务器未提供动态注册端点；请在服务器配置里手动填 oauthClientId".to_string()
    })?;
    let resp = client
        .post(reg)
        .json(&serde_json::json!({
            "client_name": "wmessage",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "redirect_uris": [],
        }))
        .send()
        .await
        .map_err(|e| format!("动态客户端注册请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "动态客户端注册失败（{}）；请在服务器配置里手动填 oauthClientId",
            resp.status()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("注册响应解析失败：{e}"))?;
    v.get("client_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "注册响应缺 client_id".to_string())
}

/// 完整登录流程（mcp_oauth_login 的实现体）：发现 → 客户端 → PKCE 授权码
/// → loopback 回调 → 换 token → keyring。返回成功文案。
async fn login_impl(app: &tauri::AppHandle, server_id: &str) -> Result<String, String> {
    let cfg = find_server(app, server_id)?;
    if cfg.transport_kind() != super::config::McpTransport::Http {
        return Err("OAuth 只支持 HTTP 传输的 MCP 服务器".into());
    }
    let url = cfg
        .url
        .as_deref()
        .ok_or_else(|| "该服务器未配置 URL".to_string())?
        .trim()
        .to_string();
    let client = oauth_http_client(&url).await?;
    let meta = discover_auth_server(&client, &url).await?;
    let client_id = match cfg.oauth_client_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => register_client(&client, &meta).await?,
    };
    // PKCE + state + loopback 回调服务器
    let (verifier, challenge) = pkce_pair();
    let state = random_state();
    let listener =
        tiny_http::Server::http(("127.0.0.1", 0)).map_err(|e| format!("回调端口绑定失败：{e}"))?;
    let port = listener
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| "回调端口解析失败".to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let origin = server_origin(&url)?;
    let authorize_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&resource={}",
        meta.authorization_endpoint,
        urlencode(&client_id),
        urlencode(&redirect_uri),
        urlencode(&state),
        urlencode(&challenge),
        urlencode(&origin),
    );
    use tauri_plugin_opener::OpenerExt as _;
    app.opener()
        .open_url(&authorize_url, None::<&str>)
        .map_err(|e| format!("打开浏览器失败：{e}"))?;
    // 回调：只收一个请求（GET /callback?code=..&state=..），5 分钟超时
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS);
    let code = loop {
        // 5s 片轮询 recv：超时 None 时未到总 deadline 就继续等
        let req = match listener.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Some(r)) => r,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return Err("登录超时（5 分钟内未完成浏览器授权）".to_string());
                }
                continue;
            }
            Err(e) => return Err(format!("回调读取失败：{e}")),
        };
        let target = req.url().to_string();
        let (got_state, got_code, err) = parse_callback(&target);
        let _ = req.respond(tiny_http::Response::from_string(
            "授权完成，可关闭此页面。若未自动关闭，请回到 wmessage 查看结果。",
        ));
        match (got_state, got_code, err) {
            (_, _, Some(e)) => return Err(format!("授权回调错误：{e}")),
            (s, c, None) if s.as_deref() == Some(state.as_str()) && c.is_some() => {
                break c.expect("上方已判 is_some");
            }
            // state 不匹配 / 无 code：拒绝（伪造回调防线）——继续等下一个请求？
            // 只收一个请求的语义下直接报错更诚实：伪造/失败即终止，用户重来。
            (s, _, None) if s.as_deref() != Some(state.as_str()) => {
                return Err("回调 state 不匹配，已拒绝（可能是伪造回调）".to_string());
            }
            _ => return Err("回调缺授权码，已终止（请重新登录）".to_string()),
        }
    };
    let (access, refresh, expires_in) = token_request(
        &client,
        &meta.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ],
    )
    .await?;
    let now = crate::db::now_ms();
    let bundle = TokenBundle {
        expires_at_ms: now + expires_in.unwrap_or(3600) * 1000,
        access_token: access,
        refresh_token: refresh,
        client_id,
    };
    oauth_write(app, server_id, &bundle)?;
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Info,
        "mcp.oauth.login",
        "server" => server_id,
    );
    Ok("授权成功，令牌已存入系统凭据存储。".to_string())
}

/// 解析 loopback 回调 URL（纯函数可测）：(state, code, error)
fn parse_callback(target: &str) -> (Option<String>, Option<String>, Option<String>) {
    let q = target.split('?').nth(1).unwrap_or_default();
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "state" => state = Some(urldecode(v)),
            "code" => code = Some(urldecode(v)),
            "error" => error = Some(urldecode(v)),
            _ => {}
        }
    }
    (state, code, error)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // %XX（两个 hex 字符都在才算转义；残缺 % 原样保留）
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// token 请求（换码 / 刷新共用）：form 表单 POST（手拼 urlencode——reqwest
/// 的 form feature 未开），2KB 上限防巨响应
async fn token_request(
    client: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<(String, Option<String>, Option<i64>), String> {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("token 请求失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let hint = if status.as_u16() == 400 && body.contains("invalid_grant") {
            "（授权已过期，请重新登录）"
        } else {
            ""
        };
        return Err(format!(
            "token 请求失败（{status}）{hint}：{}",
            truncate_body(&body)
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("token 响应解析失败：{e}"))?;
    parse_token_response(&v)
}

fn truncate_body(s: &str) -> String {
    s.chars().take(200).collect()
}

/// refresh（401/将过期共用）：refresh_token 换新 token；服务器返回新
/// refresh_token 则覆盖，否则保留旧的（OAuth 惯例）
pub(crate) async fn refresh_token(
    app: &tauri::AppHandle,
    cfg: &McpServerConfig,
    bundle: &TokenBundle,
) -> Result<TokenBundle, String> {
    let client = oauth_http_client(cfg.url.as_deref().unwrap_or_default()).await?;
    let Some(rt) = bundle.refresh_token.as_deref() else {
        // 无 refresh_token：清掉令牌要求重登（fail-closed）
        oauth_purge(app, &cfg.id);
        return Err("该授权无可刷新令牌，请重新登录".to_string());
    };
    let (access, new_refresh, expires_in) = token_request(
        &client,
        &refresh_endpoint(cfg, &client).await?,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            ("client_id", bundle.client_id.as_str()),
        ],
    )
    .await?;
    let now = crate::db::now_ms();
    let next = TokenBundle {
        expires_at_ms: now + expires_in.unwrap_or(3600) * 1000,
        access_token: access,
        refresh_token: new_refresh.or_else(|| bundle.refresh_token.clone()),
        client_id: bundle.client_id.clone(),
    };
    oauth_write(app, &cfg.id, &next)?;
    Ok(next)
}

/// refresh 的 token endpoint：从缓存 token 的授权流程里拿不到（bundle 未存
/// endpoint）——重新发现一次（一次 GET，量小；频次=每次刷新）
async fn refresh_endpoint(
    cfg: &McpServerConfig,
    client: &reqwest::Client,
) -> Result<String, String> {
    let url = cfg
        .url
        .as_deref()
        .ok_or_else(|| "该服务器未配置 URL".to_string())?;
    let meta = discover_auth_server(client, url).await?;
    Ok(meta.token_endpoint)
}

/// call_tool 入口的主动刷新：token 临近过期 → refresh 一次；失败返回 Err
///（调用方报人话错误并清 token 由用户重登）。返回是否刷新过（调用方据此
/// 重建连接）。
pub(crate) async fn ensure_fresh_token(
    app: &tauri::AppHandle,
    cfg: &McpServerConfig,
) -> Result<bool, String> {
    let Some(t) = load_token(app, &cfg.id)? else {
        return Ok(false); // 无 OAuth 令牌 = 静态鉴权服务器，不干预
    };
    let now = crate::db::now_ms();
    if !t.expiring(now) {
        return Ok(false);
    }
    refresh_token(app, cfg, &t).await?;
    Ok(true)
}

/// 401 文本兜底（主动刷新漏网时）：错误文本含 401/403 且该服务器有 OAuth
/// 令牌 → 尝试 refresh 一次；refresh 也失败 → 清 token 要求重登
pub(crate) async fn maybe_refresh_on_auth_error(
    app: &tauri::AppHandle,
    cfg: &McpServerConfig,
    err_text: &str,
) -> bool {
    let auth_err = err_text.contains("401") || err_text.contains("403");
    if !auth_err {
        return false;
    }
    let Ok(Some(t)) = load_token(app, &cfg.id) else {
        return false;
    };
    match refresh_token(app, cfg, &t).await {
        Ok(_) => true,
        Err(_) => {
            oauth_purge(app, &cfg.id);
            false
        }
    }
}

#[tauri::command]
pub async fn mcp_oauth_login(
    app: tauri::AppHandle,
    server_id: String,
) -> crate::error::CommandResult<String> {
    login_impl(&app, &server_id)
        .await
        .map_err(crate::error::CommandError::Internal)
}

#[tauri::command]
pub fn mcp_oauth_logout(
    app: tauri::AppHandle,
    server_id: String,
) -> crate::error::CommandResult<()> {
    oauth_purge(&app, &server_id);
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Info,
        "mcp.oauth.logout",
        "server" => server_id,
    );
    Ok(())
}

#[tauri::command]
pub fn mcp_oauth_status(
    app: tauri::AppHandle,
    server_id: String,
) -> crate::error::CommandResult<serde_json::Value> {
    let t = load_token(&app, &server_id).map_err(crate::error::CommandError::Internal)?;
    Ok(serde_json::json!({
        "authorized": t.is_some(),
        "expiresAtMs": t.map(|x| x.expires_at_ms),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PKCE 对：verifier/challenge 非空且 challenge 是 verifier 的 S256 base64url
    #[test]
    fn pkce_pair_is_valid_s256() {
        let (v, c) = pkce_pair();
        assert!(!v.is_empty() && !c.is_empty());
        assert_ne!(v, c);
        let expect = base64url(&sha2::Sha256::digest(v.as_bytes()));
        assert_eq!(c, expect);
        // 两次调用不同（CSPRNG）
        let (v2, _) = pkce_pair();
        assert_ne!(v, v2);
    }

    /// 回调解析：code/state/error 三字段 + urldecode
    #[test]
    fn parse_callback_extracts_fields() {
        let (s, c, e) = parse_callback(
            "http://127.0.0.1:1/callback?code=a%20b&state=st%2Fx&error=access_denied",
        );
        assert_eq!(c.as_deref(), Some("a b"));
        assert_eq!(s.as_deref(), Some("st/x"));
        assert_eq!(e.as_deref(), Some("access_denied"));
    }

    /// state 不匹配 → Err（伪造回调防线由 login_impl 的 match 分支保证）
    #[test]
    fn token_bundle_roundtrip() {
        let t = TokenBundle {
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_at_ms: 12345,
            client_id: "cid".into(),
        };
        let back = TokenBundle::from_blob(&t.to_blob().unwrap()).unwrap();
        assert_eq!(back.access_token, "at");
        assert_eq!(back.refresh_token.as_deref(), Some("rt"));
        assert!(t.expiring(12345 + 30 * 1000), "已过期 30s 视为将过期");
        assert!(
            !t.expiring(12345 - 120 * 1000),
            "距过期还有 120s（大于提前窗 60s）不算将过期"
        );
    }

    /// 授权服务器根推导：有 authorization_servers 取之，否则 server 根
    #[test]
    fn auth_server_root_fallback() {
        let with = serde_json::json!({"authorization_servers": ["https://auth.example.com"]});
        assert_eq!(
            authorization_server_root(&with, "https://api.example.com").as_deref(),
            Some("https://auth.example.com")
        );
        let without = serde_json::json!({});
        assert_eq!(
            authorization_server_root(&without, "https://api.example.com").as_deref(),
            Some("https://api.example.com")
        );
    }

    /// token 响应解析：全字段 / 缺 access_token
    #[test]
    fn parse_token_response_fields() {
        let v = serde_json::json!({"access_token":"a","refresh_token":"r","expires_in":600});
        let (a, r, e) = parse_token_response(&v).unwrap();
        assert_eq!(a, "a");
        assert_eq!(r.as_deref(), Some("r"));
        assert_eq!(e, Some(600));
        assert!(parse_token_response(&serde_json::json!({})).is_err());
    }
}
