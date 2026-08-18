//! 联网工具：web_search（Bing 抓取）+ fetch_url（网页正文提取）。
//!
//! 安全设计（对齐 Harness 网关）：
//! - fetch_url 只允许 http/https；拒绝本机/内网地址（loopback/私网 IP 段/本地域名后缀）
//! - 超时：connect 15s / 总 30s；响应体上限 2MB；只处理 HTML/文本类内容
//! - 输出截断在工具层做（搜索结果 6000 字、网页正文 30000 字）；审计由 bot.rs 留痕

use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

const FETCH_MAX_BYTES: usize = 2 * 1024 * 1024;
const SEARCH_MAX_RESULTS: usize = 8;
const SEARCH_OUTPUT_CAP: usize = 6000;

/// 全局复用的 HTTP client（连接池复用，二次审计 P3：原先每请求新建 client）
fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(30))
                .build()
                // 理论不可达：builder 失败意味着超时配置无效
                .unwrap_or_else(|_| reqwest::Client::new())
        })
        .clone()
}

// ───────────────────────── 搜索（Bing + 百度双引擎） ─────────────────────────

/// 双引擎搜索：Bing + 百度并行，结果按标题去重合并，最多 8 条
pub async fn web_search(query: &str) -> Result<String, String> {
    let (bing, baidu) = futures_util::future::join(search_bing(query), search_baidu(query)).await;
    let mut merged: Vec<(String, String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for engine in [bing, baidu] {
        match engine {
            Ok(list) => {
                for (title, link, snip) in list {
                    let key = title.trim().to_string();
                    if seen.contains(&key) || merged.len() >= SEARCH_MAX_RESULTS {
                        continue;
                    }
                    seen.push(key);
                    merged.push((title, link, snip));
                }
            }
            Err(e) => errs.push(e),
        }
    }
    if merged.is_empty() {
        return Err(if errs.is_empty() {
            "搜索没有返回结果".into()
        } else {
            format!("所有搜索引擎均失败：{}", errs.join("；"))
        });
    }
    let mut out = String::new();
    for (i, (title, link, snip)) in merged.iter().enumerate() {
        out.push_str(&format!("{}. {}\n{}\n{}\n\n", i + 1, title, link, snip));
    }
    if out.chars().count() > SEARCH_OUTPUT_CAP {
        out = out.chars().take(SEARCH_OUTPUT_CAP).collect();
    }
    Ok(out)
}

/// Bing 搜索：解析 `<li class="b_algo">` 块，返回 (标题, 链接, 摘要) 列表
async fn search_bing(query: &str) -> Result<Vec<(String, String, String)>, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let target = format!("https://cn.bing.com/search?q={encoded}&mkt=zh-CN");
    let resp = http_client()
        .get(&target)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("Bing 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Bing 返回 HTTP {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取 Bing 结果失败：{e}"))?;
    let results = parse_bing(&body);
    if results.is_empty() {
        return Err("Bing 没有返回结果".into());
    }
    Ok(results)
}

/// 百度搜索：解析 `class="result"` 容器，返回 (标题, 跳转链接, 摘要) 列表
/// 链接为百度 /link?url 跳转链接（浏览器可打开；机器人 fetch 需换真实链接）
async fn search_baidu(query: &str) -> Result<Vec<(String, String, String)>, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let target = format!("https://www.baidu.com/s?wd={encoded}");
    let resp = http_client()
        .get(&target)
        .header(reqwest::header::USER_AGENT, UA)
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
        .send()
        .await
        .map_err(|e| format!("百度请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("百度返回 HTTP {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取百度结果失败：{e}"))?;
    let results = parse_baidu(&body);
    if results.is_empty() {
        return Err("百度没有返回结果（可能触发验证页）".into());
    }
    Ok(results)
}

/// 取第一个 `<open ...> ... </close>` 的中间内容
fn first_between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)?;
    let tail = &s[a..];
    let gt = tail.find('>')?;
    let start = gt + 1;
    let b = tail[start..].find(close)?;
    Some(&tail[start..start + b])
}

/// 取第一个 `<open ...>` 完整开标签
fn first_open_tag<'a>(s: &'a str, open: &str) -> Option<&'a str> {
    let a = s.find(open)?;
    let gt = s[a..].find('>')?;
    Some(&s[a..a + gt + 1])
}

/// 从 `<a ... href="X" ...>` 里提取 href
fn href_from_tag(tag: &str) -> Option<String> {
    let h = tag.find("href=")?;
    let after = &tag[h + 5..];
    let q = after.chars().next()?;
    let inner = &after[q.len_utf8()..];
    let end = inner.find(q)?;
    Some(inner[..end].to_string())
}

/// 去 HTML 标签 + 实体解码（含数字实体 &#NNN; / &#xNN;）
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).trim().to_string()
}

/// 解码命名实体 + 数字实体（&#NNN; / &#xHH;）
fn decode_entities(s: &str) -> String {
    let s = s
        .replace("&nbsp;", " ")
        .replace("&ensp;", " ")
        .replace("&emsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&hellip;", "…")
        .replace("&middot;", "·")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&ldquo;", "“")
        .replace("&rdquo;", "”");
    // 数字实体：&#NNN; 或 &#xHH;
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(pos) = rest.find("&#") {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos + 2..];
        let (digits, consumed) =
            if let Some(hex) = tail.strip_prefix('x').or_else(|| tail.strip_prefix('X')) {
                let end = hex.find(';').unwrap_or(hex.len());
                (&hex[..end], 1 + end + usize::from(end < hex.len()))
            } else {
                let end = tail.find(';').unwrap_or(tail.len());
                (&tail[..end], end + usize::from(end < tail.len()))
            };
        let radix = if tail.starts_with('x') || tail.starts_with('X') {
            16
        } else {
            10
        };
        if let Ok(n) = u32::from_str_radix(digits, radix) {
            if let Some(c) = char::from_u32(n) {
                out.push(c);
            }
        }
        rest = &tail[consumed.min(tail.len())..];
        if !tail.contains(';') {
            break;
        }
    }
    out.push_str(rest);
    out
}

/// 解析 Bing 结果页：`<li class="b_algo">` 块 → (标题, 链接, 摘要)
fn parse_bing(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<li class=\"b_algo\"") {
        rest = &rest[pos..];
        let end = rest
            .find("</li>")
            .map(|i| i + "</li>".len())
            .unwrap_or(rest.len());
        let block = &rest[..end];
        rest = &rest[end..];
        let Some(h2) = first_between(block, "<h2", "</h2>") else {
            continue;
        };
        let Some(a_tag) = first_open_tag(h2, "<a") else {
            continue;
        };
        let Some(a_text) = first_between(h2, "<a", "</a>") else {
            continue;
        };
        let title = strip_tags(a_text);
        if title.is_empty() {
            continue;
        }
        let link = href_from_tag(&a_tag).unwrap_or_default();
        let snip = first_between(block, "<p", "</p>")
            .map(strip_tags)
            .unwrap_or_default();
        out.push((title, link, snip));
    }
    out
}

/// 解析百度结果页：逐个 h3 里带 /link?url 的标题 → (标题, 跳转链接, 摘要)
fn parse_baidu(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<h3") {
        let seg = &rest[pos..];
        let Some(h3_close) = seg.find("</h3>") else {
            break;
        };
        let h3end = h3_close + "</h3>".len();
        let h3 = &seg[..h3end];
        let Some(a_tag) = first_open_tag(h3, "<a") else {
            rest = &seg[h3end..];
            continue;
        };
        let Some(link) = href_from_tag(&a_tag) else {
            rest = &seg[h3end..];
            continue;
        };
        if !link.contains("baidu.com/link") {
            rest = &seg[h3end..];
            continue;
        }
        let Some(a_text) = first_between(h3, "<a", "</a>") else {
            rest = &seg[h3end..];
            continue;
        };
        let title = strip_tags(a_text);
        if title.is_empty() {
            rest = &seg[h3end..];
            continue;
        }
        // 摘要：标题后面第一个有实质内容的 span（滤掉 JSON 垃圾）
        let mut tail_end = seg.len().min(h3end + 6000);
        while !seg.is_char_boundary(tail_end) {
            tail_end += 1;
        }
        let tail = &seg[h3end..tail_end];
        let snip = extract_baidu_snippet(tail);
        out.push((title, link, snip));
        rest = &seg[h3end..];
    }
    out
}

/// 从 h3 之后的文本里抽摘要：找第一个 ≥12 字且不含 JSON 垃圾的 span 文本，截 200 字
fn extract_baidu_snippet(tail: &str) -> String {
    let mut s = tail;
    while let Some(p) = s.find("<span") {
        let after = &s[p..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let start = gt + 1;
        let Some(close) = after[start..].find("</span>") else {
            break;
        };
        let text = strip_tags(&after[start..start + close]);
        if !text.is_empty()
            && text.chars().count() >= 12
            && !text.contains("clamp")
            && !text.contains("isSingleLine")
            && !text.contains("sutil")
        {
            return text.chars().take(200).collect();
        }
        s = &after[start + close + "</span>".len()..];
    }
    String::new()
}

// ───────────────────────── 网页抓取 ─────────────────────────

/// 校验 URL：协议 + 主机名字符串 + DNS 解析出的所有 IP 均须公网（防 DNS 重绑定/内网域名）
async fn check_public_url(url: &url::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("只支持 http/https 链接（收到 {s}://）")),
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_lowercase();
    if host.is_empty() {
        return Err("网址缺少主机名".into());
    }
    if is_private_host(&host) {
        return Err("已拒绝访问本机/内网地址".into());
    }
    // DNS 解析校验：域名解析出的每个 IP 都必须是公网（防解析到 127.0.0.1 的内网域名）
    let mut any = false;
    // tokio::net::lookup_host 返回同步迭代器（解析已在 await 内完成）
    let addrs = tokio::net::lookup_host((host.as_str(), 80))
        .await
        .map_err(|e| format!("域名解析失败：{e}"))?;
    for addr in addrs {
        any = true;
        match addr.ip() {
            std::net::IpAddr::V4(v4) => {
                if ipv4_is_private(v4) {
                    return Err("已拒绝：域名解析到本机/内网地址".into());
                }
            }
            std::net::IpAddr::V6(v6) => {
                if v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unique_local()
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                {
                    return Err("已拒绝：域名解析到本机/内网地址".into());
                }
            }
        }
    }
    if !any {
        return Err("域名没有解析到任何地址".into());
    }
    Ok(())
}

/// 抓取网页正文：http/https、公网地址校验（含 DNS 解析与重定向逐跳）、HTML→纯文本、GBK 兜底解码
pub async fn fetch_text(raw_url: &str) -> Result<String, String> {
    // 重定向逐跳校验：不跟随 reqwest 自动重定向，3xx 时手动校验 Location 目标
    //（公网 URL 302 到内网地址是 SSRF 常见绕过，审计 P1）
    const MAX_REDIRECTS: usize = 5;
    let mut url_cursor = url::Url::parse(raw_url.trim()).map_err(|_| "网址格式无效".to_string())?;
    let mut hops = 0usize;
    let mut resp;
    loop {
        check_public_url(&url_cursor).await?;
        resp = http_client()
            .get(url_cursor.clone())
            .header(reqwest::header::USER_AGENT, UA)
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            if hops >= MAX_REDIRECTS {
                return Err(format!("重定向超过 {MAX_REDIRECTS} 次，已停止"));
            }
            let Some(loc) = resp.headers().get(reqwest::header::LOCATION) else {
                return Err(format!("网页返回重定向 {status} 但缺少 Location"));
            };
            let loc = loc.to_str().map_err(|_| "Location 头编码无效")?.to_string();
            url_cursor = url_cursor
                .join(&loc)
                .map_err(|_| format!("重定向目标无效：{loc}"))?;
            hops += 1;
            continue;
        }
        break;
    }
    if !resp.status().is_success() {
        return Err(format!("网页返回 HTTP {}", resp.status()));
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
        return Err(format!("不是网页文本（Content-Type: {ctype}）"));
    }
    if let Some(len) = resp.content_length() {
        if len > FETCH_MAX_BYTES as u64 {
            return Err("页面过大（超过 2MB）已拒绝".into());
        }
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取失败：{e}"))?;
    if bytes.len() > FETCH_MAX_BYTES {
        return Err("页面过大（超过 2MB）已拒绝".into());
    }
    let text = decode_html(&bytes);
    let plain = html2text::from_read(&mut text.as_bytes(), 120)
        .map_err(|e| format!("解析 HTML 失败：{e}"))?
        .trim()
        .to_string();
    if plain.is_empty() {
        return Err("页面没有可提取的文本内容".into());
    }
    Ok(plain)
}

/// IPv4 是否为内网/本机段
fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
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
fn is_private_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => ipv4_is_private(v4),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unique_local()
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bing_fixture() {
        let html = std::fs::read_to_string("/tmp/bing.html").unwrap_or_default();
        if html.is_empty() {
            eprintln!("fixture 不存在，跳过");
            return;
        }
        let results = parse_bing(&html);
        assert!(!results.is_empty(), "fixture 应能解析出结果");
        assert!(results[0].1.starts_with("http"), "首条应带链接");
        eprintln!(
            "parsed {} results, first: {} | {}",
            results.len(),
            results[0].0,
            results[0].1
        );
    }

    #[test]
    fn parse_baidu_fixture() {
        let html = std::fs::read_to_string("/tmp/baidu.html").unwrap_or_default();
        if html.is_empty() {
            eprintln!("fixture 不存在，跳过");
            return;
        }
        let results = parse_baidu(&html);
        assert!(!results.is_empty(), "百度 fixture 应能解析出结果");
        assert!(
            results[0].1.contains("baidu.com/link"),
            "链接应为百度跳转链接"
        );
        for (t, l, s) in results.iter().take(5) {
            eprintln!(
                "baidu: {} | {} | snip {} 字",
                t.chars().take(30).collect::<String>(),
                l.chars().take(40).collect::<String>(),
                s.chars().count()
            );
        }
    }

    #[test]
    fn private_host_guard() {
        for h in [
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "172.16.0.1",
            "169.254.1.1",
            "localhost",
            "nas.local",
            "x.internal",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(is_private_host(h), "应拦截 {h}");
        }
        for h in ["www.bing.com", "example.com", "api.minimaxi.com"] {
            assert!(!is_private_host(h), "不应拦截 {h}");
        }
    }

    #[test]
    fn strip_tags_entities() {
        assert_eq!(
            strip_tags("<p>你好 &amp; 世界&nbsp;！</p>"),
            "你好 & 世界 ！"
        );
        assert_eq!(strip_tags("<a href=\"x\">标题</a>"), "标题");
    }

    #[test]
    fn network_search_and_fetch() {
        // 真实网络测试：环境可达时验证（cn.bing.com 从 Mac 可达）
        match tauri::async_runtime::block_on(web_search("北京今天天气")) {
            Ok(out) => {
                assert!(out.contains("http"), "搜索结果应带链接");
                eprintln!("SEARCH OK:\n{}", out.chars().take(200).collect::<String>());
            }
            Err(e) => eprintln!("search failed (环境相关): {e}"),
        }
        match tauri::async_runtime::block_on(fetch_text("https://example.com")) {
            Ok(t) => eprintln!(
                "FETCH OK, {} chars, head: {}",
                t.chars().count(),
                t.chars().take(80).collect::<String>()
            ),
            Err(e) => eprintln!("fetch failed (环境相关): {e}"),
        }
        // 内网地址应被拒
        let blocked =
            tauri::async_runtime::block_on(fetch_text("http://127.0.0.1:4763/api/health"));
        assert!(blocked.is_err(), "本机地址必须被拒绝");
        let bad_scheme = tauri::async_runtime::block_on(fetch_text("file:///etc/hosts"));
        assert!(bad_scheme.is_err(), "非 http(s) 协议必须被拒绝");
    }
}
