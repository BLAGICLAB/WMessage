//! Tool registry（阶段 2，2026-09-13）。
//!
//! 单源真相 - 消除三源漂移（TOOLS / MUTATING_TOOLS / execute_tool_impl 大 match）。
//! - TOOLS JSON 由 `tools_json()` 从 TOOLS_TABLE 顺序拼装
//! - MUTATING_TOOLS 由 `mutating_tools()` 从 TOOLS_TABLE 过滤（mutating == true）
//! - execute_tool_impl 通过 TOOLS_TABLE lookup + fn 指针 call
//!
//! schema 常量是原 const TOOLS 的逐条字节拷贝（基线全文见 tests/fixtures/tools_baseline.json）；
//! tools_json() 按 TOOLS_TABLE 顺序以统一 `,\n  ` 缩进拼接，与原 const 只差
//! link_file_to_task 条目前的 2 空格（原 const 该条目顶格写），反序列化后语义一致。

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use tauri::AppHandle;

use crate::bot::tools::{
    tool_add_subtask, tool_complete_task, tool_create_excel, tool_create_pdf, tool_create_ppt,
    tool_create_task, tool_create_word, tool_create_word_revisions, tool_delete_task,
    tool_edit_task, tool_extract_document, tool_fetch_url, tool_get_current_time,
    tool_link_file_to_task, tool_list_tasks, tool_query_single_task, tool_remove_subtask,
    tool_run_python, tool_search_tasks, tool_toggle_subtask, tool_web_search,
};
use crate::bot_skills::tool_use_skill;

pub struct ToolCtx<'a> {
    pub app: &'a AppHandle,
    pub stop: Option<&'a crate::bot_slash::StopGuard>,
    pub interactive: bool,
    pub session_id: Option<&'a str>,
}

pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = (String, Vec<crate::bot_chat::TaskRef>)> + Send + 'a>>;

pub struct ToolDef {
    pub name: &'static str,
    pub schema: &'static str,
    pub mutating: bool,
    pub call: for<'a> fn(&'a ToolCtx<'a>, &'a str) -> ToolFuture<'a>,
}

// ─────────────────── 29 个 schema 常量（baseline 字节级一致）───────────────────
pub const SCHEMA_LIST_TASKS: &str = r##"{"type":"function","function":{"name":"list_tasks","description":"列出未完成任务（含状态列）","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_QUERY_SINGLE_TASK: &str = r##"{"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除状态指示；白名单单点，区别于 list_tasks 批量清单与 search_tasks 关键词检索）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}}"##;
pub const SCHEMA_CREATE_TASK: &str = r##"{"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DD HH:mm，可选"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"绑定文件列表（可选，最多 10 个；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件，其余会被丢弃；要绑其它文件请引导用户用 bind_file 手选）"}
  },"required":["title"]}}}"##;
pub const SCHEMA_COMPLETE_TASK: &str = r##"{"type":"function","function":{"name":"complete_task","description":"完成任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id（来自用户消息的 [已选任务] 引用块或 list_tasks 输出），可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}}"##;
pub const SCHEMA_DELETE_TASK: &str = r##"{"type":"function","function":{"name":"delete_task","description":"删除任务到回收站（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}}"##;
pub const SCHEMA_EDIT_TASK: &str = r##"{"type":"function","function":{"name":"edit_task","description":"编辑任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；改标题/备注/截止时间/标签/状态列，空串清字段）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时用于定位任务"},
    "newTitle":{"type":"string","description":"新标题，可选"},
    "note":{"type":"string","description":"新备注，可选；空串清除"},
    "due":{"type":"string","description":"新截止时间，可选；空串清除"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"新绑定文件列表（可选，整体替换，最多 10 个；空数组清除；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件）"}
  },"required":[]}}}"##;
pub const SCHEMA_ADD_SUBTASK: &str = r##"{"type":"function","function":{"name":"add_subtask","description":"给任务添加子任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容"}
  },"required":["text"]}}}"##;
pub const SCHEMA_TOGGLE_SUBTASK: &str = r##"{"type":"function","function":{"name":"toggle_subtask","description":"勾选/取消勾选子任务（任务用 taskId 优先；子任务按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容关键词"}
  },"required":["text"]}}}"##;
pub const SCHEMA_REMOVE_SUBTASK: &str = r##"{"type":"function","function":{"name":"remove_subtask","description":"删除单条子任务（彻底移除，区别于 toggle_subtask 的取消勾选；任务用 taskId 优先；子任务按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"要删除的子任务内容关键词"}
  },"required":["text"]}}}"##;
pub const SCHEMA_READ_TEXT_FILE: &str = r##"{"type":"function","function":{"name":"read_text_file","description":"读取本地文本文件内容（白名单目录内直接读，白名单外自动弹窗请用户授权；大文件用 offset/limit 分页读；Office/PDF 用 extract_document，图片用户会直接发图）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（支持 ~ 开头）"},
    "offset":{"type":"integer","description":"起始行号，从 1 开始，可选"},
    "limit":{"type":"integer","description":"读取行数，默认 500，最多 2000，可选"}
  },"required":["path"]}}}"##;
pub const SCHEMA_OCR_IMAGE: &str = r##"{"type":"function","function":{"name":"ocr_image","description":"本地 OCR 识别图片文字，逐行返回（白名单目录内直接识别，白名单外自动弹窗请用户授权；隐私红线：图片仅在内存处理、绝不上传外网——macOS 用系统 Vision，Windows 用本地 PP-OCRv6 模型）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"图片绝对路径（支持 ~ 开头；仅本地文件，不接受网络地址）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_GREP_FILES: &str = r##"{"type":"function","function":{"name":"grep_files","description":"按正则搜索本地文件内容，返回 path:行号:内容（最多 50 条；白名单目录内直接搜，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "pattern":{"type":"string","description":"正则表达式（非法正则自动按字面量搜）"},
    "dir":{"type":"string","description":"搜索目录，可选，缺省搜第一个白名单目录"},
    "glob":{"type":"string","description":"文件名过滤，如 *.rs，可选"},
    "max":{"type":"integer","description":"最多返回条数，默认 50，可选"}
  },"required":["pattern"]}}}"##;
pub const SCHEMA_LIST_FILES: &str = r##"{"type":"function","function":{"name":"list_files","description":"列出本地目录内的文件/子目录（递归 ≤5 层，最多 200 条；可用 pattern 按文件名过滤；白名单目录内直接列，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "dir":{"type":"string","description":"目录绝对路径（支持 ~ 开头）"},
    "pattern":{"type":"string","description":"文件名过滤，如 *.pdf 或 报告*，可选"}
  },"required":["dir"]}}}"##;
pub const SCHEMA_LINK_FILE_TO_TASK: &str = r##"{"type":"function","function":{"name":"link_file_to_task","description":"登记产物到本任务卡执行流程的产物清单。文件必须在 AI_Gen_Files 目录内。流程结束、任务完成、有产物时弹汇总窗口让你勾选绑定（默认全选，每个文件绑一次）；任务未完成、中断、只有中间产物都不弹。普通对话场景调用此工具不报错也不绑（不反复尝试）。仅任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内登记有效。","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "path":{"type":"string","description":"产物文件绝对路径，必须在 AI_Gen_Files 目录内且文件已存在"},
    "kind":{"type":"string","enum":["final","intermediate"],"default":"final","description":"final=最终产物，参与流程结束汇总弹窗；intermediate=中间产物，不参与弹窗。本会话在 AI_Gen_Files 目录没新建过的路径不参与绑定。"}
  },"required":["path"]}}}"##;
pub const SCHEMA_SEARCH_TASKS: &str = r##"{"type":"function","function":{"name":"search_tasks","description":"按关键词搜索所有任务卡（待办/进行中/已完成/已归档；匹配标题/备注/标签/子任务）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"}
  },"required":["query"]}}}"##;
pub const SCHEMA_EXTRACT_DOCUMENT: &str = r##"{"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件；长文档用 offset 参数续读后续部分）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；返回里带『已截断』提示时用提示的 offset 值续读）"},
    "limit":{"type":"integer","description":"本页字符数（可选，默认 30000，上限 60000）"}
  }}}}"##;
pub const SCHEMA_CREATE_WORD: &str = r##"{"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表，每段一个字符串"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_CREATE_WORD_REVISIONS: &str = r##"{"type":"function","function":{"name":"create_word_revisions","description":"生成带修订标记（修订模式）的 Word 到 AI_Gen_Files：在原文档副本上就地对比原文与润色后的段落打 Word 原生 track changes（保留原文格式/字体），可在 Word 审阅中逐条接受/拒绝（引擎：.NET OpenXML 优先，Python 兜底）","parameters":{"type":"object","properties":{
    "originalPath":{"type":"string","description":"原文 Word 路径（extract_document 返回的 [文档路径]）"},
    "original":{"type":"array","items":{"type":"string"},"description":"原文行列表（提取被截断时必须传，保证对比范围一致），可选"},
    "revised":{"type":"array","items":{"type":"string"},"description":"润色后的段落列表"},
    "title":{"type":"string","description":"文档标题，可选"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["revised"]}}}"##;
pub const SCHEMA_CREATE_EXCEL: &str = r##"{"type":"function","function":{"name":"create_excel","description":"生成 Excel 到 AI_Gen_Files（单元格以 = 开头会写入原生公式如 =SUM(A1:A10)）","parameters":{"type":"object","properties":{
    "sheets":{"type":"array","items":{"type":"object","properties":{
      "name":{"type":"string"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}}},
    "description":"工作表列表：name 表名、rows 二维数组"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["sheets"]}}}"##;
pub const SCHEMA_CREATE_PPT: &str = r##"{"type":"function","function":{"name":"create_ppt","description":"生成专业排版 PPT 到 AI_Gen_Files（多版式：封面/目录/章节页/内容页/表格页/结束页 + 10 套配色主题，可用 customColors 自定义覆盖）","parameters":{"type":"object","properties":{
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
  },"required":["slides"]}}}"##;
pub const SCHEMA_CREATE_PDF: &str = r##"{"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_RUN_PYTHON: &str = r##"{"type":"function","function":{"name":"run_python","description":"执行 Python 代码（本机沙箱：独立临时目录 + 默认超时 60s；默认需用户在设置页开启 Python 编程，授权模式为 yolo 时免开关）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"},
    "timeoutSecs":{"type":"integer","description":"超时秒数（可选，默认 60；大计算可调大，上限 300）"}
  },"required":["code"]}}}"##;
pub const SCHEMA_WEB_SEARCH: &str = r##"{"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（配置 Tavily 或 Brave key 时走对应 API、双开报错，否则 Bing+百度网页抓取；返回标题/链接/摘要）","parameters":{"type":"object","properties":{"query":{"type":"string","description":"搜索关键词"}},"required":["query"]}}}"##;
pub const SCHEMA_FETCH_URL: &str = r##"{"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址"}
  },"required":["url"]}}}"##;
pub const SCHEMA_GET_CURRENT_TIME: &str = r##"{"type":"function","function":{"name":"get_current_time","description":"获取当前日期时间和星期（涉及「今天/明天/昨天/周几/几点」类判断前必须先调，不要凭训练数据猜日期）","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_REMEMBER_FACT: &str = r##"{"type":"function","function":{"name":"remember_fact","description":"记住一条用户偏好/事实（跨会话长期记忆，重启不丢；key 简短规范名词 ≤50 字，value 内容 ≤500 字；同 key 覆盖更新；value 传空串删除该条；写入结果若提示相似已有记忆，优先用同 key 覆盖更新而非另开新 key 堆积）","parameters":{"type":"object","properties":{
    "key":{"type":"string","description":"简短规范名词，如「称呼」「偏好语言」「常用目录」"},
    "value":{"type":"string","description":"要记住的内容；空串 = 删除该条"},
    "category":{"type":"string","description":"分类（可选，默认 general）：profile 画像 / preference 偏好 / project 项目上下文 / general"},
    "importance":{"type":"integer","description":"重要度 1-5（可选，默认 3；用户明确要求长期遵守的给 4-5，琐碎信息 1-2）"},
    "source":{"type":"string","description":"来源（可选，默认 user_stated）：user_stated 用户明确说的 / model_inferred 模型推断的"}
  },"required":["key","value"]}}}"##;
pub const SCHEMA_RECALL_FACTS: &str = r##"{"type":"function","function":{"name":"recall_facts","description":"回忆长期记忆（相关记忆每轮已自动注入，一般无需调用；只在要浏览全部记忆或按关键词检索时才调）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"可选；给了按相关度检索返回 top-5，不给则全量读回"}
  }}}}"##;
pub const SCHEMA_RECORD_LESSON: &str = r##"{"type":"function","function":{"name":"record_lesson","description":"记录一条经验教训（跨会话长期记忆；被用户纠正、工具调用连续失败、发现更优做法时调用；同类场景的教训会自动合并，不会堆积）","parameters":{"type":"object","properties":{
    "lesson":{"type":"string","description":"教训内容（≤800 字）：什么场景下应该/不应该怎么做，以及原因"},
    "scenario":{"type":"string","description":"场景标签（可选 ≤50 字），如工具名或任务类型：create_ppt、批量执行、文档修订"}
  },"required":["lesson"]}}}"##;
pub const SCHEMA_USE_SKILL: &str = r##"{"type":"function","function":{"name":"use_skill","description":"读取已安装技能（skill）的完整文档并按文档步骤执行。任务涉及的每个相关技能都要读（可多次调用）：例如做 PPT 时，若清单里同时有编排、生成、配色、风格类技能，应逐个读取、取长补短综合运用，不要只读一个","parameters":{"type":"object","properties":{
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"}
  },"required":["name"]}}}"##;
// ─────────────────── 29 个适配器（统一签名，按需拆 ctx 字段）───────────────────
// list_tasks 在原 bot.rs:155 收 (app) 不收 args——阶段 1 拆出来后已修正。
// link_file_to_task 是 async fn，必须 .await——阶段 1 拆出来后已修正。
fn call_list_tasks<'a>(ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_list_tasks(ctx.app).await })
}

fn call_query_single_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_query_single_task(ctx.app, args).await })
}

fn call_create_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_task(ctx.app, args).await })
}

fn call_complete_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_complete_task(ctx.app, args).await })
}

fn call_delete_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move { tool_delete_task(ctx.app, args, interactive, session_id).await })
}

fn call_edit_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_edit_task(ctx.app, args).await })
}

fn call_add_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_add_subtask(ctx.app, args).await })
}

fn call_toggle_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_toggle_subtask(ctx.app, args).await })
}

fn call_remove_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_remove_subtask(ctx.app, args).await })
}

fn call_read_text_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_fs::tool_read_text_file(ctx.app, args, interactive, session_id).await
    })
}

fn call_ocr_image<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::ocr::tool_ocr_image(ctx.app, args, interactive, session_id).await },
    )
}

fn call_grep_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_grep_files(ctx.app, args, interactive, session_id).await },
    )
}

fn call_list_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_list_files(ctx.app, args, interactive, session_id).await },
    )
}

fn call_link_file_to_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { tool_link_file_to_task(ctx.app, args, session_id).await })
}

fn call_search_tasks<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_search_tasks(ctx.app, args).await })
}

fn call_extract_document<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move { tool_extract_document(ctx.app, args, interactive, session_id).await })
}

fn call_create_word<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_word(ctx.app, args).await })
}

fn call_create_word_revisions<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { tool_create_word_revisions(ctx.app, args, interactive, session_id).await },
    )
}

fn call_create_excel<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_excel(ctx.app, args).await })
}

fn call_create_ppt<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_ppt(ctx.app, args).await })
}

fn call_create_pdf<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_pdf(ctx.app, args).await })
}

fn call_run_python<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let stop = ctx.stop;
    Box::pin(async move { tool_run_python(ctx.app, args, stop).await })
}

fn call_web_search<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_web_search(ctx.app, args).await })
}

fn call_fetch_url<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_fetch_url(ctx.app, args).await })
}

fn call_get_current_time<'a>(_ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_get_current_time() })
}

fn call_remember_fact<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_remember_fact(ctx.app, args).await })
}

fn call_recall_facts<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_recall_facts(ctx.app, args).await })
}

fn call_record_lesson<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_record_lesson(ctx.app, args).await })
}

fn call_use_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { tool_use_skill(ctx.app, args, session_id) })
}

// ─────────────────── TOOLS_TABLE（29 工具单源真相）───────────────────
pub static TOOLS_TABLE: &[ToolDef] = &[
    ToolDef {
        name: "list_tasks",
        schema: SCHEMA_LIST_TASKS,
        mutating: false,
        call: call_list_tasks,
    },
    ToolDef {
        name: "query_single_task",
        schema: SCHEMA_QUERY_SINGLE_TASK,
        mutating: false,
        call: call_query_single_task,
    },
    ToolDef {
        name: "create_task",
        schema: SCHEMA_CREATE_TASK,
        mutating: true,
        call: call_create_task,
    },
    ToolDef {
        name: "complete_task",
        schema: SCHEMA_COMPLETE_TASK,
        mutating: true,
        call: call_complete_task,
    },
    ToolDef {
        name: "delete_task",
        schema: SCHEMA_DELETE_TASK,
        mutating: true,
        call: call_delete_task,
    },
    ToolDef {
        name: "edit_task",
        schema: SCHEMA_EDIT_TASK,
        mutating: true,
        call: call_edit_task,
    },
    ToolDef {
        name: "add_subtask",
        schema: SCHEMA_ADD_SUBTASK,
        mutating: true,
        call: call_add_subtask,
    },
    ToolDef {
        name: "toggle_subtask",
        schema: SCHEMA_TOGGLE_SUBTASK,
        mutating: true,
        call: call_toggle_subtask,
    },
    ToolDef {
        name: "remove_subtask",
        schema: SCHEMA_REMOVE_SUBTASK,
        mutating: true,
        call: call_remove_subtask,
    },
    ToolDef {
        name: "read_text_file",
        schema: SCHEMA_READ_TEXT_FILE,
        mutating: false,
        call: call_read_text_file,
    },
    ToolDef {
        name: "ocr_image",
        schema: SCHEMA_OCR_IMAGE,
        mutating: false,
        call: call_ocr_image,
    },
    ToolDef {
        name: "grep_files",
        schema: SCHEMA_GREP_FILES,
        mutating: false,
        call: call_grep_files,
    },
    ToolDef {
        name: "list_files",
        schema: SCHEMA_LIST_FILES,
        mutating: false,
        call: call_list_files,
    },
    ToolDef {
        name: "link_file_to_task",
        schema: SCHEMA_LINK_FILE_TO_TASK,
        mutating: true,
        call: call_link_file_to_task,
    },
    ToolDef {
        name: "search_tasks",
        schema: SCHEMA_SEARCH_TASKS,
        mutating: false,
        call: call_search_tasks,
    },
    ToolDef {
        name: "extract_document",
        schema: SCHEMA_EXTRACT_DOCUMENT,
        mutating: false,
        call: call_extract_document,
    },
    ToolDef {
        name: "create_word",
        schema: SCHEMA_CREATE_WORD,
        mutating: true,
        call: call_create_word,
    },
    ToolDef {
        name: "create_word_revisions",
        schema: SCHEMA_CREATE_WORD_REVISIONS,
        mutating: true,
        call: call_create_word_revisions,
    },
    ToolDef {
        name: "create_excel",
        schema: SCHEMA_CREATE_EXCEL,
        mutating: true,
        call: call_create_excel,
    },
    ToolDef {
        name: "create_ppt",
        schema: SCHEMA_CREATE_PPT,
        mutating: true,
        call: call_create_ppt,
    },
    ToolDef {
        name: "create_pdf",
        schema: SCHEMA_CREATE_PDF,
        mutating: true,
        call: call_create_pdf,
    },
    ToolDef {
        name: "run_python",
        schema: SCHEMA_RUN_PYTHON,
        mutating: false,
        call: call_run_python,
    },
    ToolDef {
        name: "web_search",
        schema: SCHEMA_WEB_SEARCH,
        mutating: false,
        call: call_web_search,
    },
    ToolDef {
        name: "fetch_url",
        schema: SCHEMA_FETCH_URL,
        mutating: false,
        call: call_fetch_url,
    },
    ToolDef {
        name: "get_current_time",
        schema: SCHEMA_GET_CURRENT_TIME,
        mutating: false,
        call: call_get_current_time,
    },
    ToolDef {
        name: "remember_fact",
        schema: SCHEMA_REMEMBER_FACT,
        mutating: true,
        call: call_remember_fact,
    },
    ToolDef {
        name: "recall_facts",
        schema: SCHEMA_RECALL_FACTS,
        mutating: false,
        call: call_recall_facts,
    },
    ToolDef {
        name: "record_lesson",
        schema: SCHEMA_RECORD_LESSON,
        mutating: true,
        call: call_record_lesson,
    },
    ToolDef {
        name: "use_skill",
        schema: SCHEMA_USE_SKILL,
        mutating: false,
        call: call_use_skill,
    },
];
/// TOOLS JSON 由 TOOLS_TABLE 顺序拼装（schema 常量原文直拼，不做 parse + re-serialize）。
/// 少一次运行期解析，也不给「schema 非法 → expect panic 杀聊天」留路径
/// （合法性 + 与 baseline 的一致性由 registry_tests 锁死）。
pub fn tools_json() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut s = String::with_capacity(16 * 1024);
        s.push('[');
        for (i, tool) in TOOLS_TABLE.iter().enumerate() {
            s.push_str(if i > 0 { ",\n  " } else { "\n  " });
            s.push_str(tool.schema);
        }
        s.push_str("\n]");
        s
    })
}

/// MUTATING_TOOLS 由 TOOLS_TABLE 过滤（mutating == true）。
pub fn mutating_tools() -> Vec<&'static str> {
    TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .map(|t| t.name)
        .collect()
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tools_table_contains_29_unique_tools() {
        let v: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        assert_eq!(arr.len(), 29, "TOOLS 必须含 29 个工具");

        let mut seen: HashSet<String> = HashSet::new();
        for t in arr.iter() {
            let func = t.get("function").expect("tool.function 必存");
            let name = func
                .get("name")
                .and_then(|n| n.as_str())
                .expect("name 必为 string");
            assert!(seen.insert(name.to_string()), "工具名重复: {name}");
            assert_eq!(t.get("type").and_then(|v| v.as_str()), Some("function"));
            assert!(func.get("description").is_some(), "{name} 缺 description");
            assert!(func.get("parameters").is_some(), "{name} 缺 parameters");
        }

        let names: HashSet<String> = arr
            .iter()
            .map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap()
                    .to_string()
            })
            .collect();
        let expected: HashSet<String> = [
            "list_tasks",
            "query_single_task",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "read_text_file",
            "ocr_image",
            "grep_files",
            "list_files",
            "link_file_to_task",
            "search_tasks",
            "extract_document",
            "create_word",
            "create_word_revisions",
            "create_excel",
            "create_ppt",
            "create_pdf",
            "run_python",
            "web_search",
            "fetch_url",
            "get_current_time",
            "remember_fact",
            "recall_facts",
            "record_lesson",
            "use_skill",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(names, expected, "工具名集合失配");

        let order: Vec<String> = arr
            .iter()
            .map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap()
                    .to_string()
            })
            .collect();
        let expected_order: Vec<String> = [
            "list_tasks",
            "query_single_task",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "read_text_file",
            "ocr_image",
            "grep_files",
            "list_files",
            "link_file_to_task",
            "search_tasks",
            "extract_document",
            "create_word",
            "create_word_revisions",
            "create_excel",
            "create_ppt",
            "create_pdf",
            "run_python",
            "web_search",
            "fetch_url",
            "get_current_time",
            "remember_fact",
            "recall_facts",
            "record_lesson",
            "use_skill",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(order, expected_order, "工具名顺序失配");
    }

    #[test]
    fn mutating_tools_matches_baseline() {
        let derived: HashSet<&str> = mutating_tools().into_iter().collect();
        let expected: HashSet<&str> = [
            "create_task",
            "edit_task",
            "complete_task",
            "delete_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "link_file_to_task",
            "create_word",
            "create_word_revisions",
            "create_excel",
            "create_ppt",
            "create_pdf",
            "remember_fact",
            "record_lesson",
        ]
        .into_iter()
        .collect();
        assert_eq!(derived, expected);
    }

    /// 阶段 2 spec 1: TOOLS JSON 与基线一致。
    /// 读 tests/fixtures/tools_baseline.json（原 const TOOLS 全文），提取其字符串正文
    /// 反序列化后与 tools_json() 做 Value 级比对：锁住工具顺序 + 每条 schema 的
    /// 字段名 / 描述 / 参数结构。
    ///
    /// 不比字节：tools_json() 用统一的 `,\n  ` 缩进拼装，而原 const 的
    /// link_file_to_task 条目顶格写（无 2 空格），故两者只差这一处空白。
    #[test]
    fn tools_json_matches_baseline() {
        let fixture = include_str!("../../tests/fixtures/tools_baseline.json");
        let r_str_marker = fixture.find("r#\"").expect("fixture 缺 r#\"") + 3;
        let end = fixture.rfind("]\"#;").expect("fixture 缺 ]\"#;") + 1;
        let baseline: serde_json::Value = serde_json::from_str(&fixture[r_str_marker..end])
            .expect("fixture 内 const TOOLS 正文必须是合法 JSON");
        let derived: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        assert_eq!(derived, baseline, "tools_json() 与 baseline 不一致");
    }

    /// 单源真相的核心不变式：ToolDef.name 必须等于它自己 schema 里的 function.name。
    /// 二者漂移 = 模型按 schema 调的工具名在 dispatch 查表时找不到（silent bug）。
    #[test]
    fn tool_def_name_matches_schema_name() {
        for t in TOOLS_TABLE {
            let v: serde_json::Value =
                serde_json::from_str(t.schema).expect("schema 常量必须是合法 JSON");
            assert_eq!(
                v.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str()),
                Some(t.name),
                "ToolDef.name 与 schema 内的 function.name 漂移: {}",
                t.name
            );
        }
    }
}
