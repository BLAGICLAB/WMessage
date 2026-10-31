//! 工具实现层（22 个 tool_* 函数 + 测试段）。
//!
//! 原 bot.rs 行 1393–2661 全部工具代码 +
//! 行 3070–3138 (tool_extract_document_tests) /
//! 行 3195–3344 (task_files_arg_tests) /
//! 行 3346–3370 (phase4_facts_tests) /
//! 行 3372–3390 (background_dialog_tests) 测试段搬入。
//!
//! 全部 tool_* / files_audit_kv 均为 pub(crate) fn（跨子模块可见，dispatch.rs 经 use 调）；
//! 外部入口为 bot::dispatch::execute_tool（pub）。
//!
//! 本文件是门面：实现按面拆到 `tools/` 子模块（common / task / doc / service），
//! 测试段拆成 `tools/<name>.rs`；这里只留类型、use 与逐项转出，
//! `crate::bot::tools::X` 旧路径与旧可见性保持不变。

use tauri::{AppHandle, Emitter};

use crate::bot::dispatch::{commit_and_report, parse_args};
use crate::bot::format::status_label;
use crate::bot::registry::ToolResult;
use crate::bot::{
    audit_log, check_len, escape_for_log, load_config, MAX_DUE, MAX_KEYWORD, MAX_NOTE,
    MAX_SUBTASK_TEXT, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE,
};
use crate::db::{prepare_for_upsert, TaskStatus};
use crate::error::CommandError;

mod common;
mod doc;
mod http;
mod plan;
mod service;
mod task;
mod trace;

// 拆出后的子模块转出：保持 `crate::bot::tools::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub use common::broadcast_after_mutation;
pub(crate) use common::{files_audit_kv, tool_get_current_time};
pub(crate) use doc::{
    tool_create_excel, tool_create_pdf, tool_create_ppt, tool_create_word,
    tool_create_word_revisions, tool_extract_document, tool_fetch_url, tool_run_python,
    tool_web_search,
};
pub(crate) use http::{
    preview_args as preview_http_request_args, redact_result_text as redact_http_result_text,
    tool_http_request,
};
pub(crate) use plan::tool_update_plan;
#[cfg(debug_assertions)]
pub use service::{
    test_hook_tool_list_services, test_hook_tool_start_service, test_hook_tool_stop_service,
};
pub(crate) use service::{tool_list_services, tool_start_service, tool_stop_service};
pub(crate) use task::{
    tool_add_subtask, tool_complete_task, tool_create_task, tool_delete_task, tool_edit_task,
    tool_link_file_to_task, tool_query_single_task, tool_query_tasks, tool_remove_subtask,
    tool_toggle_subtask,
};
pub(crate) use trace::tool_lookup_receipt;

// 跨模块共用项：门面转出后由 tools/ 各子模块经 `use super::*` 取用
use common::{active_tasks, sanitize_task_files_arg};
use doc::probe_paths_sync;

// 仅测试用到的内部项：转出后由 tools/*_tests.rs 经 `use super::*` 取用
#[cfg(test)]
use common::{parse_task_files_arg, sanitize_task_files_in};
#[cfg(test)]
use doc::{
    filter_images_in, format_extract_output, parse_search_count, pdf_text_looks_empty,
    slice_fetch_output, EXTRACT_DEFAULT_LIMIT,
};
#[cfg(test)]
use task::{
    due_param_denied, keyword_hit, locate_subtask, parse_limit, parse_view, render_task_line,
    resolve_task_key, tag_keep, view_keep, TaskView,
};

#[cfg(test)]
mod background_dialog_tests;
#[cfg(test)]
mod n3_toolpolish_tests;
#[cfg(test)]
mod phase4_facts_tests;
#[cfg(test)]
mod query_tasks_tests;
#[cfg(test)]
mod sync_syscall_off_runtime_tests;
#[cfg(test)]
mod task_files_arg_tests;
#[cfg(test)]
mod tool_due_format_tests;
#[cfg(test)]
mod tool_extract_document_tests;
#[cfg(test)]
mod tool_run_python_tests;
#[cfg(test)]
mod tool_web_search_arg_tests;
#[cfg(test)]
mod trace_tests;
