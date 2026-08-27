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

// ───────────────────────── 防幻觉汇报守卫（2026-08-19） ─────────────────────────
// 实锤事故：MiniMax-M3 多次不调任何工具就回复「已添加子任务」「已移至回收站」，
// 数据实际没变，用户以为操作成功。提示词约束（SYSTEM_PROMPT 规则 8）不够，
// 这里在循环出口做确定性拦截：最终文本声称完成变更、但本轮 0 次变更类工具调用 →
// 注入系统提醒并补一轮（每次对话最多补一次），让模型实际调工具或如实说明。

/// 会改动任务卡/文件系统的工具（判定「本轮是否真的动手了」）
const MUTATING_TOOLS: [&str; 15] = [
    "create_task",
    "edit_task",
    "complete_task",
    "delete_task",
    "add_subtask",
    "toggle_subtask",
    "remove_subtask",
    "bind_file",
    // 2026-08-27 审计 P1-10：与 bind_file 同写路径（技能/任务卡流程内绑产物），漏了它
    // 会让「产物已绑定」的如实汇报被幻觉守卫误拦
    "link_file_to_task",
    "create_word",
    "create_word_revisions",
    "create_excel",
    "create_ppt",
    "create_pdf",
    "remember_fact",
];

/// 变更工具是否真的成功落库/落盘（幻觉守卫 mutation_done 的判定依据）。
/// 2026-08-27 审计 P0-4：原先在工具执行前按名字置位——被门禁拦截（⚠️）、
/// 用户拒绝、执行失败的调用都算「动过手」，之后的幻觉汇报就不再被拦，守卫被架空。
/// 改为按执行结果判定，失败口径走全链路统一的 `audit::tool_call_failed`（P1-6）。
fn mutation_succeeded(name: &str, result: &str) -> bool {
    MUTATING_TOOLS.contains(&name) && !crate::audit::tool_call_failed(name, result)
}

/// 最终文本是否含「变更已完成」表述（任务卡/文件类；纯查询汇报不命中）。
/// 枚举完整话术是打地鼠（实锤漏网：「已彻底删除」不含「已删除」字面），
/// 改成模式匹配：完成态标记「已」+ 其后 8 字窗口内含变更动词（覆盖 已彻底删除/已经把…移除 等变体），
/// 另加若干无「已」的高频话术兜底。
/// 2026-08-27 审计 P1-10：动词表去掉「完成」（「已完成搜索/分析」这类只读汇报误拦），
/// 补「保存/记住」（「已保存到 AI_Gen_Files」「已记住偏好」原先漏拦）；
/// 「已完成任务」走 PLAIN 整段匹配保住任务完成话术。
fn claims_mutation(text: &str) -> bool {
    const VERBS: [&str; 14] = [
        "添加", "删除", "移除", "修改", "更新", "绑定", "清空", "恢复", "勾选", "创建",
        "生成", "移至", "保存", "记住",
    ];
    const PLAIN: [&str; 4] = ["移至回收站", "标记为完成", "添加子任务", "已完成任务"];
    if PLAIN.iter().any(|p| text.contains(p)) {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '已' {
            let window: String = chars[i + 1..].iter().take(8).collect();
            if VERBS.iter().any(|v| window.contains(v)) {
                return true;
            }
        }
    }
    false
}

// ───────────────────────── TOOLS schema（编译期字符串，运行期 JSON 解析） ─────────────────────────

const TOOLS: &str = r#"[
  {"type":"function","function":{"name":"list_tasks","description":"列出未完成任务（含状态列）","parameters":{"type":"object","properties":{}}}},
  {"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除状态指示；白名单单点，区别于 list_tasks 批量清单与 search_tasks 关键词检索）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}},
  {"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DD HH:mm，可选"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"绑定文件列表（可选，最多 10 个；isDir=true 为文件夹）"}
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
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"新绑定文件列表（可选，整体替换，最多 10 个；空数组清除）"}
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
  {"type":"function","function":{"name":"remove_subtask","description":"删除单条子任务（彻底移除，区别于 toggle_subtask 的取消勾选；任务用 taskId 优先；子任务按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"要删除的子任务内容关键词"}
  },"required":["text"]}}},
  {"type":"function","function":{"name":"read_text_file","description":"读取本地文本文件内容（白名单目录内直接读，白名单外自动弹窗请用户授权；大文件用 offset/limit 分页读；Office/PDF 用 extract_document，图片用户会直接发图）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（支持 ~ 开头）"},
    "offset":{"type":"integer","description":"起始行号，从 1 开始，可选"},
    "limit":{"type":"integer","description":"读取行数，默认 500，最多 2000，可选"}
  },"required":["path"]}}},
  {"type":"function","function":{"name":"grep_files","description":"按正则搜索本地文件内容，返回 path:行号:内容（最多 50 条；白名单目录内直接搜，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "pattern":{"type":"string","description":"正则表达式（非法正则自动按字面量搜）"},
    "dir":{"type":"string","description":"搜索目录，可选，缺省搜第一个白名单目录"},
    "glob":{"type":"string","description":"文件名过滤，如 *.rs，可选"},
    "max":{"type":"integer","description":"最多返回条数，默认 50，可选"}
  },"required":["pattern"]}}},
  {"type":"function","function":{"name":"list_files","description":"列出本地目录内的文件/子目录（递归 ≤5 层，最多 200 条；可用 pattern 按文件名过滤；白名单目录内直接列，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "dir":{"type":"string","description":"目录绝对路径（支持 ~ 开头）"},
    "pattern":{"type":"string","description":"文件名过滤，如 *.pdf 或 报告*，可选"}
  },"required":["dir"]}}},
  {"type":"function","function":{"name":"bind_file","description":"给任务绑定文件或文件夹（弹系统选择框由用户挑选；任务用 taskId 优先定位）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "isDir":{"type":"boolean","description":"true=选文件夹，false 选文件"}
  },"required":[]}}},
  {"type":"function","function":{"name":"link_file_to_task","description":"把 AI_Gen_Files 目录内的生成文件绑定到任务卡（不弹选择框；只允许该目录内的文件，其他文件请在任务卡上手动绑定）。内部原子：仅技能运行中或任务卡执行流程里可调用，聊天里裸调会被拦截","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "path":{"type":"string","description":"要绑定的文件绝对路径（必须位于 AI_Gen_Files 目录内）"}
  },"required":["path"]}}},
  {"type":"function","function":{"name":"search_tasks","description":"按关键词搜索所有任务卡（待办/进行中/已完成/已归档；匹配标题/备注/标签/子任务）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"}
  },"required":["query"]}}},
  {"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件；长文档用 offset 参数续读后续部分）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；返回里带『已截断』提示时用提示的 offset 值续读）"},
    "limit":{"type":"integer","description":"本页字符数（可选，默认 30000，上限 60000）"}
  }}}},
  {"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表，每段一个字符串"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}},
  {"type":"function","function":{"name":"create_word_revisions","description":"生成带修订标记（修订模式）的 Word 到 AI_Gen_Files：自动对比原文与润色后的段落，删除内容标删除线、新增内容标红色下划线，可在 Word 审阅中逐条接受/拒绝。内部原子：仅技能运行中或任务卡执行流程里可调用，聊天里裸调会被拦截（被拦时改用 create_word 生成润色版）","parameters":{"type":"object","properties":{
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
  {"type":"function","function":{"name":"create_ppt","description":"生成专业排版 PPT 到 AI_Gen_Files（多版式：封面/目录/章节页/内容页/表格页/结束页 + 10 套配色主题，可用 customColors 自定义覆盖）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"演示文稿主标题"},
    "theme":{"type":"string","enum":["blue","navy","teal","forest","wine","sky","plum","coral","dark","green"],"description":"配色主题（按场合选）：blue 商务与权威（默认，汇报/金融）/ navy 科技与夜景（深色发布会）/ teal 现代与健康（医疗/护肤）/ forest 自然与户外（环保/农业）/ wine 复古与学院（学术/历史）/ sky 纯净科技蓝（AI/云计算）/ plum 轻奢与神秘（珠宝/高端咨询）/ coral 海岸珊瑚（旅游/夏日）/ dark 深色通用 / green 清新绿"},
    "customColors":{"type":"object","description":"可选：自定义配色覆盖主题（6 位 hex 如 1E40AF，可带 #）。键：bg 背景 / accent 强调色 / text 正文 / sub 次要文字 / band 大面积色块（必深色）/ bandtext 色块上文字 / alt 表格斑马纹。用户给了 VI 色/品牌色时用","properties":{
      "bg":{"type":"string"},"accent":{"type":"string"},"text":{"type":"string"},"sub":{"type":"string"},"band":{"type":"string"},"bandtext":{"type":"string"},"alt":{"type":"string"}
    }},
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
  {"type":"function","function":{"name":"run_python","description":"执行 Python 代码（本机沙箱：独立临时目录 + 默认超时 60s；默认需用户在设置页开启 Python 编程，授权模式为 yolo 时免开关）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"},
    "timeoutSecs":{"type":"integer","description":"超时秒数（可选，默认 60；大计算可调大，上限 300）"}
  },"required":["code"]}}},
  {"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（配置 Tavily key 时走 Tavily，否则 Bing+百度网页抓取；返回标题/链接/摘要）","parameters":{"type":"object","properties":{"query":{"type":"string","description":"搜索关键词"}},"required":["query"]}}},
  {"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址"}
  },"required":["url"]}}},
  {"type":"function","function":{"name":"get_current_time","description":"获取当前日期时间和星期（涉及「今天/明天/昨天/周几/几点」类判断前必须先调，不要凭训练数据猜日期）","parameters":{"type":"object","properties":{}}}},
  {"type":"function","function":{"name":"remember_fact","description":"记住一条用户偏好/事实（跨会话长期记忆，重启不丢；key 简短描述 ≤50 字，value 内容 ≤500 字；同 key 覆盖更新；value 传空串删除该条）","parameters":{"type":"object","properties":{
    "key":{"type":"string","description":"简短描述，如「称呼」「偏好语言」「常用目录」"},
    "value":{"type":"string","description":"要记住的内容；空串 = 删除该条"}
  },"required":["key","value"]}}},
  {"type":"function","function":{"name":"recall_facts","description":"回忆所有已记住的用户偏好/事实（用户问「你记得我吗/我的偏好」或回答可能依赖用户偏好时先调）","parameters":{"type":"object","properties":{}}}},
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

/// 默认对话轮数（聊天 / 任务执行 / 逐步执行统一；2026-08-26 老板拍板 20 → 50）；
/// 多步 Skill 可在 SKILL.md frontmatter 自报 max_rounds 覆盖（见 resolve_max_rounds）。
pub(crate) const DEFAULT_MAX_ROUNDS: usize = 50;

/// 本轮工具循环的轮数上限：Skill 自报 max_rounds 优先，未声明 → DEFAULT_MAX_ROUNDS。
pub(crate) fn resolve_max_rounds(skill_max_rounds: Option<usize>) -> usize {
    skill_max_rounds.unwrap_or(DEFAULT_MAX_ROUNDS)
}

// Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
// max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
//
// 阈值演变：
// - 2026-08-18 老板拍板 5 → 10：5 太激进（PPT 编排 + 配色 + 归档就要 8-10）；
//   10 覆盖 90% 真实复合任务；15+ 掩护 LLM 死循环 / 幻觉调工具
// - 2026-08-19 老板拍板 10 → 30（全局：聊天/任务卡执行/Skill 统一）：
//   实锤 10 不够用——「列计划 + 按计划新增子任务」复合任务在 22:56 真触发熔断
//   （bot.log `fuse | 单轮 Function 调用超过 10 次`）。失控防护改靠：
//   幻觉守卫（claims_mutation）+ 软警告 + /stop，不再靠压低上限
// - 2026-08-20 老板拍板 30 → 10：30 太宽松，会掩护 LLM 幻觉/死循环；
//   软警告 20 → 7（按 ~30% buffer：10-3=7，与原 20/30 的 ~33% 保持比例）
// - 2026-08-26 老板拍板 10 → 50（与对话轮数上限拉齐）：复杂多步任务 10 次不够用；
//   软警告 7 → 35（保持 ~30% buffer：50-15=35）
const MAX_FUNCTION_CALLS_PER_TURN: usize = 50;
const SOFT_WARN_AT: usize = 35;

/// 熔断判定：第 n 次（1-based 累计）Function 调用是否超上限
fn should_fuse(calls_so_far: usize) -> bool {
    calls_so_far > MAX_FUNCTION_CALLS_PER_TURN
}

/// 熔断返回消息（与主循环文案同源，单测直接断言）
fn fuse_message(final_text: &str, hint: &str) -> String {
    format!(
        "{final_text}\n\n⏹ 已熔断：本轮 Function 调用超过 {} 次上限（安全保护），已停止后续执行{hint}",
        MAX_FUNCTION_CALLS_PER_TURN
    )
}

/// 模型工具循环核心：配置/Key 检查、流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。
/// 轮数上限由调用方传入：默认 DEFAULT_MAX_ROUNDS（50），多步 Skill 可自报 max_rounds 覆盖。
/// plan_state（2026-08-26 PREVR 第 2 层）：复杂任务的动态计划；工具连续失败时触发
/// Replan（重规划剩余步骤，≤MAX_REPLANS 次）。None = 无计划自由循环。
pub async fn run_model_loop(
    app: AppHandle,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
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
    // 2026-08-26 会话隔离：流式事件（bot-chat-delta 等）只由交互实例广播；
    // 后台定时任务（interactive=false）不向挂件推流——否则后台执行的输出会
    // 串进用户当前会话的 streaming 气泡（审计 P0）。Skill 归属同理按 session 过滤。
    let stream_to_widget = stop.is_interactive();
    let session_id: Option<&str> = stop.session_id();
    // 流式事件出口统一收口（2026-08-26 会话隔离）：非交互实例（后台定时任务）
    // 不向挂件发任何流式增量，防后台执行输出串进用户当前会话的 streaming 气泡
    let emit_stream = |event: &str, payload: serde_json::Value| {
        if stream_to_widget {
            let _ = app.emit_to("widget", event, payload);
        }
    };
    // 僵尸终态清理：上轮 Skill 失败/完成的遗留 run 会在第 0 轮短路主循环（agent 假死根因）
    crate::bot_skills::clear_terminal_skill_runs();
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // 防幻觉汇报守卫：本轮是否实际执行过变更类工具；补一轮机会每次对话只用一次
    let mut mutation_done: bool = false;
    let mut claim_retry_used: bool = false;
    // soft_warn 待注入标志：本轮 tool 响应全部回填后才真正 push（见循环内注释）
    let mut soft_warn_queued: bool = false;
    // PREVR 第 1 层（2026-08-26）：工具失败检测——同工具连续失败计数，
    // 第 1 次失败注入「换策略」提示；连续 2 次失败：有计划则 Replan，无计划则要求如实告知
    let mut last_failed_tool: Option<String> = None;
    let mut last_fail_reason: Option<String> = None;
    let mut consec_failures: usize = 0;
    // Replan 预算耗尽审计只记一次（2026-08-27 P2：耗尽后每轮连续失败仍会发生，不刷屏）
    let mut replan_exhausted_logged: bool = false;
    let mut fail_hint_queued: Option<String> = None;
    let mut plan_state = plan_state;
    // 上轮 streamed 文本快照（Block 2 接入，2026-08-17 22:26）：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    for _round in 0..max_rounds {
        if stop.stopped() {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止", session_id);
            return Ok((format!("⏹ 已停止{hint}"), collected_refs));
        }
        // 状态机推进决策（Block 2 2026-08-17 22:26）：集中 Skill 推进逻辑
        // 未来横切关注点（审批/沙箱/上下文压缩）只动 advance_skill，主循环不重构
        if let Some(run) = crate::bot_skills::active_skill_run_for(session_id) {
            use crate::bot_skills::{advance_skill, AdvanceAction};
            match advance_skill(&run, chrono::Utc::now().timestamp_millis()) {
                AdvanceAction::NoActive | AdvanceAction::Continue => {} // 继续本轮
                AdvanceAction::AwaitConfirm => {
                    // Skill 暂停等用户确认，跳出主循环等待 bot_confirm_response 唤起。
                    // P2（2026-08-27 审计）：并发新消息在第 0 轮命中此分支时 last_streamed
                    // 是空串，用户得到空白回复——空串时给一句可读提示
                    if last_streamed.is_empty() {
                        return Ok((
                            "⏸ 上一个操作正在等待你的确认——请先处理确认弹窗，再继续对话。".into(),
                            collected_refs,
                        ));
                    }
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Finish => {
                    let _hint = crate::bot_skills::skill_finish(&app, true, "", session_id);
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Fail(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason, session_id);
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Terminate(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason, session_id);
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

        let resp = match client
            .post(&url)
            .bearer_auth(api_key.trim())
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // 2026-08-27 审计 P2：LLM 网络失败原先零审计，与 API 错误分支不对称
                crate::audit_event!(
                    &app,
                    crate::audit::AuditLevel::Error,
                    "llm.request_failed",
                    "err" => e.to_string(),
                );
                let hint = crate::bot_skills::skill_finish(&app, false, "大模型请求失败", session_id);
                return Err(format!("请求大模型失败：{e}{hint}").into());
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Warn,
                "llm.response",
                "status" => status.as_u16(),
            );
            let hint = crate::bot_skills::skill_finish(&app, false, "大模型 API 错误", session_id);
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
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    // 2026-08-27 审计 P2：流式中断原先静默 Err，无审计留痕
                    crate::audit_event!(
                        &app,
                        crate::audit::AuditLevel::Error,
                        "llm.stream_failed",
                        "err" => e.to_string(),
                    );
                    return Err(format!("流式读取失败：{e}").into());
                }
            };
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
                                emit_stream("bot-think-delta", serde_json::json!({ "text": think }));
                            }
                            if !normal.is_empty() {
                                final_text.push_str(&normal);
                                emit_stream("bot-chat-delta", serde_json::json!({ "text": normal }));
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
                                emit_stream("bot-tool", serde_json::json!({ "id": t.0, "name": t.1 }));
                            }
                        }
                        if let Some(name) = tc_delta.name_chunk {
                            if !name.is_empty() {
                                t.1.push_str(&name);
                                if !t.0.is_empty() {
                                    emit_stream("bot-tool-name", serde_json::json!({ "id": t.0, "name": t.1 }));
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
                emit_stream("bot-think-delta", serde_json::json!({ "text": tail }));
            } else {
                final_text.push_str(&tail);
                emit_stream("bot-chat-delta", serde_json::json!({ "text": tail }));
            }
        }

        if stopped {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止", session_id);
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }

        if tool_calls.is_empty() {
            // 防幻觉汇报守卫（2026-08-19）：声称完成变更但本轮没动过手 →
            // 注入系统提醒补一轮，逼模型实际调工具或如实说明（最多补一次）
            if !mutation_done && !claim_retry_used && claims_mutation(&final_text) {
                claim_retry_used = true;
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "hallucination_guard | 声称变更但未调工具，补一轮: {}",
                        crate::bot::truncate_for_log(&final_text, 100)
                    ),
                );
                msgs.push(serde_json::json!({"role": "assistant", "content": final_text}));
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "【系统提示】你刚才声称完成了变更，但本轮没有任何变更类工具调用成功，数据实际没有变化。请立即调用对应工具实际执行（删除用 delete_task、完成用 complete_task、编辑用 edit_task、子任务用 add_subtask/remove_subtask、绑定文件用 bind_file、生成文档用 create_word/create_excel/create_ppt/create_pdf；逐步执行模式下子任务勾选由系统完成，不要代调 toggle_subtask）；若确实无法执行（任务不存在/被安全闸门拦截/无权限等），如实向用户说明原因，禁止再次声称已完成。"
                }));
                continue;
            }
            let _ = crate::bot_skills::skill_finish(&app, true, "", session_id);
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
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止", session_id);
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }
        for (id, name, args) in &tool_calls {
            // P1-8（2026-08-27 审计）：工具批中途可停——/stop 后剩余调用不执行，
            // 但必须回填占位 tool 响应（tool_calls → tool 消息协议完整性，
            // 缺响应会让下一轮请求被 API 拒为 400 invalid params）
            if stop.stopped() {
                msgs.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": "⏹ 已停止，该工具未执行"
                }));
                continue;
            }
            function_calls_total += 1;
            if should_fuse(function_calls_total) {
                let hint = crate::bot_skills::skill_finish(&app, false, "单轮 Function 调用超上限", session_id);
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "fuse | 单轮 Function 调用超过 {} 次，已熔断",
                        MAX_FUNCTION_CALLS_PER_TURN
                    ),
                );
                return Ok((fuse_message(&final_text, &hint), collected_refs));
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
            // P0-4（2026-08-27 审计）：按执行结果置位——被门禁拦截/用户拒绝/执行失败的
            // 变更工具不算「动过手」，幻觉守卫对后续虚假汇报保持拦截能力
            if mutation_succeeded(name, &result) {
                mutation_done = true;
            }
            emit_stream("bot-tool-done", serde_json::json!({ "id": id, "name": name, "args": args }));
            crate::bot::audit_log(
                &app,
                &format!(
                    "tool: {name} | args: {} | result: {}",
                    crate::bot::truncate_for_log(args, 500),
                    crate::bot::truncate_for_log(&result, 300)
                ),
            );
            collected_refs.extend(refs);
            // PREVR 第 1 层（2026-08-26）：工具失败检测。判定走全链路统一口径
            // （P1-6：audit::tool_call_failed）——门禁拦截/熔断/暂停/拒绝都能识别；
            // 同工具连续失败才升级——单次失败先提示换策略。
            // 与 soft_warn 同理：提示推迟到本轮 tool 响应全部回填后注入（协议安全）
            let failed = crate::audit::tool_call_failed(name, &result);
            if failed {
                if last_failed_tool.as_deref() == Some(name.as_str()) {
                    consec_failures += 1;
                } else {
                    consec_failures = 1;
                    last_failed_tool = Some(name.clone());
                }
                let reason = crate::bot::truncate_for_log(&result, 200);
                last_fail_reason = Some(reason.clone());
                fail_hint_queued = Some(if consec_failures >= 2 {
                    format!(
                        "【系统提示】工具 {name} 已连续失败 {consec_failures} 次（最近原因：{reason}）。禁止再次以相同方式调用该工具；如果换参数/换路径仍无法完成，如实向用户说明失败原因与当前进度，由用户决定下一步。"
                    )
                } else {
                    format!(
                        "【系统提示】上一步调用的工具 {name} 失败了（原因：{reason}）。不要重复同样的调用；分析原因后换策略：换参数、换工具、或把任务拆成更小的步骤再试一次。"
                    )
                });
            } else if !result.is_empty() {
                // 成功调用重置连续失败链（空结果不算成功也不算失败，不重置）
                last_failed_tool = None;
                last_fail_reason = None;
                consec_failures = 0;
            }
            msgs.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }
        // PREVR 第 2 层（2026-08-26）：同工具连续失败 ≥2 且有计划 → Replan 一次
        // （重规划剩余步骤，替换计划文本；≤MAX_REPLANS 次硬上限，防重规划死循环）。
        // P1-7（2026-08-27 审计）：预算不管成败都消耗——原先失败 replan 不计数，
        // Planner 持续故障时每轮白烧一次调用，硬上限名不副实；fail_reason 补真实错误
        // 文本（原先只传工具名，Planner 拿不到任何失败细节）。
        // Replan 是同步阻塞本轮的 LLM 调用：放在 tool 响应全部回填后、注入提示前，
        // 这样提示里带的就是新计划。
        if consec_failures >= 2 {
            if let Some(plan) = plan_state.as_deref_mut() {
                if plan.replans_used < crate::bot_plan::MAX_REPLANS {
                    plan.replans_used += 1;
                    let reason = format!(
                        "工具 {} 连续失败 {} 次，最近错误：{}",
                        last_failed_tool.as_deref().unwrap_or("?"),
                        consec_failures,
                        last_fail_reason.as_deref().unwrap_or("（无错误详情）")
                    );
                    if let Some(new_steps) =
                        crate::bot_plan::replan(&app, plan, &reason).await
                    {
                        plan.steps = new_steps;
                        fail_hint_queued = Some(format!(
                            "【系统提示】原计划执行受阻，已重新规划剩余步骤：\n{}\n请按新计划继续；若仍无法推进，如实向用户说明。",
                            plan.steps.join("\n")
                        ));
                    }
                } else if !replan_exhausted_logged {
                    // P2（2026-08-27 审计）：预算耗尽留痕（只记一次）——
                    // 「连续失败持续发生但不再重规划」这件事原先零痕迹
                    replan_exhausted_logged = true;
                    crate::audit_event!(
                        &app,
                        crate::audit::AuditLevel::Warn,
                        "plan.replan_budget_exhausted",
                        "max" => crate::bot_plan::MAX_REPLANS,
                    );
                }
            }
        }
        // 本轮 tool 响应已全部回填（tool_calls → tool×N 序列完整），此时注入提示才合法
        if let Some(hint) = fail_hint_queued.take() {
            msgs.push(serde_json::json!({
                "role": "user",
                "content": hint,
            }));
        }
        if soft_warn_queued {
            soft_warn_queued = false;
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】你已累计调用 {SOFT_WARN_AT} 个工具（全程累计），最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    MAX_FUNCTION_CALLS_PER_TURN - SOFT_WARN_AT
                ),
            }));
        }
        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
        last_streamed = final_text.clone();
    }
    // 2026-08-27 审计 P2：轮数熔断补审计（原先只有单轮工具熔断有 fuse 日志，不对称）
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Warn,
        "fuse_rounds",
        "max_rounds" => max_rounds,
    );
    let hint = crate::bot_skills::skill_finish(&app, false, "对话轮数超限", session_id);
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
mod hallucination_guard_tests {
    use super::*;

    #[test]
    fn claims_mutation_hits_common_claims() {
        // 实锤事故话术（2026-08-19 bot.log）：声称删除/添加子任务
        assert!(claims_mutation("已将「你们好」移至回收站 🗑️"));
        assert!(claims_mutation("已给「你们好」任务添加子任务「买菜」✅"));
        assert!(claims_mutation("已将任务标记为完成"));
        assert!(claims_mutation("已清空绑定文件"));
        // 变体话术（第二轮实锤漏网）：副词插在「已」和动词之间
        assert!(claims_mutation("「你们好」下的子任务「买菜」已彻底删除。"));
        assert!(claims_mutation("已经把附件全部移除"));
        assert!(claims_mutation("「买菜」之前已经彻底删除了，这次没有可删除的内容。"));
    }

    #[test]
    fn claims_mutation_passes_pure_query_answers() {
        assert!(!claims_mutation("你有 3 个待办任务：A、B、C"));
        assert!(!claims_mutation("「你们好」当前没有绑定任何附件，无需删除。"));
        assert!(!claims_mutation("未找到匹配的任务，请确认标题"));
        assert!(!claims_mutation(""));
        // P1-10（2026-08-27 审计）：只读任务的收尾话术不再误拦（动词表去掉「完成」）
        assert!(!claims_mutation("已完成搜索，找到 3 条结果"));
        assert!(!claims_mutation("分析已完成，结论如下"));
    }

    #[test]
    fn claims_mutation_p1_10_wording_adjustments() {
        // 「已完成任务」走 PLAIN 整段匹配保住任务完成话术
        assert!(claims_mutation("已完成任务「买菜」"));
        // 补「保存/记住」动词（原先漏拦）
        assert!(claims_mutation("已保存到 AI_Gen_Files"));
        assert!(claims_mutation("已记住你的偏好"));
    }

    #[test]
    fn mutating_tools_cover_task_and_file_writes() {
        // 守卫白名单与工具分发保持一致的关键几个
        for t in ["delete_task", "add_subtask", "toggle_subtask", "complete_task", "edit_task", "create_task", "bind_file", "link_file_to_task"] {
            assert!(MUTATING_TOOLS.contains(&t), "{t} 应算变更类工具");
        }
        // 纯查询工具不算变更
        for t in ["list_tasks", "search_tasks", "query_single_task", "web_search", "fetch_url"] {
            assert!(!MUTATING_TOOLS.contains(&t), "{t} 不应算变更类工具");
        }
    }

    // ── P0-4（2026-08-27 审计）：mutation_done 按执行结果置位 ──

    #[test]
    fn mutation_succeeded_true_on_real_success() {
        assert!(mutation_succeeded("complete_task", "已完成任务「买菜」"));
        assert!(mutation_succeeded("link_file_to_task", "已绑定文件到任务卡"));
        assert!(mutation_succeeded("create_word", "已生成 Word 文档：/tmp/x.docx"));
    }

    #[test]
    fn mutation_succeeded_false_on_gate_block() {
        // 原子工具被 AtomicGuard 拦截：⚠️ 开头 → 不算动过手，幻觉守卫保持拦截能力
        assert!(!mutation_succeeded(
            "create_word_revisions",
            "⚠️ create_word_revisions 是 Word 修订 Skill 的内部原子，不允许裸调。"
        ));
    }

    #[test]
    fn mutation_succeeded_false_on_user_reject_and_failure() {
        assert!(!mutation_succeeded("delete_task", "用户拒绝了删除，任务未删除"));
        assert!(!mutation_succeeded("create_task", "新建任务失败：磁盘只读"));
        assert!(!mutation_succeeded("complete_task", "未知工具：complete_task"));
    }

    #[test]
    fn mutation_succeeded_false_for_readonly_tools() {
        // 只读工具即使返回成功文本也不算变更
        assert!(!mutation_succeeded("list_tasks", "已完成任务「买菜」"));
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
// ────────────────────────────────────────────────────────────────────
// 测试：max_rounds 解析 / fallback + 单轮 Function 调用熔断（2026-08-20）
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod rounds_fuse_tests {
    use super::*;

    #[test]
    fn skill_frontmatter_max_rounds_overrides_default() {
        // Skill frontmatter 声明 max_rounds: 25 → 生效，run_model_loop 收到 25
        let meta = crate::bot_skills::parse_meta(
            "---\nname: minimax-docx\nmax_rounds: 25\n---\n# body\n",
            "minimax-docx",
        );
        assert_eq!(meta.max_rounds, Some(25));
        assert_eq!(resolve_max_rounds(meta.max_rounds), 25);
    }

    #[test]
    fn skill_without_max_rounds_falls_back_to_default_50() {
        // 现有 Skill 未声明 max_rounds → None → fallback 默认 50（兼容不崩）
        let meta =
            crate::bot_skills::parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert_eq!(meta.max_rounds, None);
        assert_eq!(resolve_max_rounds(meta.max_rounds), DEFAULT_MAX_ROUNDS);
        assert_eq!(DEFAULT_MAX_ROUNDS, 50);
    }

    #[test]
    fn fuse_trips_on_51st_function_call() {
        // MAX_FUNCTION_CALLS_PER_TURN = 50 实际生效：模拟主循环计数，
        // 构造 51 个 tool_calls → 第 51 次触发熔断并返回「⏹ 已熔断」消息
        assert_eq!(MAX_FUNCTION_CALLS_PER_TURN, 50);
        assert_eq!(SOFT_WARN_AT, 35);
        let mut calls = 0usize;
        let mut fused_msg: Option<String> = None;
        for _ in 0..51 {
            calls += 1;
            if should_fuse(calls) {
                fused_msg = Some(fuse_message("前文", ""));
                break;
            }
        }
        let msg = fused_msg.expect("51 次 Function 调用内必须触发熔断");
        assert!(msg.contains("⏹ 已熔断"), "熔断消息应含「⏹ 已熔断」：{msg}");
        assert!(msg.contains("50 次上限"), "熔断消息应带上限值：{msg}");
        // 边界：第 50 次放行，第 51 次熔断
        assert!(!should_fuse(50));
        assert!(should_fuse(51));
    }
}
