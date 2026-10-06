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
    tool_link_file_to_task, tool_query_single_task, tool_query_tasks, tool_remove_subtask,
    tool_run_python, tool_toggle_subtask, tool_web_search,
};
use crate::bot_desktop::{tool_clipboard_write, tool_open_url, tool_reveal_path, tool_screenshot};
use crate::bot_skills::tool_use_skill;

pub struct ToolCtx<'a> {
    pub app: &'a AppHandle,
    pub stop: Option<&'a crate::bot_slash::StopGuard>,
    pub interactive: bool,
    pub session_id: Option<&'a str>,
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

/// T3 B1：工具返回的审计级别。代替 audit::classify_text 的字符串匹配。
/// 工具在返回 ToolResult 时显式声明 status，dispatcher 据此写 audit 事件。
///
/// B1 阶段：所有工具仍返 (String, Vec<TaskRef>)，From 过渡层默认 Ok。
/// B2 阶段：工具逐个改为显式 ok/warn/error；B1 的 From 过渡层在 cleanup 删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    /// 工具成功完成主职责
    Ok,
    /// 工具成功但有部分告警
    Warn,
    /// 工具明确返回错误
    Error,
}

/// T3 B1：工具返回值。代替原 `(String, Vec<TaskRef>)` 元组。
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub text: String,
    pub refs: Vec<crate::bot_chat::TaskRef>,
    pub status: ToolStatus,
    /// N5：随结果附给模型的图片绝对路径（如 screenshot 产物）。
    /// 模型循环把图作为紧随 tool 消息的 user 消息（image_url data-URL）注入——
    /// OpenAI 协议 tool 消息只收文本，图走 user 消息（官方视觉示例同款）；
    /// Anthropic 协议转换器自动把 [tool, user(图)] 合并成单条 user
    /// [tool_result, image]（官方 tool_result 附图形态）。默认空 = 无图。
    pub images: Vec<String>,
    /// P1-b（Agent 透明化设计 §4.2）：文件类工具成功落盘后附的变更证据
    /// （path/kind/±行/unified diff/回滚证据链）。dispatch 侧消费：
    /// 落 `file_changes` 表 + emit `bot-file-changed`。默认空 = 本调用无文件修改。
    pub file_changes: Vec<crate::bot_fs::FileChangeReceipt>,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>, refs: Vec<crate::bot_chat::TaskRef>) -> Self {
        Self {
            text: text.into(),
            refs,
            status: ToolStatus::Ok,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }
    /// 带图返回（N5）：images 为绝对路径，模型循环负责读文件转 data-URL 注入。
    /// 读取失败/超限的图会被跳过（循环侧逐图校验），不阻断文本结果。
    pub fn ok_with_images(
        text: impl Into<String>,
        refs: Vec<crate::bot_chat::TaskRef>,
        images: Vec<String>,
    ) -> Self {
        Self {
            text: text.into(),
            refs,
            status: ToolStatus::Ok,
            images,
            file_changes: Vec::new(),
        }
    }
    pub fn warn(text: impl Into<String>, refs: Vec<crate::bot_chat::TaskRef>) -> Self {
        Self {
            text: text.into(),
            refs,
            status: ToolStatus::Warn,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }
    pub fn error(text: impl Into<String>, refs: Vec<crate::bot_chat::TaskRef>) -> Self {
        Self {
            text: text.into(),
            refs,
            status: ToolStatus::Error,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }
    /// P1-b：附一条文件变更证据（edit_file/write_file 成功路径）。
    /// 只加字段不改语义——既有 33 工具不受影响（默认空 Vec）。
    pub fn with_file_change(mut self, c: crate::bot_fs::FileChangeReceipt) -> Self {
        self.file_changes.push(c);
        self
    }
}

/// T3 B1 过渡层：B2 工具逐个迁移后删除。已删。
///
/// B1 期间用 From impl 将工具返 (String, Vec<TaskRef>) 隐式转 ToolResult::ok；
/// B2 完成后所有 29 工具已显式返 ToolResult，From 过渡层失去作用，删除。
/// （若 B1/B2 期间遗留未迁移工具仍存在，该工具会爆「expected ToolResult」编译错——
/// 这是期望的强制迁移信号，不是回归。）

pub struct ToolDef {
    pub name: &'static str,
    pub schema: &'static str,
    pub mutating: bool,
    /// 模型声称「我做了这个工具的语义动作」的常见表达。
    /// 只在 `mutating == true` 时有意义；非 mutating 填 `&[]`。
    /// `claims_mutation()` 从所有 mutating 工具的 patterns 合并后检索。
    pub claims_patterns: &'static [&'static str],
    /// T6：工具返回文本的字符软上限（仅 audit，不截断）。
    /// 超阈值仅 audit_event!("tool.output.over_budget")，不丢字符、不压缩。
    /// 默认 8192（8KB）；长输出工具（read_text_file/fetch_url/list_files）自定 65536。
    pub max_output_chars: usize,
    pub call: for<'a> fn(&'a ToolCtx<'a>, &'a str) -> ToolFuture<'a>,
}

// ─────────────────── schema 常量（baseline 前缀一致）───────────────────
pub const SCHEMA_QUERY_TASKS: &str = r##"{"type":"function","function":{"name":"query_tasks","description":"查询任务卡：不传 query=列清单（view 默认 active 未完成）；传 query=按关键词检索（匹配标题/备注/标签/子任务，此时 view 默认 all 全库）。输出行带（工作流：名称）标记，工作流相关问题可按标记汇总回答；定位任务不确定时先调本工具确认","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"关键词，可选；不传=列清单"},
    "view":{"type":"string","enum":["active","done","archived","trash","all"],"description":"视图范围，可选：active=未完成 / done=已完成未归档 / archived=已归档 / trash=回收站 / all=除回收站外全部；默认随 query 自动定（无 query=active，有 query=all）"},
    "tag":{"type":"string","description":"按标签过滤，可选"},
    "limit":{"type":"integer","description":"最多返回条数，可选，默认 50，上限 200"}
}}}}"##;
pub const SCHEMA_QUERY_SINGLE_TASK: &str = r##"{"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 创建时间/定时/所属工作流/依赖 + 归档/删除状态指示；白名单单点，区别于 query_tasks 批量清单与关键词检索）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}}"##;
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
pub const SCHEMA_EDIT_TASK: &str = r##"{"type":"function","function":{"name":"edit_task","description":"编辑任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；改标题/备注/截止时间/标签/状态列/执行模型/归属人，空串清字段）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时用于定位任务"},
    "newTitle":{"type":"string","description":"新标题，可选"},
    "note":{"type":"string","description":"新备注，可选；空串清除"},
    "due":{"type":"string","description":"新截止时间，可选；空串清除"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"新绑定文件列表（可选，整体替换，最多 10 个；空数组清除；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件）"},
    "model":{"type":"string","description":"每卡执行模型（执行这张卡时覆盖全局激活模型），可选；空串清除=恢复跟随全局"},
    "owner":{"type":"string","description":"归属成员名或 personId（任务图谱按成员过滤用），可选；空串=归属本人；名字歧义时会返回候选名单，先向用户确认再重试"}
  },"required":[]}}}"##;
pub const SCHEMA_ADD_SUBTASK: &str = r##"{"type":"function","function":{"name":"add_subtask","description":"给任务添加子任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容"}
  },"required":["text"]}}}"##;
pub const SCHEMA_TOGGLE_SUBTASK: &str = r##"{"type":"function","function":{"name":"toggle_subtask","description":"勾选/取消勾选子任务（任务用 taskId 优先；子任务用 subtaskId 精确优先，无 id 时按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "subtaskId":{"type":"string","description":"子任务 id（query_single_task 输出），可选，优先于 text"},
    "text":{"type":"string","description":"子任务内容关键词，无 subtaskId 时必填"}
  },"required":[]}}}"##;
pub const SCHEMA_REMOVE_SUBTASK: &str = r##"{"type":"function","function":{"name":"remove_subtask","description":"删除单条子任务（彻底移除，区别于 toggle_subtask 的取消勾选；任务用 taskId 优先；子任务用 subtaskId 精确优先，无 id 时按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "subtaskId":{"type":"string","description":"要删除的子任务 id（query_single_task 输出），可选，优先于 text"},
    "text":{"type":"string","description":"要删除的子任务内容关键词，无 subtaskId 时必填"}
  },"required":[]}}}"##;
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
    "max":{"type":"integer","description":"最多返回条数，默认 50，可选"},
    "context":{"type":"integer","description":"命中行上下文行数，可选，0-5，默认 0（显示匹配行前后各 N 行，便于读懂语境）"}
  },"required":["pattern"]}}}"##;
pub const SCHEMA_LIST_FILES: &str = r##"{"type":"function","function":{"name":"list_files","description":"列出本地目录内的文件/子目录（递归 ≤5 层，最多 200 条；可用 pattern 按文件名过滤；白名单目录内直接列，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "dir":{"type":"string","description":"目录绝对路径（支持 ~ 开头）"},
    "pattern":{"type":"string","description":"文件名过滤，如 *.pdf 或 报告*，可选"}
  },"required":["dir"]}}}"##;
pub const SCHEMA_LINK_FILE_TO_TASK: &str = r##"{"type":"function","function":{"name":"link_file_to_task","description":"登记产物到本任务卡执行流程的产物清单。文件必须在 AI_Gen_Files 目录内。流程结束、任务完成、有产物时系统会落一条「通知中心」消息让用户勾选绑定（默认全选，每个文件绑一次；不再弹窗）；任务未完成、中断、只有中间产物都不产生待绑定消息。普通对话场景调用此工具不报错也不绑（不反复尝试）。仅任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内登记有效。","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "path":{"type":"string","description":"产物文件绝对路径，必须在 AI_Gen_Files 目录内且文件已存在"},
    "kind":{"type":"string","enum":["final","intermediate"],"default":"final","description":"final=最终产物，参与流程结束汇总弹窗；intermediate=中间产物，不参与弹窗。本会话在 AI_Gen_Files 目录没新建过的路径不参与绑定。"}
  },"required":["path"]}}}"##;
pub const SCHEMA_EXTRACT_DOCUMENT: &str = r##"{"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件；长文档用 offset 参数续读后续部分；扫描版/图片型 PDF 文本层为空时自动转图走本地 OCR 兜底，前 20 页）","parameters":{"type":"object","properties":{
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
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"},
    "images":{"type":"array","items":{"type":"string"},"description":"要插入的图片绝对路径列表，可选（仅 AI_Gen_Files 目录内的已存在图片，其余被丢弃并提示；按顺序插在正文之后、表格之前）"}
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
pub const SCHEMA_CREATE_PDF: &str = r##"{"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持，自动分页）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_RUN_PYTHON: &str = r##"{"type":"function","function":{"name":"run_python","description":"执行 Python 代码（资源受限：CPU/内存/时长限额 + 独立临时目录，无文件系统隔离；默认超时 60s。默认需用户在设置页开启 Python 编程，授权模式为 yolo 时免开关）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"},
    "timeoutSecs":{"type":"integer","description":"超时秒数（可选，默认 60；大计算可调大，上限 300）"}
  },"required":["code"]}}}"##;
pub const SCHEMA_WEB_SEARCH: &str = r##"{"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（配置 Tavily 或 Brave key 时走对应 API、双开报错，否则 Bing+百度网页抓取；返回标题/链接/摘要）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"},
    "count":{"type":"integer","description":"结果条数，可选，1-10，默认 8"},
    "timeRange":{"type":"string","enum":["day","week","month","year"],"description":"时间范围，可选：day=24小时内 / week=一周内 / month=一月内 / year=一年内（Tavily/Brave 原生支持；Bing+百度抓取模式不支持并会明确提示）"},
    "site":{"type":"string","description":"限定站点域名，可选，如 github.com（不要带 https:// 前缀）"}
  },"required":["query"]}}}"##;
pub const SCHEMA_FETCH_URL: &str = r##"{"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容；长网页带「已截断」提示时用 offset 参数续读）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；上一页返回的「已截断」提示里给了 offset 值，用它续读）"}
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
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"},
    "params":{"type":"object","description":"技能参数（可选；键=参数名，值=字符串）。技能声明了必填参数时必须提供（缺失会拒绝启动并列出缺什么），声明了默认值的参数可省略","additionalProperties":{"type":"string"}}
  },"required":["name"]}}}"##;
// ─────────────────── SUBA-2：子 agent 编排三工具（主 agent 可见） ───────────────────
pub const SCHEMA_SPAWN_SUBAGENT: &str = r##"{"type":"function","function":{"name":"spawn_subagent","description":"派发受管子 agent 执行单一目标长任务（非阻塞，立即返回 subagentId/taskId/status）。适用：预计超 5 轮工具调用、多来源调研、写代码跑脚本、用户要求后台/并行。objective 单一目标；acceptanceCriteria 必填且每条可检验（不要写「调研清楚」，要写「覆盖至少 5 个产品，每个含官网 URL，输出 report.md」）；contextSummary 只给必要背景，不要倒主对话全文。完成后用 check_subagent 轮询结果再汇总","parameters":{"type":"object","properties":{
    "objective":{"type":"string","description":"单一目标（一句话说清做什么）"},
    "profile":{"type":"string","enum":["research","coder","general"],"description":"工具档位：research=联网调研写报告 / coder=读代码跑 Python 写文件 / general=两者并集"},
    "acceptanceCriteria":{"type":"array","items":{"type":"string"},"minItems":1,"description":"验收标准，每条可检验"},
    "contextSummary":{"type":"string","description":"必要上下文摘要（可选；不含主对话全文）"},
    "budget":{"type":"object","properties":{"maxTurns":{"type":"integer","description":"工具循环轮数上限，默认 30，硬顶 50"},"maxToolCalls":{"type":"integer","description":"累计工具调用上限，默认 100"},"maxWallSeconds":{"type":"integer","description":"墙钟秒数上限，默认 600（排队不计）"}},"description":"预算三硬顶（可选，默认值见上；上卡可见）"},
    "modelProfile":{"type":"string","description":"指定模型名（可选；未指定/无效回退当前激活模型）"},
    "parentTaskId":{"type":"string","description":"主卡 id（可选；编排计划卡，缺卡报错）"}
  },"required":["objective","profile","acceptanceCriteria"]}}}"##;
pub const SCHEMA_CHECK_SUBAGENT: &str = r##"{"type":"function","function":{"name":"check_subagent","description":"查询子 agent 状态（幂等可轮询）：status=queued|running|succeeded|failed|cancelled|budget_exceeded + 进度 + 收尾结果（summary/artifacts/blockers）。waitMs>0 时短等待至终态或超时（上限 5000，不要高频空转）","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id（spawn 返回值），与 taskId 二选一"},
    "taskId":{"type":"string","description":"子任务卡 id，与 subagentId 二选一"},
    "waitMs":{"type":"integer","description":"短等待毫秒数（可选，0=立即返回，上限 5000）"}
  },"required":[]}}}"##;
pub const SCHEMA_CANCEL_SUBAGENT: &str = r##"{"type":"function","function":{"name":"cancel_subagent","description":"取消子 agent（与任务卡停止按钮同 API）：置 cancelled 并停止其执行；已终态则 no-op","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id，与 taskId 二选一"},
    "taskId":{"type":"string","description":"子任务卡 id，与 subagentId 二选一"},
    "reason":{"type":"string","description":"取消原因（可选）"}
  },"required":[]}}}"##;
// ─────────────────── SUBA-2：子 agent 白名单内工具（不进主 agent schema） ───────────────────
pub const SCHEMA_WRITE_ARTIFACT_FILE: &str = r##"{"type":"function","function":{"name":"write_artifact_file","description":"把文本内容写进自己的产物目录（gen_dir/subagents/{subagentId}/，报告/中间结果落地用）。filename 为纯文件名（不含路径分隔符），已存在会自动加 (n) 序号不覆盖","parameters":{"type":"object","properties":{
    "filename":{"type":"string","description":"纯文件名（不含路径分隔符 / 与 \\、不允许 .. 路径段与 NUL；Windows 保留设备名如 CON/NUL 拒绝），如 report.md / data.csv；同名自动加 (n) 序号"},
    "content":{"type":"string","description":"完整文本内容"}
  },"required":["filename","content"]}}}"##;
pub const SCHEMA_READ_OWN_CARD: &str = r##"{"type":"function","function":{"name":"read_own_card","description":"重读自己的任务卡（标题/验收标准 note/子任务清单/预算）：每轮开始建议先调，subtasks/note 有变更则调整计划；deletedAt 非空 = 卡片已被软删，立即停止新探索并收尾","parameters":{"type":"object","properties":{}}}}"##;
// ─────────────────── N4：电脑辅助 Tier1（只「看」与「打开」，无鼠标键盘） ───────────────────
pub const SCHEMA_REVEAL_PATH: &str = r##"{"type":"function","function":{"name":"reveal_path","description":"在访达（macOS）/资源管理器（Windows）中定位显示文件或文件夹（只定位，不打开文件本身；仅限白名单目录内路径，与读文件同一权限闸）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件或目录绝对路径（支持 ~ 开头）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_OPEN_URL: &str = r##"{"type":"function","function":{"name":"open_url","description":"用系统默认浏览器打开网页（仅 http/https 公网地址；本机/内网地址会被拒绝）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要打开的网页地址"}
  },"required":["url"]}}}"##;
pub const SCHEMA_CLIPBOARD_WRITE: &str = r##"{"type":"function","function":{"name":"clipboard_write","description":"把文本写入系统剪贴板，用户可直接粘贴（会覆盖剪贴板原内容；超长内容请分段）","parameters":{"type":"object","properties":{
    "text":{"type":"string","description":"要复制的文本"}
  },"required":["text"]}}}"##;
pub const SCHEMA_SCREENSHOT: &str = r##"{"type":"function","function":{"name":"screenshot","description":"截取主显示器画面。截图会直接作为图片附在工具结果之后，用你的视觉能力读取内容（PNG 同时落 AI_Gen_Files 留档；需要系统屏幕录制权限，macOS 未授权时会得到壁纸/黑图）","parameters":{"type":"object","properties":{}}}}"##;
// ─────────────────── N6：文件编辑（写白名单 ≠ 读白名单，白名单外逐次确认） ───────────────────
pub const SCHEMA_EDIT_FILE: &str = r##"{"type":"function","function":{"name":"edit_file","description":"对文本文件做精确字符串替换（小步修改首选；先 read_text_file 确认原文再改）。oldString 必须与文件内容一致且唯一（多处命中报错；行尾空白/CRLF 差异自动容错）。改完建议 read_text_file 复核","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；白名单外会弹确认）"},
    "oldString":{"type":"string","description":"要替换的原文（精确匹配，须唯一；含足够上下文）"},
    "newString":{"type":"string","description":"替换后的新文本（可为空串=删除该段）"}
  },"required":["path","oldString","newString"]}}}"##;
pub const SCHEMA_WRITE_FILE: &str = r##"{"type":"function","function":{"name":"write_file","description":"创建新文件或整体写入内容（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；覆盖已存在文件需要用户确认）。修改已有文件优先用 edit_file（精确替换更安全）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"目标文件绝对路径（父目录必须已存在）"},
    "content":{"type":"string","description":"完整文件内容（UTF-8 文本；不能含 NUL 字节；上限 2MB）"}
  },"required":["path","content"]}}}"##;
// ─────────────────── 适配器（统一签名，按需拆 ctx 字段）───────────────────
// link_file_to_task 是 async fn，必须 .await——拆出来后已修正。
// list_tasks/search_tasks 合并为 query_tasks（T1-QUERYTASKS）：收 args。
fn call_query_tasks<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_query_tasks(ctx.app, args).await })
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

// ─────────────────── SUBA-2 适配器：编排三工具 + 子 agent 白名单工具 ───────────────────
fn call_spawn_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(
        async move { crate::bot_orchestrator::tool_spawn_subagent(&app, &args, session_id).await },
    )
}

fn call_check_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    Box::pin(async move { crate::bot_orchestrator::tool_check_subagent(&app, &args).await })
}

fn call_cancel_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(
        async move { crate::bot_orchestrator::tool_cancel_subagent(&app, &args, session_id).await },
    )
}

fn call_write_artifact_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_orchestrator::tool_write_artifact_file(&app, &args, session_id).await
    })
}

fn call_read_own_card<'a>(ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move { crate::bot_orchestrator::tool_read_own_card(&app, session_id).await })
}

// ─────────────────── N4：电脑辅助 Tier1 适配器 ───────────────────
fn call_reveal_path<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_desktop::tool_reveal_path(ctx.app, args, interactive, session_id).await
    })
}

fn call_open_url<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_open_url(ctx.app, args).await })
}

fn call_clipboard_write<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_clipboard_write(ctx.app, args).await })
}

fn call_screenshot<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_screenshot(ctx.app, args).await })
}

// ─────────────────── N6：文件编辑适配器 ───────────────────
fn call_edit_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_edit_file(ctx.app, args, interactive, session_id).await },
    )
}

fn call_write_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_write_file(ctx.app, args, interactive, session_id).await },
    )
}

/// T4：合并 TOOLS_TABLE 中所有 mutating 工具的 claims_patterns，
/// 检查 text 是否含任一变更声称表述。
///
/// 原 bot_model_loop.rs::claims_mutation 的「14 个动词 + 4 个整段」硬编码
/// 改为从 ToolDef 表派生——加新 mutating 工具时只需填自己的 claims_patterns，
/// 这里的检测逻辑零修改。
///
/// 检测是 2 阶段：
/// 1. 子串包含（catches 「移至回收站」、「已完成任务」等整段）
/// 2. 「已」+ 8 字窗口包含任一 pattern（catches 「已...移除」类变体话术；
///    原实现同款，避免漏掉「已把附件全部移除」之类的中间插入副词话术）
///
/// 两阶段并集 OR，原 hallucination_guard_tests 6 个负面用例仍会正确返回 false。
pub fn claims_mutation(text: &str) -> bool {
    let patterns: Vec<&str> = TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .flat_map(|t| t.claims_patterns.iter())
        .copied()
        .collect();
    // 阶段 1：子串匹配
    if patterns.iter().any(|p| text.contains(p)) {
        return true;
    }
    // 阶段 2：「已」后 8 字窗口包含任一 pattern（strip 前缀「已」后）
    // 原 bot_model_loop.rs VERBS 表不含「已」前缀（「删除」不是「已删除」），
    // 故 window 检查也需剥去 pattern 的前缀「已」才能覆盖「已...移除」类变体。
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '已' {
            let window: String = chars[i + 1..].iter().take(8).collect();
            if patterns.iter().any(|p| {
                let stripped = p.strip_prefix('已').unwrap_or(p);
                window.contains(stripped)
            }) {
                return true;
            }
        }
    }
    false
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// T7：TOOLS_TABLE 的 O(1) HashMap 索引。
/// dispatch 的 O(n) `TOOLS_TABLE.iter().find(...)` 改为 O(1) `tools_index().get(name)`。
/// OnceLock 保证只有一次初始化；索引里全部为 `&'static ToolDef` 引用，零额外分配（除 HashMap 表本身）。
static TOOLS_INDEX: OnceLock<HashMap<&'static str, &'static ToolDef>> = OnceLock::new();

pub fn tools_index() -> &'static HashMap<&'static str, &'static ToolDef> {
    TOOLS_INDEX.get_or_init(|| TOOLS_TABLE.iter().map(|t| (t.name, t)).collect())
}

// ─────────────────── TOOLS_TABLE（33 工具单源真相：31 主可见 + 2 子 agent 专属）───────────────────
pub static TOOLS_TABLE: &[ToolDef] = &[
    ToolDef {
        name: "query_tasks",
        schema: SCHEMA_QUERY_TASKS,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_query_tasks,
    },
    ToolDef {
        name: "query_single_task",
        schema: SCHEMA_QUERY_SINGLE_TASK,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_query_single_task,
    },
    ToolDef {
        name: "create_task",
        schema: SCHEMA_CREATE_TASK,
        mutating: true,
        claims_patterns: &["已创建", "已新建", "新建了", "创建了", "已新增"],
        max_output_chars: 8192,
        call: call_create_task,
    },
    ToolDef {
        name: "complete_task",
        schema: SCHEMA_COMPLETE_TASK,
        mutating: true,
        claims_patterns: &["已标记为完成", "标记为完成", "已完成任务", "已完成「"],
        max_output_chars: 8192,
        call: call_complete_task,
    },
    ToolDef {
        name: "delete_task",
        schema: SCHEMA_DELETE_TASK,
        mutating: true,
        claims_patterns: &[
            "已删除",
            "已经删除",
            "已彻底删除",
            "已经彻底删除",
            "已搬移",
            "已搬到回收站",
            "移至回收站",
            "已移除",
            "已扔掉",
        ],
        max_output_chars: 8192,
        call: call_delete_task,
    },
    ToolDef {
        name: "edit_task",
        schema: SCHEMA_EDIT_TASK,
        mutating: true,
        claims_patterns: &["已修改", "已更新", "已改成", "已改", "已调整", "已编辑"],
        max_output_chars: 8192,
        call: call_edit_task,
    },
    ToolDef {
        name: "add_subtask",
        schema: SCHEMA_ADD_SUBTASK,
        mutating: true,
        claims_patterns: &[
            "已添加子任务",
            "添加子任务",
            "已加上子任务",
            "已新增子任务",
            "已加子任务",
        ],
        max_output_chars: 8192,
        call: call_add_subtask,
    },
    ToolDef {
        name: "toggle_subtask",
        schema: SCHEMA_TOGGLE_SUBTASK,
        mutating: true,
        claims_patterns: &["已勾选", "勾选了", "已标记子任务完成", "子任务已勾选"],
        max_output_chars: 8192,
        call: call_toggle_subtask,
    },
    ToolDef {
        name: "remove_subtask",
        schema: SCHEMA_REMOVE_SUBTASK,
        mutating: true,
        claims_patterns: &[
            "已删除子任务",
            "已移除子任务",
            "子任务已删除",
            "子任务已移除",
            "已清掉子任务",
        ],
        max_output_chars: 8192,
        call: call_remove_subtask,
    },
    ToolDef {
        name: "read_text_file",
        schema: SCHEMA_READ_TEXT_FILE,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 65536,
        call: call_read_text_file,
    },
    ToolDef {
        name: "ocr_image",
        schema: SCHEMA_OCR_IMAGE,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_ocr_image,
    },
    ToolDef {
        name: "grep_files",
        schema: SCHEMA_GREP_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_grep_files,
    },
    ToolDef {
        name: "list_files",
        schema: SCHEMA_LIST_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 65536,
        call: call_list_files,
    },
    ToolDef {
        name: "link_file_to_task",
        schema: SCHEMA_LINK_FILE_TO_TASK,
        mutating: true,
        claims_patterns: &[
            "已绑定",
            "已关联",
            "绑定到任务",
            "已绑定文件",
            "已清空",
            "已清空文件",
            "已解绑",
        ],
        max_output_chars: 8192,
        call: call_link_file_to_task,
    },
    ToolDef {
        name: "extract_document",
        schema: SCHEMA_EXTRACT_DOCUMENT,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_extract_document,
    },
    ToolDef {
        name: "create_word",
        schema: SCHEMA_CREATE_WORD,
        mutating: true,
        claims_patterns: &[
            "已生成 Word",
            "已生成 word",
            "已创建 Word",
            "Word 文件已生成",
            "已写入 Word",
        ],
        max_output_chars: 8192,
        call: call_create_word,
    },
    ToolDef {
        name: "create_word_revisions",
        schema: SCHEMA_CREATE_WORD_REVISIONS,
        mutating: true,
        claims_patterns: &["已修订 Word", "已应用修订", "已写入修订", "修订已应用"],
        max_output_chars: 8192,
        call: call_create_word_revisions,
    },
    ToolDef {
        name: "create_excel",
        schema: SCHEMA_CREATE_EXCEL,
        mutating: true,
        claims_patterns: &[
            "已生成 Excel",
            "已生成 excel",
            "已创建 Excel",
            "Excel 文件已生成",
            "已写入 Excel",
        ],
        max_output_chars: 8192,
        call: call_create_excel,
    },
    ToolDef {
        name: "create_ppt",
        schema: SCHEMA_CREATE_PPT,
        mutating: true,
        claims_patterns: &[
            "已生成 PPT",
            "已生成 ppt",
            "已生成幻灯片",
            "PPT 文件已生成",
            "已写入 PPT",
        ],
        max_output_chars: 8192,
        call: call_create_ppt,
    },
    ToolDef {
        name: "create_pdf",
        schema: SCHEMA_CREATE_PDF,
        mutating: true,
        claims_patterns: &[
            "已生成 PDF",
            "已生成 pdf",
            "已创建 PDF",
            "PDF 文件已生成",
            "已写入 PDF",
        ],
        max_output_chars: 8192,
        call: call_create_pdf,
    },
    ToolDef {
        name: "run_python",
        schema: SCHEMA_RUN_PYTHON,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_run_python,
    },
    ToolDef {
        name: "web_search",
        schema: SCHEMA_WEB_SEARCH,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_web_search,
    },
    ToolDef {
        name: "fetch_url",
        schema: SCHEMA_FETCH_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 65536,
        call: call_fetch_url,
    },
    ToolDef {
        name: "get_current_time",
        schema: SCHEMA_GET_CURRENT_TIME,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_get_current_time,
    },
    ToolDef {
        name: "remember_fact",
        schema: SCHEMA_REMEMBER_FACT,
        mutating: true,
        claims_patterns: &[
            "已记住",
            "已记下",
            "已记录偏好",
            "偏好已记",
            "已记住你的偏好",
        ],
        max_output_chars: 8192,
        call: call_remember_fact,
    },
    ToolDef {
        name: "recall_facts",
        schema: SCHEMA_RECALL_FACTS,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_recall_facts,
    },
    ToolDef {
        name: "record_lesson",
        schema: SCHEMA_RECORD_LESSON,
        mutating: true,
        claims_patterns: &[
            "已保存",
            "已保存到 AI_Gen_Files",
            "已记住你的偏好",
            "已记下教训",
            "已存档",
        ],
        max_output_chars: 8192,
        call: call_record_lesson,
    },
    ToolDef {
        name: "use_skill",
        schema: SCHEMA_USE_SKILL,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_use_skill,
    },
    // ─────────────────── SUBA-2：子 agent 编排（主 agent 可见，追加表尾） ───────────────────
    ToolDef {
        name: "spawn_subagent",
        schema: SCHEMA_SPAWN_SUBAGENT,
        mutating: true,
        claims_patterns: &[
            "已派发",
            "已派出",
            "已派子任务",
            "已启动子任务",
            "已创建子任务",
        ],
        max_output_chars: 8192,
        call: call_spawn_subagent,
    },
    ToolDef {
        name: "check_subagent",
        schema: SCHEMA_CHECK_SUBAGENT,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_check_subagent,
    },
    ToolDef {
        name: "cancel_subagent",
        schema: SCHEMA_CANCEL_SUBAGENT,
        mutating: true,
        claims_patterns: &["已取消子任务", "已取消派发", "已终止子任务"],
        max_output_chars: 8192,
        call: call_cancel_subagent,
    },
    // ─────────────────── N4：电脑辅助 Tier1（只「看」与「打开」） ───────────────────
    ToolDef {
        name: "reveal_path",
        schema: SCHEMA_REVEAL_PATH,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_reveal_path,
    },
    ToolDef {
        name: "open_url",
        schema: SCHEMA_OPEN_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_open_url,
    },
    ToolDef {
        name: "clipboard_write",
        schema: SCHEMA_CLIPBOARD_WRITE,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_clipboard_write,
    },
    ToolDef {
        name: "screenshot",
        schema: SCHEMA_SCREENSHOT,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_screenshot,
    },
    // ─────────────────── N6：文件编辑（写白名单 ≠ 读白名单） ───────────────────
    ToolDef {
        name: "edit_file",
        schema: SCHEMA_EDIT_FILE,
        mutating: true,
        claims_patterns: &["已修改", "已编辑"],
        max_output_chars: 8192,
        call: call_edit_file,
    },
    ToolDef {
        name: "write_file",
        schema: SCHEMA_WRITE_FILE,
        mutating: true,
        claims_patterns: &["已写入", "已创建文件"],
        max_output_chars: 8192,
        call: call_write_file,
    },
    // ─────────────────── SUBA-2：子 agent 白名单工具（不进主 agent 默认 schema） ───────────────────
    ToolDef {
        name: "write_artifact_file",
        schema: SCHEMA_WRITE_ARTIFACT_FILE,
        mutating: true,
        claims_patterns: &["已写入产物", "产物已写入", "已保存产物", "已写入报告"],
        max_output_chars: 8192,
        call: call_write_artifact_file,
    },
    ToolDef {
        name: "read_own_card",
        schema: SCHEMA_READ_OWN_CARD,
        mutating: false,
        claims_patterns: &[],
        max_output_chars: 8192,
        call: call_read_own_card,
    },
];
/// TOOLS JSON 由 TOOLS_TABLE 顺序拼装（schema 常量原文直拼，不做 parse + re-serialize）。
/// 少一次运行期解析，也不给「schema 非法 → expect panic 杀聊天」留路径
/// （合法性 + 与 baseline 的一致性由 registry_tests 锁死）。
///
/// SUBA-2：仅子 agent 可见的工具（write_artifact_file / read_own_card）不出现在
/// 主 agent 的默认清单里——主可见 = 28 核心 + 编排三 + 电脑辅助四（T1 后 31，N4 后 35）。
pub fn tools_json() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut s = String::with_capacity(16 * 1024);
        s.push('[');
        let mut first = true;
        for tool in TOOLS_TABLE.iter() {
            if SUBAGENT_ONLY_TOOLS.contains(&tool.name) {
                continue;
            }
            s.push_str(if first { "\n  " } else { ",\n  " });
            first = false;
            s.push_str(tool.schema);
        }
        s.push_str("\n]");
        s
    })
}

/// 仅子 agent 白名单可见的工具（主 agent 默认 schema 不含）。
pub const SUBAGENT_ONLY_TOOLS: &[&str] = &["write_artifact_file", "read_own_card"];

/// 子 agent 工具白名单（设计 §5.1 → 本仓工具映射）。
/// 禁止（所有 profile）：spawn_subagent / check_subagent / cancel_subagent /
/// 任务卡主状态写（create_task/complete_task/edit_task/toggle_subtask 等全部不在列）/
/// 任务执行 / 权限变更——白名单外一律由 dispatch 闸拒绝。
pub const RESEARCH_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "list_files",
    "read_text_file",
    "write_artifact_file",
    "read_own_card",
];
pub const CODER_TOOLS: &[&str] = &[
    "read_text_file",
    "list_files",
    "grep_files",
    "edit_file",
    "write_file",
    "run_python",
    "write_artifact_file",
    "read_own_card",
];
pub const GENERAL_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "read_text_file",
    "list_files",
    "grep_files",
    "edit_file",
    "write_file",
    "run_python",
    "write_artifact_file",
    "read_own_card",
];

pub fn profile_whitelist(profile: crate::db::SubagentProfile) -> &'static [&'static str] {
    match profile {
        crate::db::SubagentProfile::Research => RESEARCH_TOOLS,
        crate::db::SubagentProfile::Coder => CODER_TOOLS,
        crate::db::SubagentProfile::General => GENERAL_TOOLS,
    }
}

/// 按会话选工具 schema：子 agent 会话 → profile 白名单 JSON；其他 → 默认全量。
/// run_model_loop_core 每轮经此处取 tools（设计 §5.1 白名单 + §10 递归双保险①）。
pub fn tools_json_for(session_id: Option<&str>) -> &'static str {
    let profile = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .map(|c| c.profile);
    match profile {
        None => tools_json(),
        Some(p) => profile_tools_json(p),
    }
}

/// 主 agent 的完整工具清单（阶段 3 MCP 挂载点，拍板 2A 机制 A）：
/// 内置静态 JSON 尾部追加外部 MCP 工具（增量挂载，内置 31 工具 schema 字节不动）。
/// - 子 agent 会话：短路返回白名单（外部 MCP 工具不进子 agent，§5.1 边界不破）；
/// - 无 MCP 连接：原样返回静态 &'static str（零分配，热路径不变）；
/// - 有连接：Owned String = 静态 JSON 摘尾 + `mount::mcp_tools_json_body()` + 收尾。
/// 消费点 bot_model_loop 对拼装结果 fail-soft（B0-1）：schema 构建层保证 JSON
/// 合法（mount/registry 单测锁），解析失败记审计走空工具表，不再 panic。
pub fn tools_json_with_mcp(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    // 子 agent 判定显式化（B0 评审：原 ptr::eq 指针比较对 tools_json_for 的
    // 返回形态有隐式契约，重构易碎）——subagent_ctx 是内存查表，重复一次开销可忽略
    let is_subagent = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .is_some();
    let base = tools_json_for(session_id);
    if is_subagent {
        // 子 agent 白名单：静态借用返回
        return std::borrow::Cow::Borrowed(base);
    }
    let body = crate::bot::mcp::mount::mcp_tools_json_body();
    if body.is_empty() {
        return std::borrow::Cow::Borrowed(base);
    }
    // base 恒以 "\n]" 收尾（tools_json 拼装格式）。契约破坏属"永不发生"态：
    // 每轮模型循环都走这里，告警限频防刷屏——第 1 次 + 之后每 100 次
    //（B0 评审：OnceLock 只告一次，持续破坏会彻底静默）；回退 base 后
    // 消费点的 fail-soft 兜底接住（B0-1：release 无 debug_assert，不产出非法 JSON）。
    static CONTRACT_BREAKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if !base.ends_with("\n]") {
        let n = CONTRACT_BREAKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n == 1 || n % 100 == 0 {
            eprintln!(
                "[registry] tools_json 格式契约破坏（未以 \\n] 收尾，第 {n} 次），MCP 工具本轮不挂载"
            );
        }
    }
    splice_mcp_body(base, &body)
}

/// 摘尾拼接内核（纯函数，单测直打）：base 恒以 `"\n]"` 收尾、动态段恒以
/// `",\n"` 起头（build_tools_json_body 构造保证），摘尾接段再补收尾；
/// 任一侧契约破坏回退 base 原样返回（宁可本轮少挂 MCP 工具，不产出非法 JSON。
/// B0-1，AUDIT-FIX-PLAN-2026-09-29；body 侧对偶检查为 B0 评审补充）。
fn splice_mcp_body<'a>(base: &'a str, body: &str) -> std::borrow::Cow<'a, str> {
    let Some(head) = base.strip_suffix("\n]") else {
        return std::borrow::Cow::Borrowed(base);
    };
    if !body.starts_with(",\n") {
        return std::borrow::Cow::Borrowed(base);
    }
    let mut s = String::with_capacity(head.len() + body.len() + 2);
    s.push_str(head);
    s.push_str(body);
    s.push_str("\n]");
    std::borrow::Cow::Owned(s)
}

pub fn profile_tools_json(profile: crate::db::SubagentProfile) -> &'static str {
    static CACHE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut map = HashMap::new();
            for p in [
                crate::db::SubagentProfile::Research,
                crate::db::SubagentProfile::Coder,
                crate::db::SubagentProfile::General,
            ] {
                let whitelist = profile_whitelist(p);
                let mut s = String::with_capacity(4 * 1024);
                s.push('[');
                let mut first = true;
                for tool in TOOLS_TABLE.iter() {
                    if !whitelist.contains(&tool.name) {
                        continue;
                    }
                    s.push_str(if first { "\n  " } else { ",\n  " });
                    first = false;
                    s.push_str(tool.schema);
                }
                s.push_str("\n]");
                map.insert(p.as_str(), s);
            }
            map
        })
        .get(profile.as_str())
        .map(|s| s.as_str())
        .unwrap_or_else(|| tools_json())
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
    fn tools_table_contains_37_main_visible_tools() {
        let v: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        // SUBA-2 + T1 + N4 + N6：主可见 = 28 核心 + 编排三 + 电脑辅助四 + 文件编辑两；
        // write_artifact_file / read_own_card 仅子 agent 白名单可见
        assert_eq!(arr.len(), 37, "主 agent 可见工具必须为 37");
        assert_eq!(
            TOOLS_TABLE.len(),
            39,
            "TOOLS_TABLE 全量 39（含 2 个 subagent-only）"
        );

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
            "query_tasks",
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
            "spawn_subagent",
            "check_subagent",
            "cancel_subagent",
            "reveal_path",
            "open_url",
            "clipboard_write",
            "screenshot",
            "edit_file",
            "write_file",
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
            "query_tasks",
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
            "spawn_subagent",
            "check_subagent",
            "cancel_subagent",
            "reveal_path",
            "open_url",
            "clipboard_write",
            "screenshot",
            "edit_file",
            "write_file",
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
            "spawn_subagent",
            "cancel_subagent",
            "write_artifact_file",
            "edit_file",
            "write_file",
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
    ///
    /// T1/N4：核心 28 工具基线（T1 重排为 query_tasks；N4 电脑辅助四工具追加在
    /// 编排三工具之后、子 agent 专属之前——不进 baseline 前缀）。
    /// baseline 必须是 derived 的**前缀**（核心 schema 变更走「显式更新本批 +
    /// 重生成 fixture」流程）。
    #[test]
    fn tools_json_matches_baseline() {
        let fixture = include_str!("../../tests/fixtures/tools_baseline.json");
        let r_str_marker = fixture.find("r#\"").expect("fixture 缺 r#\"") + 3;
        let end = fixture.rfind("]\"#;").expect("fixture 缺 ]\"#;") + 1;
        let baseline: serde_json::Value = serde_json::from_str(&fixture[r_str_marker..end])
            .expect("fixture 内 const TOOLS 正文必须是合法 JSON");
        let derived: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        let base_arr = baseline.as_array().unwrap();
        let der_arr = derived.as_array().unwrap();
        assert_eq!(
            &der_arr[..base_arr.len()],
            base_arr.as_slice(),
            "tools_json() 前 28 项与 baseline 漂移（核心 schema 变更须显式重生成 fixture）"
        );
        assert_eq!(der_arr.len(), base_arr.len() + 9, "主可见应为 28+3+4+2");
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

    /// T4 验收：每个 mutating 工具至少有一个 claims_pattern 能匹配到一段典型 LLM 回复。
    /// 挑 「+ pattern」作为测试输入（含「已」前缀，触发两阶段检测的第一阶段）。
    /// 非 mutating 工具不做此检查（它们的 claims_patterns 是 &[]）。
    #[test]
    fn every_mutating_tool_has_matching_claim_pattern() {
        let samples: &[(&str, &str)] = &[
            ("create_task", "已创建任务「买菜」"),
            ("complete_task", "已将任务标记为完成"),
            ("delete_task", "已将「你们好」移至回收站 🗑️"),
            ("edit_task", "已修改任务标题为「买菜」"),
            ("add_subtask", "已给任务添加子任务「买菜」✅"),
            ("toggle_subtask", "已勾选子任务「买菜」"),
            ("remove_subtask", "已删除子任务「买菜」"),
            ("link_file_to_task", "已绑定文件到任务"),
            ("create_word", "已生成 Word 文档"),
            ("create_word_revisions", "已修订 Word 文档"),
            ("create_excel", "已生成 Excel 表格"),
            ("create_ppt", "已生成 PPT 幻灯片"),
            ("create_pdf", "已生成 PDF 文件"),
            ("remember_fact", "已记住你的偏好"),
            ("record_lesson", "已保存到 AI_Gen_Files"),
            ("spawn_subagent", "已派发子 agent 执行调研"),
            ("cancel_subagent", "已取消子任务派发"),
            ("write_artifact_file", "已写入产物 report.md"),
        ];
        for (tool_name, sample) in samples {
            let t = TOOLS_TABLE
                .iter()
                .find(|t| t.name == *tool_name)
                .unwrap_or_else(|| panic!("TOOLS_TABLE 缺 {tool_name}"));
            assert!(
                t.mutating,
                "{tool_name} 应为 mutating=true（样本：{sample}）"
            );
            assert!(
                !t.claims_patterns.is_empty(),
                "{tool_name} 缺 claims_patterns（mutating 工具必须有 pattern）"
            );
            assert!(
                t.claims_patterns.iter().any(|p| sample.contains(p)),
                "{tool_name} 的所有 claims_patterns {:#?} 都匹配不上典型样本 `{sample}`",
                t.claims_patterns
            );
            // 同时验证 claims_mutation() 能从 TOOLS_TABLE 联动检测到该声称
            assert!(
                claims_mutation(sample),
                "{tool_name} 的 pattern 集合 OK 但 claims_mutation() 未返 true（拼接逻辑问题）"
            );
        }
    }

    /// 非 mutating 工具的 claims_patterns 必须为 &[]（错填会带入伪变更检测）。
    #[test]
    fn non_mutating_tools_have_empty_patterns() {
        for t in TOOLS_TABLE.iter().filter(|t| !t.mutating) {
            assert!(
                t.claims_patterns.is_empty(),
                "{} 是非 mutating 但 claims_patterns 不空: {:?}",
                t.name,
                t.claims_patterns
            );
        }
    }

    /// T7：TOOLS_TABLE 索引长度等于 TOOLS_TABLE 长度——保证无重复 name。
    #[test]
    fn tools_index_len_matches_table_len() {
        let idx = tools_index();
        assert_eq!(
            idx.len(),
            TOOLS_TABLE.len(),
            "tools_index().len() ({}) != TOOLS_TABLE.len() ({})，含重复 name",
            idx.len(),
            TOOLS_TABLE.len()
        );
    }

    /// T7：TOOLS_TABLE 里每个 name 都能在 index 里查到——索引完整覆盖。
    /// ToolDef 不 impl PartialEq/Debug，改用 std::ptr::eq 校验指针同一性。
    #[test]
    fn tools_index_covers_all_table_names() {
        let idx = tools_index();
        for t in TOOLS_TABLE {
            assert!(idx.contains_key(t.name), "tools_index 缺 key `{}`", t.name);
            // idx.get(t.name) 是 Option<&&'static ToolDef>；.copied() 拆一层外引用
            let got = idx
                .get(t.name)
                .copied()
                .expect("contains_key 已返 true，这里不可能 None");
            // 指针同一性校验（不需 PartialEq）
            assert!(
                std::ptr::eq(got, t),
                "tools_index[`{}`] 指针与 TOOLS_TABLE 不一致",
                t.name
            );
        }
    }

    // ─────────────────── SUBA-2：子 agent 白名单 schema ───────────────────

    /// 设计 §5/§13：子 agent 工具清单断言无 spawn（递归双保险①——schema 层）；
    /// 三个 profile 都不含任务卡主状态写工具与编排工具。
    #[test]
    fn subagent_whitelists_exclude_spawn_and_task_writes() {
        let forbidden = [
            "spawn_subagent",
            "check_subagent",
            "cancel_subagent",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "link_file_to_task",
            "use_skill",
        ];
        for p in [
            crate::db::SubagentProfile::Research,
            crate::db::SubagentProfile::Coder,
            crate::db::SubagentProfile::General,
        ] {
            let json = profile_tools_json(p);
            let v: serde_json::Value = serde_json::from_str(json).expect("白名单必须是合法 JSON");
            let names: Vec<&str> = v
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["function"]["name"].as_str().unwrap())
                .collect();
            for f in forbidden {
                assert!(
                    !names.contains(&f),
                    "{:?} 白名单不得含 {f}（递归禁用/主状态只读）",
                    p
                );
            }
            assert!(
                names.contains(&"read_own_card"),
                "白名单必须含 read_own_card"
            );
            assert!(names.contains(&"write_artifact_file"));
        }
    }

    /// 白名单内容精确性：research 联网 + 读列；coder 读列 + edit/write + run_python；
    /// general = research ∪ coder 去重。
    #[test]
    fn profile_whitelists_match_design_mapping() {
        assert_eq!(
            RESEARCH_TOOLS,
            &[
                "web_search",
                "fetch_url",
                "list_files",
                "read_text_file",
                "write_artifact_file",
                "read_own_card",
            ]
        );
        assert_eq!(
            CODER_TOOLS,
            &[
                "read_text_file",
                "list_files",
                "grep_files",
                "edit_file",
                "write_file",
                "run_python",
                "write_artifact_file",
                "read_own_card",
            ]
        );
        let mut union: Vec<&str> = RESEARCH_TOOLS
            .iter()
            .chain(CODER_TOOLS.iter())
            .copied()
            .collect();
        union.sort();
        union.dedup();
        let mut general: Vec<&str> = GENERAL_TOOLS.to_vec();
        general.sort();
        assert_eq!(general, union, "general 必须是 research ∪ coder 去重");
        // 白名单里的名字全部真实存在（防拼写漂移）
        for name in GENERAL_TOOLS {
            assert!(
                tools_index().contains_key(name),
                "白名单工具 {name} 不在 TOOLS_TABLE"
            );
        }
    }

    /// 无会话上下文 → 默认全量（含编排三工具，不含 subagent-only）
    #[test]
    fn tools_json_for_plain_session_is_default() {
        assert_eq!(tools_json_for(Some("not-a-subagent-session")), tools_json());
        assert_eq!(tools_json_for(None), tools_json());
    }

    // ─────────────────── 阶段 3：MCP 动态挂载点 ───────────────────

    /// 无 MCP 连接：tools_json_with_mcp 必须原样借用静态 JSON（零分配、零漂移）。
    /// 与 manager e2e 共用测试锁（B0-3）：e2e 的连接存活窗口会让「无连接」断言
    /// 在 cargo test 并行下 flaky；先清残留（前序用例 panic 可能留下连接槽）。
    #[test]
    fn tools_json_with_mcp_without_connections_is_borrowed_static() {
        // blocking_lock 须在 block_on 进入 runtime 上下文之前取（tokio Mutex 语义）
        let _serial = crate::bot::mcp::manager::SHARED_MCP_TEST_LOCK.blocking_lock();
        let _ = tauri::async_runtime::block_on(crate::bot::mcp::manager::shared().shutdown());
        let with = tools_json_with_mcp(None);
        assert!(matches!(with, std::borrow::Cow::Borrowed(_)));
        assert_eq!(with.as_ref(), tools_json());
        // 子 agent 会话短路：不碰 MCP（挂载层根本不被调用）
        let sub = tools_json_with_mcp(Some("not-a-subagent-session"));
        assert!(matches!(sub, std::borrow::Cow::Borrowed(_)));
    }

    /// B0-1：摘尾拼接契约——base 正常时摘尾接段补收尾；契约破坏（未以 \n] 收尾）
    /// 回退 base 原样返回，绝不产出非法 JSON（release 无 debug_assert 保护）。
    #[test]
    fn splice_mcp_body_falls_back_when_base_contract_broken() {
        let broken = "[\n  {\"type\":\"function\"}";
        assert!(matches!(
            super::splice_mcp_body(broken, ",\n  {}"),
            std::borrow::Cow::Borrowed(_)
        ));
        let out = super::splice_mcp_body("[\n  {}\n]", ",\n  {}");
        assert_eq!(out.as_ref(), "[\n  {},\n  {}\n]");
        // 拼装结果恒为合法 JSON 数组
        let v: serde_json::Value = serde_json::from_str(out.as_ref()).expect("拼装结果应合法");
        assert_eq!(v.as_array().map(|a| a.len()), Some(2));
        // body 侧对偶契约（B0 评审）：动态段不以 ",\n" 起头（mount 层构造破坏）
        // 同样回退 base，不产出非法 JSON
        assert!(matches!(
            super::splice_mcp_body("[\n  {}\n]", "garbage"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            super::splice_mcp_body("[\n  {}\n]", ",garbage"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    /// 动态拼装契约（纯格式）：拿 mount 的纯函数造一个假 body，
    /// 拼装结果必须是合法 JSON 数组、内置 35 工具在前 + MCP 条目在后。
    /// （真实连接路径由 manager e2e 测试覆盖：连接后 with_mcp 含 mcp_echo_echo。）
    #[test]
    fn tools_json_with_mcp_assembly_contract() {
        let base = tools_json();
        let body = crate::bot::mcp::mount::build_tools_json_body(&[]);
        assert!(body.is_empty(), "无服务器时空段");
        // 模拟 registry 拼装路径（与 tools_json_with_mcp Owned 分支同构）
        let fake_body = r#",
  {"type":"function","function":{"name":"mcp_fake_x","description":"d","parameters":{"type":"object","properties":{}}}}"#;
        let mut s = String::with_capacity(base.len() + fake_body.len() + 2);
        s.push_str(&base[..base.len() - 2]);
        s.push_str(fake_body);
        s.push_str("\n]");
        let v: serde_json::Value = serde_json::from_str(&s).expect("拼装结果必须合法");
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 38, "37 内置 + 1 假 MCP");
        assert_eq!(arr[37]["function"]["name"], "mcp_fake_x");
        // 内置前 37 项顺序不变（增量挂载不漂移）
        let base_arr = serde_json::from_str::<serde_json::Value>(base).unwrap();
        assert_eq!(&arr[..37], base_arr.as_array().unwrap().as_slice());
    }
}
