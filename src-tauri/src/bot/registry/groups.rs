//! 工具分组按需挂载（差距收编方案 D-3）：主聊天的休眠组 + activate_tools 元工具。
//!
//! 设计要点（方案 §四）：
//! - 「13 组」是文档归类口径，这里第一次落成代码结构：组名 → 工具名清单。
//! - 缩面必须双保险：schema 层（default_tools_json 不含休眠组）+ dispatch
//!   兜底闸（deferred_gate，幻觉调用休眠工具必须拒）——同子 agent 白名单构。
//! - 子 agent 白名单链路与本模块正交：profile 交集在 tools_json_for 更早分支
//!   短路，子 agent 永远走白名单（白名单外 0.55 闸先拒），不到这里。
//! - 会话激活状态是进程级 store（同 tool_guard::SESSION_ORIGINS /
//!   plan_state 模式）：tools_json_for 无 app 参数，AppState 拿不到。
//! - 激活语义：activate_tools 写 store → 下一轮 prepare_round 重取 schema
//!   即含新组（每次激活 tools 参数变化，缓存链从 tools 层起重算一次——
//!   低频动作，方案 §十一偏离 1 登记在案）。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// 13 组工具清单（组名 → 工具名）。顺序无关，清单是组结构的唯一真相源。
pub const TOOL_GROUPS: &[(&str, &[&str])] = &[
    (
        "tasks",
        &[
            "query_tasks",
            "query_single_task",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "link_file_to_task",
        ],
    ),
    (
        "file_read",
        &[
            "read_text_file",
            "ocr_image",
            "grep_files",
            "list_files",
            "extract_document",
        ],
    ),
    ("file_write", &["edit_file", "write_file"]),
    (
        "docs",
        &[
            "create_word",
            "create_word_revisions",
            "create_excel",
            "create_ppt",
            "create_pdf",
        ],
    ),
    ("web", &["web_search", "fetch_url", "http_request"]),
    ("python", &["run_python", "ast_inspect"]),
    (
        "memory",
        &["remember_fact", "recall_facts", "record_lesson"],
    ),
    (
        "skills",
        &[
            "use_skill",
            "create_skill",
            "install_skill",
            "search_skills",
        ],
    ),
    (
        "subagent",
        &["spawn_subagent", "check_subagent", "cancel_subagent"],
    ),
    (
        "desktop",
        &[
            "reveal_path",
            "open_url",
            "clipboard_write",
            "screenshot",
            "copy_file_to_clipboard",
            "list_windows",
            "activate_window",
            "ui_inspect",
            "ui_click",
            "ui_type",
            "wait_for_element",
        ],
    ),
    (
        "services",
        &["start_service", "stop_service", "list_services"],
    ),
    ("apps", &["apps_list", "apps_create", "apps_update_files"]),
    (
        "misc",
        &[
            "ask_user",
            "update_plan",
            "activate_tools",
            "get_current_time",
            "lookup_receipt",
            "schedule_reminder",
            "list_reminders",
            "cancel_reminder",
            "unpack_archive",
            "pack_archive",
            "manage_file",
            "write_artifact_file",
            "read_own_card",
        ],
    ),
];

/// 默认休眠组：桌面系统（11 工具，独立总开关默认关的 UI 自动化族为主）/
/// 受管服务（3）/ 程序（3）。量化数字见本文件量化测试与方案 §4.5 回填。
pub const DEFERRED_GROUPS: &[&str] = &["desktop", "services", "apps"];

/// system 广告行：让模型知道休眠能力存在（防「不知道所以不用」）。
/// 开关开启时进 GenDir 槽（每轮同值，缓存前缀稳定）。
pub const DORMANT_HINT: &str = "## 休眠工具组\n桌面系统、受管服务、程序（apps）三组工具默认休眠，不在当前工具清单里。需要时先调用 activate_tools（groups 传组名）启用，下一轮即可使用；不需要就不启用，省上下文。";

fn deferred_tool_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        TOOL_GROUPS
            .iter()
            .filter(|(g, _)| DEFERRED_GROUPS.contains(g))
            .flat_map(|(_, names)| names.iter().copied())
            .collect()
    })
}

/// 组清单覆盖完整性守卫：TOOL_GROUPS 的并集必须等于内置工具全集（否则新工具
/// 忘了入组，休眠判定会漏）。测试锁住；添加新工具时同步本表。
#[cfg(test)]
fn all_grouped_names() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        TOOL_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect()
    })
}

/// 分组挂载总开关（灰度，默认关=行为零变化）。同步点：bot-config 的
/// `deferredToolGroups` 字段——load_config 与 update_config_file 两处回写。
static GROUPS_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn groups_enabled() -> bool {
    GROUPS_ENABLED.load(Ordering::Relaxed)
}

pub fn set_groups_enabled(v: bool) {
    GROUPS_ENABLED.store(v, Ordering::Relaxed);
}

/// 会话级已激活休眠组（进程级 store，tools_json_for 无 app 参数）
fn active_groups() -> &'static Mutex<HashMap<String, HashSet<&'static str>>> {
    static MAP: OnceLock<Mutex<HashMap<String, HashSet<&'static str>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// activate_tools 的执行体：groups 只接受休眠组名；子 agent 会话拒（白名单
/// 链路正交，不该走到这）。返回确认文案。
pub fn activate_groups(session_id: &str, groups: &[String]) -> Result<String, String> {
    if groups.is_empty() {
        return Err("groups 不能为空（可用的休眠组：desktop / services / apps）".into());
    }
    let mut activated = Vec::new();
    {
        let mut map = active_groups().lock().unwrap_or_else(|p| p.into_inner());
        let entry = map.entry(session_id.to_string()).or_default();
        for g in groups {
            if !DEFERRED_GROUPS.contains(&g.as_str()) {
                return Err(format!(
                    "未知或非休眠组：{g}（只有 desktop / services / apps 三组可激活）"
                ));
            }
            entry.insert(leak_group_name(g)?);
            activated.push(g.clone());
        }
    }
    Ok(format!(
        "已启用工具组：{}。下一轮起这些工具进入你的工具清单（本轮请求已发出，看不到变化）。",
        activated.join("、")
    ))
}

/// 组名 → 'static（DEFERRED_GROUPS 白名单校验通过后才 leak，集合有限可控）
fn leak_group_name(g: &str) -> Result<&'static str, String> {
    DEFERRED_GROUPS
        .iter()
        .find(|n| **n == g)
        .copied()
        .ok_or_else(|| format!("未知休眠组：{g}"))
}

/// dispatch 兜底闸（0.55 allowed-tools 之后、0.6 规则闸之前插）：开关关或
/// 非休眠工具 → None 放行；休眠且该会话未激活 → Denied（文案含 activate_tools
/// 指引，模型可自愈）。子 agent 会话永远拒（休眠工具不在其白名单，0.55 已拒，
/// 这里是纵深）。
pub fn deferred_gate(
    name: &str,
    session_id: Option<&str>,
) -> Option<crate::bot::registry::ToolResult> {
    if !groups_enabled() || !deferred_tool_set().contains(name) {
        return None;
    }
    let active = session_id
        .and_then(|sid| {
            active_groups()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(sid)
                .cloned()
        })
        .map(|set| set.iter().any(|g| group_contains(g, name)))
        .unwrap_or(false);
    if active {
        return None;
    }
    Some(crate::bot::registry::ToolResult::denied(
        "该工具属于休眠组（桌面系统/受管服务/程序），当前未启用。先调用 activate_tools（groups 传组名）启用，下一轮即可使用。",
    ))
}

fn group_contains(group: &str, name: &str) -> bool {
    TOOL_GROUPS
        .iter()
        .filter(|(g, _)| *g == group)
        .any(|(_, names)| names.contains(&name))
}

/// 会话工具清单拼装：默认组（OnceLock 缓存静态串）+ 已激活组现拼。
/// 无激活 → 借静态串（热路径零分配）；拼装走 registry::assemble 的过滤闭包，
/// JSON 合法性由消费端 fail-soft + 本文件测试锁。
pub fn session_tools_json(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    let active: Vec<&'static str> = session_id
        .and_then(|sid| {
            active_groups()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(sid)
                .cloned()
        })
        .map(|set| {
            let mut v: Vec<&'static str> = set.into_iter().collect();
            v.sort_unstable();
            v
        })
        .unwrap_or_default();
    if active.is_empty() {
        return std::borrow::Cow::Borrowed(default_tools_json());
    }
    let deferred = deferred_tool_names_of(&active);
    // TOOLS_TABLE 顺序即拼装序：与全量清单同一稳定顺序；休眠组工具只挂该会话
    // 已激活的那部分
    let body = crate::bot::registry::assemble_tools_json_body(|t| {
        (!deferred_tool_set().contains(t.name) || deferred.contains(&t.name))
            && !crate::bot::registry::SUBAGENT_ONLY_TOOLS.contains(&t.name)
    });
    std::borrow::Cow::Owned(format!("[\n{body}\n]"))
}

fn deferred_tool_names_of(groups: &[&'static str]) -> Vec<&'static str> {
    TOOL_GROUPS
        .iter()
        .filter(|(g, _)| groups.contains(g))
        .flat_map(|(_, names)| names.iter().copied())
        .collect()
}

/// 默认（休眠组不挂）的工具清单 JSON：OnceLock 缓存静态串
fn default_tools_json() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        let body = crate::bot::registry::assemble_tools_json_body(|t| {
            !deferred_tool_set().contains(t.name)
                && !crate::bot::registry::SUBAGENT_ONLY_TOOLS.contains(&t.name)
        });
        format!("[\n{body}\n]")
    })
}

/// activate_tools 工具 handler：写会话激活 store，返回确认文案。
pub fn tool_activate_tools(
    session_id: Option<&str>,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let Some(sid) = session_id else {
        return crate::bot::registry::ToolResult::denied("activate_tools 需要会话上下文");
    };
    let v = crate::bot::parse_args(args);
    let groups: Vec<String> = v["groups"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if groups.is_empty() {
        // 单串容错：groups: "desktop" 也收
        if let Some(one) = v["groups"].as_str() {
            return match activate_groups(sid, std::slice::from_ref(&one.to_string())) {
                Ok(msg) => crate::bot::registry::ToolResult::ok(msg, Vec::new()),
                Err(e) => crate::bot::registry::ToolResult::error(e, Vec::new()),
            };
        }
        return crate::bot::registry::ToolResult::error(
            "groups 缺失：传组名数组，如 [\"desktop\"]（可用：desktop / services / apps）",
            Vec::new(),
        );
    }
    match activate_groups(sid, &groups) {
        Ok(msg) => crate::bot::registry::ToolResult::ok(msg, Vec::new()),
        Err(e) => crate::bot::registry::ToolResult::error(e, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 组清单覆盖完整性：TOOL_GROUPS 并集 == TOOLS_TABLE 全集（新增工具忘入组
    /// 会在这红）
    #[test]
    fn groups_cover_every_builtin_tool() {
        let table: HashSet<&str> = crate::bot::registry::TOOLS_TABLE
            .iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(
            &table,
            all_grouped_names(),
            "组清单与 TOOLS_TABLE 必须一一对应"
        );
    }

    /// 量化（方案 §4.5 回填数据源）：cargo test --lib registry::groups -- --nocapture
    #[test]
    fn quantify_group_sizes() {
        let full = crate::bot::registry::tools_json();
        let default = default_tools_json();
        println!("全量清单 = {} 字符", full.len());
        println!("默认清单 = {} 字符", default.len());
        for g in DEFERRED_GROUPS {
            let n: usize = TOOL_GROUPS
                .iter()
                .filter(|(name, _)| *name == *g)
                .flat_map(|(_, names)| names.iter())
                .filter_map(|n| {
                    crate::bot::registry::TOOLS_TABLE
                        .iter()
                        .find(|t| t.name == *n)
                        .map(|t| t.schema.len())
                })
                .sum();
            println!("休眠组 {g} = {n} 字符（{n} 为 schema 原文合计）");
        }
        assert!(default.len() < full.len(), "默认清单必须小于全量");
    }

    #[test]
    fn default_json_excludes_deferred_and_subagent_only() {
        let j = default_tools_json();
        for name in deferred_tool_set() {
            assert!(
                !j.contains(&format!("\"name\":\"{name}\""))
                    && !j.contains(&format!("\"name\": \"{name}\"")),
                "休眠工具 {name} 不得出现在默认清单"
            );
        }
        assert!(j.contains("activate_tools"), "activate_tools 必须常挂");
    }

    #[test]
    fn activate_then_session_json_includes_group() {
        set_groups_enabled(true);
        let sid = "s-groups-test";
        assert!(session_tools_json(Some(sid)).starts_with('['));
        let before = session_tools_json(Some(sid));
        assert!(
            !before.contains("\"start_service\""),
            "未激活前休眠工具不挂"
        );
        activate_groups(sid, &["services".to_string()]).unwrap();
        let after = session_tools_json(Some(sid));
        assert!(after.contains("start_service"), "激活后组工具挂载");
        assert!(!after.contains("\"list_windows\""), "其他休眠组仍不挂");
        set_groups_enabled(false);
        active_groups()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    #[test]
    fn deferred_gate_blocks_only_when_enabled_and_inactive() {
        set_groups_enabled(false);
        assert!(
            deferred_gate("start_service", Some("s-x")).is_none(),
            "开关关=直通"
        );
        set_groups_enabled(true);
        let r = deferred_gate("start_service", Some("s-x"));
        assert!(r.is_some(), "开关开且未激活=拒");
        activate_groups("s-x", &["services".to_string()]).unwrap();
        assert!(
            deferred_gate("start_service", Some("s-x")).is_none(),
            "激活后放行"
        );
        assert!(
            deferred_gate("read_text_file", Some("s-x")).is_none(),
            "非休眠工具直通"
        );
        set_groups_enabled(false);
        active_groups()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    #[test]
    fn activate_rejects_unknown_group_and_empty() {
        assert!(activate_groups("s-y", &[]).is_err());
        assert!(
            activate_groups("s-y", &["tasks".to_string()]).is_err(),
            "非休眠组拒"
        );
        assert!(activate_groups("s-y", &["bogus".to_string()]).is_err());
    }
}
