//! 模型流式调用 + 工具循环 核心（F-6 step 5 拆分 2026-08-18）：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「决策/调用」与「执行/工具循环」层职责：
//! - 流式 SSE 解析（parse_sse_chunk）
//! - 思考块拆分（feed_think + tail_prefix_len）
//! - 工具循环（run_model_loop）：单轮 Function 调用熔断 + 软警告 +
//!   Skill 状态机推进 + 进程内执行工具 + 续聊
//! - TOOLS schema 编译期保证合法（tests/llm_integration.rs 接入）
//!
//! 本模块与 bot_chat 的边界：bot_chat.rs 调 run_model_loop 拿到最终文本；
//! 本模块只关心「怎么流式拿到最终文本 + 工具执行结果」，不关心输入侧组装。

use crate::bot_chat::{merge_task_refs_dedup, TaskRef};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};

// ───────────────────────── TOOLS schema（编译期字符串，运行期 JSON 解析） ─────────────────────────

const TOOLS: &str = r#"[
  {"type":"function","function":{"name":"list_tasks","description":"列出未完成任务（含状态列）","parameters":{"type":"object","properties":{}}}},
  {"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除状态指示；白名单单点，区别于 list_tasks 批量清单与 search_tasks 关键词检索）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}},
  {"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DD HH:mm，可选"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo"}
  },"required":["title"]}}},
  {"type":"function","function":{"name":"complete_task","description":"完成任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id（来自用户消息的 [已选任务] 引用块或 list_tasks 输出），可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}},
  {"type":"function","function":{"name":"delete_task","description":"删除任务到回收站（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}},
  {"type":"function","function":{"name":"edit_task","description":"编辑任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；改标题/备注/截止时间/标签/状态列，空串清字段）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时用于定位任务"},
    "newTitle":{"type":"string","description":"新标题，可选"},
    "note":{"type":"string","description":"新备注，可选；空串清除"},
    "due":{"type":"string","description":"新截止时间，可选；空串清除"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"}
  },"required":[]}}},
  {"type":"function","function":{"name":"add_subtask","description":"给任务添加子任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容"}
  },"required":["text"]}}},
  {"type":"function","function":{"name":"toggle_subtask","description":"勾选/取消勾选子任务（任务用 taskId 优先；子任务按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容关键词"}
  },"required":["text"]}}},
  {"type":"function","function":{"name":"bind_file","description":"给任务绑定文件或文件夹（弹系统选择框由用户挑选；任务用 taskId 优先定位）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "isDir":{"type":"boolean","description":"true=选文件夹，false 选文件"}
  },"required":[]}}},
  {"type":"function","function":{"name":"link_file_to_task","description":"把 AI_Gen_Files 目录内的生成文件绑定到任务卡（不弹选择框；只允许该目录内的文件，其他文件请在任务卡上手动绑定）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "path":{"type":"string","description":"要绑定的文件绝对路径（必须位于 AI_Gen_Files 目录内）"}
  },"required":["path"]}}},
  {"type":"function","function":{"name":"search_tasks","description":"按关键词搜索所有任务卡（待办/进行中/已完成/已归档；匹配标题/备注/标签/子任务）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"}
  },"required":["query"]}}},
  {"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"}
  }}}},
  {"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表，每段一个字符串"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}},
  {"type":"function","function":{"name":"create_word_revisions","description":"生成带修订标记（修订模式）的 Word 到 AI_Gen_Files：自动对比原文与润色后的段落，删除内容标删除线、新增内容标红色下划线，可在 Word 审阅中逐条接受/拒绝","parameters":{"type":"object","properties":{
    "originalPath":{"type":"string","description":"原文 Word 路径（extract_document 返回的 [文档路径]）"},
    "original":{"type":"array","items":{"type":"string"},"description":"原文行列表（提取被截断时必须传，保证对比范围一致），可选"},
    "revised":{"type":"array","items":{"type":"string"},"description":"润色后的段落列表"},
    "title":{"type":"string","description":"文档标题，可选"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["revised"]}}},
  {"type":"function","function":{"name":"create_excel","description":"生成 Excel 到 AI_Gen_Files（单元格以 = 开头会写入原生公式如 =SUM(A1:A10)）","parameters":{"type":"object","properties":{
    "sheets":{"type":"array","items":{"type":"object","properties":{
      "name":{"type":"string"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}}},
    "description":"工作表列表：name 表名、rows 二维数组"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["sheets"]}}},
  {"type":"function","function":{"name":"create_ppt","description":"生成专业排版 PPT 到 AI_Gen_Files（多版式：封面/目录/章节页/内容页/表格页/结束页 + 三套配色主题）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"演示文稿主标题"},
    "theme":{"type":"string","enum":["blue","navy","teal","forest","wine","sky","plum","coral","dark","green"],"description":"配色主题（按场合选）：blue 商务与权威（默认，汇报/金融）/ navy 科技与夜景（深色发布会）/ teal 现代与健康（医疗/护肤）/ forest 自然与户外（环保/农业）/ wine 复古与学院（学术/历史）/ sky 纯净科技蓝（AI/云计算）/ plum 轻奢与神秘（珠宝/高端咨询）/ coral 海岸珊瑚（旅游/夏日）/ dark 深色通用 / green 清新绿"},
    "slides":{"type":"array","description":"幻灯片列表，按展示顺序；每页一个 type","items":{"type":"object","properties":{
      "type":{"type":"string","enum":["cover","toc","section","content","table","closing"],"description":"页面类型：cover 封面（title+subtitle）/ toc 目录（items 列表）/ section 章节分隔页 / content 内容要点页 / table 表格页（rows 二维数组首行表头）/ closing 结束页"},
      "title":{"type":"string","description":"页面标题"},
      "subtitle":{"type":"string","description":"副标题（cover/section/closing 用）"},
      "bullets":{"type":"array","items":{"type":"string"},"description":"要点列表（content 页；≤5 条大字号，6-8 条中号，8 条以上自动双栏）"},
      "items":{"type":"array","items":{"type":"string"},"description":"目录条目（toc 页）"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}},"description":"表格数据（table 页；第一行是表头）"}
    },"required":["type","title"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["slides"]}}},
  {"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}},
  {"type":"function","function":{"name":"run_python","description":"执行 Python 代码（本机沙箱：独立临时目录 + 超时 60s；需用户在设置页开启 Python 编程）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"}
  },"required":["code"]}}},
  {"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（Bing+百度双引擎，返回标题/链接/摘要）","parameters":{"type":"object","properties":{"query":{"type":"string","description":"搜索关键词"}},"required":["query"]}}},
  {"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址"}
  },"required":["url"]}}},
  {"type":"function","function":{"name":"use_skill","description":"读取已安装技能（skill）的完整文档并按文档步骤执行。任务涉及的每个相关技能都要读（可多次调用）：例如做 PPT 时，若清单里同时有编排、生成、配色、风格类技能，应逐个读取、取长补短综合运用，不要只读一个","parameters":{"type":"object","properties":{
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"}
  },"required":["name"]}}}
]"#;

/// <think> 标签拆分：喂入流式文本，返回 (正文, 思考)。标签跨流式块时用 think_buf 缓冲。
fn feed_think(in_think: &mut bool, buf: &mut String, text: &str) -> (String, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    buf.push_str(text);
    let mut normal = String::new();
    let mut think = String::new();
    loop {
        if !*in_think {
            if let Some(pos) = buf.find(OPEN) {
                normal.push_str(&buf[..pos]);
                buf.drain(..pos + OPEN.len());
                *in_think = true;
            } else {
                // 结尾可能是不完整的 <think> 前缀，留着等下一块
                let keep = tail_prefix_len(buf, OPEN);
                if keep > 0 {
                    let cut = buf.len() - keep; // keep 为 ASCII 前缀，字节数=字符数
                    normal.push_str(&buf[..cut]);
                    buf.drain(..cut);
                } else {
                    normal.push_str(buf);
                    buf.clear();
                }
                break;
            }
        } else if let Some(pos) = buf.find(CLOSE) {
            think.push_str(&buf[..pos]);
            buf.drain(..pos + CLOSE.len());
            *in_think = false;
        } else {
            let keep = tail_prefix_len(buf, CLOSE);
            if keep > 0 {
                let cut = buf.len() - keep;
                think.push_str(&buf[..cut]);
                buf.drain(..cut);
            } else {
                think.push_str(buf);
                buf.clear();
            }
            break;
        }
    }
    (normal, think)
}

/// s 结尾与 tag 开头重合的长度（如 s 尾是 "<thi"、tag "<think>" → 4）。匹配部分必是 ASCII，字节数=字符数。
fn tail_prefix_len(s: &str, tag: &str) -> usize {
    let mut k = tag.len().min(s.len());
    while k > 0 {
        if s.is_char_boundary(s.len() - k) && tag.starts_with(&s[s.len() - k..]) {
            break;
        }
        k -= 1;
    }
    k
}

// ────────────────────────────────────────────────────────────────────
// SSE chunk 解析（F-6 step 4 refactor 2026-08-18 10:31）
//
// 动机：消除 tests/llm_integration.rs 与 run_model_loop 的解析逻辑重复。
// 提取后：测试调 wmessage_lib::bot::parse_sse_chunk，生产代码同样调之，
//        OpenAI SSE 格式演化只改这一处。
// ────────────────────────────────────────────────────────────────────

/// 一次 SSE chunk 解析结果（content / tool_calls / finish_reason / [DONE]）
#[derive(Debug, Default, Clone)]
pub struct ParsedChunk {
    /// delta.content（仅在非空字符串时 Some，与原代码 `!t.is_empty()` 语义一致）
    pub content: Option<String>,
    /// delta.tool_calls 增量（多 chunk 拼成一个完整 tool_call）
    pub tool_calls: Vec<ToolCallDelta>,
    /// choices[0].finish_reason（最后一 chunk 通常为 "stop" / "tool_calls"）
    pub finish_reason: Option<String>,
    /// `data: [DONE]` 标记
    pub is_done: bool,
}

/// 单个 tool_call 增量字段
#[derive(Debug, Default, Clone)]
pub struct ToolCallDelta {
    /// tool_calls[*].index（默认 0）
    pub index: usize,
    /// tool_calls[*].id（Some = 原 JSON 含此字段，值可能为空串）
    pub id: Option<String>,
    /// tool_calls[*].function.name 追加块（多 chunk 拼接）
    pub name_chunk: Option<String>,
    /// tool_calls[*].function.arguments 追加块（多 chunk 拼接成完整 JSON）
    pub arguments_chunk: Option<String>,
}

/// 解析一行 OpenAI 兼容 SSE（`data: <json>` 或 `data: [DONE]`）。
/// 返回 None = 非 data 行 / JSON 解析失败 / choices 为空（与原代码 `else continue` 语义一致）。
///
/// 注意：纯函数，不产生任何 side effect（无 widget emit、无 think-block 处理、无 final_text push）。
/// 调用方（run_model_loop）负责 feed_think + emit + 累积。
pub fn parse_sse_chunk(line: &str) -> Option<ParsedChunk> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(ParsedChunk {
            is_done: true,
            ..Default::default()
        });
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let delta = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .map(|c| &c["delta"])?;

    let mut chunk = ParsedChunk::default();

    if let Some(t) = delta["content"].as_str() {
        if !t.is_empty() {
            chunk.content = Some(t.to_string());
        }
    }

    if let Some(tcs) = delta["tool_calls"].as_array() {
        for tc in tcs {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            let mut delta_tc = ToolCallDelta {
                index: idx,
                ..Default::default()
            };
            if let Some(id) = tc["id"].as_str() {
                delta_tc.id = Some(id.to_string());
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                delta_tc.name_chunk = Some(name.to_string());
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                delta_tc.arguments_chunk = Some(args.to_string());
            }
            chunk.tool_calls.push(delta_tc);
        }
    }

    if let Some(reason) = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c["finish_reason"].as_str())
    {
        chunk.finish_reason = Some(reason.to_string());
    }

    Some(chunk)
}

/// 模型工具循环核心：配置/Key 检查、流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。聊天 8 轮、任务执行 10 轮。
pub async fn run_model_loop(
    app: AppHandle,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
) -> Result<(String, Vec<TaskRef>), CommandError> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    if api_key.trim().is_empty() {
        return Err(CommandError::ApiKeyMissing);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let tools: serde_json::Value = serde_json::from_str(TOOLS).unwrap();

    let mut msgs = msgs;
    // 僵尸终态清理：上轮 Skill 失败/完成的遗留 run 会在第 0 轮短路主循环（agent 假死根因）
    crate::bot_skills::clear_terminal_skill_runs();
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    // Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
    // max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
    //
    // 阈值设定理由（2026-08-18 老板拍板从 5 提到 10）：
    // - 5 太激进：实际 Skill 复合流程（例：minimax-archive = list + query + edit + bind_file + verify）就要 5+，
    //   复杂 Skill（PPT 编排 + 配色 + 归档）需 8-10
    // - 10 中间偏严：覆盖 90% 真实复合任务，留 1.5x 余量给多技能联动
    // - 15+ 太宽：掩护 LLM 死循环 / 幻觉调工具
    // - 软警告（7）收尾提醒：避免刚警告完就熔断
    const MAX_FUNCTION_CALLS_PER_TURN: usize = 10;
    const SOFT_WARN_AT: usize = 7;
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // soft_warn 待注入标志：本轮 tool 响应全部回填后才真正 push（见循环内注释）
    let mut soft_warn_queued: bool = false;
    // 上轮 streamed 文本快照（Block 2 接入，2026-08-17 22:26）：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    for _round in 0..max_rounds {
        if stop.stopped() {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("⏹ 已停止{hint}"), collected_refs));
        }
        // 状态机推进决策（Block 2 2026-08-17 22:26）：集中 Skill 推进逻辑
        // 未来横切关注点（审批/沙箱/上下文压缩）只动 advance_skill，主循环不重构
        if let Some(run) = crate::bot_skills::active_skill_run() {
            use crate::bot_skills::{advance_skill, AdvanceAction};
            match advance_skill(&run, chrono::Utc::now().timestamp_millis()) {
                AdvanceAction::NoActive | AdvanceAction::Continue => {} // 继续本轮
                AdvanceAction::AwaitConfirm => {
                    // Skill 暂停等用户确认，跳出主循环等待 bot_confirm_response 唤起
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Finish => {
                    let _hint = crate::bot_skills::skill_finish(&app, true, "");
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Fail(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Terminate(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
            }
        }
        let body = serde_json::json!({
            "model": cfg.model,
            "messages": msgs,
            "tools": tools,
            "stream": true
        });

        // LLM 请求前记录（F-3 第四步 2026-08-18）
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "llm.request",
            "model" => cfg.model.clone(),
            "msgs_count" => msgs.len(),
        );

        let resp = client
            .post(&url)
            .bearer_auth(api_key.trim())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求大模型失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Warn,
                "llm.response",
                "status" => status.as_u16(),
            );
            let hint = crate::bot_skills::skill_finish(&app, false, "大模型 API 错误");
            return Err(CommandError::LlmApiError {
                status: status.as_u16(),
                body_preview: format!("{}{hint}", text.chars().take(300).collect::<String>()),
            });
        }
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "llm.response",
            "status" => status.as_u16(),
        );

        let mut stream = resp.bytes_stream();
        let mut line_buf = String::new();
        let mut final_text = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments)
                                                                        // <think> 思考块拆分：思考走 bot-think-delta，正文走 bot-chat-delta
        let mut think_mode = false;
        let mut think_buf = String::new();

        let mut stopped = false;
        while let Some(chunk) = stream.next().await {
            if stop.stopped() {
                stopped = true;
                break;
            }
            let chunk = chunk.map_err(|e| format!("流式读取失败：{e}"))?;
            line_buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = line_buf.find('\n') {
                let line: String = line_buf.drain(..=nl).collect();
                let line = line.trim();
                if let Some(parsed) = parse_sse_chunk(&line) {
                    if parsed.is_done {
                        continue;
                    }
                    if let Some(t) = parsed.content {
                        if !t.is_empty() {
                            let (normal, think) = feed_think(&mut think_mode, &mut think_buf, &t);
                            if !think.is_empty() {
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-think-delta",
                                    serde_json::json!({ "text": think }),
                                );
                            }
                            if !normal.is_empty() {
                                final_text.push_str(&normal);
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-chat-delta",
                                    serde_json::json!({ "text": normal }),
                                );
                            }
                        }
                    }
                    for tc_delta in parsed.tool_calls {
                        while tool_calls.len() <= tc_delta.index {
                            tool_calls.push((String::new(), String::new(), String::new()));
                        }
                        let t = &mut tool_calls[tc_delta.index];
                        if let Some(id) = tc_delta.id {
                            if t.0.is_empty() {
                                t.0 = id;
                                // 新工具调用开始：推折叠行给挂件
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-tool",
                                    serde_json::json!({ "id": t.0, "name": t.1 }),
                                );
                            }
                        }
                        if let Some(name) = tc_delta.name_chunk {
                            if !name.is_empty() {
                                t.1.push_str(&name);
                                if !t.0.is_empty() {
                                    let _ = app.emit_to(
                                        "widget",
                                        "bot-tool-name",
                                        serde_json::json!({ "id": t.0, "name": t.1 }),
                                    );
                                }
                            }
                        }
                        if let Some(args) = tc_delta.arguments_chunk {
                            t.2.push_str(&args);
                        }
                    }
                }
            }
        }

        // 回合结束：冲刷思考缓冲（丢弃未闭合标签碎片）
        let tail = std::mem::take(&mut think_buf)
            .replace("<think>", "")
            .replace("</think>", "");
        if !tail.is_empty() {
            if think_mode {
                let _ = app.emit_to(
                    "widget",
                    "bot-think-delta",
                    serde_json::json!({ "text": tail }),
                );
            } else {
                final_text.push_str(&tail);
                let _ = app.emit_to(
                    "widget",
                    "bot-chat-delta",
                    serde_json::json!({ "text": tail }),
                );
            }
        }

        if stopped {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }

        if tool_calls.is_empty() {
            let _ = crate::bot_skills::skill_finish(&app, true, "");
            collected_refs = merge_task_refs_dedup(collected_refs);
            return Ok((final_text.clone(), collected_refs));
        }

        // 模型请求工具：进程内执行，结果回填后继续下一轮
        msgs.push(serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })).collect::<Vec<_>>()
        }));
        if stop.stopped() {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }
        for (id, name, args) in &tool_calls {
            function_calls_total += 1;
            if function_calls_total > MAX_FUNCTION_CALLS_PER_TURN {
                let hint = crate::bot_skills::skill_finish(&app, false, "单轮 Function 调用超上限");
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "fuse | 单轮 Function 调用超过 {} 次，已熔断",
                        MAX_FUNCTION_CALLS_PER_TURN
                    ),
                );
                return Ok((
                    format!(
                        "{final_text}\n\n⏹ 已熔断：本轮 Function 调用超过 {} 次上限（安全保护），已停止后续执行{hint}",
                        MAX_FUNCTION_CALLS_PER_TURN
                    ),
                    collected_refs,
                ));
            }
            // 软警告（SOFT_WARN_AT）：置标志，推迟到本轮 tool 响应全部回填后再注入——
            // 若在此直接 push user 消息，会插进 assistant(tool_calls) 与 tool 响应之间，
            // 破坏「tool_calls 后必须紧跟 tool 消息」的协议，下一轮请求被 API 拒为
            // 400 invalid params（2026-08-18 两次 400 均紧跟 soft_warn 注入，已实锤）
            if !soft_warn_sent && function_calls_total >= SOFT_WARN_AT {
                soft_warn_sent = true;
                soft_warn_queued = true;
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "soft_warn | Function 调用达 {} 次（上限 {}），追加收尾提醒",
                        SOFT_WARN_AT, MAX_FUNCTION_CALLS_PER_TURN
                    ),
                );
            }
            // NEW-C-4：把 /stop 守卫透传给 execute_tool，run_python 在途可中断
            let (result, refs) =
                crate::bot::execute_tool_with_stop(&app, name, args, Some(stop)).await;
            let _ = app.emit_to(
                "widget",
                "bot-tool-done",
                serde_json::json!({ "id": id, "name": name, "args": args }),
            );
            crate::bot::audit_log(
                &app,
                &format!(
                    "tool: {name} | args: {} | result: {}",
                    crate::bot::truncate_for_log(args, 500),
                    crate::bot::truncate_for_log(&result, 300)
                ),
            );
            collected_refs.extend(refs);
            msgs.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }
        // 本轮 tool 响应已全部回填（tool_calls → tool×N 序列完整），此时注入软警告才合法
        if soft_warn_queued {
            soft_warn_queued = false;
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】你已连续调用 {SOFT_WARN_AT} 个工具，最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    MAX_FUNCTION_CALLS_PER_TURN - SOFT_WARN_AT
                ),
            }));
        }
        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
        last_streamed = final_text.clone();
    }
    let hint = crate::bot_skills::skill_finish(&app, false, "对话轮数超限");
    Err(CommandError::Internal(format!("对话轮数超限{hint}")))
}

// ────────────────────────────────────────────────────────────────────
// 测试：feed_think / parse_sse_chunk / TOOLS schema
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod think_tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String) {
        let mut mode = false;
        let mut buf = String::new();
        let mut normal = String::new();
        let mut think = String::new();
        for c in chunks {
            let (n, t) = feed_think(&mut mode, &mut buf, c);
            normal.push_str(&n);
            think.push_str(&t);
        }
        (normal, think)
    }

    #[test]
    fn think_split_across_chunks() {
        // 标签和内容都跨块
        let (n, t) = run(&["<thi", "nk>思考中…", "</th", "ink>答案是 42"]);
        assert_eq!(n, "答案是 42");
        assert_eq!(t, "思考中…");
    }

    #[test]
    fn think_whole_in_one_chunk() {
        let (n, t) = run(&["<think>先想一下</think>好的"]);
        assert_eq!(n, "好的");
        assert_eq!(t, "先想一下");
    }

    #[test]
    fn plain_text_no_tags() {
        let (n, t) = run(&["直接回答，没有思考"]);
        assert_eq!(n, "直接回答，没有思考");
        assert_eq!(t, "");
    }

    #[test]
    fn multiple_think_blocks() {
        let (n, t) = run(&["<think>A</think>正文1<think>B</think>正文2"]);
        assert_eq!(n, "正文1正文2");
        assert_eq!(t, "AB");
    }
}

#[cfg(test)]
mod parse_sse_chunk_tests {
    use super::*;

    #[test]
    fn parses_text_content_chunk() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(!parsed.is_done);
        assert_eq!(parsed.content.as_deref(), Some("hello"));
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn parses_done_marker() {
        let parsed = parse_sse_chunk("data: [DONE]").unwrap();
        assert!(parsed.is_done);
        assert!(parsed.content.is_none());
        assert!(parsed.tool_calls.is_empty());
        assert!(parsed.finish_reason.is_none());
    }

    #[test]
    fn parses_tool_call_delta_with_index() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"list_tasks","arguments":"{}"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        let tc = &parsed.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name_chunk.as_deref(), Some("list_tasks"));
        assert_eq!(tc.arguments_chunk.as_deref(), Some("{}"));
    }

    #[test]
    fn parses_multiple_tool_calls_with_distinct_indices() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"a"}},{"index":1,"function":{"name":"b"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].name_chunk.as_deref(), Some("a"));
        assert_eq!(parsed.tool_calls[1].name_chunk.as_deref(), Some("b"));
    }

    #[test]
    fn parses_finish_reason_stop() {
        let line = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_finish_reason_tool_calls() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!parsed.tool_calls.is_empty());
    }

    #[test]
    fn empty_content_string_treated_as_absent() {
        // SSE 中 content="" 时应等同 None（不触发内容推送）
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(
            parsed.content.is_none(),
            "空 content 应等同 None（不触发推送）"
        );
    }

    #[test]
    fn empty_name_treated_as_append_noop() {
        // name="" 时原代码 `!name.is_empty()` 跳过 push；这里 chunk 仍 Some("") 但 caller 决定是否 append
        let line =
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":""}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(
            parsed.tool_calls[0].name_chunk.as_deref(),
            Some(""),
            "name 字段存在但为空 — caller 决定是否跳过 append"
        );
    }

    #[test]
    fn returns_none_for_non_data_line() {
        assert!(parse_sse_chunk("event: ping").is_none());
        assert!(parse_sse_chunk("").is_none());
        assert!(parse_sse_chunk("data: not-json{").is_none());
        assert!(parse_sse_chunk("data:").is_none());
        assert!(parse_sse_chunk("data:    ").is_none());
    }

    #[test]
    fn returns_none_for_empty_choices() {
        // 网关异常：200 OK + choices:[] → 跳过（与原代码 else continue 语义一致）
        let line = r#"data: {"choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    #[test]
    fn returns_none_for_missing_choices() {
        let line = r#"data: {"id":"x","error":"auth_failed"}"#;
        assert!(
            parse_sse_chunk(line).is_none(),
            "异常响应（error 字段）应被忽略"
        );
    }

    #[test]
    fn parses_chinese_content_utf8() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好世界"}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("你好世界"));
    }

    #[test]
    fn parses_arguments_split_across_chunks() {
        // 模拟 arguments 跨多个 SSE chunk（流式追加）
        let chunk1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"ti"}}]}}]}"#;
        let chunk2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tle\":\"x\""}}]}}]}"#;
        let p1 = parse_sse_chunk(chunk1).unwrap();
        let p2 = parse_sse_chunk(chunk2).unwrap();
        assert_eq!(p1.tool_calls[0].arguments_chunk.as_deref(), Some(r#"{"ti"#));
        assert_eq!(
            p2.tool_calls[0].arguments_chunk.as_deref(),
            Some(r#"tle":"x""#)
        );
        // 生产代码会按顺序 push 拼成完整 JSON
    }

    #[test]
    fn done_marker_data_is_correctly_parsed() {
        // data:    [DONE]（中间多空格）也应正确识别
        let parsed = parse_sse_chunk("data:    [DONE]").unwrap();
        assert!(parsed.is_done);
    }

    #[test]
    fn defaults_index_to_zero_when_missing() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"a"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls[0].index, 0, "缺 index 时默认 0");
    }
}

#[cfg(test)]
mod tools_schema_tests {
    use super::*;

    /// TOOLS 是编译期字符串、运行期解析：语法坏会 panic 杀死聊天（历史 bug）。
    /// 此测试守住：加/改工具后必须合法且字段完整。
    #[test]
    fn tools_schema_parses() {
        let v: serde_json::Value = serde_json::from_str(TOOLS).expect("TOOLS 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        assert!(!arr.is_empty(), "TOOLS 不能为空");
        for t in arr {
            assert_eq!(
                t["type"].as_str(),
                Some("function"),
                "每项 type 必须是 function"
            );
            let name = t["function"]["name"]
                .as_str()
                .expect("每项必须有 function.name");
            assert!(!name.is_empty(), "工具名不能为空");
            assert!(
                t["function"]["description"].as_str().is_some(),
                "{name} 缺 description"
            );
        }
        // 关键工具必须存在（与 execute_tool match 对齐，改名会在此暴露）
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        for required in [
            "list_tasks",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "run_python",
            "web_search",
            "fetch_url",
        ] {
            assert!(names.contains(&required), "缺少工具 {required}");
        }
    }
}