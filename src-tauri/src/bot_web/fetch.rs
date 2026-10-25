//! 抓取面：公网闸（拒绝本机/内网）+ 正文提取 + 渲染代理兜底。

use super::html::extract_main_content;
use super::*;

impl UrlGateError {
    /// 裸原因（不含域标签）。
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Denied { reason } | Self::Unresolved { reason } => reason,
        }
    }

    /// 公网闸判定的展示文案（带 `[web] ` 域标签）。
    pub(crate) fn gate_message(&self) -> String {
        format!("[web] {}", self.reason())
    }

    /// 补上 `[web] ` 域标签（公网闸判定用；参数级错误不带标签）。
    fn labeled(self) -> Self {
        match self {
            Self::Denied { reason } => Self::Denied {
                reason: format!("[web] {reason}"),
            },
            Self::Unresolved { reason } => Self::Unresolved {
                reason: format!("[web] {reason}"),
            },
        }
    }
}
/// 校验 URL 并返回通过校验的解析地址：协议 + 主机名字符串 + DNS 解析出的所有 IP
/// 均须公网（防 DNS 重绑定/内网域名）。返回的地址由调用方钉给 reqwest
/// （resolve_to_addrs，FIX-PLAN #7a），校验与请求之间不再二次解析，堵 DNS TOCTOU。
pub(crate) async fn check_public_url(
    url: &url::Url,
) -> Result<Vec<std::net::SocketAddr>, UrlGateError> {
    check_public_url_with(url, |host| async move {
        tokio::net::lookup_host((host.as_str(), 80))
            .await
            .map(|it| it.collect())
            .map_err(|e| e.to_string())
    })
    .await
}

/// 公网闸的判定主体：协议 + 主机名 + 解析结果 → 公网地址集合。
///
/// 解析结果由调用方给：生产走真实 DNS，测试给桩。解析要触网，而「解析失败」
/// 与「解析到内网」这两条判定必须能被离线钉住——真 DNS 依赖网络环境，
/// 无 DNS 的机器上会一直等到系统超时。
pub(super) async fn check_public_url_with<F, Fut>(
    url: &url::Url,
    lookup: F,
) -> Result<Vec<std::net::SocketAddr>, UrlGateError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<std::net::SocketAddr>, String>>,
{
    match url.scheme() {
        "http" | "https" => {}
        s => {
            return Err(UrlGateError::Denied {
                reason: format!("只支持 http/https 链接（收到 {s}://）"),
            })
        }
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_lowercase();
    if host.is_empty() {
        return Err(UrlGateError::Denied {
            reason: "网址缺少主机名".to_string(),
        });
    }
    if is_private_host(&host) {
        return Err(UrlGateError::Denied {
            reason: "已拒绝访问本机/内网地址".to_string(),
        });
    }
    // DNS 解析校验：域名解析出的每个 IP 都必须是公网（防解析到 127.0.0.1 的内网域名）
    let mut out: Vec<std::net::SocketAddr> = Vec::new();
    // 端口与解析无关，查完由 with_url_port 统一重写成 URL 真实端口
    let addrs = lookup(host.clone())
        .await
        .map_err(|e| UrlGateError::Unresolved {
            reason: format!("域名解析失败：{e}"),
        })?;
    for addr in addrs {
        match addr.ip() {
            std::net::IpAddr::V4(v4) => {
                if ipv4_is_private(v4) {
                    return Err(UrlGateError::Denied {
                        reason: "已拒绝：域名解析到本机/内网地址".to_string(),
                    });
                }
            }
            std::net::IpAddr::V6(v6) => {
                if ipv6_is_private(v6) {
                    return Err(UrlGateError::Denied {
                        reason: "已拒绝：域名解析到本机/内网地址".to_string(),
                    });
                }
            }
        }
        out.push(addr);
    }
    if out.is_empty() {
        return Err(UrlGateError::Unresolved {
            reason: "域名没有解析到任何地址".to_string(),
        });
    }
    Ok(with_url_port(out, url))
}

/// 解析结果重写成 URL 真实端口（显式端口优先，否则 scheme 默认 443/80）。
/// reqwest 0.13 的 resolve_to_addrs 在 https 场景按 addr 自带端口连接（URL 端口
/// 不覆盖）——曾因 addr 端口用 80 占位导致所有 https 抓取 TLS 发去 80 端口被回
/// InvalidContentType、fetch_url 全挂（实测定位）。
pub(super) fn with_url_port(
    addrs: Vec<std::net::SocketAddr>,
    url: &url::Url,
) -> Vec<std::net::SocketAddr> {
    let port = url.port_or_known_default().unwrap_or(80);
    addrs
        .into_iter()
        .map(|mut a| {
            a.set_port(port);
            a
        })
        .collect()
}

/// 正文薄到多少字符就当「没抓到」：静态抓取拿到 JS 空壳时正文近于 0，
/// 低于此值改走渲染代理。经验阈值，不是协议约束。
const THIN_BODY_CHARS: usize = 100;

/// 直连失败的两类，决定还有没有「换一个出口再试」的价值。
pub(super) enum FetchErr {
    /// 目标明确拒绝：网址非法、SSRF 闸拦截、重定向策略拒绝、4xx、超大页面。
    /// 换代理只会拿到同一个拒绝，还白等一次。
    Rejected(CommandError),
    /// 这次请求没走通：连接超时/被拒、5xx、响应不是网页文本、正文解析不出来。
    /// 本机到目标的那条路不通（或目标在报错），但渲染代理走的是另一条出网路径。
    Unreachable(String),
}

/// 状态码该不该换代理再试（纯函数，供测试钉住）。
///
/// 5xx/408 是目标站在自己那边出错，换个出口往往能拿到它渲染后的内容；
/// 4xx 是目标明确表态——404 资源不存在、403 不许访问，代理会拿到同一个答案。
/// 400 这类请求本身有问题的不在考虑内：它们本来就到不了这一步。
pub(super) fn status_worth_proxy_retry(status: u16) -> bool {
    (500..600).contains(&status) || status == 408
}

/// 抓取网页正文：直连一次，不成再换渲染代理出网一次。
///
/// 直连通了但正文太短（JS 渲染的空壳），和直连压根没走通（连接超时、5xx、
/// 返回的不是网页文本），都汇到同一条兜底路上——后者过去是直接失败的，于是
/// 「本机到某个站点不通」直接等于「这个页面抓不到」，哪怕换个出口就能拿到。
/// 隐私边界不变：目标 URL 会发给 r.jina.ai（公网地址本身，低风险），
/// 且只发到公网代理一处，不做二次转发。
pub async fn fetch_text(raw_url: &str) -> Result<String, CommandError> {
    let (mut plain, why_thin) = match fetch_direct(raw_url).await {
        Ok(text) => (text, "正文过短（多半是 JS 渲染的空壳）".to_string()),
        Err(FetchErr::Rejected(e)) => return Err(e),
        Err(FetchErr::Unreachable(why)) => (String::new(), why),
    };
    // 留痕：回退属基本审计面——采用/未采用/失败三路 stderr 记录。
    if plain.chars().count() < THIN_BODY_CHARS {
        match fetch_jina_reader(raw_url).await {
            Ok(jina) => {
                if jina.chars().count() > plain.chars().count() {
                    eprintln!("[bot_web] Jina Reader 回退并采用：{raw_url}");
                    plain = jina;
                } else {
                    eprintln!("[bot_web] Jina Reader 回退未采用（不优于直连）：{raw_url}");
                }
            }
            Err(e) => eprintln!("[bot_web] Jina Reader 回退失败：{raw_url} | {e}"),
        }
    }
    if plain.is_empty() {
        // 直连空正文 + 代理也没拿到：报直连的实因（超时/5xx），别报成
        // 「页面没有文本」——那会把排查引向目标页面，而真因在本机到目标的路上。
        return Err(CommandError::Internal(format!(
            "两条路都没拿到内容：直连{why_thin}；渲染代理回退也没成功（免费公共代理限流/超时时会这样）"
        )));
    }
    Ok(plain)
}

/// 直连抓取：逐跳校验公网地址、钉 IP、手工跟随重定向、HTML→纯文本。
///
/// 与 `fetch_text` 的分工：这一层只管「直连这一条路」，成功就直接给正文；
/// 失败时用 `FetchErr` 说明失败属于「换个出口有救」还是「换也没用」，
/// 免得编排层把 SSRF 拦截也送去第三方代理重试一遍。
pub(super) async fn fetch_direct(raw_url: &str) -> Result<String, FetchErr> {
    // 重定向逐跳校验：不跟随 reqwest 自动重定向，3xx 时手动校验 Location 目标
    //（公网 URL 302 到内网地址是 SSRF 常见绕过）
    const MAX_REDIRECTS: usize = 5;
    let mut url_cursor =
        url::Url::parse(raw_url.trim()).map_err(|_| FetchErr::Rejected("网址格式无效".into()))?;
    let mut hops = 0usize;
    let mut resp;
    loop {
        // 每跳都走「解析 → 校验 → 钉 IP」：check_public_url 返回校验过的地址，
        // per-hop client 用 resolve_to_addrs 钉住，reqwest 不再二次 DNS（FIX-PLAN #7a DNS TOCTOU）
        let addrs = check_public_url(&url_cursor).await.map_err(|e| {
            FetchErr::Rejected(CommandError::DomainRule {
                domain: "web".to_string(),
                reason: e.reason().to_string(),
            })
        })?;
        resp = http_client_pinned(&url_cursor, &addrs)
            .map_err(|e| FetchErr::Rejected(e.into()))?
            .get(url_cursor.clone())
            .header(reqwest::header::USER_AGENT, UA)
            .send()
            .await
            .map_err(|e| FetchErr::Unreachable(format!("请求失败：{}", err_chain(&e))))?;
        let status = resp.status();
        if status.is_redirection() {
            if hops >= MAX_REDIRECTS {
                return Err(FetchErr::Rejected(CommandError::DomainRule {
                    domain: "web".to_string(),
                    reason: format!("重定向超过 {MAX_REDIRECTS} 次，已停止"),
                }));
            }
            let Some(loc) = resp.headers().get(reqwest::header::LOCATION) else {
                return Err(FetchErr::Rejected(CommandError::DomainRule {
                    domain: "web".to_string(),
                    reason: format!("网页返回重定向 {status} 但缺少 Location"),
                }));
            };
            let loc = loc
                .to_str()
                .map_err(|_| FetchErr::Rejected("Location 头编码无效".into()))?
                .to_string();
            let prev_scheme = url_cursor.scheme().to_string();
            url_cursor = url_cursor
                .join(&loc)
                .map_err(|_| FetchErr::Rejected(format!("重定向目标无效：{loc}").into()))?;
            // https→http 降级重定向拒：首跳 TLS 不保护后续明文跳（中间人可篡改返回内容）；
            // 用户直接给 http 起始 URL 不受影响（既有语义）
            if prev_scheme == "https" && url_cursor.scheme() == "http" {
                return Err(FetchErr::Rejected(CommandError::DomainRule {
                    domain: "web".to_string(),
                    reason: format!(
                        "拒绝 https→http 降级重定向：{}",
                        crate::bot::escape_for_log(&loc, 100)
                    ),
                }));
            }
            hops += 1;
            continue;
        }
        break;
    }
    let status = resp.status();
    if !status.is_success() {
        // 带最终 URL：重定向之后报 404/5xx 时，模型据此才知道真正缺的是哪个地址
        // （它给的原地址可能是活的，挂在重定向目标上）。
        let reason = format!("网页返回 HTTP {status}（{url_cursor}）");
        return if status_worth_proxy_retry(status.as_u16()) {
            // 5xx 是目标站在自己那边出错，换出口还有救
            Err(FetchErr::Unreachable(reason))
        } else {
            Err(FetchErr::Rejected(CommandError::DomainRule {
                domain: "web".to_string(),
                reason,
            }))
        };
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    if !ctype.is_empty()
        && !ctype.contains("html")
        && !ctype.contains("xml")
        && !ctype.contains("text")
    {
        return Err(FetchErr::Unreachable(format!(
            "不是网页文本（Content-Type: {ctype}）"
        )));
    }
    if let Some(len) = resp.content_length() {
        if len > FETCH_MAX_BYTES as u64 {
            return Err(FetchErr::Rejected(CommandError::DomainRule {
                domain: "web".to_string(),
                reason: format!(
                    "页面过大（超过 {}MB）已拒绝",
                    FETCH_MAX_BYTES / (1024 * 1024)
                ),
            }));
        }
    }
    let bytes = read_body_capped(resp, FETCH_MAX_BYTES)
        .await
        .map_err(FetchErr::Unreachable)?;
    let text = decode_html(&bytes);
    // 先抽正文主块再转纯文本（去导航/广告/页脚）；提取结果过短说明误伤，退回整页转换
    let main = extract_main_content(&text);
    let mut plain = html2text::from_read(&mut main.as_bytes(), 120)
        .map_err(|e| FetchErr::Unreachable(format!("解析 HTML 失败：{e}")))?
        .trim()
        .to_string();
    if plain.chars().count() < 100 && main.len() != text.len() {
        if let Ok(full) = html2text::from_read(&mut text.as_bytes(), 120) {
            if full.trim().chars().count() > plain.chars().count() {
                plain = full.trim().to_string();
            }
        }
    }
    Ok(plain)
}

/// Jina Reader 代理 URL（纯函数便于单测）：https://r.jina.ai/<原 URL>
pub(super) fn jina_reader_url(raw_url: &str) -> String {
    format!("https://r.jina.ai/{}", raw_url.trim())
}

/// open_url 工具的 URL 准入门——parse + 仅 http/https + 与 fetch 同一公网闸
/// （DNS 解析后拒绝本机/内网/保留地址）。返回校验过的 URL 供打开与日志。
///
/// 两类失败分开返回：策略拒绝（换地址也没用）与 DNS 解析失败（网络故障）。
pub(crate) async fn ensure_public_http_url(raw: &str) -> Result<url::Url, UrlGateError> {
    let trimmed = raw.trim();
    let url = url::Url::parse(trimmed).map_err(|_| UrlGateError::Denied {
        reason: format!("无效网址：{trimmed}"),
    })?;
    let scheme = url.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(UrlGateError::Denied {
            reason: format!("仅支持 http/https 网页地址，拒绝 {scheme}: 协议"),
        });
    }
    check_public_url(&url)
        .await
        .map_err(UrlGateError::labeled)?;
    Ok(url)
}

/// Jina Reader 回退抓取：公共代理服务端渲染页面返回 markdown 文本。
/// 失败（超时/限流/目标不可达）由调用方忽略，不影响主路径。
async fn fetch_jina_reader(raw_url: &str) -> Result<String, String> {
    let url =
        url::Url::parse(&jina_reader_url(raw_url)).map_err(|_| "Jina URL 无效".to_string())?;
    let addrs = check_public_url(&url).await.map_err(|e| e.gate_message())?;
    let resp = http_client_pinned(&url, &addrs)?
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("Jina 请求失败：{}", err_chain(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("Jina 返回 HTTP {}", resp.status()));
    }
    // 与 fetch_text 同一道 Content-Type 白名单：代理异常时返回的错误 JSON/
    // 二进制不得原样混进正文（r.jina.ai 正常返回 text/plain 走 "text" 分支）
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    if !ctype.is_empty()
        && !ctype.contains("text")
        && !ctype.contains("markdown")
        && !ctype.contains("html")
        && !ctype.contains("xml")
    {
        return Err(format!("Jina 返回非文本内容（Content-Type: {ctype}）"));
    }
    let bytes = read_body_capped(resp, FETCH_MAX_BYTES).await?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// IPv4 是否为内网/本机段
pub(super) fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        // CGNAT 段 100.64.0.0/10（RFC 6598，Tailscale/运营商大内网）：
        // std is_private 不含此段，须显式判定
        || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
}

/// IPv6 是否为内网/本机段（含 IPv4-mapped——
/// AAAA 应答 `::ffff:127.0.0.1` 时 hyper 连接会映射回 IPv4 打内网，须一并判定）
pub(super) fn ipv6_is_private(v6: std::net::Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return ipv4_is_private(v4);
    }
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_unique_local()
        || (v6.segments()[0] & 0xffc0) == 0xfe80
}

/// 整数/十六进制/八进制形式的 IPv4 字面量识别（"2130706433"、"0x7f000001"、"017700000001"）
fn parse_alt_ipv4(host: &str) -> Option<std::net::Ipv4Addr> {
    let (num_str, radix): (Option<&str>, u32) = if let Some(hex) =
        host.strip_prefix("0x").or_else(|| host.strip_prefix("0X"))
    {
        (Some(hex), 16)
    } else if host.len() > 1 && host.starts_with('0') && host.chars().all(|c| c.is_ascii_digit()) {
        (Some(&host[1..]), 8)
    } else if !host.is_empty() && host.chars().all(|c| c.is_ascii_digit()) {
        (Some(host), 10)
    } else {
        (None, 10)
    };
    let n = u64::from_str_radix(num_str?, radix).ok()?;
    (n <= u32::MAX as u64).then(|| std::net::Ipv4Addr::from(n as u32))
}

/// 本机/内网地址判断：IP 字面量（含十进制/十六进制/八进制整数形式）查段，主机名查本地后缀
pub(super) fn is_private_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => ipv4_is_private(v4),
            std::net::IpAddr::V6(v6) => ipv6_is_private(v6),
        };
    }
    // 整数/十六进制/八进制 IPv4 字面量（SSRF 常见绕过形式）
    if let Some(v4) = parse_alt_ipv4(host) {
        return ipv4_is_private(v4);
    }
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
}

/// HTML 字节 → 文本：嗅探 meta charset（GB18030/GB2312/GBK/Big5），默认 UTF-8 宽松解码
fn decode_html(bytes: &[u8]) -> String {
    let head_len = bytes.len().min(4096);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_lowercase();
    let candidates: [(Option<&'static encoding_rs::Encoding>, &[&str]); 4] = [
        (
            encoding_rs::Encoding::for_label(b"gb18030"),
            &["charset=gb18030", "charset=\"gb18030\""],
        ),
        (
            encoding_rs::Encoding::for_label(b"gb2312"),
            &["charset=gb2312", "charset=\"gb2312\""],
        ),
        (
            encoding_rs::Encoding::for_label(b"gbk"),
            &["charset=gbk", "charset=\"gbk\""],
        ),
        (
            encoding_rs::Encoding::for_label(b"big5"),
            &["charset=big5", "charset=\"big5\""],
        ),
    ];
    for (enc, needles) in candidates {
        if let Some(enc) = enc {
            if needles.iter().any(|n| head.contains(n)) {
                let (text, _, _) = enc.decode(bytes);
                return text.into_owned();
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}
