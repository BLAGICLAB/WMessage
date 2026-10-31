//! 联网工具：web_search（Bing 抓取）+ fetch_url（网页正文提取）。
//!
//! 安全设计（对齐 Harness 网关）：
//! - fetch_url 只允许 http/https；拒绝本机/内网地址（loopback/私网 IP 段/本地域名后缀）
//! - 超时：connect 15s / 总 30s；页面响应体上限 10MB（搜索结果体另有 2MB 线）；只处理 HTML/文本类内容
//! - 输出截断在工具层做（搜索结果 6000 字、网页正文 30000 字）；审计由 bot.rs 留痕
//!
//! 本文件是门面：实现按面拆到 `bot_web/` 子模块（search 搜索 / html 页面解析 /
//! fetch 抓取与公网闸），这里只留类型、常量、共享工具与逐项转出，
//! `crate::bot_web::X` 旧路径与旧可见性保持不变。
use crate::error::CommandError;
use futures_util::StreamExt;
use std::time::Duration;

mod destination;
mod fetch;
mod html;
mod http;
mod search;

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
/// 有界流式读取响应体：累计超限即中断——
/// Content-Length 撒谎（声明小/缺省 chunked）时全量读进内存会在超时窗口内收进数百 MB；
/// 超限立即中断，不进内存。
pub(crate) async fn read_body_capped(
    resp: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, String> {
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

// 拆出后的子模块转出：保持 `crate::bot_web::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub use fetch::fetch_text;
pub use search::sanitize_site;
pub use search::web_search_with_config;
// `bot_web` 是私有模块，pub use 的可达性只到 crate 内。`web_search` 目前只被
// search.rs 内部的 web_search_with_config 调用，crate 内没有第二个调用方——
// 但旧路径 `crate::bot_web::web_search` 要原样留住（它是搬走前就有的公开入口），
// 所以显式压制 unused 警告。
pub(crate) use destination::invalidate_grants;
pub(crate) use fetch::check_public_url;
pub(crate) use fetch::ensure_public_http_url;
pub(crate) use http::{execute as execute_http, GateCtx, HttpError, RequestSpec};
#[allow(unused_imports)]
pub use search::web_search;

#[cfg(test)]
mod tests;
