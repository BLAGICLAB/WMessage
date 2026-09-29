//! MCP 服务器配置数据结构 + 校验 + 工具命名（纯逻辑层，无 IO / 无锁）。
//!
//! 落盘位置：bot-config.json 顶层可选字段 `mcpServers`（Option<Vec>，
//! 老配置缺字段 → None = 空，struct 级 #[serde(default)] 自动兼容，
//! 与 models_by_provider 同模式）。
//!
//! 敏感边界（有意为之，非遗漏）：stdio 的 `env` 值明文存 bot-config.json——
//! 大多数 env 是非敏感运行配置（NODE_HOME 等）；与主 LLM key / 搜索 key
//! 「永不落盘」策略不同，MCP env 是用户给**外部服务器**的配置，本机配置文件
//! （数据目录，非系统全局）是它的常规存放处。若未来出现高频敏感 token 场景，
//! 再按 KeySlot 模式迁 keyring。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CommandError, CommandResult};

use super::super::BotConfig;

// ───────────────────────── 常量 ─────────────────────────

/// 服务器条目数上限：防配置膨胀拖慢启动全量重连。
pub const MAX_MCP_SERVERS: usize = 20;
pub const MAX_MCP_NAME_LEN: usize = 50;
pub const MAX_MCP_COMMAND_LEN: usize = 4096;
pub const MAX_MCP_URL_LEN: usize = 2048;
/// stdio 参数 / env 条目上限（正常 MCP 服务器远用不满）。
pub const MAX_MCP_ARGS: usize = 32;
pub const MAX_MCP_ENV_ENTRIES: usize = 32;
/// http 自定义头条目上限
pub const MAX_MCP_HEADERS: usize = 16;
/// stdio 单次工具调用超时钳制（秒）：默认 60，可调 5..=600。
pub const MCP_TIMEOUT_DEFAULT_SECS: u64 = 60;
pub const MCP_TIMEOUT_MIN_SECS: u64 = 5;
pub const MCP_TIMEOUT_MAX_SECS: u64 = 600;
/// LLM function name 硬上限（OpenAI / Anthropic 均为 64）。
pub const MCP_TOOL_NAME_MAX_BYTES: usize = 64;
/// 服务器 id 上限（uuid simple = 32 字节，64 留余量；防脏 id 入库后按 id
/// 查表/删改全受影响——B0 评审）。
pub const MAX_MCP_ID_LEN: usize = 64;

/// stdio 启动器白名单。原拍板 3A（仅确认弹窗、无白名单）在实现期被 Mimosa
/// 安全闸强制升级为 3B（白名单硬闸，老板已留「可作为后续硬化项」的口子，
/// 提前落地，偏差在阶段报告里报备）：配置驱动的进程执行收敛到已知启动器，
/// 覆盖 npm / pypi / 容器生态的绝大多数 MCP 服务器发行形态。
/// 单一事实源（B0 评审）：保存校验（contains）、错误文案、manager 进程构造
/// 全查此表——扩展只改这一处，不存在第二份可漂移的清单。
pub const STDIO_LAUNCH_ALLOWLIST: &[&str] = &[
    "npx", "bunx", "uvx", "pipx", "node", "deno", "python", "python3", "docker", "podman",
];

// ───────────────────────── 类型 ─────────────────────────

/// 传输类型（协议字符串归一化）。**存储用 String 不用枚举**：手改配置文件写错值
/// 时枚举会让整份 BotConfig 反序列化失败（load_config 回退默认 = 视图全丢），
/// String + from_cfg 防御回退与 api_provider / perm_mode 同风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    /// 本地子进程（stdin/stdout 管道）
    Stdio,
    /// 远程 HTTP（Streamable HTTP，阶段 7）
    Http,
}

impl McpTransport {
    pub fn from_cfg(s: &str) -> Self {
        match s.trim() {
            "http" => McpTransport::Http,
            _ => McpTransport::Stdio, // 缺失/非法 → stdio（最常见形态）
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            McpTransport::Stdio => "stdio",
            McpTransport::Http => "http",
        }
    }
}

/// 单个外部 MCP 服务器配置。字段级 skip + struct 级 default：
/// 老配置无 mcpServers 字段 → Vec 缺省；条目内缺可选字段 → None。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerConfig {
    /// 稳定 id（uuid）：编辑/启停/删除的定位键；新建由后端生成
    pub id: String,
    /// 显示名（设置页 + 工具名前缀来源），保存时校验非空且全列表唯一
    pub name: String,
    /// "stdio" | "http"（McpTransport::as_str）；非法值读取时按 stdio
    pub transport: String,
    /// stdio：可执行命令（npx / uvx / 绝对路径二进制）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// stdio：参数列表
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// stdio：环境变量（BTreeMap：落盘键序稳定，diff 可读）
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// http：MCP 端点 URL（Streamable HTTP）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// http：随每个请求发送的自定义头（鉴权头等）。与 stdio env 同策略：
    /// 明文存本机 bot-config.json（数据目录），不进 keyring——见模块头敏感边界。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// 单次工具调用超时秒数；None = 60（钳 5..=600）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 启用开关：false = 该服务器工具立即从 agent 工具清单消失（不重连不调用）
    pub enabled: bool,
}

impl McpServerConfig {
    pub fn transport_kind(&self) -> McpTransport {
        McpTransport::from_cfg(&self.transport)
    }

    /// 实际生效的调用超时（秒）：None → 默认 60，钳 5..=600
    pub fn effective_timeout_secs(&self) -> u64 {
        self.timeout_secs
            .unwrap_or(MCP_TIMEOUT_DEFAULT_SECS)
            .clamp(MCP_TIMEOUT_MIN_SECS, MCP_TIMEOUT_MAX_SECS)
    }
}

// ───────────────────────── 校验 ─────────────────────────

/// 单条服务器配置校验（纯逻辑）：字段随 transport 分流必填。
/// 全列表级校验（名称唯一 / 条目数）在 upsert_in_config。
pub fn validate_server(s: &McpServerConfig) -> Result<(), String> {
    // id 基本卫生（B0 评审）：空 id 合法 = 新建（mcp_server_save 在校验前生成
    // uuid）；非空时防超长/控制字符——id 是编辑/启停/删除与连接池的定位键
    if !s.id.is_empty() {
        if s.id.len() > MAX_MCP_ID_LEN {
            return Err(format!("id 过长（上限 {MAX_MCP_ID_LEN} 字节）"));
        }
        if s.id.chars().any(char::is_control) {
            return Err("id 含控制字符".into());
        }
    }
    let name = s.name.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name.chars().count() > MAX_MCP_NAME_LEN {
        return Err(format!("名称过长（上限 {MAX_MCP_NAME_LEN} 字）"));
    }
    match s.transport_kind() {
        McpTransport::Stdio => {
            let cmd = s.command.as_deref().map(str::trim).unwrap_or("");
            if cmd.is_empty() {
                return Err("stdio 服务器必须填写命令（command）".into());
            }
            if !STDIO_LAUNCH_ALLOWLIST.contains(&cmd) {
                return Err(format!(
                    "命令「{cmd}」不在启动器白名单内（允许：{}）",
                    STDIO_LAUNCH_ALLOWLIST.join(" / ")
                ));
            }
            if s.args.len() > MAX_MCP_ARGS {
                return Err(format!("参数过多（上限 {MAX_MCP_ARGS} 个）"));
            }
            for a in &s.args {
                if a.len() > MAX_MCP_COMMAND_LEN {
                    return Err(format!("单个参数过长（上限 {MAX_MCP_COMMAND_LEN} 字节）"));
                }
            }
        }
        McpTransport::Http => {
            let u = s.url.as_deref().map(str::trim).unwrap_or("");
            if u.is_empty() {
                return Err("http 服务器必须填写端点 URL".into());
            }
            if u.len() > MAX_MCP_URL_LEN {
                return Err(format!("URL 过长（上限 {MAX_MCP_URL_LEN} 字节）"));
            }
            // scheme 白名单 + host 安全闸（保存口；发请求前 manager 侧再验一次）
            if let Err(reason) = http_url_is_public(u) {
                return Err(reason);
            }
        }
    }
    if s.env.len() > MAX_MCP_ENV_ENTRIES {
        return Err(format!("环境变量过多（上限 {MAX_MCP_ENV_ENTRIES} 条）"));
    }
    for (k, v) in &s.env {
        if k.trim().is_empty() {
            return Err("环境变量名不能为空".into());
        }
        if k.len() > 128 {
            return Err("环境变量名过长（上限 128 字节）".into());
        }
        if v.len() > MAX_MCP_COMMAND_LEN {
            return Err(format!("环境变量值过长（上限 {MAX_MCP_COMMAND_LEN} 字节）"));
        }
    }
    if s.headers.len() > MAX_MCP_HEADERS {
        return Err(format!("自定义头过多（上限 {MAX_MCP_HEADERS} 条）"));
    }
    for (k, v) in &s.headers {
        let key = k.trim();
        if key.is_empty() {
            return Err("自定义头名不能为空".into());
        }
        // 头名合法性：reqwest/http 的 HeaderName 只收 token 字符集，保存时就拦
        if reqwest::header::HeaderName::from_bytes(key.as_bytes()).is_err() {
            return Err(format!("非法的 HTTP 头名：{key}"));
        }
        if reqwest::header::HeaderValue::from_str(v.trim()).is_err() {
            return Err(format!("自定义头的值含非法字符（需为可见 ASCII）：{k}"));
        }
    }
    Ok(())
}

/// 超时入参钳制（保存路径）：None 保留 None（= 默认），Some 钳 5..=600。
pub fn clamp_timeout(v: Option<u64>) -> Option<u64> {
    v.map(|n| n.clamp(MCP_TIMEOUT_MIN_SECS, MCP_TIMEOUT_MAX_SECS))
}

/// 规范化：trim 各字符串字段 + 丢弃空 env 键 + 超时钳制。保存前调用。
pub fn normalize_server(mut s: McpServerConfig) -> McpServerConfig {
    // id 一并 trim（B0 评审：带空白 id 会被原样入库，后续按 id 定位全歪）
    s.id = s.id.trim().to_string();
    s.name = s.name.trim().to_string();
    if let Some(c) = s.command.as_deref() {
        s.command = Some(c.trim().to_string()).filter(|c| !c.is_empty());
    }
    if let Some(u) = s.url.as_deref() {
        s.url = Some(u.trim().to_string()).filter(|u| !u.is_empty());
    }
    s.args = s.args.iter().map(|a| a.trim().to_string()).collect();
    // 只按键判定丢弃（B0 评审 HIGH）：「空键非空值」若按 `|| 值非空` 保留，
    // 下方 map 会把键 trim 成空串留在表里——与 docstring「丢弃空 env 键」相悖，
    // 且下一次 validate 必因空键报错（能保存但保存即坏）
    s.env.retain(|k, _| !k.trim().is_empty());
    s.env = s
        .env
        .iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    s.headers.retain(|k, _| !k.trim().is_empty());
    s.headers = s
        .headers
        .iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    s.timeout_secs = clamp_timeout(s.timeout_secs);
    s
}

/// URL scheme + host 安全闸（Mimosa 约束，保存口与连接口双重把关）：
/// HTTP 传输仅允许 http/https 且只连**公网**地址——拒绝 localhost、环回、
/// 私有和保留地址（agent 会把上下文发给该端点，内网地址一律不放行防 SSRF）。
/// 字符串/字面量级校验，不做 DNS 解析（解析结果可变且 TOCTOU；DNS 级校验二期再议）。
pub fn http_url_is_public(u: &str) -> Result<(), String> {
    // 解析失败不回显原文（B0 评审：URL 可能内嵌凭据 user:pass@ 或敏感 query）
    let parsed = url::Url::parse(u).map_err(|_| {
        format!(
            "URL 无法解析（长度 {} 字符，原文可能含凭据不回显）",
            u.len()
        )
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("URL 仅支持 http/https".into()),
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => {
            if is_reserved_ipv4(ip) {
                Err(format!("拒绝内网/保留 IPv4 地址：{ip}"))
            } else {
                Ok(())
            }
        }
        Some(url::Host::Ipv6(ip)) => {
            if is_reserved_ipv6(ip) {
                Err(format!("拒绝内网/保留 IPv6 地址：{ip}"))
            } else {
                Ok(())
            }
        }
        Some(url::Host::Domain(d)) => {
            // 尾点归一（评审发现）：URL 规范保留尾点，"localhost." 会绕过
            // 精确比较但实际解析到环回——比较前剥掉全部尾点
            let bare = d.to_ascii_lowercase();
            let bare = bare.trim_end_matches('.');
            // localhost 家族（B0 评审补：/etc/hosts 惯常映射到环回的别名一并拦）
            if bare == "localhost"
                || bare == "localhost.localdomain"
                || bare == "ip6-localhost"
                || bare == "ip6-loopback"
                || bare == "broadcasthost"
                || bare.ends_with(".localhost")
            {
                Err(format!("拒绝 localhost 地址：{d}"))
            // B0 评审（SSRF 旁路两则）：
            // ① 纯数字 / 0x 十六进制形式主机名（http://2130706433/ = 127.0.0.1、
            //    0177.0.0.1、0x7f.0.0.1——inet_aton 宽松解析的 IPv4 变体）被 url
            //    crate 归为 Domain，整条 IP 闸线失效。公网域名不存在全数字标签
            //    （无数字 TLD），按内网风险面一律拒绝，公网请写点分十进制
            // ② .local 是 RFC 6762 mDNS 名，resolver 会指到链路本地/局域网设备，
            //    与 localhost 同性质的内网入口
            } else if looks_like_numeric_host(bare) {
                Err(format!("拒绝数字形式主机名（按内网地址处理）：{d}"))
            } else if bare == "local" || bare.ends_with(".local") {
                Err(format!("拒绝 mDNS .local 地址：{d}"))
            } else {
                Ok(())
            }
        }
        None => Err("URL 缺少 host".into()),
    }
}

/// 主机名是否为「数字书写形态」（各段全为十进制数字或 0x 十六进制）：
/// 实为 inet_aton 宽松解析的 IPv4 变体（2130706433 / 0177.0.0.1 / 0x7f.0.0.1），
/// 走 Domain 分支绕过 IP 闸——B0 评审新增的判定（B0-2 同批安全闸加固）。
fn looks_like_numeric_host(bare: &str) -> bool {
    !bare.is_empty()
        && bare.split('.').all(|part| {
            !part.is_empty()
                && (part.bytes().all(|b| b.is_ascii_digit())
                    || (part.len() > 2
                        && (part.starts_with("0x") || part.starts_with("0X"))
                        && part[2..].bytes().all(|b| b.is_ascii_hexdigit())))
        })
}

/// IPv4 保留/内网段：环回、私网、链路本地、0/8、CGNAT 100.64/10、
/// 基准测试 198.18/15、组播 224/4、保留 240/4、广播、文档段。
fn is_reserved_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || o[0] == 0                                        // 0.0.0.0/8 "this network"
        || (o[0] == 100 && (o[1] & 0xC0) == 64)             // 100.64/10 CGNAT
        || (o[0] == 198 && (o[1] & 0xFE) == 18)             // 198.18/15 benchmark
        || o[0] >= 240 // 240/4 reserved + broadcast
}

/// IPv6 保留段：未指定、环回、unique-local fc00::/7、link-local fe80::/10。
/// IPv4-mapped（::ffff:a.b.c.d，评审发现的 SSRF 绕过）先还原成 IPv4 判定；
/// IPv4-compatible（::/96 已弃用段）整段按保留处理。
/// 转换前缀（B0 评审 M-10）：6to4 2002::/16、Teredo 2001::/32、NAT64 64:ff9b::/96
/// 都把 IPv4 嵌进地址、经公网中继可达内网（如 [2002:0a00:0001::1] = 10.0.0.1）——
/// 整组按保留拒绝。
fn is_reserved_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_reserved_ipv4(v4);
    }
    let s = ip.segments();
    let ipv4_compatible = s[..6].iter().all(|&seg| seg == 0);
    ipv4_compatible
        || ip.is_loopback()
        || ip.is_unspecified()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || s[0] == 0x2002                      // 6to4（嵌 IPv4，中继可达内网）
        || (s[0] == 0x2001 && s[1] == 0x0000)  // Teredo（嵌混淆 IPv4）
        || (s[0] == 0x0064 && s[1] == 0xff9b)  // NAT64 well-known /96（嵌 IPv4）
        || (s[0] == 0x2001 && s[1] == 0x0db8) // 文档段 2001:db8::/32（B0 评审：与 v4 侧 is_documentation 对齐）
}

// ───────────────────────── 工具命名（阶段 3 消费） ─────────────────────────

/// 服务器名 → 工具名前缀段：ASCII 字母数字与 `-` 保留（小写化），其余一律 `_`。
/// 输出恒为 ASCII，最长 24 字节（可安全按字节截断）。
pub fn tool_prefix(server_name: &str) -> String {
    let mut s: String = server_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if s.len() > 24 {
        s.truncate(24);
    }
    if s.is_empty() {
        s.push('_');
    }
    s
}

/// 外部工具在 agent 工具清单里的全名：`mcp_{服务器前缀}_{工具名}`。
/// 工具名同样净化为 [a-z0-9_-]（非 ASCII → `_`）；全名超 64 字节
/// （LLM function name 硬上限）时截断 + 6 位哈希尾防撞。
///
/// 已知边界：不同服务器净化后前缀相同（「my tool」与「my.tool」）且工具同名时
/// 会产出同名 function——阶段 3 构建 schema 时做全列表查重，冲突条目由
/// 挂载层追加序号消歧（不在本函数内处理，此处保持纯函数）。
pub fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    let prefix = tool_prefix(server_name);
    let tool: String = tool_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let raw = format!("mcp_{prefix}_{tool}");
    if raw.len() <= MCP_TOOL_NAME_MAX_BYTES {
        return raw;
    }
    // 超长：整名哈希取 6 位十六进制尾；raw 已全 ASCII，按字节截断安全
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut h);
    let suffix = format!("{:06x}", h.finish() & 0xffffff);
    let keep = MCP_TOOL_NAME_MAX_BYTES - suffix.len() - 1;
    format!("{}_{suffix}", &raw[..keep])
}

// ───────────────────────── 配置表内核（纯逻辑，供命令 + 单测） ─────────────────────────

fn servers_mut(cfg: &mut BotConfig) -> &mut Vec<McpServerConfig> {
    cfg.mcp_servers.get_or_insert_with(Vec::new)
}

/// 校验类错误统一走 InvalidArgument（前端按表单字段定位高亮）。
fn invalid(field: &str, value: &str, reason: impl Into<String>) -> CommandError {
    CommandError::InvalidArgument {
        field: field.into(),
        value: value.into(),
        reason: reason.into(),
    }
}

/// 新增或按 id 整体替换。校验失败响亮报错（InvalidArgument），不静默修正。
pub fn upsert_in_config(cfg: &mut BotConfig, server: McpServerConfig) -> CommandResult<()> {
    let server = normalize_server(server);
    validate_server(&server).map_err(|reason| invalid("server", &server.name, reason))?;
    let list = servers_mut(cfg);
    if list.len() >= MAX_MCP_SERVERS && !list.iter().any(|s| s.id == server.id) {
        return Err(invalid(
            "server",
            &server.name,
            format!("MCP 服务器数量已达上限（{MAX_MCP_SERVERS}）"),
        ));
    }
    // 名称唯一（排除自身）：前端展示 + 工具名前缀的来源，重名会让 mcp_ 前缀歧义
    if list
        .iter()
        .any(|s| s.id != server.id && s.name.eq_ignore_ascii_case(&server.name))
    {
        return Err(invalid("name", &server.name, "已被其他 MCP 服务器使用"));
    }
    match list.iter_mut().find(|s| s.id == server.id) {
        Some(slot) => *slot = server,
        None => list.push(server),
    }
    Ok(())
}

/// 按 id 删除；返回是否真的删了（幂等：不存在 = false，不报错）。
pub fn remove_from_config(cfg: &mut BotConfig, id: &str) -> bool {
    let Some(list) = cfg.mcp_servers.as_mut() else {
        return false;
    };
    let before = list.len();
    list.retain(|s| s.id != id);
    let removed = list.len() < before;
    if list.is_empty() {
        cfg.mcp_servers = None; // 清空后字段整个消失，保持配置文件干净
    }
    removed
}

/// 按 id 启停；返回是否找到（不存在 = false）。
pub fn set_enabled_in_config(cfg: &mut BotConfig, id: &str, enabled: bool) -> bool {
    cfg.mcp_servers
        .as_mut()
        .and_then(|list| list.iter_mut().find(|s| s.id == id))
        .map(|s| {
            s.enabled = enabled;
        })
        .is_some()
}

// ───────────────────────── 单测 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: name.into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
            ],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            timeout_secs: None,
            enabled: true,
        }
    }

    // ── validate_server ──

    #[test]
    fn valid_stdio_server_passes() {
        assert!(validate_server(&stdio_server("fs")).is_ok());
    }

    #[test]
    fn empty_or_overlong_name_rejected() {
        let mut s = stdio_server("  ");
        assert_eq!(validate_server(&s).unwrap_err(), "名称不能为空");
        s.name = "长".repeat(MAX_MCP_NAME_LEN + 1);
        assert!(validate_server(&s).unwrap_err().contains("名称过长"));
    }

    #[test]
    fn stdio_requires_command() {
        let mut s = stdio_server("fs");
        s.command = Some("  ".into());
        assert!(validate_server(&s).unwrap_err().contains("必须填写命令"));
    }

    #[test]
    fn stdio_command_must_be_in_launch_allowlist() {
        let mut s = stdio_server("fs");
        s.command = Some("curl".into());
        let err = validate_server(&s).unwrap_err();
        assert!(err.contains("白名单"), "实际：{err}");
        // 白名单内命令通过
        for cmd in STDIO_LAUNCH_ALLOWLIST {
            s.command = Some((*cmd).into());
            assert!(validate_server(&s).is_ok(), "{cmd} 应放行");
        }
    }

    #[test]
    fn http_requires_parsable_http_url() {
        let mut s = stdio_server("remote");
        s.transport = "http".into();
        s.command = None;
        s.args = Vec::new();
        // 缺 URL
        assert!(validate_server(&s).unwrap_err().contains("必须填写端点"));
        // 非法 URL
        s.url = Some("not a url".into());
        assert!(validate_server(&s).unwrap_err().contains("无法解析"));
        // 非 http/https scheme
        s.url = Some("ftp://mcp.example.com/mcp".into());
        assert!(validate_server(&s).unwrap_err().contains("http/https"));
        // 合法
        s.url = Some("https://mcp.example.com/mcp".into());
        assert!(validate_server(&s).is_ok());
    }

    // ── URL host 安全闸（阶段 7，Mimosa 约束） ──

    #[test]
    fn http_headers_validated_and_normalized() {
        let mut s = stdio_server("remote");
        s.transport = "http".into();
        s.command = None;
        s.args = Vec::new();
        s.url = Some("https://mcp.example.com/mcp".into());
        // 合法鉴权头
        s.headers
            .insert("Authorization".into(), "Bearer tok".into());
        assert!(validate_server(&s).is_ok());
        // 非法头名（空格）
        let mut bad = s.clone();
        bad.headers.insert("Bad Header".into(), "v".into());
        assert!(validate_server(&bad)
            .unwrap_err()
            .contains("非法的 HTTP 头名"));
        // 非法值（控制字符）
        let mut bad2 = s.clone();
        bad2.headers.insert("X-Token".into(), "a\nb".into());
        assert!(validate_server(&bad2).unwrap_err().contains("非法字符"));
        // normalize：trim + 空头名丢弃
        let mut n = s.clone();
        n.headers.insert("  X-A  ".into(), "  v  ".into());
        // 「空键非空值」变体（B0 评审 HIGH 同款）：必须整条消失而非键改写成空串留下
        n.headers.insert(" ".into(), "leak".into());
        let n = normalize_server(n);
        assert_eq!(n.headers.get("X-A").map(String::as_str), Some("v"));
        assert!(n.headers.keys().all(|k| !k.trim().is_empty()));
        assert_eq!(n.headers.len(), 2, "Authorization + X-A，空键条目应消失");
    }

    #[test]
    fn http_url_gate_rejects_local_loopback_and_private() {
        // localhost（域名 + .localhost 后缀 + 尾点变体，评审加固）
        assert!(http_url_is_public("http://localhost:3000/mcp").is_err());
        assert!(http_url_is_public("http://LOCALHOST:3000/mcp").is_err());
        assert!(http_url_is_public("http://localhost./mcp").is_err());
        assert!(http_url_is_public("https://api.localhost/mcp").is_err());
        assert!(http_url_is_public("https://api.localhost./mcp").is_err());
        assert!(http_url_is_public("http://LOCALHOST../mcp").is_err());
        // 环回（字面量）
        assert!(http_url_is_public("http://127.0.0.1:8080/mcp").is_err());
        assert!(http_url_is_public("http://127.5.5.5/mcp").is_err());
        assert!(http_url_is_public("http://[::1]/mcp").is_err());
        // 私网
        assert!(http_url_is_public("http://10.1.2.3/mcp").is_err());
        assert!(http_url_is_public("http://172.16.0.9/mcp").is_err());
        assert!(http_url_is_public("http://192.168.1.1/mcp").is_err());
        assert!(http_url_is_public("http://169.254.1.1/mcp").is_err());
        assert!(http_url_is_public("http://[fc00::1]/mcp").is_err());
        assert!(http_url_is_public("http://[fe80::1]/mcp").is_err());
        // IPv4-mapped IPv6（评审加固：dual-stack 还原成 IPv4 判定）
        assert!(http_url_is_public("http://[::ffff:127.0.0.1]/mcp").is_err());
        assert!(http_url_is_public("http://[::ffff:10.0.0.1]/mcp").is_err());
        assert!(http_url_is_public("http://[::ffff:7f00:1]/mcp").is_err());
        assert!(http_url_is_public("http://[::ffff:192.168.1.1]/mcp").is_err());
        // IPv4-compatible（::/96 弃用段整组拒绝）
        assert!(http_url_is_public("http://[::127.0.0.1]/mcp").is_err());
        // 转换前缀（B0 评审 M-10）：嵌 IPv4 经中继可达内网，整组拒绝
        assert!(http_url_is_public("http://[2002:0a00:0001::1]/mcp").is_err());
        assert!(http_url_is_public("http://[2001:0:8765:1::1]/mcp").is_err());
        assert!(http_url_is_public("http://[64:ff9b::10.0.0.1]/mcp").is_err());
        // 保留 / CGNAT / 组播 / 0/8
        assert!(http_url_is_public("http://0.1.2.3/mcp").is_err());
        assert!(http_url_is_public("http://100.64.0.1/mcp").is_err());
        assert!(http_url_is_public("http://224.0.0.1/mcp").is_err());
        assert!(http_url_is_public("http://240.0.0.1/mcp").is_err());
        // 数字形式 IPv4 变体（B0 评审：url 归为 Domain，绕过字面量 IP 闸）
        assert!(http_url_is_public("http://2130706433/mcp").is_err());
        assert!(http_url_is_public("http://0177.0.0.1/mcp").is_err());
        assert!(http_url_is_public("http://0x7f.0.0.1/mcp").is_err());
        assert!(http_url_is_public("http://127.1/mcp").is_err());
        // mDNS .local（B0 评审：链路本地/局域网设备入口）
        assert!(http_url_is_public("http://printer.local/mcp").is_err());
        assert!(http_url_is_public("http://local/mcp").is_err());
        // localhost 家族别名（/etc/hosts 惯常映射，B0 评审补）
        assert!(http_url_is_public("http://localhost.localdomain/mcp").is_err());
        assert!(http_url_is_public("http://ip6-localhost/mcp").is_err());
        assert!(http_url_is_public("http://broadcasthost/mcp").is_err());
        // IPv6 zone-id（url crate 不支持 scope-id：要么解析失败要么命中保留段，都须拒）
        assert!(http_url_is_public("http://[fe80::1%25eth0]/mcp").is_err());
        // 文档段 2001:db8::/32（B0 评审：与 v4 侧 is_documentation 对齐）
        assert!(http_url_is_public("http://[2001:db8::1]/mcp").is_err());
        // 解析失败不回显原文（B0 评审：URL 可能内嵌凭据）
        assert!(http_url_is_public("ht!tp://x").is_err());
        // 非 http(s) scheme
        assert!(http_url_is_public("file:///etc/passwd").is_err());
    }

    #[test]
    fn http_url_gate_allows_public_hosts() {
        assert!(http_url_is_public("https://mcp.example.com/mcp").is_ok());
        assert!(http_url_is_public("http://93.184.216.34/mcp").is_ok());
        assert!(http_url_is_public("https://MCP.Example.COM").is_ok());
        // 8.8.8.8 公网、非保留段
        assert!(http_url_is_public("http://8.8.8.8:443/mcp").is_ok());
        // 非转换前缀的公网 IPv6 不受影响
        assert!(http_url_is_public("http://[2606:4700::1111]/mcp").is_ok());
    }

    #[test]
    fn args_and_env_caps_enforced() {
        let mut s = stdio_server("fs");
        s.args = (0..=MAX_MCP_ARGS).map(|i| format!("arg{i}")).collect();
        assert!(validate_server(&s).unwrap_err().contains("参数过多"));
        let mut s = stdio_server("fs");
        s.env.insert("K".repeat(129), "v".into());
        assert!(validate_server(&s).unwrap_err().contains("环境变量名过长"));
    }

    #[test]
    fn validate_server_checks_id_hygiene() {
        // 空 id 合法 = 新建（save 路径在校验前生成 uuid）
        let mut s = stdio_server("fs");
        s.id = String::new();
        assert!(validate_server(&s).is_ok());
        // 超长 / 控制字符（B0 评审：id 是定位键，脏 id 入库后删改查全受影响）
        let mut s = stdio_server("fs");
        s.id = "x".repeat(MAX_MCP_ID_LEN + 1);
        assert!(validate_server(&s).unwrap_err().contains("id 过长"));
        let mut s = stdio_server("fs");
        s.id = "a\nb".into();
        assert!(validate_server(&s).unwrap_err().contains("控制字符"));
    }

    // ── normalize / timeout 钳制 ──

    #[test]
    fn normalize_trims_and_drops_empty_env_keys() {
        let mut s = stdio_server("  fs  ");
        s.command = Some(" npx ".into());
        s.env.insert("  ".into(), "x".into());
        s.env.insert("HOME".into(), " /tmp ".into());
        s.timeout_secs = Some(999_999);
        let s = normalize_server(s);
        assert_eq!(s.name, "fs");
        assert_eq!(s.command.as_deref(), Some("npx"));
        // 「空键非空值」必须整条丢弃（B0 评审 HIGH）：不许被 trim 成空串键留下
        //（旧断言 !contains_key("  ") 是假阳性——键被改写成 "" 仍留在表里也通过）
        assert!(s.env.keys().all(|k| !k.trim().is_empty()));
        assert_eq!(s.env.len(), 1, "只应剩 HOME 一条");
        assert_eq!(s.env.get("HOME").map(String::as_str), Some("/tmp"));
        assert_eq!(s.timeout_secs, Some(MCP_TIMEOUT_MAX_SECS));
    }

    #[test]
    fn effective_timeout_defaults_and_clamps() {
        let mut s = stdio_server("fs");
        assert_eq!(s.effective_timeout_secs(), MCP_TIMEOUT_DEFAULT_SECS);
        s.timeout_secs = Some(1);
        assert_eq!(s.effective_timeout_secs(), MCP_TIMEOUT_MIN_SECS);
        assert_eq!(clamp_timeout(Some(0)), Some(MCP_TIMEOUT_MIN_SECS));
        assert_eq!(clamp_timeout(None), None);
    }

    // ── 工具命名 ──

    #[test]
    fn tool_prefix_lowercases_and_sanitizes() {
        assert_eq!(tool_prefix("Filesystem"), "filesystem");
        assert_eq!(tool_prefix("my tools!"), "my_tools_");
        assert_eq!(tool_prefix("中文"), "__");
        assert_eq!(tool_prefix(""), "_");
        let long = tool_prefix(&"a".repeat(100));
        assert_eq!(long.len(), 24);
    }

    #[test]
    fn mcp_tool_name_shapes_and_caps_at_64_bytes() {
        assert_eq!(mcp_tool_name("fs", "read_file"), "mcp_fs_read_file");
        // 非 ASCII 工具名净化（不会产出非法 function name）
        let n = mcp_tool_name("fs", "读取文件");
        assert!(n.is_ascii());
        assert!(n.starts_with("mcp_fs_"));
        // 超长截断：恒 ≤ 64 字节且带 6 位哈希尾
        let n = mcp_tool_name("fs", &"x".repeat(200));
        assert!(n.len() <= 64, "实际 {n} ({} bytes)", n.len());
        assert!(n.ends_with(|c: char| c.is_ascii_hexdigit()));
        // 哈希尾防撞：两个不同超长名不重名
        let a = mcp_tool_name("fs", &"a".repeat(200));
        let b = mcp_tool_name("fs", &"b".repeat(200));
        assert_ne!(a, b);
    }

    // ── 配置表内核 ──

    #[test]
    fn upsert_inserts_then_replaces_by_id_rejects_dup_name() {
        let mut cfg = BotConfig::default();
        let s1 = stdio_server("fs");
        upsert_in_config(&mut cfg, s1.clone()).unwrap();
        assert_eq!(cfg.mcp_servers.as_ref().unwrap().len(), 1);
        // 同 id = 整体替换
        let mut s1b = s1.clone();
        s1b.command = Some("uvx".into());
        upsert_in_config(&mut cfg, s1b).unwrap();
        assert_eq!(cfg.mcp_servers.as_ref().unwrap().len(), 1);
        assert_eq!(
            cfg.mcp_servers.as_ref().unwrap()[0].command.as_deref(),
            Some("uvx")
        );
        // 重名（不同 id，大小写不敏感）拒绝
        let mut s2 = stdio_server("FS");
        s2.id = uuid::Uuid::new_v4().simple().to_string();
        let err = upsert_in_config(&mut cfg, s2).unwrap_err();
        assert!(err.to_string().contains("已被其他"), "实际：{err}");
    }

    #[test]
    fn upsert_normalizes_before_validate_and_store() {
        let mut cfg = BotConfig::default();
        let mut s = stdio_server("fs");
        s.name = "  fs  ".into(); // normalize 后合法
        upsert_in_config(&mut cfg, s).unwrap();
        assert_eq!(cfg.mcp_servers.as_ref().unwrap()[0].name, "fs");
    }

    #[test]
    fn remove_and_toggle_are_idempotent_shape_keeping() {
        let mut cfg = BotConfig::default();
        let s = stdio_server("fs");
        upsert_in_config(&mut cfg, s.clone()).unwrap();
        // 启停
        assert!(set_enabled_in_config(&mut cfg, &s.id, false));
        assert!(!cfg.mcp_servers.as_ref().unwrap()[0].enabled);
        assert!(!set_enabled_in_config(&mut cfg, "nope", true));
        // 删除 + 空表回收（字段整个消失）
        assert!(remove_from_config(&mut cfg, &s.id));
        assert!(cfg.mcp_servers.is_none(), "空列表应回收为 None");
        assert!(!remove_from_config(&mut cfg, &s.id));
        // 删除不存在时 None 不 panic
        assert!(!remove_from_config(&mut BotConfig::default(), "x"));
    }

    // ── serde：老配置兼容 + 往返 ──

    #[test]
    fn old_config_without_mcp_servers_parses_to_none() {
        let raw = serde_json::json!({
            "baseUrl": "https://api.deepseek.com/v1",
            "model": "deepseek-v4-flash",
            "bypassLlmOnPreStepHit": true
        });
        let cfg: BotConfig = serde_json::from_value(raw).unwrap();
        assert!(cfg.mcp_servers.is_none());
    }

    #[test]
    fn mcp_servers_roundtrip_keeps_entries_and_skips_empty_shape() {
        let mut cfg = BotConfig::default();
        // None 时不序列化字段（老文件形状不变）
        assert!(serde_json::to_value(&cfg)
            .unwrap()
            .get("mcpServers")
            .is_none());
        upsert_in_config(&mut cfg, stdio_server("fs")).unwrap();
        let v = serde_json::to_value(&cfg).unwrap();
        let arr = v["mcpServers"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["transport"], "stdio");
        assert_eq!(arr[0]["enabled"], true);
        assert!(arr[0].get("env").is_none(), "空 env 不落盘");
        assert!(arr[0].get("timeoutSecs").is_none(), "None 超时不落盘");
        let cfg2: BotConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg2.mcp_servers, cfg.mcp_servers);
    }
}
