//!
//! 单源真相 - 消除三源漂移（TOOLS / MUTATING_TOOLS / execute_tool_impl 大 match）。
//! - TOOLS JSON 由 `tools_json()` 从 TOOLS_TABLE 顺序拼装
//! - MUTATING_TOOLS 由 `mutating_tools()` 从 TOOLS_TABLE 过滤（mutating == true）
//! - execute_tool_impl 通过 TOOLS_TABLE lookup + fn 指针 call
//!
//! schema 常量是原 const TOOLS 的逐条字节拷贝（基线全文见 tests/fixtures/tools_baseline.json）；
//! tools_json() 按 TOOLS_TABLE 顺序以统一 `,\n  ` 缩进拼接，与原 const 只差
//! link_file_to_task 条目前的 2 空格（原 const 该条目顶格写），反序列化后语义一致。
//!
//! 本文件是门面：实现按面拆到 `registry/` 子模块（schemas 字面量与工具表 /
//! handlers 转发 / claims 声明判据），这里只留类型、use 与逐项转出，
//! `crate::bot::registry::X` 旧路径与旧可见性保持不变。

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use tauri::AppHandle;

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

mod claims;
mod handlers;
mod schemas;

// 拆出后的子模块转出：保持 `crate::bot::registry::X` 旧路径与旧可见性不变
pub use claims::{
    claimed_doc_paths, claimed_executed_tools, claimed_result_numbers, claims_execution,
    claims_execution_with, claims_mutation, default_execution_claim_patterns, is_echo_call,
    mentioned_tool_names, numbers_in_text, path_missing_on_disk, states_a_result,
    states_execution_not_done,
};
pub use schemas::{
    mutating_tools, profile_tools_json, profile_whitelist, tools_index, tools_json, tools_json_for,
    tools_json_with_mcp, BUILTIN_TOOL_COUNT, CODER_TOOLS, GENERAL_TOOLS, RESEARCH_TOOLS,
    SCHEMA_ACTIVATE_WINDOW, SCHEMA_ADD_SUBTASK, SCHEMA_APPS_CREATE, SCHEMA_APPS_LIST,
    SCHEMA_APPS_UPDATE_FILES, SCHEMA_ASK_USER, SCHEMA_AST_INSPECT, SCHEMA_CANCEL_REMINDER,
    SCHEMA_CANCEL_SUBAGENT, SCHEMA_CHECK_SUBAGENT, SCHEMA_CLIPBOARD_WRITE, SCHEMA_COMPLETE_TASK,
    SCHEMA_COPY_FILE_TO_CLIPBOARD, SCHEMA_CREATE_EXCEL, SCHEMA_CREATE_PDF, SCHEMA_CREATE_PPT,
    SCHEMA_CREATE_SKILL, SCHEMA_CREATE_TASK, SCHEMA_CREATE_WORD, SCHEMA_CREATE_WORD_REVISIONS,
    SCHEMA_DELETE_TASK, SCHEMA_EDIT_FILE, SCHEMA_EDIT_TASK, SCHEMA_EXTRACT_DOCUMENT,
    SCHEMA_FETCH_URL, SCHEMA_GET_CURRENT_TIME, SCHEMA_GREP_FILES, SCHEMA_INSTALL_SKILL,
    SCHEMA_LINK_FILE_TO_TASK, SCHEMA_LIST_FILES, SCHEMA_LIST_REMINDERS, SCHEMA_LIST_SERVICES,
    SCHEMA_LIST_WINDOWS, SCHEMA_LOOKUP_RECEIPT, SCHEMA_MANAGE_FILE, SCHEMA_OCR_IMAGE,
    SCHEMA_OPEN_URL, SCHEMA_PACK_ARCHIVE, SCHEMA_QUERY_SINGLE_TASK, SCHEMA_QUERY_TASKS,
    SCHEMA_READ_OWN_CARD, SCHEMA_READ_TEXT_FILE, SCHEMA_RECALL_FACTS, SCHEMA_RECORD_LESSON,
    SCHEMA_REMEMBER_FACT, SCHEMA_REMOVE_SUBTASK, SCHEMA_REVEAL_PATH, SCHEMA_RUN_PYTHON,
    SCHEMA_SCHEDULE_REMINDER, SCHEMA_SCREENSHOT, SCHEMA_SEARCH_SKILLS, SCHEMA_SPAWN_SUBAGENT,
    SCHEMA_START_SERVICE, SCHEMA_STOP_SERVICE, SCHEMA_TOGGLE_SUBTASK, SCHEMA_UI_CLICK,
    SCHEMA_UI_INSPECT, SCHEMA_UI_TYPE, SCHEMA_UNPACK_ARCHIVE, SCHEMA_UPDATE_PLAN, SCHEMA_USE_SKILL,
    SCHEMA_WAIT_FOR_ELEMENT, SCHEMA_WEB_SEARCH, SCHEMA_WRITE_ARTIFACT_FILE, SCHEMA_WRITE_FILE,
    SUBAGENT_ONLY_TOOLS, TOOLS_TABLE,
};

// 仅测试用到的内部项：转出后由 registry/*_tests.rs 经 `use super::*` 取用
#[cfg(test)]
use claims::result_numbers_in;
#[cfg(test)]
use schemas::{narrowed_tools_json, splice_mcp_body, splice_mcp_body_owned};

#[cfg(test)]
mod claimed_executed_tools_tests;
#[cfg(test)]
mod mentioned_tool_names_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod result_claim_tests;
