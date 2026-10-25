//! 搜索面：引擎路由（Tavily / Brave / Bing+百度双引擎）与结果拼装。

use super::html::{parse_baidu, parse_bing};
use super::*;

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
pub(super) fn tavily_payload(key: &str, query: &str, opts: &SearchOpts) -> serde_json::Value {
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
pub(super) fn brave_query_and_params(query: &str, opts: &SearchOpts) -> (String, String) {
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
pub(super) fn http_client() -> reqwest::Client {
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
// 搜索（Bing + 百度双引擎）

/// 双引擎搜索：Bing + 百度并行，结果按标题去重合并，最多 8 条
/// 同一域名最多保留条数（百度跳转链接除外——host 都是 baidu.com，去重会误杀）
const SEARCH_MAX_PER_DOMAIN: usize = 2;

/// 取链接的域名（解析失败返回空串）
pub(super) fn domain_of(link: &str) -> String {
    url::Url::parse(link)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default()
}

/// 摘要清理：压缩空白、去结尾省略号残留（抓取的摘要常被截断带「……」尾巴）
pub(super) fn clean_snippet(s: &str) -> String {
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
pub(super) fn parse_brave_results(body: &str) -> Result<Vec<(String, String, String)>, String> {
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
pub(super) enum SearchRoute {
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
pub(super) fn resolve_search_route(
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
/// 搜索引擎结果体解码：lossy 而非严格 UTF-8——结果页混入个别非 UTF-8 字节时，
/// 严格解码让整个引擎假失败「无结果」（与 fetch_jina_reader / decode_html 同一策略）
pub(super) fn decode_search_body(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
