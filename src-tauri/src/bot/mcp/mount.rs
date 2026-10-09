//! MCP 外部工具挂载层（阶段 3，机制 A）：连接槽 × 工具快照 → agent 工具清单。
//!
//! 职责（纯增量，不动内置 32 工具的任何注册路径）：
//! - `mcp_mounted_tools()`：当前可挂载条目（函数名已消歧、schema 已整形）
//! - `mcp_tools_json_body()`：拼进主 agent tools JSON 尾部的动态段
//! - `find_mounted(func_name)`：阶段 4 dispatch 反查（函数名 → 服务器 id + 原始工具名）
//!
//! 命名：`mcp_{服务器前缀}_{工具名}`（config::mcp_tool_name，64 字节硬上限内）。
//! 不同服务器净化后前缀相同且工具同名会撞名——`build_mounted_tools` 按构建序
//! 加 `_2`/`_3` 序号消歧，schema 与 dispatch 反查共用同一份构建结果，不漂移。
//!
//! 会话边界：MCP 工具只进**主 agent** 清单（registry::tools_json_with_mcp 对
//! 子 agent 会话短路返回白名单——设计 §5.1：子 agent 工具面静态、可枚举、可审计）。

use rmcp::model::{CallToolResult, Tool};
use serde_json::json;

use super::config::mcp_tool_name;
use super::config::McpServerConfig;
use super::manager::shared;

/// 单个可挂载的外部工具条目（schema 与反查的单一来源）
#[derive(Debug, Clone)]
pub struct McpMountedTool {
    /// agent 看到的 function 名（消歧后）
    pub func_name: String,
    /// 来源服务器配置 id（挂载反查回 `config::io` 按 id 取配置 + call 路由）
    pub server_id: String,
    /// 来源服务器显示名（错误文案/描述兜底用，区别于 id）
    pub server_name: String,
    /// 服务器侧原始工具名（调用时透传）
    pub tool_name: String,
    /// OpenAI function schema（parameters = 服务器给的 inputSchema 原样）
    pub schema: serde_json::Value,
}

/// 工具结果文本总长上限（fetch_url 同口径，防爆上下文）
const MAX_RESULT_CHARS: usize = 30_000;

/// 单工具 schema 序列化体积上限（字节）。：
/// inputSchema 服务器侧原样透传，无上限会每轮全量拼进 tools JSON。超限不丢
/// 工具（保功能）：参数 schema 降级为宽松空对象，模型按描述调用、服务器侧校验兜底。
const MAX_SCHEMA_BYTES: usize = 8 * 1024;

/// 描述整形：trim + 512 字符截断 + 缺省兜底（描述是模型选工具的主要依据，
/// 超长服务器描述不能挤爆主上下文）
fn shape_description(server_name: &str, t: &Tool) -> String {
    let from_server = t
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let desc = match from_server {
        Some(d) => d.to_string(),
        None => format!("外部 MCP 工具（服务器：{server_name}）"),
    };
    desc.chars().take(512).collect()
}

/// 构建单条挂载 schema（纯函数）：parameters 用服务器 inputSchema 原样
/// （类型上恒为 object Map；空 Map 也是合法空 schema）。
/// 序列化超 MAX_SCHEMA_BYTES 时降级——宽松空参数 + 描述标注，不丢工具。
fn tool_schema(server_name: &str, func_name: &str, t: &Tool) -> serde_json::Value {
    let desc = shape_description(server_name, t);
    let params = serde_json::Value::Object((*t.input_schema).clone());
    let schema = json!({
        "type": "function",
        "function": {
            "name": func_name,
            "description": desc.clone(),
            "parameters": params,
        }
    });
    let oversized = serde_json::to_string(&schema)
        .map(|s| s.len() > MAX_SCHEMA_BYTES)
        .unwrap_or(true);
    if oversized {
        return json!({
            "type": "function",
            "function": {
                "name": func_name,
                "description": format!("{desc}（inputSchema 过大已省略，按无固定参数调用）"),
                "parameters": { "type": "object", "additionalProperties": true },
            }
        });
    }
    schema
}

/// 64 字节函数名钳制（消歧后缀可能顶破上限）：超限时尾部 8 字节换 6 位哈希。
/// 与 config::mcp_tool_name 同款套路；输入恒 ASCII，按字节截断安全。
fn clamp_fn_name(name: String) -> String {
    const MAX: usize = 64;
    if name.len() <= MAX {
        return name;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let suffix = format!("{:06x}", h.finish() & 0xffffff);
    let keep = MAX - suffix.len() - 1;
    format!("{}_{suffix}", &name[..keep])
}

/// 构建挂载条目（纯函数内核，单测直打）。函数名全局唯一：
/// 同 base 撞名按构建序加 `_2`/`_3`；后缀撞到**真实工具名**也继续递增
///（评审：A 服务器有 "read" 和 "read 2"、B 服务器（同前缀）有 "read" 时，
/// 朴素计数会产出两个 mcp_p_read_2）——used 集合查重保证 schema 无重名。
pub fn build_mounted_tools(servers: &[(String, String, Vec<Tool>)]) -> Vec<McpMountedTool> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (server_id, server_name, tools) in servers {
        for t in tools {
            let base = mcp_tool_name(server_name, t.name.as_ref());
            let mut n = 1usize;
            let mut func = base.clone();
            while !used.insert(func.clone()) {
                n += 1;
                func = clamp_fn_name(format!("{base}_{n}"));
            }
            out.push(McpMountedTool {
                schema: tool_schema(server_name, &func, t),
                func_name: func,
                server_id: server_id.clone(),
                server_name: server_name.clone(),
                tool_name: t.name.to_string(),
            });
        }
    }
    out
}

/// 当前可挂载条目（读全局连接槽；无连接 → 空表）
pub fn mcp_mounted_tools() -> Vec<McpMountedTool> {
    build_mounted_tools(&shared().mounted())
}

/// dispatch 反查（阶段 4）：函数名 → 挂载条目（拿 server_id 查配置、tool_name 调远端）。
/// 现场重建挂载表做线性查——每轮模型循环最多 miss 一次才走到这，开销可忽略。
pub fn find_mounted(func_name: &str) -> Option<McpMountedTool> {
    mcp_mounted_tools()
        .into_iter()
        .find(|t| t.func_name == func_name)
}

// 阶段 4：dispatch 路由桥

/// dispatch 查表 miss 时的 MCP 兜底路由。
/// 边界（全部由上游保证，此处不重复）：
/// - 子 agent 会话到不了这里——dispatch 的白名单闸在查表前已拒（MCP 工具不在任何 profile 白名单）；
/// - 审计由 execute_tool_impl 统一包（tool.call / tool.return 已发出）。
/// 服务器删除/禁用/未连接都是**文本返回**（仓库惯例：不炸主循环，模型拿到
/// 明确错误自己绕开）；只有反查不到挂载表（模型编造的工具名）走 Error 状态。
pub(crate) async fn execute_mcp_tool(
    app: &tauri::AppHandle,
    name: &str,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let Some(mounted) = find_mounted(name) else {
        return crate::bot::registry::ToolResult::error(format!("未知工具：{name}"), Vec::new());
    };
    // load_config 是同步文件读（ 评审 HIGH）：挪 spawn_blocking，不阻塞
    // tokio worker——dispatch 的工具调用全在 runtime 线程上 await 这里
    let app = app.clone();
    let servers = tauri::async_runtime::spawn_blocking(move || {
        crate::bot::config::io::load_config(&app)
            .mcp_servers
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let server = servers.into_iter().find(|s| s.id == mounted.server_id);
    match server {
        Some(cfg) if cfg.enabled => execute_mcp_direct(&cfg, &mounted, args).await,
        Some(_) => crate::bot::registry::ToolResult::warn(
            format!(
                "MCP 服务器「{}」已被禁用，工具 {name} 不可用；下轮清单刷新后不再出现",
                mounted.server_name
            ),
            Vec::new(),
        ),
        None => crate::bot::registry::ToolResult::warn(
            format!(
                "MCP 服务器「{}」已被删除，工具 {name} 不可用；下轮清单刷新后不再出现",
                mounted.server_name
            ),
            Vec::new(),
        ),
    }
}

/// 调用内核（无 AppHandle，单测直打）：带超时调远端 + 结果整形。
pub(crate) async fn execute_mcp_direct(
    cfg: &McpServerConfig,
    mounted: &McpMountedTool,
    args: &str,
) -> crate::bot::registry::ToolResult {
    // 参数解析（ 评审：不再把 Null 静默透传远端）——非法 JSON 本地明确回
    // warn 文本，模型拿错误自己重发；空串按空对象（无参工具的常见形态）
    let parsed: serde_json::Value = if args.trim().is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(args) {
            Ok(v) => v,
            Err(e) => {
                return crate::bot::registry::ToolResult::warn(
                    format!("MCP 参数不是合法 JSON，未发起调用：{e}；请以 JSON 对象重发参数"),
                    Vec::new(),
                );
            }
        }
    };
    // 合法 JSON 但非对象（数组/字符串/数字等）也不透传：透传后远端按对象收参
    // 会拿不到字段 → 无参静默调用。本地明确回 warn，模型拿错误自己重发。
    if !parsed.is_object() {
        return crate::bot::registry::ToolResult::warn(
            format!(
                "MCP 工具参数必须是 JSON 对象，未发起调用：{}；请以 JSON 对象重发参数",
                mounted.func_name
            ),
            Vec::new(),
        );
    }
    match shared().call_tool(cfg, &mounted.tool_name, parsed).await {
        Ok(result) => {
            if result.is_error == Some(true) {
                crate::bot::registry::ToolResult::warn(
                    format!("MCP 工具报告错误：\n{}", shape_result_text(&result)),
                    Vec::new(),
                )
            } else {
                crate::bot::registry::ToolResult::ok(shape_result_text(&result), Vec::new())
            }
        }
        // 失败走 ok 文本返回（severity classifier 误判 fatal 防御，与仓库惯例一致）；
        // 首字「MCP」非 error/warn 前缀，机械判定按 ok 处理
        Err(e) => crate::bot::registry::ToolResult::ok(format!("MCP 调用失败：{e}"), Vec::new()),
    }
}

/// CallToolResult → 模型可读文本：text 内容原样拼接，非文本块占位说明，
/// structuredContent 附后。总长钳 MAX_RESULT_CHARS（fetch_url 同口径，防爆上下文）。
fn shape_result_text(result: &CallToolResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in &result.content {
        match block {
            rmcp::model::ContentBlock::Text(t) => parts.push(t.text.to_string()),
            rmcp::model::ContentBlock::Image(_) => {
                parts.push("（图片内容：本工具无法在文本回话中展示，请让用户直接查看）".into())
            }
            rmcp::model::ContentBlock::Audio(_) => parts.push("（音频内容，略）".into()),
            rmcp::model::ContentBlock::Resource(_) | rmcp::model::ContentBlock::ResourceLink(_) => {
                parts.push("（内嵌资源内容，略）".into())
            }
            // #[non_exhaustive]：协议升级新增内容类型时兜底
            _ => parts.push("（未知内容类型，略）".into()),
        }
    }
    if let Some(sc) = &result.structured_content {
        // 先序列化按上限钳制再入栈（内存优化：
        // 超大 structuredContent 不先全量入 parts 再 join）；截断标注由下方
        // 整段截断统一给出（sc 钳到上限后加前缀必然再触发总长钳制）
        let mut s = sc.to_string();
        let n = s.chars().count();
        if n > MAX_RESULT_CHARS {
            s = s.chars().take(MAX_RESULT_CHARS).collect();
        }
        parts.push(format!("[structuredContent] {s}"));
    }
    let mut text = if parts.is_empty() {
        "（工具执行完成，无返回内容）".to_string()
    } else {
        parts.join("\n")
    };
    let total = text.chars().count();
    if total > MAX_RESULT_CHARS {
        text = text.chars().take(MAX_RESULT_CHARS).collect();
        text.push_str(&format!("\n\n（内容过长已截断：原 {total} 字符）"));
    }
    text
}

/// 拼进主 agent tools JSON 的动态段：`",\n  {schema}..."` 序列（空表 = 空串）。
/// registry::tools_json_with_mcp 把它插进静态 JSON 的尾部 `"\n]"` 之前。
pub fn mcp_tools_json_body() -> String {
    build_tools_json_body(&shared().mounted())
}

/// 同上（纯函数内核，单测直打）
pub fn build_tools_json_body(servers: &[(String, String, Vec<Tool>)]) -> String {
    let entries = build_mounted_tools(servers);
    let mut s = String::new();
    for e in &entries {
        s.push_str(",\n  ");
        s.push_str(&e.schema.to_string());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, desc: &str) -> Tool {
        serde_json::from_value(json!({
            "name": name,
            "description": desc,
            "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}}
        }))
        .unwrap()
    }

    fn server(id: &str, name: &str, tools: Vec<Tool>) -> (String, String, Vec<Tool>) {
        (id.into(), name.into(), tools)
    }

    #[test]
    fn mounted_entries_carry_names_schema_and_lookup_key() {
        let out = build_mounted_tools(&[server("s1", "fs", vec![tool("read", "读文件")])]);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.func_name, "mcp_fs_read");
        assert_eq!(e.server_id, "s1");
        assert_eq!(e.tool_name, "read");
        assert_eq!(e.schema["type"], "function");
        assert_eq!(e.schema["function"]["name"], "mcp_fs_read");
        assert_eq!(e.schema["function"]["description"], "读文件");
        assert_eq!(e.schema["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn same_prefix_tools_are_disambiguated_in_build_order() {
        // 「my tool」和「my.tool」前缀同为 my_tool，read 工具撞名 → 第二个加 _2
        let out = build_mounted_tools(&[
            server("a", "my tool", vec![tool("read", "A 的读")]),
            server("b", "my.tool", vec![tool("read", "B 的读")]),
        ]);
        let names: Vec<&str> = out.iter().map(|e| e.func_name.as_str()).collect();
        assert_eq!(names, ["mcp_my_tool_read", "mcp_my_tool_read_2"]);
        // 反查语义：名字唯一可定位（阶段 4 的路由键）
        assert_eq!(out[1].server_id, "b");
    }

    #[test]
    fn disambiguation_suffix_colliding_with_real_tool_name_stays_unique() {
        // 评审：A 有 "read" + "read 2"（净化后 read_2），B（同前缀）有 "read"。
        // 朴素计数会给 B 产出 mcp_p_read_2 —— 与 A 的真实 read_2 重名。
        // used 集合查重后 B 应顺延到 read_3，schema 无重名。
        let out = build_mounted_tools(&[
            server("a", "p", vec![tool("read", "A"), tool("read 2", "A2")]),
            server("b", "p", vec![tool("read", "B")]),
        ]);
        let names: Vec<&str> = out.iter().map(|e| e.func_name.as_str()).collect();
        assert_eq!(names, ["mcp_p_read", "mcp_p_read_2", "mcp_p_read_3"]);
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "函数名必须全局唯一");
    }

    #[test]
    fn disambiguation_is_deterministic_for_sorted_input() {
        // 评审：同一份输入必须产出同一份命名（manager::mounted() 已按
        // server_id 排序后再喂进来）；这里锁纯函数层的确定性。
        let input = [
            server("a", "p", vec![tool("read", "A")]),
            server("b", "p", vec![tool("read", "B")]),
        ];
        let run1: Vec<String> = build_mounted_tools(&input)
            .into_iter()
            .map(|e| e.func_name)
            .collect();
        let run2: Vec<String> = build_mounted_tools(&input)
            .into_iter()
            .map(|e| e.func_name)
            .collect();
        assert_eq!(run1, run2);
    }

    #[test]
    fn missing_description_falls_back_and_long_is_truncated() {
        let mut long = tool("t", "");
        long.description = Some("长".repeat(2000).into());
        let out = build_mounted_tools(&[server("s", "srv", vec![tool("nodesc", ""), long])]);
        assert_eq!(
            out[0].schema["function"]["description"],
            "外部 MCP 工具（服务器：srv）"
        );
        let d = out[1].schema["function"]["description"].as_str().unwrap();
        assert_eq!(d.chars().count(), 512);
    }

    #[test]
    fn json_body_is_comma_separated_and_empty_when_no_servers() {
        assert_eq!(build_tools_json_body(&[]), "");
        let body = build_tools_json_body(&[server("s", "fs", vec![tool("a", ""), tool("b", "")])]);
        assert!(body.starts_with(",\n  "), "以逗号开头供拼接：{body}");
        assert!(body.contains("\"mcp_fs_a\""));
        assert!(body.contains("\"mcp_fs_b\""));
        // 必须是合法 JSON 片段：去掉开头逗号、包上数组括号后能 parse
        let v: serde_json::Value =
            serde_json::from_str(&format!("[{}]\n", body.trim_start_matches(','))).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    /// 超大 inputSchema 降级为宽松空参数 + 描述标注，单 schema 不超上限
    #[test]
    fn oversized_input_schema_degrades_to_permissive_params() {
        let mut props = serde_json::Map::new();
        for i in 0..400 {
            props.insert(
                format!("very_long_property_name_{i}_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                json!({ "type": "string", "description": "x".repeat(64) }),
            );
        }
        let big: Tool = serde_json::from_value(json!({
            "name": "big",
            "description": "大 schema 工具",
            "inputSchema": { "type": "object", "properties": props }
        }))
        .unwrap();
        let out = build_mounted_tools(&[server("s", "srv", vec![big])]);
        let schema = &out[0].schema;
        assert!(
            serde_json::to_string(schema).unwrap().len() <= 8 * 1024,
            "降级后 schema 应在上限内"
        );
        assert_eq!(schema["function"]["parameters"]["type"], "object");
        assert_eq!(
            schema["function"]["parameters"]["additionalProperties"],
            true
        );
        assert!(
            schema["function"]["description"]
                .as_str()
                .unwrap()
                .contains("过大已省略"),
            "描述应标注降级"
        );
    }

    /// 超大 structuredContent 不再无界拼入结果——总量被钳在上限附近，
    /// 截断标注由整段截断统一给出
    #[test]
    fn overlong_structured_content_is_capped() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [],
            "structuredContent": { "data": "x".repeat(50_000) },
        }))
        .expect("构造 CallToolResult");
        let text = shape_result_text(&result);
        assert!(
            text.chars().count() <= MAX_RESULT_CHARS + 60,
            "应在上限附近：{}",
            text.chars().count()
        );
        assert!(text.contains("内容过长已截断"), "应有截断标注");
    }

    /// 非对象合法 JSON 参数（数组/字符串）不透传远端——透传会变成无参静默调用，
    /// 本地 warn 回显（校验在 call_tool 之前，无需真实连接）
    #[tokio::test]
    async fn non_object_args_are_rejected_before_remote_call() {
        let cfg = McpServerConfig::default();
        let mounted = McpMountedTool {
            func_name: "mcp_fs_read".into(),
            server_id: "s1".into(),
            server_name: "fs".into(),
            tool_name: "read".into(),
            schema: json!({}),
        };
        for args in ["[1,2]", "\"x\"", "42"] {
            let out = execute_mcp_direct(&cfg, &mounted, args).await;
            assert!(
                out.text.contains("必须是 JSON 对象"),
                "args={args} 应本地拒绝：{:?}",
                out.text
            );
        }
        // 空参（无参工具常见形态）仍按空对象放行——不会卡在本地校验
        let empty = execute_mcp_direct(&cfg, &mounted, "").await;
        assert!(
            !empty.text.contains("必须是 JSON 对象"),
            "空串应放行：{:?}",
            empty.text
        );
    }
}
