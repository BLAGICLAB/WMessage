//! 联网工具：web_search（Bing 抓取）+ fetch_url（网页正文提取）。
//!
//! 安全设计（对齐 Harness 网关）：
//! - fetch_url 只允许 http/https；拒绝本机/内网地址（loopback/私网 IP 段/本地域名后缀）
//! - 超时：connect 15s / 总 30s；响应体上限 2MB；只处理 HTML/文本类内容
//! - 输出截断在工具层做（搜索结果 6000 字、网页正文 30000 字）；审计由 bot.rs 留痕

use std::time::Duration;
use futures_util::StreamExt;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

const FETCH_MAX_BYTES: usize = 2 * 1024 * 1024;
const SEARCH_MAX_RESULTS: usize = 8;
const SEARCH_OUTPUT_CAP: usize = 6000;

/// 全局复用的 HTTP client（连接池复用，二次审计 P3：原先每请求新建 client）
/// 2026-08-27 安全审计 SEC-P0-1：禁用自动重定向——原先默认 policy 自动跟随 10 跳，
/// fetch_text 里的「3xx 逐跳校验 Location」是死代码，公网 URL 302 到内网地址可绕过
/// check_public_url（SSRF）。禁自动重定向后由 fetch_text 手工逐跳校验接管。
fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            http_client_builder()
                .build()
                // 理论不可达：builder 失败意味着超时配置无效
                .unwrap_or_else(|_| reqwest::Client::new())
        })
        .clone()
}

/// client 公共配置（超时 + 禁自动重定向）；钉 IP 的 per-request client 复用同款
fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
}

/// 钉住解析结果的 client（FIX-PLAN #7a DNS TOCTOU）：host 固定解析到
/// check_public_url 校验过的地址，reqwest 不再二次 DNS——校验与请求之间
/// 攻击者改 DNS 应答的窗口被关掉。TLS SNI/证书校验仍按原域名；
/// 端口以 URL 为准（reqwest 用目标端口覆盖，addrs 里的端口仅占位）。
fn http_client_pinned(
    url: &url::Url,
    addrs: &[std::net::SocketAddr],
) -> Result<reqwest::Client, String> {
    let host = url.host_str().unwrap_or_default();
    http_client_builder()
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))
}

// ───────────────────────── 搜索（Bing + 百度双引擎） ─────────────────────────

/// 双引擎搜索：Bing + 百度并行，结果按标题去重合并，最多 8 条
/// 同一域名最多保留条数（百度跳转链接除外——host 都是 baidu.com，去重会误杀）
const SEARCH_MAX_PER_DOMAIN: usize = 2;

/// 取链接的域名（解析失败返回空串）
fn domain_of(link: &str) -> String {
    url::Url::parse(link)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default()
}

/// 摘要清理：压缩空白、去结尾省略号残留（抓取的摘要常被截断带「……」尾巴）
fn clean_snippet(s: &str) -> String {
    let mut t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for suf in ["……", "...", "…"] {
        if let Some(x) = t.strip_suffix(suf) {
            t = x.trim_end().to_string();
        }
    }
    t
}

pub async fn web_search(query: &str) -> Result<String, String> {
    let (bing, baidu) = futures_util::future::join(search_bing(query), search_baidu(query)).await;
    // (引擎, 标题, 链接, 摘要)
    let mut merged: Vec<(&str, String, String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut domain_count: Vec<(String, usize)> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for (engine, result) in [("Bing", bing), ("百度", baidu)] {
        match result {
            Ok(list) => {
                for (title, link, snip) in list {
                    let key = title.trim().to_string();
                    if seen.contains(&key) || merged.len() >= SEARCH_MAX_RESULTS {
                        continue;
                    }
                    // 域名去重：同一站点最多 2 条（百度跳转链接除外，host 全是 baidu.com）
                    let d = domain_of(&link);
                    if !d.is_empty() && d != "baidu.com" && !d.ends_with(".baidu.com") {
                        let cnt = domain_count
                            .iter()
                            .find(|(dom, _)| *dom == d)
                            .map(|(_, c)| *c)
                            .unwrap_or(0);
                        if cnt >= SEARCH_MAX_PER_DOMAIN {
                            continue;
                        }
                        match domain_count.iter_mut().find(|(dom, _)| *dom == d) {
                            Some((_, c)) => *c += 1,
                            None => domain_count.push((d, 1)),
                        }
                    }
                    seen.push(key);
                    merged.push((engine, title, link, clean_snippet(&snip)));
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
    for (i, (engine, title, link, snip)) in merged.iter().enumerate() {
        out.push_str(&format!("{}. [{}] {}\n{}\n{}\n\n", i + 1, engine, title, link, snip));
    }
    if out.chars().count() > SEARCH_OUTPUT_CAP {
        out = out.chars().take(SEARCH_OUTPUT_CAP).collect();
    }
    Ok(out)
}

/// Tavily 搜索 API（设置页「Tavily 搜索」开关开启后 web_search 走这里）
async fn search_tavily(key: &str, query: &str) -> Result<String, String> {
    let resp = http_client()
        .post("https://api.tavily.com/search")
        .json(&serde_json::json!({
            "api_key": key,
            "query": query,
            "max_results": SEARCH_MAX_RESULTS,
            "search_depth": "basic",
            "include_answer": false
        }))
        .send()
        .await
        .map_err(|e| format!("Tavily 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Tavily 返回 HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Tavily 响应解析失败：{e}"))?;
    let arr = v["results"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return Err("Tavily 没有返回结果".into());
    }
    let mut out = String::new();
    for (i, r) in arr.iter().enumerate() {
        let title = r["title"].as_str().unwrap_or("");
        let url = r["url"].as_str().unwrap_or("");
        let content = clean_snippet(r["content"].as_str().unwrap_or(""));
        out.push_str(&format!("{}. [Tavily] {}\n{}\n{}\n\n", i + 1, title, url, content));
    }
    if out.chars().count() > SEARCH_OUTPUT_CAP {
        out = out.chars().take(SEARCH_OUTPUT_CAP).collect();
    }
    Ok(out)
}

/// Brave Web Search API（2026-09-05，设置页「Brave 搜索」开关开启后 web_search 走这里）：
/// GET https://api.search.brave.com/res/v1/web/search，key 走 X-Subscription-Token 头。
/// 与 Tavily 互斥（同时开启明确报错，见 resolve_search_route）。
async fn search_brave(key: &str, query: &str) -> Result<String, String> {
    // 查询串手工编码（同 search_bing 的 form_urlencoded 风格；reqwest 0.13 无 .query()）
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let target =
        format!("https://api.search.brave.com/res/v1/web/search?q={encoded}&count=8");
    let resp = http_client()
        .get(&target)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        .map_err(|e| format!("Brave 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Brave 返回 HTTP {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Brave 响应读取失败：{e}"))?;
    let results = parse_brave_results(&body)?;
    if results.is_empty() {
        return Err("Brave 没有返回结果".into());
    }
    let mut out = String::new();
    for (i, (title, url, desc)) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. [Brave] {}\n{}\n{}\n\n",
            i + 1,
            title,
            url,
            clean_snippet(desc)
        ));
    }
    if out.chars().count() > SEARCH_OUTPUT_CAP {
        out = out.chars().take(SEARCH_OUTPUT_CAP).collect();
    }
    Ok(out)
}

/// Brave 响应解析（纯函数，无网络可测）：`{"web":{"results":[{title,url,description}]}}`
/// → (标题, 链接, 摘要) 列表。web 字段缺失按空结果处理（防御）；JSON 非法明确报错。
fn parse_brave_results(body: &str) -> Result<Vec<(String, String, String)>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Brave 响应解析失败：{e}"))?;
    let arr = v["web"]["results"].as_array().cloned().unwrap_or_default();
    Ok(arr
        .iter()
        .map(|r| {
            (
                r["title"].as_str().unwrap_or("").to_string(),
                r["url"].as_str().unwrap_or("").to_string(),
                r["description"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect())
}

/// 分流决策（纯函数，可测）：Tavily/Brave 两组开关 + key → 走哪条搜索路径
#[derive(Debug, PartialEq)]
enum SearchRoute {
    /// Bing+百度双引擎抓取
    Dual,
    /// Tavily API（带 trim 后的 key）
    Tavily(String),
    /// Brave Web Search API（带 trim 后的 key，2026-09-05）
    Brave(String),
    /// Tavily 开关开了但没填 key
    MissingKey,
    /// Brave 开关开了但没填 key（2026-09-05）
    MissingBraveKey,
    /// Tavily 与 Brave 同时开启（2026-09-05）：互斥，明确报错不静默猜
    Conflict,
}

/// 开关语义：None（老配置从未显式设置）保持旧行为——配了 key 就当开启；
/// Some(false) 强制不走对应引擎（即使配了 key）；Some(true) 强制走对应引擎。
/// Brave 优先判定（与 Tavily 同时开启 → Conflict 明确报错）；Tavily 逻辑保持原样。
fn resolve_search_route(
    tavily_enabled: Option<bool>,
    tavily_key: Option<&str>,
    brave_enabled: Option<bool>,
    brave_key: Option<&str>,
) -> SearchRoute {
    let tkey = tavily_key.unwrap_or("").trim().to_string();
    let bkey = brave_key.unwrap_or("").trim().to_string();
    let t_on = tavily_enabled.unwrap_or(!tkey.is_empty());
    let b_on = brave_enabled.unwrap_or(!bkey.is_empty());
    if t_on && b_on {
        return SearchRoute::Conflict;
    }
    if b_on {
        return if bkey.is_empty() {
            SearchRoute::MissingBraveKey
        } else {
            SearchRoute::Brave(bkey)
        };
    }
    match (t_on, tkey.is_empty()) {
        (false, _) => SearchRoute::Dual,
        (true, true) => SearchRoute::MissingKey,
        (true, false) => SearchRoute::Tavily(tkey),
    }
}

/// 搜索入口（bot 工具调用）：按设置页「Tavily 搜索」/「Brave 搜索」开关分流——
/// 都关 → Bing+百度双引擎；开一个 → 对应 API。开了但没填 key / 双开 / 请求失败都
/// 明确报错回传给模型（不静默回退百度，避免「以为在用 API 实际走的百度」）。
/// 2026-09-05：Tavily/Brave key 改从系统凭据存储读（不再明文落 bot-config.json）；
/// keyring 真实故障按无 key 处理（走 MissingKey 报错文案引导用户去设置页），
/// 不弄挂 web_search 工具本身。
pub async fn web_search_with_config(app: &tauri::AppHandle, query: &str) -> Result<String, String> {
    let cfg = crate::bot::load_config(app);
    let tavily_key = crate::bot::read_search_key(crate::bot::KeySlot::Tavily).unwrap_or_default();
    let brave_key = crate::bot::read_search_key(crate::bot::KeySlot::Brave).unwrap_or_default();
    match resolve_search_route(
        cfg.tavily_enabled,
        Some(tavily_key.as_str()),
        cfg.brave_enabled,
        Some(brave_key.as_str()),
    ) {
        SearchRoute::Dual => web_search(query).await,
        SearchRoute::MissingKey => Err(
            "Tavily 搜索已开启，但设置页还没填 Tavily API Key。请到设置页「机器人设置」填写 key，或关闭「Tavily 搜索」开关改用 Bing+百度双引擎。"
                .into(),
        ),
        SearchRoute::MissingBraveKey => Err(
            "Brave 搜索已开启，但设置页还没填 Brave API Key。请到设置页「机器人设置」填写 key，或关闭「Brave 搜索」开关改用 Bing+百度双引擎。"
                .into(),
        ),
        SearchRoute::Conflict => Err(
            "Tavily 与 Brave 搜索不能同时开启，请到设置页关闭其中一个".into(),
        ),
        SearchRoute::Tavily(key) => search_tavily(&key, query).await.map_err(|e| {
            crate::bot::audit_log(app, &format!("web_search.tavily_failed | {e}"));
            format!(
                "Tavily 搜索失败：{e}。请检查 key 是否有效/网络是否可达，或在设置页关闭「Tavily 搜索」开关回退 Bing+百度双引擎。"
            )
        }),
        SearchRoute::Brave(key) => search_brave(&key, query).await.map_err(|e| {
            crate::bot::audit_log(app, &format!("web_search.brave_failed | {e}"));
            format!(
                "Brave 搜索失败：{e}。请检查 key 是否有效/网络是否可达，或在设置页关闭「Brave 搜索」开关回退 Bing+百度双引擎。"
            )
        }),
    }
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
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
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
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
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

// ───────────────────────── 正文提取（2026-08-19 Phase 2） ─────────────────────────
// fetch_url 从「整页 HTML→纯文本」升级为「先抽正文主块再转换」：
// 去 script/style/nav/footer 等噪声块 → article/main 语义标签 → 语义 class/id 的最大 div → 兜底全文。

/// 大小写不敏感的子串查找（needle 为 ASCII 标签；返回原串字节索引，不做全串 lowercase 避免变长错位）
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack.as_bytes()[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
        .map(|i| from + i)
}

/// 从 start（`<tag` 的位置）起按嵌套深度找匹配闭合标签，返回块字节区间 [start, end)
fn tag_block_span(html: &str, start: usize, tag: &str) -> Option<(usize, usize)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut depth = 0usize;
    let mut cur = start;
    while cur < html.len() {
        let next_open = find_ci(html, &open, cur);
        let next_close = find_ci(html, &close, cur);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                cur = o + open.len();
            }
            (_, Some(c)) => {
                depth = depth.saturating_sub(1);
                cur = c + close.len();
                if depth == 0 {
                    return Some((start, cur));
                }
            }
            _ => return None,
        }
    }
    None
}

/// 删除指定标签的完整块（含嵌套同名标签）：去 script/style/nav/footer 等噪声。
/// 未闭合的块只丢开标签本身（自闭合写法如 <iframe/> 不至于吞掉整页残余）
fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let mut out = String::with_capacity(html.len());
    let mut rest = 0usize;
    while let Some(pos) = find_ci(html, &open, rest) {
        out.push_str(&html[rest..pos]);
        rest = match tag_block_span(html, pos, tag) {
            Some((_, end)) => end,
            None => pos + open.len(),
        };
    }
    out.push_str(&html[rest.min(html.len())..]);
    out
}

/// 提取正文主块：去噪声块 → article/main 标签 → 语义 class/id 的最大 div → 兜底整页
fn extract_main_content(html: &str) -> String {
    let mut cleaned = html.to_string();
    for tag in [
        "script", "style", "noscript", "iframe", "form", "nav", "footer", "header", "aside",
    ] {
        cleaned = remove_tag_blocks(&cleaned, tag);
    }
    // HTML 注释
    while let Some(s) = cleaned.find("<!--") {
        match cleaned[s..].find("-->") {
            Some(e) => cleaned.replace_range(s..s + e + 3, ""),
            None => break,
        }
    }
    for tag in ["article", "main"] {
        if let Some(pos) = find_ci(&cleaned, &format!("<{tag}"), 0) {
            if let Some((s, e)) = tag_block_span(&cleaned, pos, tag) {
                if strip_tags(&cleaned[s..e]).chars().count() >= 200 {
                    return cleaned[s..e].to_string();
                }
            }
        }
    }
    // 语义 div：class/id 含 article|content|post|entry|main（排除 comment），取可见文本最多者
    let mut best: Option<(usize, usize, usize)> = None;
    let mut cur = 0;
    while let Some(pos) = find_ci(&cleaned, "<div", cur) {
        let Some(gt) = cleaned[pos..].find('>') else { break };
        let attrs = cleaned[pos..pos + gt].to_lowercase();
        cur = pos + 4;
        let hit = ["article", "content", "post", "entry", "main"]
            .iter()
            .any(|k| attrs.contains(k));
        if !hit || attrs.contains("comment") {
            continue;
        }
        if let Some((s, e)) = tag_block_span(&cleaned, pos, "div") {
            let text_len = strip_tags(&cleaned[s..e]).chars().count();
            if text_len > best.map(|b| b.0).unwrap_or(0) {
                best = Some((text_len, s, e));
            }
        }
    }
    if let Some((len, s, e)) = best {
        if len >= 200 {
            return cleaned[s..e].to_string();
        }
    }
    cleaned
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

/// 校验 URL 并返回通过校验的解析地址：协议 + 主机名字符串 + DNS 解析出的所有 IP
/// 均须公网（防 DNS 重绑定/内网域名）。返回的地址由调用方钉给 reqwest
/// （resolve_to_addrs，FIX-PLAN #7a），校验与请求之间不再二次解析，堵 DNS TOCTOU。
async fn check_public_url(url: &url::Url) -> Result<Vec<std::net::SocketAddr>, String> {
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
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("网址缺少主机名".into());
    }
    if is_private_host(&host) {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("已拒绝访问本机/内网地址".into());
    }
    // DNS 解析校验：域名解析出的每个 IP 都必须是公网（防解析到 127.0.0.1 的内网域名）
    let mut out: Vec<std::net::SocketAddr> = Vec::new();
    // tokio::net::lookup_host 返回同步迭代器（解析已在 await 内完成）
    let addrs = tokio::net::lookup_host((host.as_str(), 80))
        .await
        .map_err(|e| format!("域名解析失败：{e}"))?;
    for addr in addrs {
        match addr.ip() {
            std::net::IpAddr::V4(v4) => {
                if ipv4_is_private(v4) {
                    // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
                    return Err("已拒绝：域名解析到本机/内网地址".into());
                }
            }
            std::net::IpAddr::V6(v6) => {
                if ipv6_is_private(v6) {
                    // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
                    return Err("已拒绝：域名解析到本机/内网地址".into());
                }
            }
        }
        out.push(addr);
    }
    if out.is_empty() {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("域名没有解析到任何地址".into());
    }
    Ok(out)
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
        // 每跳都走「解析 → 校验 → 钉 IP」：check_public_url 返回校验过的地址，
        // per-hop client 用 resolve_to_addrs 钉住，reqwest 不再二次 DNS（FIX-PLAN #7a DNS TOCTOU）
        let addrs = check_public_url(&url_cursor).await?;
        resp = http_client_pinned(&url_cursor, &addrs)?
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
            // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
            return Err("页面过大（超过 2MB）已拒绝".into());
        }
    }
    let bytes = read_body_capped(resp, FETCH_MAX_BYTES).await?;
    let text = decode_html(&bytes);
    // 先抽正文主块再转纯文本（去导航/广告/页脚）；提取结果过短说明误伤，退回整页转换
    let main = extract_main_content(&text);
    let mut plain = html2text::from_read(&mut main.as_bytes(), 120)
        .map_err(|e| format!("解析 HTML 失败：{e}"))?
        .trim()
        .to_string();
    if plain.chars().count() < 100 && main.len() != text.len() {
        if let Ok(full) = html2text::from_read(&mut text.as_bytes(), 120) {
            if full.trim().chars().count() > plain.chars().count() {
                plain = full.trim().to_string();
            }
        }
    }
    // 2026-08-20：正文仍过短（多半是 JS 渲染的 SPA 页面，静态抓取只能拿到空壳）→
    // 回退 Jina Reader 公共代理（服务端渲染后返回 markdown，免费无需 key）。
    // 隐私边界：目标 URL 会发给 r.jina.ai（公网地址本身，低风险）。
    if plain.chars().count() < 100 {
        if let Ok(jina) = fetch_jina_reader(raw_url).await {
            if jina.chars().count() > plain.chars().count() {
                plain = jina;
            }
        }
    }
    if plain.is_empty() {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("页面没有可提取的文本内容".into());
    }
    Ok(plain)
}

/// Jina Reader 代理 URL（纯函数便于单测）：https://r.jina.ai/<原 URL>
fn jina_reader_url(raw_url: &str) -> String {
    format!("https://r.jina.ai/{}", raw_url.trim())
}

/// Jina Reader 回退抓取：公共代理服务端渲染页面返回 markdown 文本。
/// 失败（超时/限流/目标不可达）由调用方忽略，不影响主路径。
async fn fetch_jina_reader(raw_url: &str) -> Result<String, String> {
    let url = url::Url::parse(&jina_reader_url(raw_url)).map_err(|_| "Jina URL 无效".to_string())?;
    let addrs = check_public_url(&url).await?;
    let resp = http_client_pinned(&url, &addrs)?
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("Jina 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Jina 返回 HTTP {}", resp.status()));
    }
    let bytes = read_body_capped(resp, FETCH_MAX_BYTES).await?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// 有界流式读取响应体（2026-08-27 安全审计 SEC-P1-5）：累计超限即中断——
/// 原先 `bytes()` 全量读进内存后做事后检查，Content-Length 撒谎（声明小/缺省 chunked）
/// 时 30s 超时内可收进数百 MB；现在超限立即中断，不进内存。
async fn read_body_capped(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取失败：{e}"))?;
        if buf.len() + chunk.len() > max {
            return Err("页面过大（超过 2MB）已拒绝".into());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// IPv4 是否为内网/本机段
fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        // CGNAT 段 100.64.0.0/10（RFC 6598，Tailscale/运营商大内网）：
        // std is_private 不含此段（2026-08-27 安全审计 SEC-P1-2 补）
        || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
}

/// IPv6 是否为内网/本机段（2026-08-27 SEC-P1-2：补 IPv4-mapped——
/// AAAA 应答 `::ffff:127.0.0.1` 原先四项判定全不中，hyper 连接时映射回 IPv4 打内网）
fn ipv6_is_private(v6: std::net::Ipv6Addr) -> bool {
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
fn is_private_host(host: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_none_switch_keeps_legacy_auto() {
        // 老配置（无开关字段、无 brave 字段）：配了 key 自动走 Tavily，没配走双引擎
        assert_eq!(
            resolve_search_route(None, Some("tvly-x"), None, None),
            SearchRoute::Tavily("tvly-x".into())
        );
        assert_eq!(resolve_search_route(None, None, None, None), SearchRoute::Dual);
        assert_eq!(
            resolve_search_route(None, Some("   "), None, None),
            SearchRoute::Dual
        );
    }

    #[test]
    fn route_explicit_off_forces_dual_even_with_key() {
        assert_eq!(
            resolve_search_route(Some(false), Some("tvly-x"), None, None),
            SearchRoute::Dual
        );
        assert_eq!(resolve_search_route(Some(false), None, None, None), SearchRoute::Dual);
        // Brave 也显式关：两边都有 key 也应走双引擎
        assert_eq!(
            resolve_search_route(Some(false), Some("tvly-x"), Some(false), Some("bsa-x")),
            SearchRoute::Dual
        );
    }

    #[test]
    fn route_explicit_on_requires_key() {
        assert_eq!(
            resolve_search_route(Some(true), None, None, None),
            SearchRoute::MissingKey
        );
        assert_eq!(
            resolve_search_route(Some(true), Some("  "), None, None),
            SearchRoute::MissingKey
        );
        // key 首尾空白应裁掉
        assert_eq!(
            resolve_search_route(Some(true), Some(" tvly-x "), None, None),
            SearchRoute::Tavily("tvly-x".into())
        );
    }

    #[test]
    fn route_brave_on_with_key() {
        assert_eq!(
            resolve_search_route(None, None, Some(true), Some(" bsa-x ")),
            SearchRoute::Brave("bsa-x".into())
        );
    }

    #[test]
    fn route_brave_on_without_key_errors() {
        assert_eq!(
            resolve_search_route(None, None, Some(true), None),
            SearchRoute::MissingBraveKey
        );
        assert_eq!(
            resolve_search_route(None, None, Some(true), Some("  ")),
            SearchRoute::MissingBraveKey
        );
    }

    #[test]
    fn route_both_on_conflict() {
        // 双开（显式或自动态）都明确报错，不静默猜
        assert_eq!(
            resolve_search_route(Some(true), Some("tvly-x"), Some(true), Some("bsa-x")),
            SearchRoute::Conflict
        );
        assert_eq!(
            resolve_search_route(None, Some("tvly-x"), None, Some("bsa-x")),
            SearchRoute::Conflict
        );
    }

    #[test]
    fn route_brave_none_switch_auto_by_key() {
        // 与 Tavily 的 None+key 旧行为对齐：无显式开关但有 brave key 自动启用 Brave
        assert_eq!(
            resolve_search_route(None, None, None, Some("bsa-x")),
            SearchRoute::Brave("bsa-x".into())
        );
        // Brave 自动态优先于「都没配」的双引擎，但不影响显式关 Tavily
        assert_eq!(
            resolve_search_route(Some(false), Some("tvly-x"), None, Some("bsa-x")),
            SearchRoute::Brave("bsa-x".into())
        );
        // Brave 显式关：即使配了 brave key 也不走；Tavily 逻辑不受影响
        assert_eq!(
            resolve_search_route(None, Some("tvly-x"), Some(false), Some("bsa-x")),
            SearchRoute::Tavily("tvly-x".into())
        );
    }

    #[test]
    fn parse_brave_results_ok() {
        let body = r#"{"web":{"results":[
            {"title":"标题一","url":"https://a.com/x","description":"摘要一"},
            {"title":"标题二","url":"https://b.com/y","description":"摘要二","extra":1}
        ]},"query":{"original":"q"}}"#;
        let r = parse_brave_results(body).unwrap_or_default();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], ("标题一".into(), "https://a.com/x".into(), "摘要一".into()));
        assert_eq!(r[1].1, "https://b.com/y");
    }

    #[test]
    fn parse_brave_results_web_missing_and_empty() {
        // web 字段缺失 → 空列表（防御，不报错）
        let r = parse_brave_results(r#"{"query":{"original":"q"}}"#).unwrap_or_default();
        assert!(r.is_empty());
        // results 为空数组 → 空列表
        let r = parse_brave_results(r#"{"web":{"results":[]}}"#).unwrap_or_default();
        assert!(r.is_empty());
        // 字段缺失的条目按空串填充
        let r = parse_brave_results(r#"{"web":{"results":[{"title":"只有标题"}]}}"#)
            .unwrap_or_default();
        assert_eq!(r, vec![("只有标题".to_string(), String::new(), String::new())]);
    }

    #[test]
    fn parse_brave_results_bad_json() {
        let err = parse_brave_results("not json").unwrap_err();
        assert!(err.contains("Brave 响应解析失败"), "应明确报错：{err}");
    }

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
    fn extract_main_prefers_article_over_nav() {
        let html = r#"<html><body>
            <nav><a href="/">首页</a><a href="/about">关于我们</a></nav>
            <article><h1>正文标题</h1><p>这是正文内容，应该被提取出来，而导航链接不应该出现在结果里。正文需要足够长才能超过 200 字阈值，所以这里多写一些内容来凑够长度。继续补充正文内容，确保测试稳定通过，再多写一点点内容。</p></article>
            <footer>版权所有 2026</footer>
        </body></html>"#;
        let main = extract_main_content(html);
        assert!(main.contains("正文标题"), "应提取 article 正文");
        assert!(!main.contains("关于我们"), "nav 噪声应被剔除");
        assert!(!main.contains("版权所有"), "footer 噪声应被剔除");
    }

    #[test]
    fn extract_main_semantic_div_fallback() {
        // 没有 article/main 标签时，用语义 class 的最大 div
        let html = format!(
            r#"<html><body><div class="sidebar">侧边栏短</div><div id="content"><p>{}</p></div></body></html>"#,
            "正文内容".repeat(60)
        );
        let main = extract_main_content(&html);
        assert!(main.contains("正文内容"), "应命中 id=content 的 div");
        assert!(!main.contains("侧边栏短"), "sidebar 不应被选中");
    }

    #[test]
    fn remove_tag_blocks_nested_same_tag() {
        let html = "<div>a</div><script>var x = '<script>nested</script>';</script><p>keep</p>";
        let out = remove_tag_blocks(html, "script");
        assert!(out.contains("keep"));
        assert!(!out.contains("nested"));
    }

    #[test]
    fn clean_snippet_trims_ellipsis_and_whitespace() {
        assert_eq!(clean_snippet("  多  空白\n合并 ……"), "多 空白 合并");
        assert_eq!(clean_snippet("结尾省略…"), "结尾省略");
        assert_eq!(clean_snippet("正常文本。"), "正常文本。");
    }

    #[test]
    fn domain_of_extracts_host() {
        assert_eq!(domain_of("https://www.example.com/a/b"), "www.example.com");
        assert_eq!(domain_of("不是链接"), "");
    }

    #[test]
    fn jina_reader_url_wraps_original() {
        assert_eq!(
            jina_reader_url("https://example.com/a"),
            "https://r.jina.ai/https://example.com/a"
        );
        assert_eq!(
            jina_reader_url("  http://foo.bar  "),
            "https://r.jina.ai/http://foo.bar",
            "首尾空白应裁掉"
        );
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

    /// SEC-P0-1（2026-08-27 安全审计）回归：http_client 禁止自动重定向——
    /// 302 必须原样返回给 fetch_text 的手工逐跳校验，不得自动跟随到 Location 目标
    /// （原先默认 policy 自动跟随 10 跳，逐跳校验是死代码，公网 URL 可 302 进内网）。
    #[tokio::test]
    async fn http_client_does_not_follow_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                use std::io::{Read, Write};
                // 先读完请求头再回响应（hyper 对未消费请求即收响应会报 UnexpectedMessage）
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let resp = http_client()
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .expect("本地 302 端点应可连");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "必须原样返回 302（手工逐跳校验接管），不得自动跟随"
        );
    }

    /// SEC-P1-2 回归：IPv4-mapped IPv6（::ffff:127.0.0.1）与 CGNAT（100.64.0.0/10）判内网
    #[test]
    fn private_detection_covers_mapped_v6_and_cgnat() {
        assert!(ipv6_is_private("::ffff:127.0.0.1".parse().unwrap()));
        assert!(ipv6_is_private("::ffff:a9fe:a9fe".parse().unwrap()), "169.254.169.254 mapped");
        assert!(!ipv6_is_private("2606:4700:4700::1111".parse().unwrap()), "公网 v6 放行");
        assert!(ipv4_is_private("100.64.0.1".parse().unwrap()), "CGNAT 起始");
        assert!(ipv4_is_private("100.127.255.254".parse().unwrap()), "CGNAT 末尾");
        assert!(!ipv4_is_private("100.128.0.1".parse().unwrap()), "CGNAT 段外");
        assert!(!ipv4_is_private("99.255.0.1".parse().unwrap()), "CGNAT 段外");
    }

    /// FIX-PLAN #7a（DNS TOCTOU）回归：钉住解析结果后请求必须走钉住的地址——
    /// 用 .invalid 域名（RFC 2606，真实 DNS 必解析失败）钉到本地回环服务器，
    /// 能连通即证明 reqwest 没有二次解析；Host 头必须仍是原域名。
    /// 钉的 addr 端口故意给 80（与 URL 端口不同），顺带验证端口以 URL 为准。
    #[tokio::test]
    async fn pinned_client_uses_validated_addrs() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let got_host = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let got_host2 = got_host.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if let Some(line) = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("host:"))
                {
                    *got_host2.lock().unwrap() = line[5..].trim().to_string();
                }
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        let url = url::Url::parse(&format!("http://pinned.invalid:{port}/")).unwrap();
        let pinned = [std::net::SocketAddr::from(([127, 0, 0, 1], 80))];
        let resp = http_client_pinned(&url, &pinned)
            .expect("钉住 client 构建失败")
            .get(url)
            .send()
            .await
            .expect("钉住 127.0.0.1 后请求应成功（.invalid 真实 DNS 必失败，无二次解析）");
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            got_host.lock().unwrap().as_str(),
            format!("pinned.invalid:{port}"),
            "钉 IP 不改 Host 头（TLS 场景 SNI/证书同理仍按原域名）"
        );
    }
}
