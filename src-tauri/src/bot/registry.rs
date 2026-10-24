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
    tool_link_file_to_task, tool_list_services, tool_query_single_task, tool_query_tasks,
    tool_remove_subtask, tool_run_python, tool_start_service, tool_stop_service,
    tool_toggle_subtask, tool_web_search,
};
use crate::bot_skills::{tool_create_skill, tool_use_skill};

pub struct ToolCtx<'a> {
    pub app: &'a AppHandle,
    pub stop: Option<&'a crate::bot_slash::StopGuard>,
    pub interactive: bool,
    pub session_id: Option<&'a str>,
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

/// 工具返回的成败状态。审计级别、trace span 的 ok 字段、以及「这一步算不算
/// 成功」的调度判定全部由它派生（`AuditLevel::from_tool_status` /
/// `dispatch::span_ok`），工具返回文本只负责给模型读。
///
/// 为什么单列 `Denied`：`Error` 是工具自己没能做完（执行报错），`Denied` 是
/// 根本没执行——被闸拦下、被用户拒绝、预算触顶。两者对审计都不该同级别
/// （前者 ERROR、后者 WARN 是正常控制流），但对「这一步成功了吗」是同一个
/// 答案：不成功。合成一个状态会让日志被预期内的拒绝刷红。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    /// 工具成功完成主职责
    Ok,
    /// 工具成功但有部分告警（对「成功了吗」仍是成功）
    Warn,
    /// 工具没能完成主职责：执行报错
    Error,
    /// 工具根本没执行：被闸拦下 / 用户拒绝 / 预算或熔断触顶
    Denied,
}

impl ToolStatus {
    /// 这次工具调用算不算成功。成败的唯一真相源——审计级别、trace span 的 ok、
    /// Skill DSL 调度器的步骤成败判定都从这里派生。
    ///
    /// `Warn` 算成功：它表示「做成了但有告警」（部分写入、有降级）。`Error` 与
    /// `Denied` 都不算。
    ///
    /// 为什么不用 `!audit::tool_call_failed(text)`：那是文本匹配，和 status 两套
    /// 口径必然漂移。实际已经漂过——闸拦 / 用户拒绝 / 预算触顶 / 白名单外这 4 类
    /// 文案既不带「⚠️」也不带「失败：」，审计按 status 记 WARN，而 trace span 与
    /// Skill DSL 调度器按文本判成**成功**，被拒步骤被记成技能正常完成。
    /// 工具返回文本只负责给模型读与 `classify_error_class` 的子类细分。
    pub fn is_success(self) -> bool {
        matches!(self, ToolStatus::Ok | ToolStatus::Warn)
    }
}

/// 工具返回值。代替原 `(String, Vec<TaskRef>)` 元组。
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub text: String,
    pub refs: Vec<crate::bot_chat::TaskRef>,
    pub status: ToolStatus,
    /// 随结果附给模型的图片绝对路径（如 screenshot 产物）。
    /// 模型循环把图作为紧随 tool 消息的 user 消息（image_url data-URL）注入——
    /// OpenAI 协议 tool 消息只收文本，图走 user 消息（官方视觉示例同款）；
    /// Anthropic 协议转换器自动把 [tool, user(图)] 合并成单条 user
    /// [tool_result, image]（官方 tool_result 附图形态）。默认空 = 无图。
    pub images: Vec<String>,
    /// 文件类工具成功落盘后附的变更证据
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
    /// 带图返回：images 为绝对路径，模型循环负责读文件转 data-URL 注入。
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
    /// 门禁拒绝：工具**没有执行**（规则 deny / 用户拒绝 / 白名单外 / 预算触顶 /
    /// 熔断 / arguments 非法 JSON）。
    ///
    /// 走这里而不是 `warn`，是因为「被拒」对调度器和 trace 都是失败：用 `warn`
    /// 会被 `skill_on_step_post` 记成步骤成功、trace span 记成 ok，技能对外报完成。
    /// 文本原样透传——`audit::classify_tool_failure` 的分类（gate_denied /
    /// user_denied / skill_paused / fused）靠文案前缀，由调用方按语义写，
    /// 构造器不擅自改写给模型看的句子。
    pub fn denied(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refs: Vec::new(),
            status: ToolStatus::Denied,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }
    /// 附一条文件变更证据（edit_file/write_file 成功路径）。
    /// 只加字段不改语义——既有内置工具（BUILTIN_TOOL_COUNT 个）不受影响（默认空 Vec）。
    pub fn with_file_change(mut self, c: crate::bot_fs::FileChangeReceipt) -> Self {
        self.file_changes.push(c);
        self
    }
}

// From 过渡层已删：工具逐个迁移完成后删除。
//
// 迁移期间用 From impl 将工具返 (String, Vec<TaskRef>) 隐式转 ToolResult::ok；
// 迁移完成后所有工具已显式返 ToolResult，From 过渡层失去作用，删除。
// （若迁移期间遗留未迁移工具仍存在，该工具会爆「expected ToolResult」编译错——
// 这是期望的强制迁移信号，不是回归。）

pub struct ToolDef {
    pub name: &'static str,
    pub schema: &'static str,
    pub mutating: bool,
    /// 模型声称「我做了这个工具的语义动作」的常见表达。
    /// 只在 `mutating == true` 时有意义；非 mutating 填 `&[]`。
    /// `claims_mutation()` 从所有 mutating 工具的 patterns 合并后检索。
    pub claims_patterns: &'static [&'static str],
    /// 工具返回文本的回灌上限（字节，UTF-8）。超阈值把完整结果转存成溢出文件、
    /// 回灌节选 + 指针（见 bot_model_loop::effective_output_cap 与 overflow 模块）。
    /// 与设置页的全局 maxToolOutputBytes 取小后生效，两层都要。
    /// 默认 8192；长输出工具（read_text_file/fetch_url/list_files）自定 65536。
    pub max_output_bytes: usize,
    pub call: for<'a> fn(&'a ToolCtx<'a>, &'a str) -> ToolFuture<'a>,
}

// schema 常量（baseline 前缀一致）
pub const SCHEMA_QUERY_TASKS: &str = r##"{"type":"function","function":{"name":"query_tasks","description":"查询任务卡：不传 query=列清单（view 默认 active 未完成）；传 query=按关键词检索（匹配标题/备注/标签/子任务，此时 view 默认 all 全库）。输出行带（工作流：名称）标记，工作流相关问题可按标记汇总回答；定位任务不确定时先调本工具确认","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"关键词，可选；不传=列清单"},
    "view":{"type":"string","enum":["active","done","archived","trash","all"],"description":"视图范围，可选：active=未完成 / done=已完成未归档 / archived=已归档 / trash=回收站 / all=除回收站外全部；默认随 query 自动定（无 query=active，有 query=all）"},
    "tag":{"type":"string","description":"按标签过滤，可选"},
    "limit":{"type":"integer","description":"最多返回条数，可选，默认 50，上限 200"}
}}}}"##;
pub const SCHEMA_QUERY_SINGLE_TASK: &str = r##"{"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 创建时间/定时/所属工作流/依赖 + 归档/删除状态指示；白名单单点，区别于 query_tasks 批量清单与关键词检索；不要为凑清单循环调用本工具，批量场景一律用 query_tasks）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}}"##;
pub const SCHEMA_CREATE_TASK: &str = r##"{"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DDTHH:mm（T 分隔，空格分隔不合法），可选"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"追加绑定文件/文件夹（可选，追加不覆盖；最多 10 个，每卡一个文件夹，绑新文件夹替换旧的；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件，其余会被丢弃；要绑其它文件请引导用户用 bind_file 手选）"}
  },"required":["title"]}}}"##;
pub const SCHEMA_COMPLETE_TASK: &str = r##"{"type":"function","function":{"name":"complete_task","description":"完成任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；任务卡带【验收标准】且未逐条达标时不要调用）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id（来自用户消息的 [已选任务] 引用块或 query_tasks 输出），可选，优先于 title"},
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
    "due":{"type":"string","description":"新截止时间，可选；空串清除；格式 YYYY-MM-DD 或 YYYY-MM-DDTHH:mm（T 分隔）"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"追加绑定文件/文件夹（可选，追加不覆盖已有绑定；最多 10 个，每卡一个文件夹，绑新文件夹替换旧的；空数组 = 无操作，本工具不能清空绑定；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件，其余会被丢弃）"},
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
pub const SCHEMA_READ_TEXT_FILE: &str = r##"{"type":"function","function":{"name":"read_text_file","description":"读取本地文本文件内容（白名单目录内直接读，白名单外自动弹窗请用户授权；≤500 行的文件不传 limit 一次读完，offset/limit 分页只用于更大的文件；Office/PDF 用 extract_document，图片用户会直接发图）","parameters":{"type":"object","properties":{
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
pub const SCHEMA_LINK_FILE_TO_TASK: &str = r##"{"type":"function","function":{"name":"link_file_to_task","description":"把 AI_Gen_Files 目录内的文件或文件夹绑定到任务卡。任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内：登记到产物清单，流程结束、任务完成时落「通知中心」消息让用户勾选绑定（默认全选，每个文件绑一次）；任务未完成、中断、只有中间产物都不产生待绑定消息。普通对话内：直接绑定落卡，但必须指定一张已存在的任务卡（taskId 优先；title 需精确匹配唯一一张卡，本工具不能新建卡）。绑定一律追加，不覆盖卡上已有绑定；每卡最多一个文件夹（绑新文件夹会替换旧的），同一路径重复绑无效。","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title；普通对话内必须能命中已存在的卡"},
    "title":{"type":"string","description":"任务标题，无 taskId 时使用，须精确匹配一张已存在的卡"},
    "path":{"type":"string","description":"产物文件/文件夹绝对路径，必须在 AI_Gen_Files 目录内且已存在"},
    "kind":{"type":"string","enum":["final","intermediate"],"default":"final","description":"final=最终产物，intermediate=中间产物。仅任务执行流程的登记分支生效（参与流程结束的待绑定汇总）；普通对话直接绑定，本参数无效果。"}
  },"required":["path"]}}}"##;
pub const SCHEMA_EXTRACT_DOCUMENT: &str = r##"{"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件；长文档用 offset 参数续读后续部分；扫描版/图片型 PDF 文本层为空时自动转图走本地 OCR 兜底，前 20 页）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；返回里带『已截断』提示时用提示的 offset 值续读）"},
    "limit":{"type":"integer","description":"本页字符数（可选，默认 30000，上限 60000）"}
  }}}}"##;
pub const SCHEMA_CREATE_WORD: &str = r##"{"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{"template":{"type":"string","description":"模板名（设置页「Word 模板」已上传的）。用户指定了模板就传；未指定自动用默认模板（未设置默认则内置版式）；存在多个模板且用户意图不明时，先用 ask_user 问用户用哪个"},
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
pub const SCHEMA_CREATE_EXCEL: &str = r##"{"type":"function","function":{"name":"create_excel","description":"生成 Excel 到 AI_Gen_Files（单元格以 = 开头会写入原生公式如 =SUM(A1:A10)）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "sheets":{"type":"array","items":{"type":"object","properties":{
      "name":{"type":"string"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}}},
    "description":"工作表列表：name 表名、rows 二维数组"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["sheets"]}}}"##;
pub const SCHEMA_CREATE_PPT: &str = r##"{"type":"function","function":{"name":"create_ppt","description":"生成排版统一的 PPT 到 AI_Gen_Files（版式/配色/字体随模板：设置页可导入 .pptx 母版设默认；未导入时用内置专业母版，封面/章节页深底、内容页浅底、中文雅黑）。多版式：封面/目录/章节页/内容页/表格页/结束页；要点超 5 条自动拆页。需要设计感更强的自由版式（图表/双栏/大数字）时，若已安装 ppt 设计类技能（见技能清单）优先用技能。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"演示文稿主标题"},
    "template":{"type":"string","description":"模板名（设置页已导入的 .pptx 母版名，不含扩展名），可选；缺省用默认模板，未设默认时用内置母版。用户给过公司模板/指定过模板名时用"},
    "slides":{"type":"array","description":"幻灯片列表，按展示顺序；每页一个 type","items":{"type":"object","properties":{
      "type":{"type":"string","enum":["cover","toc","section","content","table","closing"],"description":"页面类型：cover 封面（title+subtitle）/ toc 目录（items 列表，缺省自动取各页标题）/ section 章节分隔页 / content 内容要点页 / table 表格页（rows 二维数组首行表头）/ closing 结束页"},
      "title":{"type":"string","description":"页面标题；content 页标题写结论式短句（如「营收同比 +23%」），禁「介绍」「概述」"},
      "subtitle":{"type":"string","description":"副标题（cover/section/closing 用）"},
      "bullets":{"type":"array","items":{"type":"string"},"description":"要点列表（content 页；硬规范 ≤5 条/页、每条 ≤40 字，超限自动拆页）"},
      "items":{"type":"array","items":{"type":"string"},"description":"目录条目（toc 页）"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}},"description":"表格数据（table 页；第一行是表头）"},
      "notes":{"type":"string","description":"演讲者备注/讲稿（本页口播内容，放这里不放正文），可选"}
    },"required":["type","title"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["slides"]}}}"##;
pub const SCHEMA_CREATE_PDF: &str = r##"{"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持，自动分页；自带页眉=文档标题与页脚页码）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选（同时用作每页页眉）"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表；#/##/### 前缀标记一/二/三级标题（如「## 市场分析」）"},
    "images":{"type":"array","items":{"type":"string"},"description":"要插入的图片绝对路径列表，可选（仅 AI_Gen_Files 目录内的已存在图片，其余被丢弃并提示；按顺序插在正文之后、表格之前；等比缩放到版心宽）"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_RUN_PYTHON: &str = r##"{"type":"function","function":{"name":"run_python","description":"执行 Python 代码（资源受限：CPU/内存/时长限额 + 独立临时目录，无文件系统隔离；默认超时 60s。默认需用户在设置页开启 Python 编程，授权模式为 yolo 时免开关）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"},
    "timeoutSecs":{"type":"integer","description":"超时秒数（可选，默认 60；大计算可调大，上限 1800）"}
  },"required":["code"]}}}"##;
pub const SCHEMA_START_SERVICE: &str = r##"{"type":"function","function":{"name":"start_service","description":"启动一个受管长驻进程（node/deno/python 等白名单启动器）。进程归宿主托管：活过本次工具调用，可查可停，宿主退出时自动收干净。适合 node MCP server、dev server、watcher 这类需要持续运行的东西；一次性脚本用 run_python。启动器必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman），不传 shell 字符串。默认需用户在设置页开启「允许机器人启动本地服务」，授权模式为 yolo 时免开关。服务继承宿主环境变量（含其中的密钥），只起自己写的代码","parameters":{"type":"object","properties":{
    "command":{"type":"string","description":"启动器名，必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman）"},
    "args":{"type":"array","items":{"type":"string"},"description":"参数列表，逐个传入，不做 shell 解析"},
    "cwd":{"type":"string","description":"工作目录（绝对路径），受文件授权闸约束；省略则用宿主当前目录"},
    "name":{"type":"string","description":"用途标签，便于人识别，如「前端 dev server」；可省"},
    "readyPattern":{"type":"string","description":"就绪判据：stdout 出现该正则才算启动成功。省略=进程起来 200ms 后仍活着即算成功"},
    "readyTimeoutSecs":{"type":"integer","description":"等待就绪的超时秒数，默认 30，上限 120"}
  },"required":["command"]}}}"##;
pub const SCHEMA_STOP_SERVICE: &str = r##"{"type":"function","function":{"name":"stop_service","description":"停止一个受管服务。走降级信号阶梯：SIGINT → 100ms → SIGTERM → 400ms → SIGKILL，并递归回收脱离进程组的子孙进程。幂等：已停止的服务再停一次返回 stopped=false，不报错。","parameters":{"type":"object","properties":{
    "serviceId":{"type":"string","description":"start_service 返回的服务 id"}
  },"required":["serviceId"]}}}"##;
pub const SCHEMA_LIST_SERVICES: &str = r##"{"type":"function","function":{"name":"list_services","description":"列出宿主托管的受管服务：id / 名称 / 命令 / 状态 / pid / 已运行秒数 / 退出码 / 最近输出尾部。用于确认服务是否还活着、拿服务 id 去 stop_service。","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_AST_INSPECT: &str = r##"{"type":"function","function":{"name":"ast_inspect","description":"对 Python 源码做符号级静态探查（走本机 Python 沙箱 + 标准库 ast，零新增依赖：资源受限、独立临时目录、默认 60s 超时，沙箱无文件系统隔离故调用前已过白名单闸；需设置页开启「允许机器人执行 Python」，或授权模式为 yolo 时免开关）。只支持 .py 文件，非 .py 会明确拒绝并让你改用 grep_files / read_text_file，不会静默退化成文本检索。四种模式：symbols 出符号树（类/函数/方法 + 行号）、defs 定位定义、refs 找全部引用（区分裸名与属性访问 + 所在函数）、callgraph 出调用链（按名字匹配，非作用域解析；depth 层）。调用链跨文件现扫不缓存：单次最多扫 200 个文件 / 2MB，跳过 .git/__pycache__/node_modules/venv 等目录，目录里指向外部的 symlink 一律排除；触及上限时返回 truncated=true，此时结果不完整","parameters":{"type":"object","properties":{
    "op":{"type":"string","enum":["symbols","defs","refs","callgraph"],"description":"探查模式：symbols=符号树 / defs=定义位置 / refs=全部引用 / callgraph=调用链"},
    "path":{"type":"string","description":"目标 .py 文件绝对路径（支持 ~ 开头）；op=callgraph 时可传目录，扫该目录下所有 .py"},
    "symbol":{"type":"string","description":"符号名（op 为 defs/refs/callgraph 时必填；类方法可只写方法名，如 resolve_with_perm）"},
    "depth":{"type":"integer","description":"callgraph 展开层数（可选，默认 1，上限 3）"}
  },"required":["op","path"]}}}"##;
pub const SCHEMA_UNPACK_ARCHIVE: &str = r##"{"type":"function","function":{"name":"unpack_archive","description":"解压压缩包到目录（支持 zip / tar / tar.gz，格式按文件头自动识别，不看后缀）。逐条校验条目路径：带 .. / 绝对路径 / 盘符 / 软链的条目会被跳过并列出（防压缩包把文件写到目标目录外）；单文件 64MB、整包 256MB、条目 5000 条上限，超限中止并报「疑似压缩炸弹」；目标已存在的文件不覆盖（会列出来）。解出的文件按「已生成」计入产物，可如实说「已解压出 N 个文件」","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"压缩包路径（须在读白名单内：桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs）"},
    "destDir":{"type":"string","description":"解压目标目录（须在可写目录内：AI_Gen_Files + 任务卡绑定文件夹 + allowedDirs，白名单外会弹确认）；不给则解到 AI_Gen_Files/解压/<包名>/"},
    "format":{"type":"string","enum":["auto","zip","tar","tar.gz"],"description":"压缩格式（可选，默认 auto 按文件头识别）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_PACK_ARCHIVE: &str = r##"{"type":"function","function":{"name":"pack_archive","description":"把目录或文件打包成压缩包（zip / tar.gz）：目录会连同目录名一起打包（zip -r 口径），软链跳过不跟随也不写入链接本身；输出到 AI_Gen_Files 或指定路径。打包完成按「已生成」计入产物，向用户报告时必须附上完整路径","parameters":{"type":"object","properties":{
    "sourcePath":{"type":"string","description":"要打包的目录或文件（须在读白名单内）"},
    "outputPath":{"type":"string","description":"输出压缩包路径（须在可写目录内，白名单外会弹确认）；不给则 AI_Gen_Files/<源名>.zip"},
    "format":{"type":"string","enum":["zip","tar.gz"],"description":"压缩格式（可选）：不给按输出路径后缀判定（.tar.gz / .tgz → tar.gz，否则 zip）"}
  },"required":["sourcePath"]}}}"##;
pub const SCHEMA_MANAGE_FILE: &str = r##"{"type":"function","function":{"name":"manage_file","description":"文件复制 / 移动 / 删除。copy 复制（源保留）、move 移动（源消失，同卷直接重命名、跨卷复制后源入回收站）、delete 删除（进系统回收站，删前弹窗请用户确认，60s 无响应默认拒绝）。源路径须在读白名单内（桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs），目标路径须在可写目录内（AI_Gen_Files + 任务卡绑定文件夹 + allowedDirs，白名单外弹确认）；目标已存在一律不覆盖；目录树里的符号链接既不跟随也不复制。copy/move 的目标按「已生成」计入产物，报告时必须附上完整目标路径","parameters":{"type":"object","properties":{
    "action":{"type":"string","enum":["copy","move","delete"],"description":"操作类型：copy=复制 / move=移动 / delete=删除（进回收站）"},
    "path":{"type":"string","description":"源路径（绝对路径，支持 ~ 开头；copy/move 的源、delete 的目标）"},
    "dest":{"type":"string","description":"目标路径（copy/move 必填；delete 不需要）"}
  },"required":["action","path"]}}}"##;
pub const SCHEMA_COPY_FILE_TO_CLIPBOARD: &str = r##"{"type":"function","function":{"name":"copy_file_to_clipboard","description":"把本地文件放进系统剪贴板：用户在微信 / 飞书 / 邮件等应用里按 Cmd+V（Windows Ctrl+V）就能把这个文件当附件粘出去——发文件给别人的最短路径。会覆盖剪贴板原内容。路径须在读白名单内（桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs，白名单外弹授权）。一次可放多个文件（最多 20 个），用户在目标应用里粘贴出来就是这批文件","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"单个文件路径（绝对路径，支持 ~ 开头）；与 paths 二选一"},
    "paths":{"type":"array","items":{"type":"string"},"description":"多个文件路径（最多 20 个）；与 path 二选一"}
  }}}}"##;
pub const SCHEMA_SCHEDULE_REMINDER: &str = r##"{"type":"function","function":{"name":"schedule_reminder","description":"设一个到点提醒：到点用系统通知提醒用户（只说话，不建任务卡、不执行任何内容）。用户说「30分钟后提醒我 X」「明天9点提醒我 Y」「每天早上提醒我 Z」时用它。when 支持三种写法：绝对时间 2026-10-22T09:00、今天 HH:MM（今天已过则顺延到明天）、相对现在 +30m / +2h / +1d；repeat 决定重复频率（none 仅一次 / daily 每天 / weekly 每周 / monthly 每月，重复时按 when 的时刻与星期、日序锚定）。设完返回提醒 id，可配合 list_reminders / cancel_reminder 管理","parameters":{"type":"object","properties":{
    "text":{"type":"string","description":"提醒内容（要提醒用户什么，一句话）"},
    "when":{"type":"string","description":"提醒时间：绝对时间 2026-10-22T09:00 / 今天 HH:MM / 相对现在 +30m、+2h、+1d"},
    "repeat":{"type":"string","enum":["none","daily","weekly","monthly"],"default":"none","description":"重复频率：none=仅一次（默认）/ daily=每天 / weekly=每周 / monthly=每月"}
  },"required":["text","when"]}}}"##;
pub const SCHEMA_LIST_REMINDERS: &str = r##"{"type":"function","function":{"name":"list_reminders","description":"列出当前已设的全部提醒（含提醒 id、下次触发时间与启用状态），用于回答「我有哪些提醒」或拿到 cancel_reminder 需要的 id","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_CANCEL_REMINDER: &str = r##"{"type":"function","function":{"name":"cancel_reminder","description":"取消一条已设的提醒（按提醒 id，id 用 list_reminders 查）。取消后到点不再通知","parameters":{"type":"object","properties":{
    "id":{"type":"string","description":"提醒 id（list_reminders 返回）"}
  },"required":["id"]}}}"##;
pub const SCHEMA_LIST_WINDOWS: &str = r##"{"type":"function","function":{"name":"list_windows","description":"列出可接管的桌面窗口（应用名 + 窗口标题 + 窗口 id）。UI 自动化的第一步：先拿窗口 id，再对它做 inspect / click / type。只列有标题的窗口；需要先在设置页开启「允许机器人操作桌面界面」","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_ACTIVATE_WINDOW: &str = r##"{"type":"function","function":{"name":"activate_window","description":"把指定窗口切到前台（后续 click / type 都作用在这个窗口上）","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"}
  },"required":["windowId"]}}}"##;
pub const SCHEMA_UI_INSPECT: &str = r##"{"type":"function","function":{"name":"ui_inspect","description":"读窗口里的控件树（角色 / 标题 / 当前值 / 是否可用 / 是否可直接触发），可按关键词过滤。点击与输入前先用它确认目标控件的 id","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "query":{"type":"string","description":"可选：按控件标题 / 角色 / 标识过滤（不填=全部）"},
    "maxDepth":{"type":"integer","description":"可选：树深度上限，默认 6，最大 16（网页内容控件较深，默认看不到时可显式给 16）"}
  },"required":["windowId"]}}}"##;
pub const SCHEMA_UI_CLICK: &str = r##"{"type":"function","function":{"name":"ui_click","description":"点击窗口里的某个控件（按控件 id 或标题关键词定位）。点「发送 / 提交 / 支付 / 删除」这类按钮前会弹窗请用户确认，用户拒绝则不执行——真发出去的消息撤不回来，所以这一步必须让用户过目","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "target":{"type":"string","description":"控件 id（ui_inspect 返回）或控件标题关键词"}
  },"required":["windowId","target"]}}}"##;
pub const SCHEMA_UI_TYPE: &str = r##"{"type":"function","function":{"name":"ui_type","description":"往窗口里的输入框输入文本（按控件 id 或标题关键词定位）。密码 / 验证码类控件不代填。输入完不会自动发出去——发送是另一次 ui_click。文本里不能带换行符：换行等于按回车键，目标应用会当场发出去","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "target":{"type":"string","description":"控件 id（ui_inspect 返回）或控件标题关键词"},
    "text":{"type":"string","description":"要输入的文本"}
  },"required":["windowId","target","text"]}}}"##;
pub const SCHEMA_WAIT_FOR_ELEMENT: &str = r##"{"type":"function","function":{"name":"wait_for_element","description":"等待窗口里出现某个控件（按标题 / 角色关键词），等到就返回它的 id，超时没出现就如实说明。用于「等对方发来消息再回复」这类时序","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "query":{"type":"string","description":"要找的控件标题 / 角色关键词"},
    "timeoutMs":{"type":"integer","description":"可选：等待上限毫秒，默认 5000，最大 30000"}
  },"required":["windowId","query"]}}}"##;
pub const SCHEMA_WEB_SEARCH: &str = r##"{"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（配置 Tavily 或 Brave key 时走对应 API、双开报错，否则 Bing+百度网页抓取；返回标题/链接/摘要；润色/翻译/改写用户已给的文本不需要搜索）","parameters":{"type":"object","properties":{
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
/// create_skill：机器人自建技能（SKILL.md + 可选脚本/模板附件），创建即生效并重建路由
pub const SCHEMA_CREATE_SKILL: &str = r##"{"type":"function","function":{"name":"create_skill","description":"把本次对话中沉淀的可复用流程固化为技能（skill），长期生效、重启保留。适合：用户明确说「把这个流程存成技能/记住这个做法」，或同一多步流程第二次被手动描述时。content 为完整 SKILL.md 文本（YAML frontmatter 必含 name 与 description；可声明 risk_level/mode/max_steps/intents 触发关键词；步骤用 ## 标题分节）。创建后立即生效：用户说出 intents 关键词即进入该技能。同名技能已存在、或 description/intents 命中危险关键词会被拒绝","parameters":{"type":"object","properties":{"name":{"type":"string","description":"技能名：仅字母/数字/-/_"},"content":{"type":"string","description":"完整 SKILL.md 文本（frontmatter + 正文步骤）"},"files":{"type":"array","description":"可选附件（脚本/模板），路径相对技能目录","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 scripts/run.py"},"content":{"type":"string","description":"文件文本内容"}}}}},"required":["name","content"]}}}"##;

/// install_skill：从远程源安装技能（GitHub 仓库 / 腾讯 SkillHub / zip 直链），装上即生效
pub const SCHEMA_INSTALL_SKILL: &str = r##"{"type":"function","function":{"name":"install_skill","description":"从远程源把技能（skill）装到本机技能目录，装上即生效并进入「已安装技能」清单。用户说「安装/装一下 XX 技能」「把这个 GitHub 仓库装成技能」「从 SkillHub 装 XX」时用它。source 三种写法：① GitHub 仓库地址（https://github.com/owner/repo；仓库里含多个技能时用 skill 参数指定技能目录名，或 source 直接写 owner/repo#技能名）② 腾讯 SkillHub 技能页地址（https://skillhub.cn/skills/xxx）③ zip 直链。安装会弹确认窗等用户点头；技能名不合法（如中文）时自动改用来源里的 slug 作技能名。装完后续用 use_skill 读取并执行","parameters":{"type":"object","properties":{
"source":{"type":"string","description":"技能来源：GitHub 仓库地址 / SkillHub 技能页地址 / zip 直链 / owner/repo#技能名"},
"skill":{"type":"string","description":"可选：仓库内的技能目录名——仓库含多个技能时必填（如 skill-hub-query）"}
},"required":["source"]}}}"##;

/// search_skills：在腾讯 SkillHub 上搜可安装的技能（公开 API，免鉴权）
pub const SCHEMA_SEARCH_SKILLS: &str = r##"{"type":"function","function":{"name":"search_skills","description":"在腾讯 SkillHub（中文技能商店）搜索可安装的技能。用户说「找个能做 XX 的技能」「SkillHub 上有没有 XX」「装个 XX 技能」且还不知道具体技能名时先搜它。返回候选的名称、简介、slug、热度，以及可直接交给 install_skill 的安装地址。搜完先把候选列给用户挑，用户选定再调 install_skill 装","parameters":{"type":"object","properties":{
"keyword":{"type":"string","description":"搜索关键词（中文或英文，如「周报」「PDF 处理」「前端」）"},
"limit":{"type":"integer","description":"返回条数，可选，默认 8，最多 20"}
},"required":["keyword"]}}}"##;

pub const SCHEMA_USE_SKILL: &str = r##"{"type":"function","function":{"name":"use_skill","description":"读取已安装技能（skill）的完整文档并按文档步骤执行。任务涉及的每个相关技能都要读（可多次调用）：例如做 PPT 时，若清单里同时有编排、生成、配色、风格类技能，应逐个读取、取长补短综合运用，不要只读一个","parameters":{"type":"object","properties":{
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"},
    "params":{"type":"object","description":"技能参数（可选；键=参数名，值=字符串）。技能声明了必填参数时必须提供（缺失会拒绝启动并列出缺什么），声明了默认值的参数可省略","additionalProperties":{"type":"string"}}
  },"required":["name"]}}}"##;
// 程序 M2（聊天即 IDE）：apps_list 防重名 / apps_create 落草稿 / apps_update_files 整文件集修改
pub const SCHEMA_APPS_LIST: &str = r##"{"type":"function","function":{"name":"apps_list","description":"列出当前所有程序（草稿+已启用，不含已归档），每条含 id/名称/状态/一句话说明。创建新程序前必调：防 id 重名、避免做出功能重复的程序","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_APPS_CREATE: &str = r####"{"type":"function","function":{"name":"apps_create","description":"把写好的确定性 Python 小程序登记为「程序」（草稿态）：脚本与附加文件落盘 data/apps/<id>/，用户在程序页试跑，满意后再上架。适合每月重复的固定流程（工资单匹配、名册查询等）。创建前先 apps_list 防重名；规格含糊先用 ask_user 问清、用 read_text_file 看用户给的真实样例。脚本契约（必须遵守）：python3 零依赖为佳；开头是校验段——检查 input/ 文件存在、期望列名齐全，对不上向 stderr 打「❌+中文原因」并 sys.exit(2)，宁可响亮失败不许静默空跑；用 sys.argv[1]（app 目录绝对路径）解析 data/ input/，产物写 argv[2]（本次运行产物目录 output/<run_id>/，运行器已建好），不依赖 cwd；最后一行必须是 print(\"##RESULT## \" + json.dumps({\"summary\":\"人话总结\"}, ensure_ascii=False))，哨兵行前必须有换行，summary 控制在 300 字内；禁联网；敏感数据（身份证号等）放 data/ 配置文件，不硬编码在脚本里","parameters":{"type":"object","properties":{"id":{"type":"string","description":"程序 id=目录名：仅字母/数字/-/_，不能含 / 或 .."},"name":{"type":"string","description":"程序页卡片标题，≤200 字"},"description":{"type":"string","description":"一句话说明（卡片副标题），≤500 字"},"icon":{"type":"string","description":"卡片 emoji 图标，1-4 字符，缺省 📦"},"input_schema":{"description":"输入声明：{\"kind\":\"folder\"}（文件夹模式，推荐）或 {\"kind\":\"form\",\"fields\":[{\"key\",\"label\",\"type\":\"text|number|date\",\"required\",\"placeholder\"}]}；缺省=文件夹模式"},"script":{"type":"string","description":"完整 app.py 源码全文（不许片段或 diff）"},"files":{"type":"array","description":"可选附加文件，只允许 data/ 之下（如姓名-身份证对照表 csv）","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 data/roster.csv"},"content":{"type":"string","description":"文件全文"}}}},"manual":{"type":"string","description":"给用户看的使用说明：这个程序做什么、要放什么文件、点哪里、出什么，≤8000 字"},"timeoutSecs":{"type":"integer","description":"声明这个程序单次运行需要多久（秒，可选）：耗时随输入规模增长的活（转写长录音、装依赖、批处理）必须在这里声明，否则运行按内置默认 60 秒会被中途掐掉；上限 1800"}},"required":["id","name","script"]}}}"####;
pub const SCHEMA_APPS_UPDATE_FILES: &str = r####"{"type":"function","function":{"name":"apps_update_files","description":"修改已有程序（整文件集提交）：改脚本必须传完整新 script 全文（不做增量合并、不收 diff）；也可改输入声明/名称/说明/图标/说明书/单次运行时长。status=\"enabled\" 表示上架启用（务必等用户在聊天里明确满意后再传），status=\"draft\" 回草稿；归档是用户在程序页的主动操作，工具不受理。脚本契约同 apps_create（开头校验段 + ##RESULT## 哨兵收尾 + argv[1] 解析目录 + 产物写 argv[2] + 禁联网）。程序不存在或已归档会被拒绝","parameters":{"type":"object","properties":{"id":{"type":"string","description":"要修改的程序 id（先 apps_list 查）"},"script":{"type":"string","description":"完整 app.py 源码全文；不传=脚本不动"},"files":{"type":"array","description":"附加文件（只允许 data/ 之下），同名整体覆盖","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 data/roster.csv"},"content":{"type":"string","description":"文件全文"}}}},"name":{"type":"string","description":"新标题；不传=保持不变"},"description":{"type":"string","description":"新说明；不传=保持不变"},"icon":{"type":"string","description":"新 emoji 图标；不传=保持不变"},"input_schema":{"description":"输入声明（对象或 JSON 串）；不传=保持不变"},"manual":{"type":"string","description":"新使用说明；不传=保持不变"},"timeoutSecs":{"type":"integer","description":"改声明的单次运行时长（秒）：不传=保持不变，传 0=取消声明（回落内置默认 60 秒），传 N=声明 N 秒（上限 1800）"},"status":{"type":"string","description":"draft 或 enabled（enabled=上架启用，需用户先明确满意）"}},"required":["id"]}}}"####;
// 工作流执行提问（主可见）
pub const SCHEMA_ASK_USER: &str = r##"{"type":"function","function":{"name":"ask_user","description":"向用户提一个问题并等待回答（仅任务执行中可用：工作流节点与手动执行的任务卡；聊天中不要用本工具，直接文字提问即可。提问会进通知中心，用户可能几小时后才回答）。必须携带 assumption=你的推荐假设：用户不回答时按假设继续，给不出假设的问题不许问。仅当缺关键信息且无法从任务卡/上游产出/附件推断时才用；预算为全局设置（1–5，默认 3，工作流与任务卡共用），超预算直接按假设继续","parameters":{"type":"object","properties":{
    "question":{"type":"string","description":"要问的问题，≤200 字，具体明确"},
    "why":{"type":"string","description":"一句话说明为什么要问，可选"},
    "options":{"type":"array","items":{"type":"string"},"description":"候选项（可选，≤4 个、每个 ≤40 字），用户可点选也可自由回答"},
    "assumption":{"type":"string","description":"你的推荐假设，必填——用户未回答时按它继续执行"}
  },"required":["question","assumption"]}}}"##;
// 子 agent 编排三工具（主 agent 可见）
pub const SCHEMA_SPAWN_SUBAGENT: &str = r##"{"type":"function","function":{"name":"spawn_subagent","description":"派发受管子 agent 执行单一目标长任务（非阻塞，立即返回 subagentId/status）。适用：预计超 5 轮工具调用、多来源调研、写代码跑脚本、用户要求后台/并行。objective 单一目标；acceptanceCriteria 必填且每条可检验（不要写「调研清楚」，要写「覆盖至少 5 个产品，每个含官网 URL，输出 report.md」）；contextSummary 只给必要背景，不要倒主对话全文。完成后用 check_subagent 轮询结果再汇总。同一会话里重复派发完全相同的目标（档位/目标/验收标准全同）不会建第二条活行：返回同一个 subagentId，继续 check 它即可","parameters":{"type":"object","properties":{
    "objective":{"type":"string","description":"单一目标（一句话说清做什么）"},
    "onion":{"type":"string","description":"per-onion 配置档名（可选）：probe=只读探查产出素材 / writereditor=撰写与写代码 / validator=只读验收。指定后本子 agent 的工具会被该配置收窄、角色与轮数上限一并生效，且服务端会拒绝配置清单外的工具——这是控制子 agent 权限的唯一入口。三段式流水线（探查→撰写→验收）就靠它分别派发"},
    "profile":{"type":"string","enum":["research","coder","general"],"description":"工具档位：research=联网调研写报告 / coder=读代码跑 Python 写文件 / general=两者并集（与 stage 同给时白名单档优先，stage 只换角色语义层）。不给时按 onion 配置的 profile 派生；两者都不给则按 stage 派生（recon/verifier→research，executor→coder），都没有则报错"},
    "stage":{"type":"string","enum":["recon","executor","verifier"],"description":"三段式派发阶段（可选）：recon=只读调研 / executor=基于报告动手实现 / verifier=逐条验收。不给 stage 时仅按 profile 走基线提示词；只给 stage 不给 profile 时按默认派生（recon/verifier→research，executor→coder）"},
    "acceptanceCriteria":{"type":"array","items":{"type":"string"},"minItems":1,"description":"验收标准，每条可检验"},
    "contextSummary":{"type":"string","description":"必要上下文摘要（可选；不含主对话全文）"},
    "budget":{"type":"object","properties":{"maxTurns":{"type":"integer","description":"工具循环轮数上限，默认 30，硬顶 50"},"maxToolCalls":{"type":"integer","description":"累计工具调用上限；整个 budget 不给时取全局熔断配置（设置页参数 loop.max_function_calls），此处显式给值才覆盖"},"maxWallSeconds":{"type":"integer","description":"墙钟秒数上限，默认 600（排队不计）"}},"description":"预算三硬顶（可选；不给则按上示默认与全局配置取值）"},
    "modelProfile":{"type":"string","description":"指定模型名（可选；未指定/无效回退当前激活模型）"},
    "parentTaskId":{"type":"string","description":"主卡 id（可选；编排计划卡，缺卡报错）"}
  },"required":["objective","acceptanceCriteria"]}}}"##;
pub const SCHEMA_CHECK_SUBAGENT: &str = r##"{"type":"function","function":{"name":"check_subagent","description":"查询子 agent 状态（幂等可轮询）：status=queued|running|succeeded|failed|cancelled|budget_exceeded + 进度 + 收尾结果（summary/artifacts/blockers）。waitMs 是可放心给大的等待预算（上限 600000，即 10 分钟）：服务端内部递进等待，子任务一转入终态立刻返回，不必自己反复短轮询","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id（spawn 返回值）"},
    "waitMs":{"type":"integer","description":"等待预算毫秒数（可选，0=立即返回，上限 600000）。一次给足比反复小值轮询省轮次：每查一次你都要重新想一轮"}
  },"required":[]}}}"##;
pub const SCHEMA_CANCEL_SUBAGENT: &str = r##"{"type":"function","function":{"name":"cancel_subagent","description":"取消子 agent：置 cancelled 并停止其执行；已终态则 no-op。用户说「停掉/不用继续了」且能对应到在途子 agent 时用","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id（spawn 返回值）"},
    "reason":{"type":"string","description":"取消原因（可选）"}
  },"required":[]}}}"##;
// 子 agent 白名单内工具（不进主 agent schema）
pub const SCHEMA_WRITE_ARTIFACT_FILE: &str = r##"{"type":"function","function":{"name":"write_artifact_file","description":"把文本内容写进自己的产物目录（gen_dir/subagents/{subagentId}/，报告/中间结果落地用）。filename 为纯文件名（不含路径分隔符），已存在会自动加 (n) 序号不覆盖","parameters":{"type":"object","properties":{
    "filename":{"type":"string","description":"纯文件名（不含路径分隔符 / 与 \\、不允许 .. 路径段与 NUL；Windows 保留设备名如 CON/NUL 拒绝），如 report.md / data.csv；同名自动加 (n) 序号"},
    "content":{"type":"string","description":"完整文本内容"}
  },"required":["filename","content"]}}}"##;
pub const SCHEMA_READ_OWN_CARD: &str = r##"{"type":"function","function":{"name":"read_own_card","description":"重读自己的执行契约（objective / acceptanceCriteria / contextSummary / budgetUsage 实时余量 / status）：每轮开始建议先调，契约或余量有变化则调整计划；status=cancelled = 派发已被取消，立即停止新探索并收尾","parameters":{"type":"object","properties":{}}}}"##;
// 电脑辅助 Tier1（只「看」与「打开」，无鼠标键盘）
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
// 文件编辑（写白名单 ≠ 读白名单，白名单外逐次确认）
pub const SCHEMA_EDIT_FILE: &str = r##"{"type":"function","function":{"name":"edit_file","description":"对文本文件做字符串替换（默认 literal 精确模式；可传 mode=regex 启用正则全部匹配替换；详见参数说明）。literal 模式小步修改首选（先 read_text_file 确认原文再改）：oldString 必须与文件内容一致且唯一（多处命中报错；行尾空白/CRLF 差异自动容错）。改完建议 read_text_file 复核","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；白名单外会弹确认）"},
    "oldString":{"type":"string","description":"要替换的原文：literal 模式=精确匹配（须唯一；含足够上下文）；regex 模式=正则模式串（全部匹配替换）"},
    "newString":{"type":"string","description":"替换后的新文本：literal 模式=可为空串=删除该段；regex 模式=可用 $1/$2 命名/编号捕获组引用，replace 语义与 sed g 一致"},
    "mode":{"type":"string","enum":["literal","regex"],"default":"literal","description":"匹配模式：literal=精确（默认；oldString 须唯一；多处命中报错）；regex=正则（oldString 当作 Rust regex 模式串，newString 支持 $1/$2 捕获组，全部命中替换；非法 pattern 报错；空替换会把文件清空时拒绝并改用 write_file）"}
  },"required":["path","oldString","newString"]}}}"##;
pub const SCHEMA_WRITE_FILE: &str = r##"{"type":"function","function":{"name":"write_file","description":"创建新文件或整体写入内容（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；覆盖已存在文件需要用户确认）。修改已有文件优先用 edit_file（精确替换更安全）。是否生成成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"目标文件绝对路径（父目录必须已存在）"},
    "content":{"type":"string","description":"完整文件内容（UTF-8 文本；不能含 NUL 字节；上限 2MB）"}
  },"required":["path","content"]}}}"##;
// 适配器（统一签名，按需拆 ctx 字段）
// link_file_to_task 是 async fn，必须 .await——拆出来后已修正。
// list_tasks/search_tasks 合并为 query_tasks：收 args。
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

fn call_start_service<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    let app = ctx.app;
    // 权限档与路径闸住在 Wry 层（bot-config 读写、bot_fs 授权弹窗都是 Wry 专属），
    // 这里算好传进去：工具本体因此不绑 Runtime，能在 mock runtime 下被真调用
    // （与调度器 core 注入 executor 同款思路）。
    let yolo = crate::bot::perm_mode(app) == crate::bot::PermMode::Yolo;
    // 闭包自带所有权：注入的 future 必须是 'static（工具本体不绑 Runtime）
    let app_for_cwd = app.clone();
    let sid_for_cwd = session_id.map(|s| s.to_string());
    Box::pin(async move {
        tool_start_service(app, args, session_id, yolo, move |p| {
            let app = app_for_cwd.clone();
            let sid = sid_for_cwd.clone();
            Box::pin(async move {
                crate::bot_fs::resolve_with_perm(
                    &app,
                    "start_service",
                    &p,
                    interactive,
                    sid.as_deref(),
                )
                .await
            })
        })
        .await
    })
}

fn call_stop_service<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_stop_service(ctx.app, args).await })
}

fn call_list_services<'a>(ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_list_services(ctx.app) })
}

fn call_ast_inspect<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    let stop = ctx.stop;
    Box::pin(async move {
        crate::py::ast::tool_ast_inspect(ctx.app, args, interactive, session_id, stop).await
    })
}

fn call_unpack_archive<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_archive::tool_unpack_archive(&app, &args, interactive, session_id.as_deref())
            .await
    })
}

fn call_pack_archive<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_archive::tool_pack_archive(&app, &args, interactive, session_id.as_deref()).await
    })
}

fn call_manage_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_manage_file::tool_manage_file(&app, &args, interactive, session_id.as_deref())
            .await
    })
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

/// 任务执行提问——引擎在 ask_engine（oneshot 等待 + 假设兜底），
/// 超时口径 ASK_TIMEOUT_SECS（默认 24h），测试可传短超时直打引擎。
/// ctx.stop 透传/stop 必须能打断 24h 等待，不能挂死模型循环）。
fn call_ask_user<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app;
    let session_id = ctx.session_id;
    let stop = ctx.stop;
    Box::pin(async move {
        crate::ask_engine::engine_ask_user(
            app,
            session_id,
            args,
            std::time::Duration::from_secs(crate::ask_engine::ASK_TIMEOUT_SECS),
            stop,
        )
        .await
    })
}

fn call_create_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_skill(ctx.app, args, ctx.interactive, ctx.session_id).await })
}

fn call_install_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move {
        crate::bot_skills::tool_install_skill(ctx.app, args, ctx.interactive, ctx.session_id).await
    })
}

fn call_search_skills<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_skills::tool_search_skills(ctx.app, args).await })
}

fn call_apps_list<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_list(ctx.app, args).await })
}

fn call_apps_create<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_create(ctx.app, args).await })
}

fn call_apps_update_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_update_files(ctx.app, args).await })
}

fn call_use_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { tool_use_skill(ctx.app, args, session_id) })
}

// 适配器：编排三工具 + 子 agent 白名单工具
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
    // 等待期要能被 /stop 打断（waitMs 上限已到 10 分钟，不接就是锁死停止键）：
    // StopGuard 借用活不过闭包，取 owned 令牌下传。
    let stop = ctx.stop.map(|g| g.token());
    // 会话 id 跟着走：问到终态要记进「父会话已吸收」去重表，否则 digest 与
    // 自动汇报轮会把同一份结果再念一遍。
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_orchestrator::tool_check_subagent(&app, &args, stop, session_id).await
    })
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

// 电脑辅助 Tier1 适配器
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

fn call_copy_file_to_clipboard<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_desktop::tool_copy_file_to_clipboard(ctx.app, args, interactive, session_id)
            .await
    })
}

fn call_schedule_reminder<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_schedule_reminder(ctx.app, args).await })
}

fn call_list_reminders<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_list_reminders(ctx.app, args).await })
}

fn call_cancel_reminder<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_cancel_reminder(ctx.app, args).await })
}

fn call_list_windows<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_list_windows(ctx.app, args).await })
}

fn call_activate_window<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_activate_window(ctx.app, args).await })
}

fn call_ui_inspect<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_ui_inspect(ctx.app, args).await })
}

fn call_ui_click<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_ui::tool_ui_click(ctx.app, args, interactive, session_id).await },
    )
}

fn call_ui_type<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_ui::tool_ui_type(ctx.app, args, interactive, session_id).await },
    )
}

fn call_wait_for_element<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_wait_for_element(ctx.app, args).await })
}

fn call_screenshot<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_screenshot(ctx.app, args).await })
}

// 文件编辑适配器
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

/// 合并 TOOLS_TABLE 中所有 mutating 工具的 claims_patterns，
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
///
/// 命中点带**领域豁免**：命中处前后 12 字内出现「剪贴板」时这次命中不算变更声称。
/// 「已复制到剪贴板」说的是系统剪贴板（clipboard_write / copy_file_to_clipboard
/// 都是非变更类工具），而 manage_file 的短动词「已复制」是它的子串——不豁免就会把
/// 「把这段文字复制到剪贴板」当成「文件复制」，凭空打回模型并给用户挂一条
/// 「这句没有对应凭证」的更正。剪贴板只改系统剪贴板，不改文件与任务卡，
/// 语义上确实不是变更。文件复制话术（「已复制：/a → /b」）不与「剪贴板」同现，
/// 照常命中。
pub fn claims_mutation(text: &str) -> bool {
    let patterns: Vec<&str> = TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .flat_map(|t| t.claims_patterns.iter())
        .copied()
        .collect();
    let chars: Vec<char> = text.chars().collect();
    // 阶段 1：子串匹配
    for p in &patterns {
        for (s, e) in find_occurrences(&chars, p) {
            if !claim_exempt(&chars, s, e) {
                return true;
            }
        }
    }
    // 阶段 2：「已」后 8 字窗口包含任一 pattern（strip 前缀「已」后）
    // 原 bot_model_loop.rs VERBS 表不含「已」前缀（「删除」不是「已删除」），
    // 故 window 检查也需剥去 pattern 的前缀「已」才能覆盖「已...移除」类变体。
    for (i, &c) in chars.iter().enumerate() {
        if c != '已' {
            continue;
        }
        let w_end = (i + 1 + 8).min(chars.len());
        let window = &chars[i + 1..w_end];
        for p in &patterns {
            let stripped = p.strip_prefix('已').unwrap_or(p);
            for (ws, we) in find_occurrences(window, stripped) {
                // 窗口内位置换算回原文位置再判豁免
                let abs_s = i + 1 + ws;
                let abs_e = i + 1 + we;
                if !claim_exempt(&chars, abs_s, abs_e) {
                    return true;
                }
            }
        }
    }
    false
}

/// 命中点的领域豁免：前后 12 字窗口内出现「剪贴板」则这次命中不算变更声称。
///
/// 窗口取 12 字是够用的下限：剪贴板话术的常见形态（「已复制到剪贴板」、
/// 「文件已放入剪贴板」）里动词与「剪贴板」的距离都在 6 字内；12 字保证
/// 「已复制 /a 到 /b，并把说明复制到剪贴板」这种前后两处并存时，文件复制那处
/// 仍照常命中。
const CLAIM_EXEMPT_WINDOW: usize = 12;
/// 非变更语义领域标记：系统剪贴板（只改剪贴板，不改文件与任务卡）。
const CLAIM_EXEMPT_MARK: &str = "剪贴板";

fn claim_exempt(chars: &[char], start: usize, end: usize) -> bool {
    let lo = start.saturating_sub(CLAIM_EXEMPT_WINDOW);
    let hi = (end + CLAIM_EXEMPT_WINDOW).min(chars.len());
    chars[lo..hi]
        .iter()
        .collect::<String>()
        .contains(CLAIM_EXEMPT_MARK)
}

/// pattern 在 chars 里的全部出现位置（字符下标，半开区间，不重叠）。
fn find_occurrences(chars: &[char], pattern: &str) -> Vec<(usize, usize)> {
    let pat: Vec<char> = pattern.chars().collect();
    if pat.is_empty() || pat.len() > chars.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + pat.len() <= chars.len() {
        if chars[i..i + pat.len()] == pat[..] {
            out.push((i, i + pat.len()));
            i += pat.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 执行类谎报的完成态措辞。
///
/// 与 `claims_mutation` 的词表分家而非合并：变更话术能逐工具派生（「删除」属
/// delete_task、「勾选」属 toggle_subtask），而「跑完了 / 工具返回」描述的是
/// 「调用工具」这个动作本身，与调的是哪个工具无关——挂到 ToolDef 上只能给同一
/// 组词抄 N 遍。故此处用共享表；加新工具同样不需要改检测逻辑。
///
/// 刻意不收「已完成 / 已完成搜索 / 分析已完成」这类中性收尾语——模型在一次
/// 工具都没调用的对话里也会自然说「分析已完成」，收了会把正常答复打回；
/// 「打印了」同理，用户粘一段输出问「这句什么意思」时模型复述「打印了 done」
/// 是诚实回答，不是谎报。
const EXECUTION_CLAIM_PATTERNS: &[&str] = &[
    // 跑了某件事的完成态
    "执行完毕",
    "运行完毕",
    "跑完了",
    // 「已跑完」与「跑完了」是同一个完成态的两种语序，漏了它整道闸对
    // 「已跑完，输出 5050」这种最常见的说法是瞎的
    "已跑完",
    "已执行",
    "已经执行",
    "已运行",
    // 工具给了结果（工具当主语——用户粘来的输出不会被叙述成「工具返回」）
    "工具返回",
    "返回结果是",
    "运行结果",
    "执行结果",
    "已打印",
    // 分步叙述「我按步骤跑完了」。实测编造原话（trace 51，零工具调用）：
    // 「完成计划2 步：第一步 X 触发 Y，第二步 Z 仍输出 W」。这几句对代码讲解
    // 不成立——讲解会说「可以分两步做」，不会说「完成计划 2 步」/「仍输出」。
    "完成计划",
    "已运行步骤",
    "仍输出",
    // 第一人称完成态。实测绕过的原话（trace 63，零凭证放行）：
    // 「我跑了一个 print 100 的小任务，返回了 100」——「我跑了」不匹配
    // 「跑完了」，整句没有第二个触发词点，于是不进判决。与「跑完了」的差别
    // 只在完成态后缀上，模型日常的说法反而是这一支。
    "我跑了",
    "我执行了",
    "我运行了",
    // 实测绕过的原话（trace 65，零凭证放行，且 spawn_subagent 也没调过）：
    // 「子 agent 跑 sleep(5) 实测约 5013 ms」——数字照抄上一轮真实回复的
    // 格式编的。刻意只收「约/为/结果」这几种结句，不收裸「实测」：模型引用
    // 用户粘来的计时输出时说的是「实测耗时: 2005.08 ms」（照抄原文形态），
    // 编造时说的是「实测约 X」（要显得可信又不绝对）。
    "实测约",
    "实测为",
    "实测结果",
    "跑出来",
    // 模型把工具调用结果列成清单的写法：零工具调用却报「✅ 调用成功」。
    // 实测原话形如「工具调用如下：- create_word：✅ 调用成功，返回路径 X」——
    // 上面那批词一个都不命中，整道执行类闸不开门（点名判定轮不到，词表是第一环）。
    // 只收完成态的四种：失败/被拒是相反形态（不含这些词），如实汇报失败不受影响。
    "调用成功",
    "执行成功",
    "调用结果",
    "调用完毕",
    "调用完成",
    // 刻意不收的一批（各自都有正当讲解句，扩进来即误伤，见同文件测试）：
    // 「执行完成 / 运行完成」——「任务执行完成后会通知你」
    // 「步骤执行完成」——「每步骤执行完成后记录日志」
    // 「运行抛出 / 抛出异常」——「运行抛出异常时该脚本会非零退出」
    // 「返回了 / 结果是」——「这个函数返回了 None」「1+1 的结果是 2」。
    //   这两个词抓得住 trace 63 的后半句，但那半句的抓法在上面「我跑了」已经
    //   覆盖，加进来等于拿代码讲解和算术题换一条冗余命中，不划算。
];

/// 中文/口语说法 → 工具名。
///
/// 模型写不出 snake_case 的工具名：它说「子 agent」而表里是 `spawn_subagent`，
/// 于是点名扫描（`mentioned_tool_names`）认不出这是个工具调用，判决退到
/// 「整条提问零凭证」的粗判。只靠粗判挡不住那句——模型照抄上一轮真实回复的
/// 格式报了个子 agent 的实测值（trace 65），而它压根没派过子 agent。
const TOOL_ALIASES: &[(&str, &str)] = &[
    ("子 agent", "spawn_subagent"),
    ("子代理", "spawn_subagent"),
    ("subagent", "spawn_subagent"),
];

/// 内置默认词表。既是设置页「恢复默认」的来源，也是没保存过自定义词表时的
/// 生效值。返回 owned：调用方要跨线程用（command 返回给前端）。
pub fn default_execution_claim_patterns() -> Vec<String> {
    EXECUTION_CLAIM_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 点名判据的成功形态标记（内置，不受设置页词表影响）。
///
/// 词表是用户可编辑的执行类触发器：删空就是「关掉这一档触发器」。点名判据要
/// 在词表之外还能立住，得有一个关不掉的形态标记——模型把工具结果列成清单时
/// 写的那种（`- create_word：✅ 调用成功`）。符号形态不参与措辞，模型换说法
/// 照样命中。
const BUILTIN_SUCCESS_MARKERS: &[&str] = &["✅", "✔"];

/// 终稿里「点名了某次执行」的工具。
///
/// 与 `mentioned_tool_names` 的差别有两条，都是为了不误伤：
///
/// · **执行形态词要落在工具名之后、且在同一句内**——「任务创建成功，你可以用
///   grep_files 搜」里的「成功」说的是任务，不是 grep_files；词落在名字前面
///   （同一句里前面那半截的执行叙述）不算。
/// · **只认词表里的完成态措辞**——失败形态（「调用失败」「调用被拒」）不在词表
///   里，如实汇报工具不可用是诚实行为，不该被判成编造。
pub fn claimed_executed_tools(text: &str, patterns: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for seg in split_claim_segments(text) {
        for (name, needle) in tool_needles() {
            if out.contains(&name) {
                continue;
            }
            let claimed = identifier_occurrences(seg, needle).any(|i| {
                let tail = &seg[i + needle.len()..];
                patterns
                    .iter()
                    .any(|p| !p.is_empty() && tail.contains(p.as_str()))
                    || BUILTIN_SUCCESS_MARKERS.iter().any(|m| tail.contains(m))
            });
            if claimed {
                out.push(name);
            }
        }
    }
    out
}

/// 工具名与它在正文里的写法：snake_case 名 + 中文别名。
///
/// 别名指向的名字必须是真存在的工具——别名表写错工具名时静默跳过（理由同
/// `mentioned_tool_names`）。
fn tool_needles() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> =
        TOOLS_TABLE.iter().map(|t| (t.name, t.name)).collect();
    for (alias, target) in TOOL_ALIASES {
        if TOOLS_TABLE.iter().any(|t| t.name == *target) {
            out.push((target, alias));
        }
    }
    out
}

/// 切句：换行与句末标点断开。
///
/// 判定「执行形态词落在工具名之后」要在同一句内成立，否则一句里前半段的执行
/// 叙述会被算到后半段提到的工具头上。
fn split_claim_segments(text: &str) -> Vec<&str> {
    text.split(['\n', '。', '！', '？', '；', ';'])
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// 标识符的整词命中位置（左右不得续上标识符字符）。
///
/// 与 `mentions_identifier` 同一口径：工具名互为前缀时纯子串会互相污染
/// （`create_word` ⊂ `create_word_revisions`）。
fn identifier_occurrences<'a>(text: &'a str, ident: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(ident).filter_map(move |(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + ident.len()..].chars().next();
        if before.is_some_and(is_ident_char) || after.is_some_and(is_ident_char) {
            None
        } else {
            Some(i)
        }
    })
}

/// 按传入词表判定「终稿是否在叙述一次已完成的执行」。
///
/// 判决层唯一入口：词表来自 bot-config（设置页可编辑），没配过时由薄壳回落
/// 到内置表。**词表是全量替换而不是合并**——用户在设置页删掉某个词就是要它
/// 不再生效，合并会让内置那排负例测试形同虚设。
pub fn claims_execution_with(text: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| !p.is_empty() && text.contains(p))
}

/// 终稿是否在叙述一次「已完成的执行」（按内置默认词表）。
///
/// 单独判定、不带上下文：是否该拦由调用方结合凭证表一起看（见 bot_model_loop
/// 的幻觉守卫）。纯函数的职责只有「这句话像不像在转述执行结果」。词表改了
/// 这里跟着改——设置页改的是 bot-config，不改这段常量。
pub fn claims_execution(text: &str) -> bool {
    claims_execution_with(text, &default_execution_claim_patterns())
}

/// 结果措辞：把一个数说成「某次执行的产物」的引子。
///
/// 与 `EXECUTION_CLAIM_PATTERNS` 分家：那批词判「像不像在叙述一次执行」，
/// 这批词判「这个数是不是被当成那次执行的产出」。两者要同时命中才作数——
/// 少一个就是误伤的入口：
/// · 少了执行措辞 → 「1+1 的结果是 2」这类算术与讲解句会被当成谎报
///   （词表里刻意不收「结果是/返回了」，指的就是这个坑）。
/// · 少了结果措辞 → 「已跑完，共 3 步」里的 3 是计数不是结果。
const RESULT_CLAIM_MARKERS: &[&str] = &[
    "结果",
    "输出",
    "返回",
    "打印",
    "显示",
    "算出",
    "算出来",
    "得到",
    "统计",
    "实测",
    "答案",
    "读出",
    "产出",
];

/// 终稿是否如实说明了「那次执行没做成」。
///
/// 判据一（结果无出处）的让开条件：模型说了「跑不了 / 被拦了」，那它后面给
/// 的数是它自己的推算（哪怕没标注），不是把执行结果编出来。如实汇报失败是
/// 诚实行为，不该被判成编造——这条是执行类守卫一直守着的承诺。
const EXECUTION_NOT_DONE_MARKERS: &[&str] = &[
    "跑不了",
    "跑不成",
    "没法跑",
    "无法执行",
    "执行不了",
    "执行失败",
    "运行失败",
    "调用失败",
    "调用被拒",
    "被拒绝",
    "被拦截",
    "被挡",
    "未开启",
    "没开启",
    "没跑成",
    "没能跑",
    "跑不动",
    "不可用",
    "没有权限",
    "没权限",
    "无权限",
    "白名单",
    "失败",
    "公式",
    "推算",
    "估算",
    "估计",
    "凭印象",
];

/// 终稿是否如实说明了「那次执行没做成」（含「这是推算不是执行结果」）。
pub fn states_execution_not_done(text: &str) -> bool {
    EXECUTION_NOT_DONE_MARKERS
        .iter()
        .any(|m| !m.is_empty() && text.contains(m))
}

/// 存在性/核实类措辞：终稿在给出现场结论（文件在不在、查到没有）。
///
/// 只给回声那道判据用（见 `is_echo_call`）：那次调用没取回任何新信息，结论
/// 却靠它撑着。与 `RESULT_CLAIM_MARKERS` 分开——「没有」「确实」这类词太泛，
/// 放进结果值那道判据会把「已跑完，没有报错」当成在报结果。
const EXISTENCE_CLAIM_MARKERS: &[&str] = &[
    "存在",
    "不存在",
    "只有",
    "找到",
    "没找到",
    "确实",
    "已核实",
    "已确认",
    "已检查",
    "核对过",
    "已验证",
];

/// 终稿是否给出了「结果」——数值结果或现场结论。
///
/// 供回声那道判据用：调用没取回任何新信息时，终稿若还给不出结论，那次调用
/// 就是白调的，模型拿它当依据就是虚的。
pub fn states_a_result(text: &str) -> bool {
    RESULT_CLAIM_MARKERS.iter().any(|m| text.contains(m))
        || EXISTENCE_CLAIM_MARKERS.iter().any(|m| text.contains(m))
}

/// 文本里的数值（宽松扫描：只取数字，不排形态）。
///
/// 给「出处比对」用：工具给 1234.56、模型报 1235 时字面比不上，但四舍五入到
/// 1235 是等价的——那种是如实转述，不该判成编造。
pub fn numbers_in_text(text: &str) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || (c == '.' && !cur.ends_with('.')) {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(v) = cur.parse::<f64>() {
                out.push(v);
            }
            cur.clear();
        }
    }
    if !cur.is_empty() {
        if let Ok(v) = cur.parse::<f64>() {
            out.push(v);
        }
    }
    out
}

/// 终稿里「被当成执行产出的结果」的具体数值。
///
/// 纯提取，不判真假：有没有出处要在运行期比（工具返回 + 对话上下文），那边
/// 依赖运行期状态，塞进纯函数就没法单测——与 `claimed_doc_paths` 同样的分工。
///
/// 触发要三样同时落在同一个句段里：执行完成态措辞（词表）、结果措辞、具体
/// 数值。句段按换行与句末标点切（`split_claim_segments`），跨行的那句
/// 「执行完毕：」与下一行的「输出 5050」不算同一段——宁可漏判不误伤。
pub fn claimed_result_numbers(text: &str, patterns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in split_claim_segments(text) {
        if !patterns
            .iter()
            .any(|p| !p.is_empty() && seg.contains(p.as_str()))
        {
            continue;
        }
        if !RESULT_CLAIM_MARKERS.iter().any(|m| seg.contains(m)) {
            continue;
        }
        for n in result_numbers_in(seg) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// 数值两侧的「贴边字符」：贴上就是别的 token 的一部分。
///
/// 路径（`/…/2026Q3复盘.docx`）、标识符（`2026Q3`）、日期（`2026-10-20`）
/// 里的数字都不是结果值。
fn is_value_edge_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
}

/// 数后面跟的中文量词：跟上了就是计数，不是结果值。
///
/// 「共 3 项」「3 步」「3 个文件」里的 3 是数出来的个数，模型没执行也数得出来，
/// 拿它当结果去查出处必然误伤。计时与体积单位（秒/ms/KB）不在此列——那是执行
/// 才产得出的量。
const COUNT_UNITS: &[char] = &[
    '个', '项', '条', '次', '份', '张', '页', '行', '轮', '步', '种', '类', '名', '位', '块', '道',
    '组', '批', '套', '只', '件', '本', '款', '台',
];

/// 数前面跟的估算词：跟上了就是估算，不是执行产出的确切值。
const ESTIMATE_WORDS: &[&str] = &[
    "约", "大概", "大致", "近", "超", "逾", "至少", "至多", "最多", "最少", "多达", "几百", "几千",
    "几", "余", "将", "接近", "超过", "多于", "少于", "不到",
];

/// 数前面跟的序数/条款标记：跟上了是编号，不是结果值。
const ORDINAL_WORDS: &[&str] = &["第", "规则", "版本", "条款", "条"];

/// 句段里的数值候选（排掉各自都有正当讲解句的形态）。
///
/// 排掉的形态：标识符/路径/日期的一部分、列表序号（`1.`）、计数（`3 项`）、
/// 估算（`约 5013`）、编号（`第 21 条`、`规则 21/19`）。
fn result_numbers_in(seg: &str) -> Vec<String> {
    let chars: Vec<char> = seg.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let mut end = i;
        // 小数：小数点后必须是数字，否则是列表序号或句末点号
        if i < chars.len()
            && chars[i] == '.'
            && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())
        {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            end = i;
        }
        let num: String = chars[start..end].iter().collect();
        let prev = start.checked_sub(1).map(|k| chars[k]);
        let next = chars.get(end).copied();
        // 贴边字符：路径/标识符/日期的一部分
        if prev.is_some_and(is_value_edge_char) || next.is_some_and(is_value_edge_char) {
            i = end;
            continue;
        }
        // 列表序号（`1.` `2、`）
        if next == Some('、') {
            i = end;
            continue;
        }
        // 计数：数后面（跨过空格）跟中文量词
        if next_non_space(&chars, end).is_some_and(|c| COUNT_UNITS.contains(&c)) {
            i = end;
            continue;
        }
        // 估算：数前面（跨过空格）跟估算词
        if ends_with_any(&chars, start, ESTIMATE_WORDS) {
            i = end;
            continue;
        }
        // 编号：数前面跟序数/条款标记
        if ends_with_any(&chars, start, ORDINAL_WORDS) {
            i = end;
            continue;
        }
        if !out.contains(&num) {
            out.push(num);
        }
        i = end;
    }
    out
}

/// 从 `end` 起跳过空白后的第一个字符。
fn next_non_space(chars: &[char], end: usize) -> Option<char> {
    chars[end..].iter().find(|c| !c.is_whitespace()).copied()
}

/// `start` 之前的文字（跳过空白）是否以 `words` 里的某个词结尾。
fn ends_with_any(chars: &[char], start: usize, words: &[&str]) -> bool {
    let head: String = chars[..start].iter().rev().take(4).collect();
    let head: String = head.chars().rev().collect();
    let head = head.trim_end();
    words.iter().any(|w| !w.is_empty() && head.ends_with(w))
}

/// 一次工具调用是不是「回声」——返回内容没有取回任何新信息，全是自己写进参数的那句。
///
/// 判据：返回里的内容 token 全部能在入参里找到。模型把结论写进参数、再让工具
/// 原样吐回来，回声就是「这次调用替你核实了什么」的答案——没有。
///
/// 样板不算内容：外部 MCP 工具的返回常排成 `### Result` / `### Ran … code` /
/// `### Page` 几段，格式词（Result/Page/URL…）会让每个 MCP 调用都显得「有新
/// 信息」，所以只取第一段（结果段）与入参比对。
///
/// 返回得太少判不出「有没有新信息」——不猜，不算回声。
pub fn is_echo_call(args: &str, output: &str) -> bool {
    let tokens = content_tokens(result_section(output));
    if tokens.len() < 2 || tokens.iter().map(|t| t.chars().count()).sum::<usize>() < 12 {
        return false;
    }
    let arg_tokens = content_tokens(args);
    tokens
        .iter()
        .all(|t| arg_tokens.iter().any(|a| a == t) || args.contains(t.as_str()))
}

/// 工具返回里的结果段。
///
/// 外部 MCP 工具常把返回排成 `### Result` / `### Ran … code` / `### Page` 几段，
/// 样板段不是取回的内容。第一行不是 `### ` 格式头时整段都算内容。
fn result_section(output: &str) -> &str {
    let first_line = output.split_inclusive('\n').next().unwrap_or("");
    if !first_line.trim_start().starts_with("### ") {
        return output;
    }
    let rest = &output[first_line.len()..];
    match rest.find("\n### ") {
        Some(idx) => &rest[..idx],
        None => rest,
    }
}

/// 内容 token：字母数字（含中日韩）与 `_` `.` `-` `/` 连成的片段。
fn content_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/') {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 终稿里点名的工具（按 TOOLS_TABLE 扫，snake_case 标识符不会与中文话术撞车）。
///
/// 供幻觉守卫把「模型说调了哪个工具」映射到凭证表——这是判决要查的键。
/// 返回全部命中而不是首个：模型一轮里常叙述多个工具，漏掉任何一个就等于
/// 给它留了一条白拿凭证的路。
///
/// 扫描按**标识符整词**匹配（`mentions_identifier`），不是纯子串：工具名互为
/// 前缀时纯子串会互相污染——`create_word` ⊂ `create_word_revisions`，模型如实
/// 汇报「create_word_revisions 执行完毕」时，纯子串扫描会多点出一个没调过的
/// `create_word`，凭证表查不到 → 判成编造 → 返工指令叫它去真调 `create_word`，
/// 产出一份非修订模式的 docx。为一次假指控付一次错误产物。
///
/// 再叠一层别名表（`TOOL_ALIASES`）：模型用中文称呼工具时写不出 snake_case 名，
/// 「子 agent」按原样扫不出来，会让点名判定退化成粗判。
pub fn mentioned_tool_names(text: &str) -> Vec<&'static str> {
    let mut hits: Vec<&'static str> = TOOLS_TABLE
        .iter()
        .map(|t| t.name)
        .filter(|name| mentions_identifier(text, name))
        .collect();
    for (alias, target) in TOOL_ALIASES {
        if !text.contains(alias) {
            continue;
        }
        // 别名指向的名字必须是真存在的工具——别名表写错工具名时静默跳过，
        // 拼错的名字查不到凭证，会把每次提到它的诚实汇报都判成伪造。
        if let Some(name) = TOOLS_TABLE
            .iter()
            .find(|t| t.name == *target)
            .map(|t| t.name)
        {
            if !hits.contains(&name) {
                hits.push(name);
            }
        }
    }
    hits
}

/// 标识符整词匹配：命中段的左右不得续上标识符字符（ASCII 字母数字或 `_`）。
///
/// 只判 ASCII——中文紧贴标识符是正常写法（「用 run_python 跑」），按 Unicode
/// 的 alphanumeric 判会把这类整段漏掉，等于把边界检查写成永假。
fn mentions_identifier(text: &str, ident: &str) -> bool {
    text.match_indices(ident).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + ident.len()..].chars().next();
        !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod mentioned_tool_names_tests {
    use super::{mentioned_tool_names, TOOLS_TABLE};

    /// 点名扫描按标识符整词匹配，前缀名对不互相污染。
    ///
    /// 复现的是实机可判的假指控：模型真调了 `create_word_revisions`（Word 润色的
    /// 推荐路径，prompts/system.rs 规则 10 明写），终稿报「create_word_revisions
    /// 执行完毕」——纯子串扫描会多点出 `create_word`，凭证表查不到，判决成
    /// 「你刚才说调了 create_word 并给出了结果，但没有任何 create_word 的成功
    /// 调用记录——这个结果是编造的」，返工指令再叫它去真调 create_word，
    /// 产出一份非修订模式的 docx。为一次假指控付一次错误产物。
    #[test]
    fn prefix_tool_name_is_not_matched_inside_longer_name() {
        assert_eq!(
            mentioned_tool_names("create_word_revisions 执行完毕，返回 3 个修订版本。"),
            vec!["create_word_revisions"],
            "提到 create_word_revisions 时不得同时点名 create_word"
        );
        assert_eq!(
            mentioned_tool_names("我跑完了 create_word_revisions，修订已写回原文档。"),
            vec!["create_word_revisions"]
        );
        // 反向：纯 create_word 仍要点名得出来（中文紧贴标识符是正常写法）
        assert_eq!(
            mentioned_tool_names("create_word 执行完毕，产物在 AI_Gen_Files/。"),
            vec!["create_word"]
        );
        assert_eq!(
            mentioned_tool_names("已用 create_word 生成，工具返回 3 处改动。"),
            vec!["create_word"]
        );
    }

    /// 整词匹配不能太紧：中文紧贴标识符、以及句末标点都算命中。
    #[test]
    fn chinese_adjacent_and_trailing_punctuation_still_match() {
        assert_eq!(
            mentioned_tool_names("我用 run_python 跑了一遍"),
            vec!["run_python"]
        );
        assert_eq!(
            mentioned_tool_names("跑完了 run_python。"),
            vec!["run_python"]
        );
        assert_eq!(mentioned_tool_names("（run_python）"), vec!["run_python"]);
        // 标识符内部续字符不算点名：没有 run_python2 这个工具，报它等于点名一个
        // 查不到凭证的假名字
        assert!(mentioned_tool_names("跑完了 run_python2。").is_empty());
    }

    /// 前缀名对必须互不点名——守的是**性质**而不是「表里不许有前缀名对」。
    ///
    /// 表里确实有 `create_word` ⊂ `create_word_revisions`（重命名要动提示词、
    /// schema、dispatch 三处，不值当）。要守的是整词匹配这个性质：只要点名扫描
    /// 按标识符边界匹配，前缀名对就永远各自独立；一旦有人把它退回纯子串，这条
    /// 立刻红——而不是等到某次实机偶发假指控才被发现。
    #[test]
    fn prefix_name_pairs_never_mention_each_other() {
        let names: Vec<&str> = TOOLS_TABLE.iter().map(|t| t.name).collect();
        let mut pairs = 0;
        for longer in &names {
            let expected = mentioned_tool_names(longer);
            for shorter in &names {
                if shorter != longer && longer.starts_with(shorter) {
                    pairs += 1;
                    assert!(
                        !expected.contains(shorter),
                        "点名 {longer} 时不该同时点名它的前缀 {shorter}：纯子串匹配已回来"
                    );
                    assert!(expected.contains(longer), "点名 {longer} 时必须认出它自己");
                }
            }
        }
        assert!(
            pairs > 0,
            "表里没有前缀名对了，本测试失去意义——留给下一条前缀关系出现时用"
        );
    }
}

#[cfg(test)]
mod claimed_executed_tools_tests {
    use super::{claimed_executed_tools, default_execution_claim_patterns};

    fn claimed(text: &str) -> Vec<&'static str> {
        claimed_executed_tools(text, &default_execution_claim_patterns())
    }

    /// 清单写法要点名得出来：成功标记与工具名同行。
    ///
    /// 真机 trace 101 的原话形态：零工具调用，终稿报「✅ 调用成功」。成功标记
    /// （✅）是内置的，不受设置页词表影响——词表被删空这道也得在。
    #[test]
    fn checklist_form_names_the_tool() {
        assert_eq!(
            claimed("工具调用如下：\n- create_word：✅ 调用成功，返回路径 `/tmp/x.docx`"),
            vec!["create_word"]
        );
        assert_eq!(
            claimed("- create_word（周报20261020）：✅ 调用成功，返回路径 `/tmp/x.docx`"),
            vec!["create_word"]
        );
    }

    /// 失败形态不点名：如实汇报工具不可用是诚实行为。
    ///
    /// 真机 trace 103 的原话形态。词表收的是完成态措辞，「调用失败 / 调用被拒」
    /// 不在其中。
    #[test]
    fn failure_form_is_not_a_claim() {
        assert!(claimed("- run_python：调用失败，返回错误 `[ErrNo -7]`").is_empty());
        assert!(claimed("- complete_task：❌ 任务不存在，调用被拒").is_empty());
    }

    /// 建议语气不点名：提到工具名不等于声称调过它。
    ///
    /// 词在名字前面（「任务已经执行完了，你可以用 grep_files 搜」）也不算——
    /// 那个「执行完」说的是任务，不是 grep_files。
    #[test]
    fn advice_form_is_not_a_claim() {
        assert!(claimed("这个问题你可以用 grep_files 搜，但我没权限。").is_empty());
        assert!(
            claimed("任务我已经执行完了，但这一步你可以用 grep_files 搜，我没权限。").is_empty()
        );
    }

    /// 执行措辞与工具名同句才算是「点名了这次执行」——分句边界是这道闸的护栏。
    ///
    /// 标题那行带执行措辞、工具名在下一行，说的是两件事：全局判一次会把如实
    /// 汇报失败也拖下水。
    #[test]
    fn claim_word_must_share_the_sentence_with_the_tool_name() {
        assert!(claimed("工具调用结果：\n- run_python：调用失败").is_empty());
        assert_eq!(
            claimed("工具调用如下：\n- create_word：✅ 调用成功"),
            vec!["create_word"]
        );
    }

    /// 别名同样要点名得出来：模型写「子 agent」而表里是 spawn_subagent。
    #[test]
    fn alias_form_names_the_tool() {
        assert_eq!(
            claimed("子 agent 跑 sleep(5) 实测约 5013 ms。"),
            vec!["spawn_subagent"]
        );
    }
}

/// 终稿里声称产出的文档路径（纯提取，不判真假）。
///
/// 出现「已生成/已保存/已写入/已导出」且给出文档绝对路径
/// （docx/xlsx/pptx/pdf）时逐个摘出候选。编造的路径经不起两道校验——
/// 磁盘存在性（路径压根不在）与落盘凭证（路径在、但从来不是工具写出来的）；
/// 实测「已生成XX规划 Word，文件路径：…」类话术带真实形态的假路径，
/// 绕过 claims_mutation 的窄词表，只能靠这两道拦。
///
/// **触发词与路径必须同句**：全文级提取会把「如实改口」读成声称——返工后模型
/// 照着改口提示复述那句「已生成」（还常带引号），而路径在后面另一段里，全文级
/// 提取照样把两者凑成一次声称，于是诚实更正被挂上「⚠️ 不是本次生成的」。真机
/// 原话：「之前那条「已生成」是我口胡，实际并没有落盘……你之前明确说过这份纪要
/// 要存到 `/path/x.docx`」——触发词在第一句、路径在最后一句，说的是两件事。
///
/// 真伪判定不在这里：两问都依赖运行期状态（磁盘、痕迹库），塞进纯函数就没法
/// 单测；提取与判定分开，判定在模型循环侧。
pub fn claimed_doc_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sent in split_claim_sentences(text) {
        if !["已生成", "已保存", "已写入", "已导出"]
            .iter()
            .any(|w| sent.contains(w))
        {
            continue;
        }
        for cand in doc_path_candidates(sent) {
            if !out.contains(&cand) {
                out.push(cand);
            }
        }
    }
    out
}

/// 切句：按句末标点断开，**换行不算**。
///
/// 换行不切是因为「已生成 Word 文档：\n\n`/path/x.docx`」是常见排版——路径
/// 换行到下一段仍是同一次声称。
fn split_claim_sentences(text: &str) -> Vec<&str> {
    text.split(['。', '！', '？', '；', ';'])
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// 一句里的文档路径候选（切片段 + 贪心合并）。
fn doc_path_candidates(text: &str) -> Vec<String> {
    // 候选路径切分：空白/反引号/引号/括号/书名号/中英文标点都是路径边界。
    // `\` 不入边界集（会拆碎 `C:\` 盘符路径）；UNC 路径（`\\server\share\…`）
    // 靠前缀白名单放行——split 已保证 token 起点必在行首/空白/边界符之后，
    // 故以 `\\` 起头的 token 就是完整的 UNC 前缀，原样进存在性校验
    let boundaries: &[char] = &[
        ' ', '\t', '\n', '\r', '`', '"', '\'', '「', '」', '（', '）', '(', ')', '，', '。', '；',
        '、', ',', ';', ':', '：', '　',
    ];
    // 片段先切出来再合并：含空格的路径（macOS 的 "Application Support"）会被
    // 空格切成两截，后半截不再以 '/' 开头 → 整条路径被丢掉、存在性与凭证两道
    // 闸对它失明。合并规则见 `join_path_fragment`。
    let frags: Vec<&str> = text
        .split(boundaries)
        .map(|tok| tok.trim_matches(|c: char| matches!(c, '*' | '_' | '-' | '>' | '#')))
        .filter(|tok| !tok.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < frags.len() {
        let mut cand = frags[i].to_string();
        // 贪心吸收后续片段：把被空格切断的路径接回去
        while i + 1 < frags.len() && join_path_fragment(&cand, frags[i + 1]) {
            cand.push(' ');
            cand.push_str(frags[i + 1]);
            i += 1;
        }
        if claimed_doc_path_shape(&cand) && !out.contains(&cand) {
            out.push(cand);
        }
        i += 1;
    }
    out
}

/// 片段是否以已知文档后缀结尾（不看路径形态——合并时要看的是分段，不是整条）。
fn doc_ext_suffix(seg: &str) -> bool {
    const DOC_EXTS: [&str; 4] = [".docx", ".xlsx", ".pptx", ".pdf"];
    let low = seg.to_lowercase();
    DOC_EXTS.iter().any(|e| low.ends_with(e))
}

/// 绝对路径形态：POSIX 根、盘符、UNC 前缀。
fn abs_path_shape(tok: &str) -> bool {
    tok.starts_with('/') || (tok.len() > 2 && tok.as_bytes()[1] == b':') || tok.starts_with(r"\\")
}

/// 一条候选是否成立：文档后缀 + 绝对路径形态。
fn claimed_doc_path_shape(tok: &str) -> bool {
    doc_ext_suffix(tok) && abs_path_shape(tok)
}

/// `next` 能不能并进 `cand`，把被空格切断的路径接回去。
///
/// 四条都要满足：
/// 1. `cand` 已经是绝对路径的起点（否则合并只会造出更长的垃圾串）；
/// 2. `next` 含 '/'——它是目录片段而不是普通词（"详见" 这类词不能并进来）；
/// 3. `next` 不含句读标点（"…docx 详见。" 那种，标点一进来就不是路径了）；
/// 4. 合并后按空格分段，**至多一个**分段带文档后缀，且那个分段在最后。
///    这条把「一条含空格的路径」和「两个相邻的路径」分开：
///    `/a/b.docx` + `/c/d.docx` 合并后有两个带后缀的分段 → 不并；
///    `/…/Application` + `Support/…/x.docx` 只有一个带后缀的分段 → 并。
fn join_path_fragment(cand: &str, next: &str) -> bool {
    if !abs_path_shape(cand) {
        return false;
    }
    if !next.contains('/') {
        return false;
    }
    if next.chars().any(|c| "。，；：！？、—".contains(c)) {
        return false;
    }
    let mut segs: Vec<&str> = cand.split(' ').collect();
    segs.push(next);
    let mut hits = 0;
    for (idx, seg) in segs.iter().enumerate() {
        if doc_ext_suffix(seg) {
            hits += 1;
            if idx + 1 != segs.len() {
                return false;
            }
        }
    }
    hits <= 1
}

/// 路径在磁盘上不存在（存在性判据的纯函数形态，便于单测）。
///
/// 存在性是幻觉守卫对「声称产出」的第一道判据：编造的路径经不起这一问。
/// 单独成函数是为了让它能单测——挂在循环里就没法单独钉住。
pub fn path_missing_on_disk(path: &str) -> bool {
    !std::path::Path::new(path).exists()
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// TOOLS_TABLE 的 O(1) HashMap 索引。
/// dispatch 的 O(n) `TOOLS_TABLE.iter().find(...)` 改为 O(1) `tools_index().get(name)`。
/// OnceLock 保证只有一次初始化；索引里全部为 `&'static ToolDef` 引用，零额外分配（除 HashMap 表本身）。
static TOOLS_INDEX: OnceLock<HashMap<&'static str, &'static ToolDef>> = OnceLock::new();

pub fn tools_index() -> &'static HashMap<&'static str, &'static ToolDef> {
    TOOLS_INDEX.get_or_init(|| TOOLS_TABLE.iter().map(|t| (t.name, t)).collect())
}

/// 内置工具总数（= TOOLS_TABLE 条目数 = SCHEMA_* 常量数）。
/// 注释与文档一律引用本常量而不另写数字；增删工具时只改这里一处，
/// `builtin_tool_count_matches_table` 单测负责在数目对不上时红灯。
pub const BUILTIN_TOOL_COUNT: usize = 63;

// TOOLS_TABLE（内置工具单源真相：BUILTIN_TOOL_COUNT 个，其中 SUBAGENT_ONLY_TOOLS
// 那几个仅子 agent 可见，主 agent 少这么多）
pub static TOOLS_TABLE: &[ToolDef] = &[
    ToolDef {
        name: "query_tasks",
        schema: SCHEMA_QUERY_TASKS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_query_tasks,
    },
    ToolDef {
        name: "query_single_task",
        schema: SCHEMA_QUERY_SINGLE_TASK,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_query_single_task,
    },
    ToolDef {
        name: "create_task",
        schema: SCHEMA_CREATE_TASK,
        mutating: true,
        claims_patterns: &["已创建", "已新建", "新建了", "创建了", "已新增"],
        max_output_bytes: 8192,
        call: call_create_task,
    },
    ToolDef {
        name: "complete_task",
        schema: SCHEMA_COMPLETE_TASK,
        mutating: true,
        claims_patterns: &["已标记为完成", "标记为完成", "已完成任务", "已完成「"],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_delete_task,
    },
    ToolDef {
        name: "edit_task",
        schema: SCHEMA_EDIT_TASK,
        mutating: true,
        claims_patterns: &["已修改", "已更新", "已改成", "已改", "已调整", "已编辑"],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_add_subtask,
    },
    ToolDef {
        name: "toggle_subtask",
        schema: SCHEMA_TOGGLE_SUBTASK,
        mutating: true,
        claims_patterns: &["已勾选", "勾选了", "已标记子任务完成", "子任务已勾选"],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_remove_subtask,
    },
    ToolDef {
        name: "read_text_file",
        schema: SCHEMA_READ_TEXT_FILE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
        call: call_read_text_file,
    },
    ToolDef {
        name: "ocr_image",
        schema: SCHEMA_OCR_IMAGE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ocr_image,
    },
    ToolDef {
        name: "grep_files",
        schema: SCHEMA_GREP_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_grep_files,
    },
    ToolDef {
        name: "list_files",
        schema: SCHEMA_LIST_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
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
        max_output_bytes: 8192,
        call: call_link_file_to_task,
    },
    ToolDef {
        name: "extract_document",
        schema: SCHEMA_EXTRACT_DOCUMENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_create_word,
    },
    ToolDef {
        name: "create_word_revisions",
        schema: SCHEMA_CREATE_WORD_REVISIONS,
        mutating: true,
        claims_patterns: &["已修订 Word", "已应用修订", "已写入修订", "修订已应用"],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_create_pdf,
    },
    ToolDef {
        name: "run_python",
        schema: SCHEMA_RUN_PYTHON,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_run_python,
    },
    ToolDef {
        name: "web_search",
        schema: SCHEMA_WEB_SEARCH,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_web_search,
    },
    ToolDef {
        name: "fetch_url",
        schema: SCHEMA_FETCH_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
        call: call_fetch_url,
    },
    ToolDef {
        name: "get_current_time",
        schema: SCHEMA_GET_CURRENT_TIME,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_remember_fact,
    },
    ToolDef {
        name: "recall_facts",
        schema: SCHEMA_RECALL_FACTS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
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
        max_output_bytes: 8192,
        call: call_record_lesson,
    },
    ToolDef {
        name: "use_skill",
        schema: SCHEMA_USE_SKILL,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_use_skill,
    },
    ToolDef {
        name: "create_skill",
        schema: SCHEMA_CREATE_SKILL,
        mutating: true,
        claims_patterns: &["已创建技能", "已新建技能", "已注册技能"],
        max_output_bytes: 8192,
        call: call_create_skill,
    },
    ToolDef {
        name: "install_skill",
        schema: SCHEMA_INSTALL_SKILL,
        mutating: true,
        claims_patterns: &["已安装技能", "技能已安装"],
        max_output_bytes: 8192,
        call: call_install_skill,
    },
    ToolDef {
        name: "search_skills",
        schema: SCHEMA_SEARCH_SKILLS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 16384,
        call: call_search_skills,
    },
    // 子 agent 编排（主 agent 可见，追加表尾）
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
        max_output_bytes: 8192,
        call: call_spawn_subagent,
    },
    ToolDef {
        name: "check_subagent",
        schema: SCHEMA_CHECK_SUBAGENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_check_subagent,
    },
    ToolDef {
        name: "cancel_subagent",
        schema: SCHEMA_CANCEL_SUBAGENT,
        mutating: true,
        claims_patterns: &["已取消子任务", "已取消派发", "已终止子任务"],
        max_output_bytes: 8192,
        call: call_cancel_subagent,
    },
    // 电脑辅助 Tier1（只「看」与「打开」）
    ToolDef {
        name: "reveal_path",
        schema: SCHEMA_REVEAL_PATH,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_reveal_path,
    },
    ToolDef {
        name: "open_url",
        schema: SCHEMA_OPEN_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_open_url,
    },
    ToolDef {
        name: "clipboard_write",
        schema: SCHEMA_CLIPBOARD_WRITE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_clipboard_write,
    },
    ToolDef {
        name: "screenshot",
        schema: SCHEMA_SCREENSHOT,
        // 截图真写盘（PNG 落 AI_Gen_Files）——按变更类登记，否则「已截屏 /
        // 已保存截图」这类声称对幻觉守卫完全不可见（模型不调工具也能报）。
        mutating: true,
        claims_patterns: &["已截屏", "已保存截图", "截图已保存"],
        max_output_bytes: 8192,
        call: call_screenshot,
    },
    // 文件编辑（写白名单 ≠ 读白名单）
    ToolDef {
        name: "edit_file",
        schema: SCHEMA_EDIT_FILE,
        mutating: true,
        claims_patterns: &["已修改", "已编辑"],
        max_output_bytes: 8192,
        call: call_edit_file,
    },
    ToolDef {
        name: "write_file",
        schema: SCHEMA_WRITE_FILE,
        mutating: true,
        claims_patterns: &["已写入", "已创建文件"],
        max_output_bytes: 8192,
        call: call_write_file,
    },
    // 程序 M2（聊天即 IDE）：apps_list 防重名 / apps_create 落草稿 / apps_update_files 整文件集修改
    ToolDef {
        name: "apps_list",
        schema: SCHEMA_APPS_LIST,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_apps_list,
    },
    ToolDef {
        name: "apps_create",
        schema: SCHEMA_APPS_CREATE,
        mutating: true,
        claims_patterns: &["已创建程序", "已新建程序", "已登记程序", "程序已创建"],
        max_output_bytes: 8192,
        call: call_apps_create,
    },
    ToolDef {
        name: "apps_update_files",
        schema: SCHEMA_APPS_UPDATE_FILES,
        mutating: true,
        claims_patterns: &["已更新程序", "已修改程序", "程序已更新", "已上架"],
        max_output_bytes: 8192,
        call: call_apps_update_files,
    },
    // 子 agent 白名单工具（不进主 agent 默认 schema）
    ToolDef {
        name: "write_artifact_file",
        schema: SCHEMA_WRITE_ARTIFACT_FILE,
        mutating: true,
        claims_patterns: &["已写入产物", "产物已写入", "已保存产物", "已写入报告"],
        max_output_bytes: 8192,
        call: call_write_artifact_file,
    },
    ToolDef {
        name: "read_own_card",
        schema: SCHEMA_READ_OWN_CARD,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_read_own_card,
    },
    ToolDef {
        name: "ask_user",
        schema: SCHEMA_ASK_USER,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ask_user,
    },
    // Python AST 探查。追加在表尾：tools_json_matches_baseline 只比前 28 项
    // 前缀，插在中间会无谓地改动基线前缀（基线是「核心工具集」的快照，不是
    // 全量清单的镜像）。顺序只影响主 agent 看到工具的排列，无语义。
    ToolDef {
        name: "ast_inspect",
        schema: SCHEMA_AST_INSPECT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ast_inspect,
    },
    // 归档工具。追加在表尾：tools_json_matches_baseline 只比前 28 项前缀，
    // 插在中间会无谓地改动基线前缀（同 ast_inspect 的理由）。
    ToolDef {
        name: "unpack_archive",
        schema: SCHEMA_UNPACK_ARCHIVE,
        mutating: true,
        claims_patterns: &["已解压", "已解包", "已解压出"],
        max_output_bytes: 8192,
        call: call_unpack_archive,
    },
    ToolDef {
        name: "pack_archive",
        schema: SCHEMA_PACK_ARCHIVE,
        mutating: true,
        claims_patterns: &["已打包", "已压缩", "已生成压缩包"],
        max_output_bytes: 8192,
        call: call_pack_archive,
    },
    // 文件复制 / 移动 / 删除。追加在表尾：与归档工具同理由——基线只比前缀。
    ToolDef {
        name: "manage_file",
        schema: SCHEMA_MANAGE_FILE,
        mutating: true,
        claims_patterns: &["已复制", "已拷贝", "已移动", "已移到"],
        max_output_bytes: 8192,
        call: call_manage_file,
    },
    // 文件进系统剪贴板。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    // 非 mutating：剪贴板不是文件/任务卡的变更，与 clipboard_write 同档。
    ToolDef {
        name: "copy_file_to_clipboard",
        schema: SCHEMA_COPY_FILE_TO_CLIPBOARD,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_copy_file_to_clipboard,
    },
    // 提醒三件套。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    ToolDef {
        name: "schedule_reminder",
        schema: SCHEMA_SCHEDULE_REMINDER,
        mutating: true,
        claims_patterns: &["已设提醒", "已设置提醒", "提醒已设置", "已添加提醒"],
        max_output_bytes: 8192,
        call: call_schedule_reminder,
    },
    ToolDef {
        name: "list_reminders",
        schema: SCHEMA_LIST_REMINDERS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_reminders,
    },
    ToolDef {
        name: "cancel_reminder",
        schema: SCHEMA_CANCEL_REMINDER,
        mutating: true,
        claims_patterns: &["已取消提醒", "提醒已取消"],
        max_output_bytes: 8192,
        call: call_cancel_reminder,
    },
    // UI 自动化六件套。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    // 点击 / 输入是 mutating：它们能在别的应用里产生真实副作用（消息真发出去了），
    // 声称「已点击 / 已输入」必须能对上凭证。
    ToolDef {
        name: "list_windows",
        schema: SCHEMA_LIST_WINDOWS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_windows,
    },
    ToolDef {
        name: "activate_window",
        schema: SCHEMA_ACTIVATE_WINDOW,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_activate_window,
    },
    ToolDef {
        name: "ui_inspect",
        schema: SCHEMA_UI_INSPECT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ui_inspect,
    },
    ToolDef {
        name: "ui_click",
        schema: SCHEMA_UI_CLICK,
        mutating: true,
        claims_patterns: &["已点击", "已按下"],
        max_output_bytes: 8192,
        call: call_ui_click,
    },
    ToolDef {
        name: "ui_type",
        schema: SCHEMA_UI_TYPE,
        mutating: true,
        claims_patterns: &["已输入"],
        max_output_bytes: 8192,
        call: call_ui_type,
    },
    ToolDef {
        name: "wait_for_element",
        schema: SCHEMA_WAIT_FOR_ELEMENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_wait_for_element,
    },
    // 受管长驻服务。追加在表尾：同 manage_file 的理由——基线只比前缀。
    ToolDef {
        name: "start_service",
        schema: SCHEMA_START_SERVICE,
        mutating: true,
        claims_patterns: &["已启动", "已启动服务", "服务已启动", "已在后台启动"],
        max_output_bytes: 8192,
        call: call_start_service,
    },
    ToolDef {
        name: "stop_service",
        schema: SCHEMA_STOP_SERVICE,
        mutating: true,
        claims_patterns: &["已停止", "已停止服务", "服务已停止", "已关闭服务"],
        max_output_bytes: 8192,
        call: call_stop_service,
    },
    ToolDef {
        name: "list_services",
        schema: SCHEMA_LIST_SERVICES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_services,
    },
];
/// TOOLS JSON 由 TOOLS_TABLE 顺序拼装（schema 常量原文直拼，不做 parse + re-serialize）。
/// 少一次运行期解析，也不给「schema 非法 → expect panic 杀聊天」留路径
/// （合法性 + 与 baseline 的一致性由 registry_tests 锁死）。
///
/// 按名字表拼一份工具 JSON：`[` + 每项 `\n  ` + `,\n  ` + `\n]`。
///
/// 三处清单（主 agent 默认 / profile 白名单 / per-onion 交集）共用这一个拼装，
/// 差异只是过滤依据。分开写三份的代价是 `splice_mcp_body` 依赖的「恒以 `\n]`
/// 收尾」成了口头约定——破坏它只能靠限频 eprintln 事后发现（那是运行时降级
/// 告警，本该由抽象收敛消掉）。
///
/// 空集产出 `[\n]`：模型看到零工具时该收到空清单，而不是整份默认清单——
/// 后者是「配置看着生效、实际落空」的相反版本。
fn assemble_tools_json(keep: impl Fn(&ToolDef) -> bool) -> String {
    let mut s = String::with_capacity(16 * 1024);
    s.push('[');
    let mut first = true;
    for tool in TOOLS_TABLE.iter() {
        if !keep(tool) {
            continue;
        }
        s.push_str(if first { "\n  " } else { ",\n  " });
        first = false;
        s.push_str(tool.schema);
    }
    s.push_str("\n]");
    s
}

/// 仅子 agent 可见的工具（write_artifact_file / read_own_card）不出现在
/// 主 agent 的默认清单里——主可见数 = BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len()。
pub fn tools_json() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| assemble_tools_json(|t| !SUBAGENT_ONLY_TOOLS.contains(&t.name)))
}

/// 仅子 agent 白名单可见的工具（主 agent 默认 schema 不含）。
pub const SUBAGENT_ONLY_TOOLS: &[&str] = &["write_artifact_file", "read_own_card"];

/// 子 agent 工具白名单（设计 §5.1 → 本仓工具映射）。
/// 禁止（所有 profile）：spawn_subagent / check_subagent / cancel_subagent /
/// 任务卡主状态写（create_task/complete_task/edit_task/toggle_subtask 等全部不在列）/
/// 任务执行 / 权限变更——白名单外一律由 dispatch 闸拒绝。
///
/// `grep_files` + `extract_document` 是只读扩展：Probe profile 的 yaml 要真正
/// 能「搜代码 + 读文档」，否则那份配置看着生效、实际落空（收紧模型里
/// profile 给上限、yaml 再收窄，所以上限不够就永远用不上）。
pub const RESEARCH_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "list_files",
    "read_text_file",
    "grep_files",
    "extract_document",
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
    "ast_inspect",
    "unpack_archive",
    "pack_archive",
    "write_artifact_file",
    "read_own_card",
];
pub const GENERAL_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "read_text_file",
    "list_files",
    "grep_files",
    "extract_document",
    "edit_file",
    "write_file",
    "run_python",
    "ast_inspect",
    "unpack_archive",
    "pack_archive",
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
///
/// 绑了 per-onion 配置的会话走第三态：`运行时 profile ∩ yaml.allowed`。
/// **运行时 profile 优先于 yaml 的 `profile:` 字段**——子 agent 会话的白名单
/// 由派发时的 profile 决定，若让 yaml 的 profile 覆盖就成了「放宽」，与
/// 「只有收紧、没有放宽」直接冲突。yaml 的 `profile` 只在没有子 agent 上下文的
/// 会话上兜底，或作为「这份配置自称哪一档」被校验。
///
/// 返回 `Cow`：无配置时原样借静态串（热路径零分配不变），有配置收窄才现算 owned。
pub fn tools_json_for(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    let runtime_profile = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .map(|c| c.profile);
    let cfg = crate::bot_onions::cached_for_session(session_id);
    // 上限：运行时 profile 优先；都没有时用 yaml 自称的 profile 兜底
    let profile = match (runtime_profile, cfg.as_ref().and_then(|c| c.profile_enum())) {
        (Some(p), _) => Some(p),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    };
    let Some(profile) = profile else {
        return std::borrow::Cow::Borrowed(tools_json());
    };
    let allowed = cfg.as_ref().map(|c| c.allowed.as_slice()).unwrap_or(&[]);
    let effective = crate::bot_onions::effective_tools(profile, allowed);
    // 没有额外收窄 → 沿用按 profile 缓存的静态串，不重算
    if effective.len() == profile_whitelist(profile).len() {
        return std::borrow::Cow::Borrowed(profile_tools_json(profile));
    }
    std::borrow::Cow::Owned(narrowed_tools_json(&effective))
}

/// 主 agent 的完整工具清单（阶段 3 MCP 挂载点，机制 A）：
/// 内置静态 JSON 尾部追加外部 MCP 工具（增量挂载，内置 BUILTIN_TOOL_COUNT 个工具的 schema 字节不动）。
/// - 子 agent 会话：短路返回白名单（外部 MCP 工具不进子 agent，§5.1 边界不破）；
/// - 无 MCP 连接：原样返回静态 &'static str（零分配，热路径不变）；
/// - 有连接：Owned String = 静态 JSON 摘尾 + `mount::mcp_tools_json_body()` + 收尾。
/// 消费点 bot_model_loop 对拼装结果 fail-soft：schema 构建层保证 JSON
/// 合法（mount/registry 单测锁），解析失败记审计走空工具表，不再 panic。
pub fn tools_json_with_mcp(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    // 子 agent 判定显式化（评审：原 ptr::eq 指针比较对 tools_json_for 的
    // 返回形态有隐式契约，重构易碎）——subagent_ctx 是内存查表，重复一次开销可忽略
    let is_subagent = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .is_some();
    let base = tools_json_for(session_id);
    if is_subagent {
        // 子 agent 白名单：静态借用返回
        return base;
    }
    let body = crate::bot::mcp::mount::mcp_tools_json_body();
    if body.is_empty() {
        return base;
    }
    // base 恒以 "\n]" 收尾（tools_json 拼装格式）。契约破坏属"永不发生"态：
    // 每轮模型循环都走这里，告警限频防刷屏——第 1 次 + 之后每 100 次
    //（评审：OnceLock 只告一次，持续破坏会彻底静默）；回退 base 后
    // 消费点的 fail-soft 兜底接住（release 无 debug_assert，不产出非法 JSON）。
    static CONTRACT_BREAKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if !base.ends_with("\n]") {
        let n = CONTRACT_BREAKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n == 1 || n.is_multiple_of(100) {
            eprintln!(
                "[registry] tools_json 格式契约破坏（未以 \\n] 收尾，第 {n} 次），MCP 工具本轮不挂载"
            );
        }
    }
    // base 可能是 owned（本会话绑了 onion 配置、清单是现算的交集），
    // 那种情况下没法借出去，只能再拷一份收尾。两版共用同一个拼接内核
    match base {
        std::borrow::Cow::Borrowed(b) => splice_mcp_body(b, &body),
        std::borrow::Cow::Owned(o) => std::borrow::Cow::Owned(splice_mcp_body_owned(o, &body)),
    }
}

/// 拼接内核（纯函数，单测直打）：base 恒以 `"\n]"` 收尾、动态段恒以
/// `",\n"` 起头（build_tools_json_body 构造保证），摘尾接段再补收尾。
///
/// 契约破坏返回 None，调用方各自回退原样 base（借用版回退 Borrowed、
/// owned 版回退原串）——宁可本轮少挂 MCP 工具，不产出非法 JSON。
/// 两版拼接共用这一个内核，判定逻辑只此一份。
fn join_mcp_body(base: &str, body: &str) -> Option<String> {
    let head = base.strip_suffix("\n]")?;
    if !body.starts_with(",\n") {
        return None;
    }
    let mut s = String::with_capacity(head.len() + body.len() + 2);
    s.push_str(head);
    s.push_str(body);
    s.push_str("\n]");
    Some(s)
}

fn splice_mcp_body<'a>(base: &'a str, body: &str) -> std::borrow::Cow<'a, str> {
    match join_mcp_body(base, body) {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(base),
    }
}

/// `splice_mcp_body` 的 owned 版：base 已持有所有权时借不出去，只能再拷一份。
/// 契约判断复用同一个 `join_mcp_body` 内核，破坏时回退原串。
fn splice_mcp_body_owned(base: String, body: &str) -> String {
    join_mcp_body(&base, body).unwrap_or(base)
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
                map.insert(
                    p.as_str(),
                    assemble_tools_json(|t| whitelist.contains(&t.name)),
                );
            }
            map
        })
        .get(profile.as_str())
        .map(|s| s.as_str())
        .unwrap_or_else(|| tools_json())
}

/// 按 `effective` 名字表拼一份收窄后的工具 JSON（owned）。
/// 过滤依据是「profile 白名单 ∩ per-onion allowed」的交集结果，
/// 拼装形状与 `tools_json` 同源（`assemble_tools_json`）。
fn narrowed_tools_json(effective: &[&'static str]) -> String {
    assemble_tools_json(|t| effective.contains(&t.name))
}

/// MUTATING_TOOLS 由 TOOLS_TABLE 过滤（mutating == true）。
pub fn mutating_tools() -> Vec<&'static str> {
    TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .map(|t| t.name)
        .collect()
}

/// 工具单源真相的守卫相关纯函数：结果值抽取与回声判定。
#[cfg(test)]
mod result_claim_tests {
    use super::{
        claimed_result_numbers, default_execution_claim_patterns, is_echo_call, result_numbers_in,
        states_execution_not_done,
    };

    fn patterns() -> Vec<String> {
        default_execution_claim_patterns()
    }

    /// 判据 A 的正例：执行完成态措辞 + 结果措辞 + 数值，同一段里齐了才摘。
    #[test]
    fn claimed_result_numbers_picks_execution_result_values() {
        assert_eq!(
            claimed_result_numbers("已跑完，输出 5050", &patterns()),
            vec!["5050".to_string()]
        );
        assert_eq!(
            claimed_result_numbers("执行完毕，返回 8191", &patterns()),
            vec!["8191".to_string()]
        );
        // 小数与千分位照原样摘（出处比对那侧再归一）
        assert_eq!(
            claimed_result_numbers("我跑了，结果是 2.5", &patterns()),
            vec!["2.5".to_string()]
        );
    }

    /// 算术与讲解句不摘：没有执行措辞，它们说的不是执行产物。
    /// 「1+1 的结果是 2」是 SPEC 里点名不收「结果是」的原因。
    #[test]
    fn claimed_result_numbers_skips_arithmetic_and_explanations() {
        assert!(claimed_result_numbers("1+1 的结果是 2", &patterns()).is_empty());
        assert!(claimed_result_numbers("这个函数返回了 None", &patterns()).is_empty());
        // 计数不是结果：3 是数出来的个数
        assert!(claimed_result_numbers("已跑完，共 3 步", &patterns()).is_empty());
        // 估算不是确切值
        assert!(claimed_result_numbers("实测约 5013 ms", &patterns()).is_empty());
        // 编号不是结果
        assert!(claimed_result_numbers("已执行，规则 21/19 这条我会卡死", &patterns()).is_empty());
    }

    /// 路径、日期、标识符里的数字不是结果值。
    #[test]
    fn result_numbers_in_skips_identifiers_paths_and_dates() {
        assert!(result_numbers_in("/Users/renshi/Desktop/2026Q3复盘.docx").is_empty());
        assert!(result_numbers_in("2026-10-20 的结果").is_empty());
        assert!(result_numbers_in("第 21 条").is_empty());
        assert!(result_numbers_in("1. 把新内容写进已有文件").is_empty());
        assert_eq!(result_numbers_in("输出 5050"), vec!["5050".to_string()]);
    }

    /// 判据 B 的正例：模型把结论写进参数、让工具原样吐回来。
    /// 真机原形（trace 106）：browser_evaluate 的函数体是硬编码字符串。
    #[test]
    fn is_echo_call_detects_hardcoded_assertion_echoed_back() {
        let args = r#"{"function":"async () => { return { desktopDocx: 'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found' }; }"}"#;
        let output = "### Result\n{\n  \"desktopDocx\": \"only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\"\n}\n### Ran Playwright code\n```js\nawait page.evaluate('async () => { return { desktopDocx: \\'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\\' }; }');\n```\n### Page\n- Page URL: about:blank";
        assert!(
            is_echo_call(args, output),
            "返回内容全来自入参 = 这次调用没替模型核实任何东西"
        );
    }

    /// 判据 B 的负例：真的去读了现场，返回里有入参没有的东西。
    #[test]
    fn is_echo_call_accepts_calls_that_retrieved_new_content() {
        let args = "import os\np = \"/Users/renshi/Desktop/季度复盘.docx\"\nprint(\"exists:\", os.path.exists(p))";
        let output = "exists: False\nisfile: False\ndocx on desktop: ['/Users/renshi/Desktop/2026Q3复盘.docx']";
        assert!(
            !is_echo_call(args, output),
            "返回里有入参没有的现场信息，不算回声"
        );
        // 目录列举：文件名不在入参里
        assert!(!is_echo_call(
            r#"{"dir":"/Users/renshi/Desktop","pattern":"*.docx"}"#,
            "/Users/renshi/Desktop（1 条）：\n2026Q3复盘.docx"
        ));
    }

    /// 返回得太少判不出「有没有新信息」——不猜，不算回声。
    #[test]
    fn is_echo_call_needs_enough_content_to_judge() {
        assert!(!is_echo_call(r#"{"code":"print(\"OK\")"}"#, "OK"));
    }

    /// 让开条件：终稿如实说明执行没做成（或明说那是推算）→ 结果值判据整档让开。
    #[test]
    fn states_execution_not_done_covers_failure_and_derivation_forms() {
        assert!(states_execution_not_done("我跑了，但开关没开，跑不了。"));
        assert!(states_execution_not_done("答案是 5050（按公式推算的）"));
        assert!(states_execution_not_done(
            "run_python 调用失败：Python 编程未开启"
        ));
        // 正常的执行完成态终稿不该命中让开
        assert!(!states_execution_not_done("已跑完，输出 5050"));
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use std::collections::HashSet;

    /// `TOOLS_TABLE` 里不许有 cfg-gated 条目。
    ///
    /// `BUILTIN_TOOL_COUNT` 是**手写常量**，而表长度是编译期决定的——
    /// 一旦某条工具被 `#[cfg(feature = ...)]` gate 掉，某个编译配置下
    /// 常量与表长度就不等，基线断言在**别的**配置下才炸（难查）。
    /// 这里直接扫源码当场红：加 feature-gated 工具的人立刻看到。
    #[test]
    fn tools_table_has_no_cfg_gated_entries() {
        let src = include_str!("registry.rs");
        let start = src
            .find("pub static TOOLS_TABLE")
            .expect("registry.rs 里有 TOOLS_TABLE");
        let end = start
            + src[start..]
                .find("\n];")
                .expect("TOOLS_TABLE 是以 \\n]; 收尾的静态数组");
        let body = &src[start..end];
        assert!(
            !body.contains("#[cfg("),
            "TOOLS_TABLE 里不许出现 #[cfg(...)]：表长度会随编译配置变，\
             而 BUILTIN_TOOL_COUNT 是手写常量（要么改成派生，要么别 gate 工具条目）"
        );
    }

    /// spawn_subagent schema 暴露 stage 字段（recon/executor/verifier）——锁住
    /// 三段式派发对 LLM 的契约入口；字段丢失即等于功能对模型不可见。
    #[test]
    fn spawn_subagent_schema_exposes_stage_field() {
        let v: serde_json::Value = serde_json::from_str(SCHEMA_SPAWN_SUBAGENT)
            .expect("SCHEMA_SPAWN_SUBAGENT 必须是合法 JSON");
        let stage = v
            .pointer("/function/parameters/properties/stage")
            .expect("spawn schema 必须声明 stage 字段");
        // 枚举与字符串类型 + 三档全部可枚举
        assert_eq!(stage["type"], "string");
        let enums: Vec<&str> = stage["enum"]
            .as_array()
            .expect("stage enum 必须是数组")
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        for v in ["recon", "executor", "verifier"] {
            assert!(enums.contains(&v), "stage enum 缺 {v}");
        }
        // stage 不在 required 里（与 profile 可同时给，也可只给一个）
        let required: Vec<&str> = v
            .pointer("/function/parameters/required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        assert!(
            !required.contains(&"stage"),
            "stage 是可选字段，不应进 required"
        );
    }

    /// spawn schema 暴露 `onion` 配置档，且 `profile` 不再是必填。
    ///
    /// 移出 required 消的是一处**既有矛盾**：代码一直支持「只给 stage / 配置档
    /// 让系统派生档位」，而 schema 把 profile 列为必填让模型永远先填满它，
    /// 配置档的 profile 就没有兜底生效的机会。
    #[test]
    fn spawn_subagent_schema_exposes_onion_and_makes_profile_optional() {
        let v: serde_json::Value = serde_json::from_str(SCHEMA_SPAWN_SUBAGENT)
            .expect("SCHEMA_SPAWN_SUBAGENT 必须是合法 JSON");
        let onion = v
            .pointer("/function/parameters/properties/onion")
            .expect("spawn schema 必须声明 onion 字段——否则配置档对模型不可见");
        assert_eq!(onion["type"], "string");
        // 三份内置配置档名要写进描述，模型才知道有哪些可派发
        let desc = onion["description"].as_str().unwrap_or_default();
        for name in crate::bot_onions::builtin_config_ids() {
            assert!(
                desc.contains(name),
                "onion 参数描述应列出内置配置档 {name}，否则模型无从知道有哪些"
            );
        }
        // profile 不在 required：给 stage 或给 onion 都足以定档
        let required: Vec<&str> = v
            .pointer("/function/parameters/required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        assert!(
            !required.contains(&"profile"),
            "profile 应可由 onion / stage 派生，不该是必填"
        );
        assert!(
            required.contains(&"objective") && required.contains(&"acceptanceCriteria"),
            "objective / acceptanceCriteria 仍必须必填：{required:?}"
        );
    }

    #[test]
    fn tools_table_count_matches_constant() {
        let v: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        // 数目一律从 BUILTIN_TOOL_COUNT / SUBAGENT_ONLY_TOOLS 推导，全仓只此一处
        // 硬编码工具数：注释、架构文档、本测试都引用它（数字失真过一次——
        // 注释写 33 实为 44，正是因为断言与注释各写各的）。
        assert_eq!(
            TOOLS_TABLE.len(),
            BUILTIN_TOOL_COUNT,
            "TOOLS_TABLE 全量与 BUILTIN_TOOL_COUNT 不符：增删工具后请改该常量"
        );
        assert_eq!(
            arr.len(),
            BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len(),
            "主 agent 可见工具数（表全量减去 subagent-only）与常量不符"
        );
        for name in SUBAGENT_ONLY_TOOLS {
            assert!(
                tools_index().contains_key(*name),
                "subagent-only 工具不在表里：{name}"
            );
        }

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
            "create_skill",
            "install_skill",
            "search_skills",
            "spawn_subagent",
            "check_subagent",
            "cancel_subagent",
            "reveal_path",
            "open_url",
            "clipboard_write",
            "screenshot",
            "edit_file",
            "write_file",
            "apps_list",
            "apps_create",
            "apps_update_files",
            "ask_user",
            "ast_inspect",
            "unpack_archive",
            "pack_archive",
            "manage_file",
            "copy_file_to_clipboard",
            "schedule_reminder",
            "list_reminders",
            "cancel_reminder",
            "list_windows",
            "activate_window",
            "ui_inspect",
            "ui_click",
            "ui_type",
            "wait_for_element",
            "start_service",
            "stop_service",
            "list_services",
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
            "create_skill",
            "install_skill",
            "search_skills",
            "spawn_subagent",
            "check_subagent",
            "cancel_subagent",
            "reveal_path",
            "open_url",
            "clipboard_write",
            "screenshot",
            "edit_file",
            "write_file",
            "apps_list",
            "apps_create",
            "apps_update_files",
            "ask_user",
            "ast_inspect",
            "unpack_archive",
            "pack_archive",
            "manage_file",
            "copy_file_to_clipboard",
            "schedule_reminder",
            "list_reminders",
            "cancel_reminder",
            "list_windows",
            "activate_window",
            "ui_inspect",
            "ui_click",
            "ui_type",
            "wait_for_element",
            "start_service",
            "stop_service",
            "list_services",
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
            "create_skill",
            "install_skill",
            "apps_create",
            "apps_update_files",
            "unpack_archive",
            "pack_archive",
            "manage_file",
            "schedule_reminder",
            "cancel_reminder",
            "ui_click",
            "ui_type",
            "start_service",
            "stop_service",
            "screenshot",
        ]
        .into_iter()
        .collect();
        assert_eq!(derived, expected);
    }

    /// 已下线的工具名不得再出现在任何 schema 描述里。
    ///
    /// 描述是模型看得见的说明书：让它去找一个已经不存在的工具，得到的
    /// 只会是「未知工具」，用户看到的是一次白跑的工具调用
    /// （list_tasks / search_tasks 已合并进 query_tasks）。
    #[test]
    fn retired_tool_names_absent_from_schemas() {
        const RETIRED: &[&str] = &["list_tasks", "search_tasks"];
        for t in TOOLS_TABLE {
            for gone in RETIRED {
                assert!(
                    !t.schema.contains(gone),
                    "{} 的 schema 仍提到已下线的工具 {gone}",
                    t.name
                );
            }
        }
    }

    /// 阶段 2 spec 1: TOOLS JSON 与基线一致。
    /// 读 tests/fixtures/tools_baseline.json（原 const TOOLS 全文），提取其字符串正文
    /// 反序列化后与 tools_json() 做 Value 级比对：锁住工具顺序 + 每条 schema 的
    /// 字段名 / 描述 / 参数结构。
    ///
    /// 不比字节：tools_json() 用统一的 `,\n  ` 缩进拼装，而原 const 的
    /// link_file_to_task 条目顶格写（无 2 空格），故两者只差这一处空白。
    ///
    /// 核心 28 工具基线（重排为 query_tasks；电脑辅助四工具追加在
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
        assert_eq!(
            der_arr.len(),
            base_arr.len() + 33,
            "主可见应为 28+3+4+2+1+1+3+1+13+3+2（末位 +13 = 归档两工具 + manage_file + copy_file_to_clipboard + 提醒三工具 + UI 六件套；+3 = 受管服务三工具；+2 = 远程技能安装与搜索两工具）"
        );
    }

    /// 「重生成 fixture」流程的显式入口（测试注释承诺的流程落地）：
    /// `cargo test regen_tools_baseline_fixture -- --ignored` 后人眼复核 diff。
    /// 只写前 28 项核心工具；格式无关紧要（基线比对是 Value 级）
    #[test]
    #[ignore]
    fn regen_tools_baseline_fixture() {
        let derived: serde_json::Value =
            serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
        let arr = derived.as_array().expect("tools_json() 必须是数组");
        let body = serde_json::to_string_pretty(&arr[..base_arr_len()]).unwrap();
        let text = format!("const TOOLS: &str = r#\"{body}\"#;\n");
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/tools_baseline.json"
            ),
            text,
        )
        .expect("fixture 写入失败");
    }

    fn base_arr_len() -> usize {
        let fixture = include_str!("../../tests/fixtures/tools_baseline.json");
        let r_str_marker = fixture.find("r#\"").expect("fixture 缺 r#\"") + 3;
        let end = fixture.rfind("]\"#;").expect("fixture 缺 ]\"#;") + 1;
        let baseline: serde_json::Value = serde_json::from_str(&fixture[r_str_marker..end])
            .expect("fixture 内 const TOOLS 正文必须是合法 JSON");
        baseline.as_array().unwrap().len()
    }

    /// spawn_subagent 的 maxToolCalls 描述不得写死默认条数。
    ///
    /// 真源是全局配置 maxFunctionCalls（budget 整个不给时由
    /// params::resolve_subagent_budget 取，见 params 的 loop.max_function_calls
    /// 条目），写死「默认 100」在用户改过配置后就是给模型的假信息——模型据此
    /// 判断要不要给 budget。
    #[test]
    fn spawn_subagent_tool_calls_description_points_at_the_config() {
        let v: serde_json::Value =
            serde_json::from_str(SCHEMA_SPAWN_SUBAGENT).expect("schema 常量必须是合法 JSON");
        let desc = v["function"]["parameters"]["properties"]["budget"]["properties"]
            ["maxToolCalls"]["description"]
            .as_str()
            .expect("maxToolCalls 缺 description");
        assert!(
            desc.contains("loop.max_function_calls"),
            "maxToolCalls 描述须指向全局熔断配置（设置页参数 loop.max_function_calls），实际：{desc}"
        );
        assert!(
            !desc.contains("默认 100"),
            "maxToolCalls 描述不得写死默认条数（改配置后即过期），实际：{desc}"
        );
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

    /// 每个 mutating 工具都必须至少声明一个 claims_pattern——否则该工具的
    /// 「我做完了」声称对幻觉守卫完全不可见。样本硬编码会随工具增删漂移，
    /// 这里只遍历 TOOLS_TABLE 锁「非空」不变式；具体话术能否命中由
    /// bot_model_loop 的 claims_mutation_* 用例锁。
    #[test]
    fn every_mutating_tool_has_claim_patterns() {
        for t in TOOLS_TABLE.iter().filter(|t| t.mutating) {
            assert!(
                !t.claims_patterns.is_empty(),
                "{} 是 mutating 但 claims_patterns 为空——幻觉守卫对其变更声称不可见",
                t.name
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

    /// screenshot 真写盘（PNG 落 AI_Gen_Files）——必须按变更类登记：否则
    /// 「已截屏 / 已保存截图」这类声称对幻觉守卫完全不可见，模型不调工具也能报。
    #[test]
    fn screenshot_is_registered_as_a_mutating_tool() {
        let t = TOOLS_TABLE
            .iter()
            .find(|t| t.name == "screenshot")
            .expect("screenshot 必须在工具表里");
        assert!(t.mutating, "截图真写盘，必须标 mutating");
        assert!(
            claims_mutation("已截屏：/tmp/a.png"),
            "「已截屏」应命中变更声称"
        );
        assert!(
            claims_mutation("已保存截图到 /tmp/a.png"),
            "「已保存截图」应命中变更声称"
        );
    }

    /// 截图的声称话术不得误伤「读图」：用户传图让模型看内容时，模型说的是
    /// 「已查看截图」——那是只读动作（ocr_image），打回模型等于凭空多一轮。
    #[test]
    fn screenshot_claim_patterns_do_not_fire_on_reading_an_image() {
        assert!(
            !claims_mutation("已查看截图里的内容"),
            "读图不是变更——截图的声称话术不得命中「已查看截图」"
        );
    }

    /// 剪贴板语义豁免：「已复制到剪贴板」说的是系统剪贴板（clipboard_write /
    /// copy_file_to_clipboard 都是非变更类工具），不是文件复制——而 manage_file 的
    /// 短动词「已复制」是它的子串。不豁免的话，用户让「把这段话复制到剪贴板」
    /// 会被幻觉守卫判成「声称变更但未调工具」：凭空打回模型，还给用户挂一条
    /// 「这句没有对应凭证」的更正。
    #[test]
    fn clipboard_phrasing_is_not_a_mutation_claim() {
        assert!(
            !claims_mutation("已复制到剪贴板（42 字符），可直接粘贴"),
            "剪贴板话术不是变更声称"
        );
        assert!(
            !claims_mutation("文件已放入剪贴板"),
            "剪贴板话术不是变更声称"
        );
        // 文件复制话术照常命中（豁免只在剪贴板邻域生效）
        assert!(
            claims_mutation("已复制：/a/b.txt → /c/d.txt"),
            "文件复制仍是变更声称"
        );
        // 同句前后并存时，文件复制那处不因远处出现「剪贴板」而被豁免
        assert!(
            claims_mutation("已复制 /a/b.txt 到 /c/d.txt，并把说明复制到剪贴板"),
            "同句里的文件复制那处仍要命中"
        );
    }

    /// UNC 路径不再绕过存在性校验：`\\server\share\x.docx` 形态候选此前被
    /// `/` 与盘符冒号两个前缀过滤掉，守卫对编造的 UNC 路径完全失明。
    /// 主机名用 `.invalid`（RFC 保留假 TLD，解析必败，Windows 上也快速返回）。
    /// `#[ignore]`：依赖 DNS 解析（即便 .invalid 也会查系统解析器），CI 噪声源，
    /// 手动跑：`cargo test --test registry claimed_doc_paths -- --ignored`。
    #[test]
    #[ignore = "UNC 主机名解析依赖系统 DNS resolver，CI 慢且不稳；手动 cargo test ... -- --ignored 验"]
    fn claimed_doc_paths_checks_unc_paths() {
        let text = "已生成报告，路径 \\\\nonexistent-host-.invalid\\share\\报告.docx";
        assert_eq!(
            claimed_doc_paths(text),
            vec!["\\\\nonexistent-host-.invalid\\share\\报告.docx".to_string()]
        );
        // 无声称触发词 → 不检查（与既有口径一致）
        assert!(claimed_doc_paths("\\\\nonexistent-host-.invalid\\share\\报告.docx").is_empty());
    }

    /// path tokenizer 边界契约：以下形态**不**应被拆碎成多 token，
    /// `\` 不进 boundary 集合（否则 `C:\Users\foo.docx` 拆成 `C:` + `Users` +
    /// `foo.docx` 三个碎片，盘符路径失访）。本测试用**本地可写**的 tmpdir
    /// 拼出真存在的盘符/UNC 形态路径，验 fabricated_gen_path 不返回误报。
    #[test]
    fn claimed_doc_paths_preserves_drive_letter_path() {
        // tmpdir 路径形如 /var/folders/.../T/wm-fab-XXXX——本机 fs 存在，
        // 拼一个后缀 .docx 的真存在文件做 baseline
        let tmp = std::env::temp_dir().join(format!(
            "wm-fab-drive-{}.docx",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&tmp, b"baseline").unwrap();
        // 在已存在文件旁描述一个**不存在**的同前缀路径：应被提取为候选，
        // 且存在性判据判它缺
        let ghost = tmp.with_file_name(format!("{}.docx", uuid::Uuid::new_v4().simple()));
        let text = format!("已生成 {}", ghost.display());
        // 本机绝对路径以 / 开头，已在 claimed_doc_paths 边界守卫内 → 命中
        assert_eq!(
            claimed_doc_paths(&text).first().map(|s| s.as_str()),
            Some(ghost.to_string_lossy().as_ref()),
            "本机绝对盘符路径（含 / 前缀）必须被提取为候选"
        );
        assert!(
            path_missing_on_disk(&ghost.to_string_lossy()),
            "候选路径在磁盘上不存在，存在性判据要判它缺"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    /// 边界字符不切分：本机绝对路径 / 盘符 / UNC 三种形态在 split + filter 后
    /// 仍以完整 token 形式进存在性校验（不在 boundary 处分）。
    /// 本测试**避免**测盘符路径（C:\foo）在 macOS 上的边界解析差异（macOS
    /// 把 `\` 当普通字符，行为与 Windows 不同；这正是当时 `\` 不入 boundary
    /// 的原因——盘符守卫专为 Windows 设计的）。改测**本机绝对路径**形态，
    /// 与现有 `fabricated_gen_path_preserves_drive_letter_path` 形成对照：
    /// 后者验「存在就跳过」、本测试验「不存在就命中 fabricated」。
    #[test]
    fn claimed_doc_paths_tokenizer_does_not_split_at_certain_boundaries() {
        let tmp = std::env::temp_dir();
        let ghost = tmp.join(format!("{}.docx", uuid::Uuid::new_v4().simple()));
        // 必须先创建父目录存在——否则 ghost 父路径不存在导致 exists() 在
        // Unix 上行为奇怪；这里用 temp_dir() 已经存在
        let text = format!("已生成 {}", ghost.display());
        // tmp 在 macOS 上是 /var/folders/.../T/，路径以 / 开头——
        // tok.starts_with('/') 通过守卫；Path::new(ghost).exists() 必为 false
        // （uuid 新名未创建）；find 应找到完整 token
        let got = claimed_doc_paths(&text);
        assert_eq!(
            got.first().map(|s| s.as_str()),
            Some(ghost.to_string_lossy().as_ref()),
            "本机绝对路径（含 / 前缀）必须以完整 token 进候选，不被切分"
        );
    }

    /// TOOLS_TABLE 索引长度等于 TOOLS_TABLE 长度——保证无重复 name。
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

    /// TOOLS_TABLE 里每个 name 都能在 index 里查到——索引完整覆盖。
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

    // 子 agent 白名单 schema

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
    /// general = research ∪ coder 去重。`ast_inspect` 随 run_python 一同进 coder/general，
    /// 不进 research（只读 profile 不装执行器）。
    #[test]
    fn profile_whitelists_match_design_mapping() {
        assert_eq!(
            RESEARCH_TOOLS,
            &[
                "web_search",
                "fetch_url",
                "list_files",
                "read_text_file",
                "grep_files",
                "extract_document",
                "write_artifact_file",
                "read_own_card",
            ]
        );
        assert!(
            !RESEARCH_TOOLS.contains(&"ast_inspect"),
            "research 是只读 profile，不得装 AST 执行器"
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
                "ast_inspect",
                "unpack_archive",
                "pack_archive",
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
        assert_eq!(
            tools_json_for(Some("not-a-subagent-session")).as_ref(),
            tools_json()
        );
        assert_eq!(tools_json_for(None).as_ref(), tools_json());
    }

    /// per-onion 收窄的**接线**守卫：绑了配置的会话，`tools_json_for` 真得少几个。
    ///
    /// 纯函数 `effective_tools` 的单测钉不住这条线——变异实测把
    /// `tools_json_for` 里的 `allowed` 换成一个恒空的切片（配置完全不收窄、
    /// 退回整份 profile 白名单），bot_onions 与本模块其余 32 条测试全绿，
    /// 而这套配置的全部意义就是「收窄」失效了。所以这里直接断言**最终那份
    /// JSON 里的工具名集合**，钉的是接线而不是中间量。
    #[test]
    fn tools_json_for_narrows_when_session_has_onion_config() {
        fn names_of(session_id: Option<&str>) -> Vec<String> {
            let v: serde_json::Value =
                serde_json::from_str(tools_json_for(session_id).as_ref()).expect("清单须合法");
            v.as_array()
                .expect("顶层是数组")
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        }
        let cfg = crate::bot_onions::parse(
            "onion_id: validator\nprofile: coder\nallowed: [read_text_file, grep_files]\n",
        )
        .expect("配置应合法");

        // 对照组：同一条会话，没绑配置时走默认全量清单
        crate::bot_onions::bind_for_test("reg-onion-narrow", None);
        let plain = names_of(Some("reg-onion-narrow"));
        assert_eq!(
            plain.len(),
            tools_json_names().len(),
            "未绑配置应拿默认全量"
        );

        // 实验组：绑一份只留两个工具的配置（yaml 的 profile=coder 提供上限）
        crate::bot_onions::bind_for_test("reg-onion-narrow", Some(cfg));
        let narrowed = names_of(Some("reg-onion-narrow"));
        assert_eq!(
            narrowed,
            vec!["read_text_file".to_string(), "grep_files".to_string()],
            "绑了 allowed=[read_text_file, grep_files] 的会话必须只拿到这两个"
        );
        assert!(narrowed.len() < plain.len(), "收窄必须真的变少");

        // 解除绑定回到默认（设置页换配置/解绑的路径）
        crate::bot_onions::bind_for_test("reg-onion-narrow", None);
        assert_eq!(names_of(Some("reg-onion-narrow")), plain);
    }

    /// schema 层与 dispatch 闸必须算出**同一份**工具集（期 6）。
    ///
    /// 两条路径各算一次（闸读注册时存进 ctx 的，schema 现算），一旦漂移：
    /// 漂成 schema 比闸宽 → 模型看得见却调不动，白烧一轮拿一句「白名单外」；
    /// 漂成 schema 比闸窄 → 能力被静默藏起来。本测试让两侧对着算一遍。
    #[test]
    fn gate_whitelist_and_schema_list_are_computed_from_the_same_intersection() {
        fn names_of(session_id: &str) -> Vec<String> {
            let v: serde_json::Value =
                serde_json::from_str(tools_json_for(Some(session_id)).as_ref())
                    .expect("清单须合法");
            v.as_array()
                .expect("顶层是数组")
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        }
        // 配置档自称 profile=research，运行时报 coder。allowed 里一个只属 coder
        // （write_file）、一个只属 research（web_search）——两侧算错哪一边、
        // 错几次都会在结果里露出来（本用例无区分度时这条测试就是假绿）。
        let cfg = crate::bot_onions::parse(
            "onion_id: validator\nprofile: research\nallowed: [write_file, web_search]\n",
        )
        .expect("配置应合法");
        let sid = "reg-onion-gate-parity";
        let runtime_profile = crate::db::SubagentProfile::Coder;
        let allowed = cfg.allowed.clone();
        crate::bot_onions::bind_for_test(sid, Some(cfg));
        crate::tool_guard::register_subagent_session(
            sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: "sa_parity".into(),
                task_id: None,
                profile: runtime_profile,
                tools: std::sync::Arc::new(crate::bot_onions::effective_tools(
                    runtime_profile,
                    &allowed,
                )),
                budget: crate::db::SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_parity"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );

        let gate: Vec<String> = crate::tool_guard::subagent_ctx(Some(sid))
            .expect("刚注册过")
            .tools
            .iter()
            .map(|t| t.to_string())
            .collect();
        let schema = names_of(sid);
        assert_eq!(
            schema, gate,
            "schema 清单与闸白名单必须逐项一致（都应是运行时 profile ∩ allowed）"
        );
        assert_eq!(
            schema,
            vec!["write_file".to_string()],
            "运行时 profile=coder 才是上限：web_search 只属 research，应被收掉"
        );
        crate::tool_guard::unregister_subagent_session(sid);
        crate::bot_onions::bind_for_test(sid, None);
    }

    fn tools_json_names() -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(tools_json()).expect("清单须合法");
        v.as_array()
            .expect("顶层是数组")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
            .collect()
    }

    /// yaml 的 `profile:` 不得覆盖运行时的子 agent profile（否则就是「放宽」）。
    #[test]
    fn yaml_profile_cannot_widen_beyond_runtime_profile() {
        fn names_of(session_id: Option<&str>) -> Vec<String> {
            let v: serde_json::Value =
                serde_json::from_str(tools_json_for(session_id).as_ref()).expect("清单须合法");
            v.as_array()
                .expect("顶层是数组")
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        }
        // yaml 自称 coder（含写类工具），allowed 也不收窄
        let cfg = crate::bot_onions::parse("onion_id: p\nprofile: coder\n").expect("配置应合法");
        let sid = "reg-onion-widen";
        crate::bot_onions::bind_for_test(sid, Some(cfg));
        // 没有子 agent 上下文时 yaml 的 profile 兜底生效（配置本来的用法）
        assert_eq!(names_of(Some(sid)).len(), CODER_TOOLS.len());
    }

    // 阶段 3：MCP 动态挂载点

    /// 无 MCP 连接：tools_json_with_mcp 必须原样借用静态 JSON（零分配、零漂移）。
    /// 与 manager e2e 共用测试锁：e2e 的连接存活窗口会让「无连接」断言
    /// 在 cargo test 并行下 flaky；先清残留（前序用例 panic 可能留下连接槽）。
    #[test]
    fn tools_json_with_mcp_without_connections_is_borrowed_static() {
        // blocking_lock 须在 block_on 进入 runtime 上下文之前取（tokio Mutex 语义）
        let _serial = crate::bot::mcp::manager::SHARED_MCP_TEST_LOCK.blocking_lock();
        tauri::async_runtime::block_on(crate::bot::mcp::manager::shared().shutdown());
        let with = tools_json_with_mcp(None);
        assert!(matches!(with, std::borrow::Cow::Borrowed(_)));
        assert_eq!(with.as_ref(), tools_json());
        // 子 agent 会话短路：不碰 MCP（挂载层根本不被调用）
        let sub = tools_json_with_mcp(Some("not-a-subagent-session"));
        assert!(matches!(sub, std::borrow::Cow::Borrowed(_)));
    }

    /// 摘尾拼接契约——base 正常时摘尾接段补收尾；契约破坏（未以 \n] 收尾）
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
        // body 侧对偶契约（评审）：动态段不以 ",\n" 起头（mount 层构造破坏）
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

    /// 三份清单共用 `assemble_tools_json` 后形状恒等——这正是
    /// `splice_mcp_body` 依赖的「恒以 `\n]` 收尾」契约。
    /// 契约破坏以前只能靠限频 eprintln 事后发现；现在由构造保证，这里钉住。
    #[test]
    fn all_three_assemblies_share_the_same_shape() {
        use crate::db::SubagentProfile;
        let base = tools_json();
        assert!(base.starts_with("[\n  "), "首项缩进两格：{base:.20}");
        assert!(
            base.ends_with("\n]"),
            "必须以 \\n] 收尾：{:?}",
            &base[base.len()..]
        );
        for p in [
            SubagentProfile::Research,
            SubagentProfile::Coder,
            SubagentProfile::General,
        ] {
            let s = profile_tools_json(p);
            assert!(
                s.starts_with("[\n  ") && s.ends_with("\n]"),
                "{} 档形状与主清单不一致",
                p.as_str()
            );
        }
        // 交集清单（owned 路径）同样形状
        let narrowed = super::narrowed_tools_json(&["run_python"]);
        assert!(narrowed.starts_with("[\n  ") && narrowed.ends_with("\n]"));
        // 空集也要有合法形状（模型该收到空清单，不是默认清单）
        assert_eq!(super::narrowed_tools_json(&[]), "[\n]");
    }

    /// 拼接内核只有一份判定：借用版与 owned 版对同一输入必须同进同退。
    #[test]
    fn splice_both_variants_share_one_kernel() {
        let cases = [
            ("[\n  {}\n]", ",\n  {}"), // 正常
            ("[\n  {}\n]", "garbage"), // body 契约破坏
            ("[broken", ",\n  {}"),    // base 契约破坏
        ];
        for (base, body) in cases {
            let borrowed = super::splice_mcp_body(base, body);
            let owned = super::splice_mcp_body_owned(base.to_string(), body);
            assert_eq!(
                borrowed.as_ref(),
                owned.as_str(),
                "两版对 base={base:?} body={body:?} 必须给出同一结果"
            );
        }
    }

    /// 动态拼装契约（纯格式）：拿 mount 的纯函数造一个假 body，
    /// 拼装结果必须是合法 JSON 数组、内置 BUILTIN_TOOL_COUNT 个工具 schema 在前 + MCP 条目在后。
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
        // 数目从常量推导而非写死：增删内置工具时这里跟着走，不会再漂
        let main_visible = BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len();
        assert_eq!(
            arr.len(),
            main_visible + 1,
            "{main_visible} 内置主可见工具 + 1 假 MCP"
        );
        assert_eq!(arr[main_visible]["function"]["name"], "mcp_fake_x");
        // 内置前 main_visible 项顺序不变（增量挂载不漂移）
        let base_arr = serde_json::from_str::<serde_json::Value>(base).unwrap();
        assert_eq!(
            &arr[..main_visible],
            base_arr.as_array().unwrap().as_slice()
        );
    }
}
