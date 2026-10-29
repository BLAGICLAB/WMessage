//! 子 agent 的五个 LLM 工具实现。
//!
//! 工具层只做参数解析与回执组装，派发 / 查询 / 取消落到编排器的对应入口。

use crate::bot_orchestrator::{
    cancel_subagent_async, check_subagent, db_locked, resolve_spawn_profile, spawn_subagent,
    SpawnAck, SpawnOnion, SpawnRequest, SubagentStage, CHECK_WAIT_MS_CAP,
};
use crate::bot_orchestrator::{DEFAULT_MAX_TOOL_CALLS, DEFAULT_MAX_TURNS};
use crate::db::{SubagentBudget, SubagentProfile, SubagentStatus};
use crate::error::CommandError;
use std::path::PathBuf;
use tauri::AppHandle;

/// 子 agent 系统提示的产物目录规则段（gen_dir 硬约束，同 EXECUTE 提示口径）。
pub(crate) fn gen_dir_rule_for(artifact_dir: &std::path::Path) -> String {
    format!(
        "## 产物目录（硬性约束）\n你的全部文件写入必须落在：{}\n目录外写入会被拒绝；报告/数据文件写进该目录并在收尾 JSON 的 artifacts 里登记路径。",
        artifact_dir.display()
    )
}
fn parse_tool_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
}
/// spawn_subagent 工具（递归双保险②：子 agent 会话调用 → 服务端身份校验拒绝）。
pub async fn tool_spawn_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error(
            "子 agent 不得派发子 agent（递归禁用）。请专注完成自己的任务包装目标。",
            Vec::new(),
        );
    }
    let v = parse_tool_args(args);
    let objective = v["objective"].as_str().unwrap_or_default().to_string();
    // stage 可选：recon/executor/verifier 三段式派发阶段（设计 §8.5）。
    // profile 可选：不给时按 配置档 → stage 派生（见 resolve_spawn_profile）。
    let stage = v["stage"].as_str().and_then(SubagentStage::from_str);
    // 配置档：名字非法 / 文件不在 / YAML 语法错 / 字段不合法都在这里报错。
    // 放在建行之前——加载失败的配置档若留到 runner 才炸，就是一条永远跑不起来的行。
    let onion_cfg = match v["onion"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => match crate::bot_onions::load(app, id) {
            Ok(cfg) => Some(cfg),
            Err(e) => return ToolResult::error(format!("spawn 失败：{e}"), Vec::new()),
        },
        None => None,
    };
    let profile = match resolve_spawn_profile(
        v["profile"].as_str().and_then(SubagentProfile::from_str),
        onion_cfg.as_ref().and_then(|c| c.profile_enum()),
        stage,
    ) {
        Some(p) => p,
        None => {
            return ToolResult::error(
                "spawn 失败：profile 必须是 research / coder / general 之一；或给 onion=配置档名；或给 stage=recon/executor/verifier 让系统按默认派生",
                Vec::new(),
            );
        }
    };
    let criteria: Vec<String> = v["acceptanceCriteria"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if criteria.is_empty() {
        return ToolResult::error(
            "spawn 失败：acceptanceCriteria 必填且每条可检验",
            Vec::new(),
        );
    }
    let budget = match &v["budget"] {
        serde_json::Value::Object(_) => Some(
            SubagentBudget {
                max_turns: v["budget"]["maxTurns"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TURNS as u64) as u32,
                max_tool_calls: v["budget"]["maxToolCalls"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TOOL_CALLS as u64)
                    as u32,
            }
            .clamped(),
        ),
        _ => None,
    };
    let req = SpawnRequest {
        objective,
        profile,
        stage,
        acceptance_criteria: criteria,
        context_summary: v["contextSummary"].as_str().map(str::to_string),
        budget,
        model_profile: v["modelProfile"].as_str().map(str::to_string),
        onion: onion_cfg.as_ref().map(|c| SpawnOnion {
            config: c.onion_id.clone(),
            max_turn: c.max_turn,
        }),
        parent_task_id: v["parentTaskId"].as_str().map(str::to_string),
        parent_session_id: session_id,
    };
    match spawn_subagent(app, req).await {
        Ok(ack) => ToolResult::ok(
            spawn_receipt_text(&ack),
            // 无卡：不挂 TaskRef（指向已不存在的卡，前端会渲染成死链）
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("spawn 失败：{}", e.message()), Vec::new()),
    }
}
/// spawn 的工具回执文案。命中幂等键时要说清「没新建、还是那个」——
/// 模型读到「已派发」会以为又多了一个子 agent，转述给用户时就会说错。
///
/// 回执里回显**生效**预算：调用方给的数会被钳到硬顶，模型不知道它
/// 的子 agent 实际能跑多少，就会照着自己给的数安排后续动作。
pub(crate) fn spawn_receipt_text(ack: &SpawnAck) -> String {
    let budget_note = format!(
        "生效预算：轮数 {} / 工具调用 {}。",
        ack.budget.max_turns, ack.budget.max_tool_calls
    );
    if ack.deduped {
        format!(
            "同一目标已有在跑的子 agent，未重复派发：subagentId={} status={}。{budget_note}继续用 check_subagent 轮询它。",
            ack.subagent_id,
            ack.status.as_str()
        )
    } else {
        format!(
            "已派发子 agent（非阻塞）：subagentId={} status={}。{budget_note}用 check_subagent 轮询结果，先继续回应用户。",
            ack.subagent_id,
            ack.status.as_str()
        )
    }
}
/// check_subagent 工具（幂等可轮询；waitMs 上限 600000，等待期可被 /stop 打断）。
pub async fn tool_check_subagent(
    app: &AppHandle,
    args: &str,
    stop: Option<crate::bot_slash::StopToken>,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::error("check 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let wait_ms = v["waitMs"].as_u64().unwrap_or(0).min(CHECK_WAIT_MS_CAP);
    match check_subagent(app, &key, wait_ms, stop.as_ref(), session_id.as_deref()).await {
        Ok(state) => ToolResult::ok(
            serde_json::to_string(&state).unwrap_or_else(|_| state.status.as_str().to_string()),
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("check 失败：{}", e.message()), Vec::new()),
    }
}
/// cancel_subagent 工具（与任务卡停止按钮同 API；子 agent 会话禁调）。
pub async fn tool_cancel_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error("子 agent 不得取消其他子 agent。", Vec::new());
    }
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::error("cancel 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let reason = v["reason"].as_str().unwrap_or("主 agent 取消").to_string();
    match cancel_subagent_async(app, &key, &reason).await {
        Ok(ack) => ToolResult::ok(
            if ack.already_terminal {
                format!(
                    "子 agent {} 已是终态（{}），无需取消",
                    ack.subagent_id,
                    ack.status.as_str()
                )
            } else {
                format!("已取消子 agent {}", ack.subagent_id)
            },
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("cancel 失败：{}", e.message()), Vec::new()),
    }
}
/// write_artifact_file 工具：仅限子 agent 会话 + 产物目录内（设计 §6 产物目录隔离）。
pub async fn tool_write_artifact_file(
    _app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("write_artifact_file 仅子 agent 执行会话可用。", Vec::new());
    };
    let v = parse_tool_args(args);
    let filename = v["filename"].as_str().unwrap_or_default().trim();
    let content = v["content"].as_str().unwrap_or_default();
    // content 上限（ 采纳）：失控子 agent 循环写大文件可填满磁盘——
    // 8MB 与 registry 长输出工具软上限同量级，报告/数据文件足够
    const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
    if content.len() > MAX_ARTIFACT_BYTES {
        return ToolResult::error(
            format!(
                "写产物失败：content {} 字节超上限 {}（8MB）",
                content.len(),
                MAX_ARTIFACT_BYTES
            ),
            Vec::new(),
        );
    }
    // 纯文件名校验：拒绝路径分隔符、NUL（POSIX 截断/Windows 路径组件）
    // 与 Windows 保留设备名；.. 按组件判定而非子串——
    // 不误伤 foo..bar / ...txt 这类合法名
    const WIN_RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem_upper = filename
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
        || filename.chars().any(|c| c.is_control())
        || WIN_RESERVED.contains(&stem_upper.as_str())
    {
        return ToolResult::error(
            "写产物失败：filename 必须是纯文件名（不含路径分隔符、`.`/`..`、控制字符；Windows 保留设备名亦拒绝）",
            Vec::new(),
        );
    }
    // 产物目录 canonicalize（写路径钉死规范化目录，防 symlink 语义漂移）
    let mk_dir = ctx.artifact_dir.clone();
    let canonical = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        std::fs::create_dir_all(&mk_dir).map_err(|e| e.to_string())?;
        std::fs::canonicalize(&mk_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let dir = match canonical {
        Ok(p) => p,
        Err(e) => {
            return ToolResult::error(format!("写产物失败：产物目录不可用（{e}）"), Vec::new());
        }
    };
    let filename = filename.to_string();
    let content_len = content.len();
    let content = content.to_string();
    // FS 操作不持 DB 锁；同步小文件写放 spawn_blocking 防阻塞执行器
    let write_res = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut target = dir.join(&filename);
        let stem = std::path::Path::new(&filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = std::path::Path::new(&filename)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut n = 1;
        while target.exists() {
            if n >= 1000 {
                return Err("同名产物过多（≥1000），停止追加写".into());
            }
            target = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::write(&target, &content).map_err(|e| e.to_string())?;
        Ok(target.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string());
    match write_res {
        Ok(Ok(path)) => ToolResult::ok(
            format!(
                "已写入产物 {path}（{content_len} 字节）。收尾 JSON 的 artifacts 里登记该路径。"
            ),
            Vec::new(),
        ),
        Ok(Err(e)) => ToolResult::error(format!("写产物失败：{e}"), Vec::new()),
        Err(e) => ToolResult::error(format!("写产物失败：{e}"), Vec::new()),
    }
}
/// read_own_card 工具：只读自己的执行契约。子 agent 不再建卡——契约在
/// subagents 行上（objective / acceptance_json / context_summary / budget_json），
/// 行不存在只可能是 runner 上下文串了会话，属真错误要响亮报出。
pub async fn tool_read_own_card(
    app: &AppHandle,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("read_own_card 仅子 agent 执行会话可用。", Vec::new());
    };
    let subagent_id = ctx.subagent_id.clone();
    let loaded = db_locked(app, move |conn| {
        crate::db::load_subagent(conn, &subagent_id).map_err(CommandError::DbError)
    })
    .await;
    let row = match loaded {
        Ok(Some(r)) => r,
        Ok(None) => {
            return ToolResult::error("读契约失败：会话对应的 subagent 不在库中", Vec::new())
        }
        Err(e) => return ToolResult::error(format!("读契约失败：{}", e.message()), Vec::new()),
    };
    let criteria: Vec<String> = row
        .acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let budget: Option<SubagentBudget> = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    ToolResult::ok(
        own_card_payload(
            &row,
            criteria,
            budget,
            ctx.used_tool_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            ctx.used_rounds.load(std::sync::atomic::Ordering::SeqCst),
        )
        .to_string(),
        Vec::new(),
    )
}
/// read_own_card 返回体的纯构造：行上契约 + ctx 实时仪表合成单一 JSON。
/// 抽离是因为 db 路径在 MockRuntime 测试下不可直驱，而「budgetUsage 必含
/// 实时余量、静态 budget 键不再单独出现」是模型看得见的行为契约。
pub(crate) fn own_card_payload(
    row: &crate::db::subagents::SubagentRow,
    criteria: Vec<String>,
    budget: Option<SubagentBudget>,
    tool_calls_used: usize,
    rounds_used: usize,
) -> serde_json::Value {
    let cancelled = row.status == SubagentStatus::Cancelled;
    serde_json::json!({
        "subagent_id": row.id,
        "objective": row.objective,
        "acceptanceCriteria": criteria,
        "contextSummary": row.context_summary,
        "budgetUsage": {
            "toolCallsUsed": tool_calls_used,
            "toolCallsMax": budget.as_ref().map(|b| b.max_tool_calls),
            "roundsUsed": rounds_used,
            "roundsMax": budget.as_ref().map(|b| b.max_turns),
        },
        "status": row.status.as_str(),
        "hint": if cancelled {
            "派发已被取消：立即停止新探索，输出收尾 JSON（status=cancelled）"
        } else {
            "上面的验收标准与上下文摘要是本任务的全部契约；budgetUsage 是实时余量，逐条对照完成后收尾"
        }
    })
}
/// 相对路径闸：绝对路径、盘符、`..`/`.` 分量一律不解析——判定路径若能逃出
/// 产物目录，「验收」就成了任意文件存在性探测器。
pub(crate) fn safe_rel_path(raw: &str) -> Option<&str> {
    let p = raw.trim();
    if p.is_empty() || p.starts_with('/') || p.starts_with('\\') {
        return None;
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return None; // C:\ / C:/ 盘符
    }
    if p.split(['/', '\\'])
        .any(|c| c == ".." || c == "." || c.is_empty())
    {
        return None;
    }
    Some(p)
}
/// 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/。
pub(crate) fn artifact_dir_of(gen_root: &std::path::Path, subagent_id: &str) -> std::path::PathBuf {
    gen_root.join("subagents").join(subagent_id)
}
