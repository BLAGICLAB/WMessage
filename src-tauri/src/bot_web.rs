//! 联网工具：web_search（Bing 抓取）+ fetch_url（网页正文提取）。
//!
//! 安全设计（对齐 Harness 网关）：
//! - fetch_url 只允许 http/https；拒绝本机/内网地址（loopback/私网 IP 段/本地域名后缀）
//! - 超时：connect 15s / 总 30s；页面响应体上限 10MB（搜索结果体另有 2MB 线）；只处理 HTML/文本类内容
//! - 输出截断在工具层做（搜索结果 6000 字、网页正文 30000 字）；审计由 bot.rs 留痕

use crate::error::CommandError;
use futures_util::StreamExt;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// 传输闸：防恶意超大响应打爆内存（Content-Length 可撒谎，必须流式封顶）。
// 不是内容门槛——正文提取后有 30K 字符切片 + offset 续读，大页面按段可读。
const FETCH_MAX_BYTES: usize = 10 * 1024 * 1024;
/// 搜索引擎结果体上限：与 FETCH_MAX_BYTES 是两条不同用途的容量线——
/// 这条封引擎结果体（正常远小于此），FETCH_MAX_BYTES 封页面抓取传输。
const SEARCH_BODY_CAP_BYTES: usize = 2 * 1024 * 1024;
/// 搜索结果合并渲染后的字节上限：参数表只读行展示同一个数
pub(crate) const SEARCH_OUTPUT_CAP_BYTES: usize = 6000;

/// reqwest 0.12+ 的错误 Display 只剩顶层一句（如「error sending request for url」），
/// 真正原因（DNS/连接拒绝/TLS InvalidContentType）在 source 链里——逐层展开供
/// 日志与工具结果诊断（fetch_url 全挂事故就是靠它定位的）
pub(crate) fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(&format!("（原因: {s}）"));
        src = s.source();
    }
    msg
}

/// N3-5：web_search 过滤参数（工具层解析，三后端各自映射）
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SearchOpts {
    /// 结果条数（1..=10，默认 8）
    pub count: u32,
    /// 时间范围（Tavily/Brave 原生；Bing+百度抓取链路不支持，显式提示）
    pub time_range: Option<TimeRange>,
    /// 限定站点域名（已清洗：无 scheme/路径/空白）
    pub site: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn parse(s: Option<&str>) -> Option<Self> {
        match s? {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }
    /// Tavily `time_range` 原生值（官方 API：day/week/month/year）
    pub fn tavily_value(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
    /// Brave `freshness` 原生值（官方 API：pd=24h / pw=7天 / pm=31天 / py=一年）
    pub fn brave_freshness(self) -> &'static str {
        match self {
            Self::Day => "pd",
            Self::Week => "pw",
            Self::Month => "pm",
            Self::Year => "py",
        }
    }
}

/// site 参数清洗：剥 scheme 前缀与路径/查询，仅留 host 段；空串/含空白 → None
/// （host 里的空白必然非法；路径段拼接 ` site:x/y` 对搜索引擎语义不明，直接剥掉）
pub fn sanitize_site(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let no_scheme = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let host = no_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.trim();
    if host.is_empty() || host.split_whitespace().count() > 1 {
        return None;
    }
    Some(host.to_string())
}

/// Tavily 请求体（纯函数，可测）：过滤参数按官方 API 形状映射
fn tavily_payload(key: &str, query: &str, opts: &SearchOpts) -> serde_json::Value {
    let mut body = serde_json::json!({
        "api_key": key,
        "query": query,
        "max_results": opts.count,
        "search_depth": "basic",
        "include_answer": false
    });
    if let Some(tr) = opts.time_range {
        body["time_range"] = serde_json::Value::String(tr.tavily_value().into());
    }
    if let Some(site) = &opts.site {
        body["include_domains"] = serde_json::json!([site]);
    }
    body
}

/// Brave 查询串（纯函数，可测）：site 以 ` site:host` 追加（Brave Web Search
/// 无 include_domains 参数，官方推荐用查询运算符）；freshness/count 走 query 参数
fn brave_query_and_params(query: &str, opts: &SearchOpts) -> (String, String) {
    let q = match &opts.site {
        Some(site) => format!("{query} site:{site}"),
        None => query.to_string(),
    };
    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let mut params = format!("count={}", opts.count);
    if let Some(tr) = opts.time_range {
        params.push_str(&format!("&freshness={}", tr.brave_freshness()));
    }
    (encoded, params)
}

/// 全局复用的 HTTP client（连接池复用，避免每请求新建）。
/// 禁用自动重定向：默认 policy 自动跟随多跳会让公网 URL 302 到内网地址绕过
/// check_public_url（SSRF）；由 fetch_text 手工逐跳校验接管。
fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            http_client_builder()
                .build()
                // builder 失败（TLS 后端等）≠ 放弃约束：降级 client 仍钉硬超时 +
                // 禁自动重定向（无约束 client 会架空 SSRF 逐跳校验与 30s 超时）；
                // 再失败 = panic loud（无超时 client 比 panic 更危险）
                .unwrap_or_else(|e| {
                    eprintln!("[bot_web] http_client builder 失败（{e}），降级：60s 硬超时 + 禁自动重定向");
                    reqwest::Client::builder()
                        .timeout(Duration::from_secs(60))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("bot_web 降级 client 构建失败")
                })
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
/// addrs 端口必须是真实端口（check_public_url 已重写）——reqwest 0.13 的
/// resolve_to_addrs 在 https 场景按 addr 自带端口连接，URL 端口不覆盖。
pub(crate) fn http_client_pinned(
    url: &url::Url,
    addrs: &[std::net::SocketAddr],
) -> Result<reqwest::Client, String> {
    let host = url.host_str().unwrap_or_default();
    http_client_builder()
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))
}

// 搜索（Bing + 百度双引擎）

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

pub async fn web_search(query: &str, opts: &SearchOpts) -> Result<String, CommandError> {
    // site 走查询运算符（抓取链路无域名过滤参数）；time_range 无原生支持——
    // 结果尾部显式提示（诚实降级，不静默丢弃让模型误以为已过滤）
    let (q_bing, q_baidu) = match &opts.site {
        Some(site) => (
            format!("{query} site:{site}"),
            format!("{query} site:{site}"),
        ),
        None => (query.to_string(), query.to_string()),
    };
    let (bing, baidu) =
        futures_util::future::join(search_bing(&q_bing), search_baidu(&q_baidu)).await;
    // (引擎, 标题, 链接, 摘要)
    let mut merged: Vec<(&str, String, String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut domain_count: Vec<(String, usize)> = Vec::new();
    let mut errs: Vec<CommandError> = Vec::new();
    for (engine, result) in [("Bing", bing), ("百度", baidu)] {
        match result {
            Ok(list) => {
                for (title, link, snip) in list {
                    let key = title.trim().to_string();
                    if seen.contains(&key) || merged.len() >= opts.count as usize {
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
            CommandError::DomainRule {
                domain: "search".to_string(),
                reason: "搜索没有返回结果".to_string(),
            }
        } else {
            CommandError::DomainRule {
                domain: "search".to_string(),
                reason: format!(
                    "所有搜索引擎均失败：{}",
                    errs.iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("；")
                ),
            }
        });
    }
    let mut out = String::new();
    for (i, (engine, title, link, snip)) in merged.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {}\n{}\n{}\n\n",
            i + 1,
            engine,
            title,
            link,
            snip
        ));
    }
    if out.len() > SEARCH_OUTPUT_CAP_BYTES {
        out = crate::bot::format::truncate_bytes(&out, SEARCH_OUTPUT_CAP_BYTES);
    }
    if opts.time_range.is_some() {
        out.push_str(
            "\n\n（提示：Bing/百度抓取模式暂不支持时间过滤，time_range 已忽略；需要时间过滤请在设置页配置 Tavily 或 Brave 搜索）",
        );
    }
    Ok(out)
}

/// Tavily 搜索 API（设置页「Tavily 搜索」开关开启后 web_search 走这里）
async fn search_tavily(key: &str, query: &str, opts: &SearchOpts) -> Result<String, String> {
    let resp = http_client()
        .post("https://api.tavily.com/search")
        .json(&tavily_payload(key, query, opts))
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
        out.push_str(&format!(
            "{}. [Tavily] {}\n{}\n{}\n\n",
            i + 1,
            title,
            url,
            content
        ));
    }
    if out.len() > SEARCH_OUTPUT_CAP_BYTES {
        out = crate::bot::format::truncate_bytes(&out, SEARCH_OUTPUT_CAP_BYTES);
    }
    Ok(out)
}

/// Brave Web Search API（设置页「Brave 搜索」开关开启后 web_search 走这里）：
/// GET https://api.search.brave.com/res/v1/web/search，key 走 X-Subscription-Token 头。
/// 与 Tavily 互斥（同时开启明确报错，见 resolve_search_route）。
async fn search_brave(key: &str, query: &str, opts: &SearchOpts) -> Result<String, String> {
    // 查询串手工编码（同 search_bing 的 form_urlencoded 风格；reqwest 0.13 无 .query()）
    let (encoded, params) = brave_query_and_params(query, opts);
    let target = format!("https://api.search.brave.com/res/v1/web/search?q={encoded}&{params}");
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
    let body = decode_search_body(
        &read_body_capped(resp, SEARCH_BODY_CAP_BYTES)
            .await
            .map_err(|e| format!("Brave 响应读取失败：{e}"))?,
    );
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
    if out.len() > SEARCH_OUTPUT_CAP_BYTES {
        out = crate::bot::format::truncate_bytes(&out, SEARCH_OUTPUT_CAP_BYTES);
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
    /// Brave Web Search API（带 trim 后的 key）
    Brave(String),
    /// Tavily 开关开了但没填 key
    MissingKey,
    /// Brave 开关开了但没填 key
    MissingBraveKey,
    /// Tavily 与 Brave 同时开启：互斥，明确报错不静默猜
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
/// Tavily/Brave key 从系统凭据存储读（不落 bot-config.json）；
/// keyring 真实故障按无 key 处理（走 MissingKey 报错文案引导用户去设置页），
/// 不弄挂 web_search 工具本身。
pub async fn web_search_with_config(
    app: &tauri::AppHandle,
    query: &str,
    opts: &SearchOpts,
) -> Result<String, String> {
    let cfg = crate::bot::load_config(app);
    let tavily_key = crate::bot::read_search_key(crate::bot::KeySlot::Tavily).unwrap_or_default();
    let brave_key = crate::bot::read_search_key(crate::bot::KeySlot::Brave).unwrap_or_default();
    match resolve_search_route(
        cfg.tavily_enabled,
        Some(tavily_key.as_str()),
        cfg.brave_enabled,
        Some(brave_key.as_str()),
    ) {
        SearchRoute::Dual => web_search(query, opts).await.map_err(|e| e.to_string()),
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
        SearchRoute::Tavily(key) => search_tavily(&key, query, opts).await.map_err(|e| {
            crate::bot::audit_log(app, &format!("web_search.tavily_failed | {e}"));
            format!(
                "Tavily 搜索失败：{e}。请检查 key 是否有效/网络是否可达，或在设置页关闭「Tavily 搜索」开关回退 Bing+百度双引擎。"
            )
        }),
        SearchRoute::Brave(key) => search_brave(&key, query, opts).await.map_err(|e| {
            crate::bot::audit_log(app, &format!("web_search.brave_failed | {e}"));
            format!(
                "Brave 搜索失败：{e}。请检查 key 是否有效/网络是否可达，或在设置页关闭「Brave 搜索」开关回退 Bing+百度双引擎。"
            )
        }),
    }
}

/// Bing 搜索：解析 `<li class="b_algo">` 块，返回 (标题, 链接, 摘要) 列表
async fn search_bing(query: &str) -> Result<Vec<(String, String, String)>, CommandError> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let target = format!("https://cn.bing.com/search?q={encoded}&mkt=zh-CN");
    let resp = http_client()
        .get(&target)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("Bing 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(CommandError::DomainRule {
            domain: "web".to_string(),
            reason: format!("Bing 返回 HTTP {}", resp.status()),
        });
    }
    let body = decode_search_body(
        &read_body_capped(resp, SEARCH_BODY_CAP_BYTES)
            .await
            .map_err(|e| format!("读取 Bing 结果失败：{e}"))?,
    );
    let results = parse_bing(&body);
    if results.is_empty() {
        return Err(CommandError::DomainRule {
            domain: "search".to_string(),
            reason: "Bing 没有返回结果".to_string(),
        });
    }
    Ok(results)
}

/// 百度搜索：解析 `class="result"` 容器，返回 (标题, 跳转链接, 摘要) 列表
/// 链接为百度 /link?url 跳转链接（浏览器可打开；机器人 fetch 需换真实链接）
async fn search_baidu(query: &str) -> Result<Vec<(String, String, String)>, CommandError> {
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
        return Err(CommandError::DomainRule {
            domain: "web".to_string(),
            reason: format!("百度返回 HTTP {}", resp.status()),
        });
    }
    let body = decode_search_body(
        &read_body_capped(resp, SEARCH_BODY_CAP_BYTES)
            .await
            .map_err(|e| format!("读取百度结果失败：{e}"))?,
    );
    let results = parse_baidu(&body);
    if results.is_empty() {
        return Err(CommandError::DomainRule {
            domain: "search".to_string(),
            reason: "百度没有返回结果（可能触发验证页）".to_string(),
        });
    }
    Ok(results)
}

/// 开标签精确匹配：`<a` 这类前缀 `find` 会误中 `<abbr>`/`<address>`/`<pre>`
/// 等同前缀标签——开标签后一个字符必须是空白 / `>` / `/` 才算该标签
/// （abbr 前置时标题与链接来自不同 DOM 节点）。
/// 扫描全部命中取第一个合格位置。
fn find_tag_open(s: &str, pat: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find(pat) {
        let idx = from + i;
        match s[idx + pat.len()..].chars().next() {
            None => return Some(idx),
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => return Some(idx),
            _ => from = idx + pat.len(),
        }
    }
    None
}

/// 取第一个 `<open ...> ... </close>` 的中间内容
fn first_between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = find_tag_open(s, open)?;
    let tail = &s[a..];
    let gt = tail.find('>')?;
    let start = gt + 1;
    let b = tail[start..].find(close)?;
    Some(&tail[start..start + b])
}

/// 取第一个 `<open ...>` 完整开标签
fn first_open_tag<'a>(s: &'a str, open: &str) -> Option<&'a str> {
    let a = find_tag_open(s, open)?;
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

// 正文提取
// 先抽正文主块再转换：
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
        let Some(gt) = cleaned[pos..].find('>') else {
            break;
        };
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
        let link = href_from_tag(a_tag).unwrap_or_default();
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
        let Some(link) = href_from_tag(a_tag) else {
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

// 网页抓取

/// 公网闸的两类失败：**策略拒绝**与**解析故障**。
///
/// 两者的运维含义不同：策略拒绝是「这个地址不该去」（换个地址也没用），
/// 解析故障是「这台机器到那个名字的路不通」（网络故障）。合成一个 Err 会把
/// 真实故障降级：调用方只能一律按「换个地址重试」落地（Denied / WARN），
/// 而模型看到「被拒」会换个 url 再来——把一次网络故障拖成多轮无效重试。
///
/// `reason` 是裸原因；展示文案要不要带 `[web] ` 域标签由调用方决定
/// （公网闸判定带标签，参数级错误不带——与既有文案一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UrlGateError {
    /// 策略拒绝：协议不对、地址非法、解析到本机/内网/保留地址。
    Denied { reason: String },
    /// DNS 解析失败：域名解析不出来（不是策略问题，是这台机器到不了那个名字）。
    Unresolved { reason: String },
}

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
async fn check_public_url_with<F, Fut>(
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
fn with_url_port(addrs: Vec<std::net::SocketAddr>, url: &url::Url) -> Vec<std::net::SocketAddr> {
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
enum FetchErr {
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
fn status_worth_proxy_retry(status: u16) -> bool {
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
async fn fetch_direct(raw_url: &str) -> Result<String, FetchErr> {
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
fn jina_reader_url(raw_url: &str) -> String {
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

/// 有界流式读取响应体：累计超限即中断——
/// Content-Length 撒谎（声明小/缺省 chunked）时全量读进内存会在超时窗口内收进数百 MB；
/// 超限立即中断，不进内存。
async fn read_body_capped(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取失败：{}", err_chain(&e)))?;
        if buf.len() + chunk.len() > max {
            return Err(format!(
                "内容超过 {}MB 大小上限，已中断读取",
                max / (1024 * 1024)
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// 搜索引擎结果体解码：lossy 而非严格 UTF-8——结果页混入个别非 UTF-8 字节时，
/// 严格解码让整个引擎假失败「无结果」（与 fetch_jina_reader / decode_html 同一策略）
fn decode_search_body(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// IPv4 是否为内网/本机段
fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
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
mod tests;
