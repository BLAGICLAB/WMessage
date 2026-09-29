//! MCP 服务器配置的 tauri 命令（3 个，设置页 McpPanel 用）。
//!
//! - `mcp_server_save`：新增/编辑（id 为空由后端生成 uuid；校验失败响亮报错）
//! - `mcp_server_delete`：按 id 删除（幂等）
//! - `mcp_server_toggle`：启停窄口径更新（只动 enabled，避免前端旧快照整份覆盖）
//!
//! 三个命令都返回更新后的完整列表（前端免二次读取）。
//! 写路径与 add_allowed_dir 同款：CONFIG_WRITE_LOCK 全程持锁 RMW
//!（update_config_file 的闭包不返 Result，携带校验错误的 save 走本模块
//! with_locked_config 展开）；审计在放锁后写（锁内不夹审计 IO，同仓惯例）。
//!
//! 权限模型（拍板 3A）：显式确认在前端——添加/启用前 McpPanel 完整展示
//! 将运行的命令行/参数/env，用户确认才发起 save；后端负责校验 + 审计留痕。
//! 阶段 2 起 save/delete/toggle 成功后触发 mcp manager 重载连接。

use tauri::AppHandle;

use crate::bot::BotConfig;
use crate::error::{CommandError, CommandResult};

use super::config::{
    normalize_server, remove_from_config, set_enabled_in_config, upsert_in_config, McpServerConfig,
};
use super::manager::shared;

/// 配置变更后触发连接重载（fire-and-forget：写盘已原子完成，重载自行读盘对齐；
/// 失败只记审计不回滚配置——连接态不是配置的真相源）。
fn trigger_reload(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        shared().reload_from_config(&app).await;
    });
}

/// 持锁 RMW 内核：锁内 迁移 → 读 → f（可失败，失败不落盘）→ 原子写。
/// 返回 f 的结果（f 可从 &mut BotConfig 提取需要的值带出来）。
fn with_locked_config<T>(
    app: &AppHandle,
    f: impl FnOnce(&mut BotConfig) -> CommandResult<T>,
) -> CommandResult<T> {
    let _g = crate::bot::config::io::lock_config_write();
    let _ = crate::bot::config::schema::migrate_bot_config_schema_locked(app);
    // B4-6：MCP 机密迁移（本写路径最可能首次触达老明文配置；失败中止本次写
    // ——否则写回会剥离未迁移的明文 = 丢数据，评审 CRITICAL① 采纳）
    crate::bot::config::io::migrate_mcp_server_secrets_locked(app).map_err(CommandError::from)?;
    let mut cfg = crate::bot::config::io::load_config(app);
    let out = f(&mut cfg)?;
    crate::bot::config::io::write_bot_config_file_locked(&crate::db::data_dir(app), cfg)?;
    Ok(out)
}

fn list_of(cfg: &BotConfig) -> Vec<McpServerConfig> {
    cfg.mcp_servers.clone().unwrap_or_default()
}

fn audit(app: &AppHandle, event: &str, server: &McpServerConfig) {
    crate::bot::audit_log(
        app,
        &format!(
            "{event} | id: {} | name: {} | transport: {} | enabled: {}",
            // id 来自前端（评审口径）：free-form 审计行按字符转义防伪造日志行
            crate::bot::escape_for_log(&server.id, 64),
            crate::bot::escape_for_log(&server.name, 50),
            server.transport_kind().as_str(),
            server.enabled,
        ),
    );
}

/// 新增或编辑 MCP 服务器。id 为空 = 新建（后端生成 uuid）；非空 = 按 id 整体替换。
/// 返回保存后的完整列表。
#[tauri::command]
pub fn mcp_server_save(
    app: AppHandle,
    server: McpServerConfig,
) -> CommandResult<Vec<McpServerConfig>> {
    let mut server = normalize_server(server);
    if server.id.trim().is_empty() {
        server.id = uuid::Uuid::new_v4().simple().to_string();
    }
    let server_id = server.id.clone();
    let app_for_blob = app.clone();
    let (list, saved) = with_locked_config(&app, |cfg| {
        // B4-6：机密落 keyring/降级文件——**锁内、迁移之后**（评审 CRITICAL②：
        // 若在锁外先写，紧随其后的迁移会读文件里的旧明文同 id 覆盖刚写的新值）。
        // 写败 → f 返 Err → 配置不动（先 keyring 后配置的次序仍成立）
        crate::bot::mcp::secrets::store_server_secrets(
            &app_for_blob,
            &server_id,
            &server.env,
            &server.headers,
        )
        .map_err(CommandError::KeyringError)?;
        upsert_in_config(cfg, server.clone())?;
        let saved = cfg
            .mcp_servers
            .as_ref()
            .and_then(|l| l.iter().find(|s| s.id == server.id))
            .cloned();
        Ok((list_of(cfg), saved))
    })?;
    if let Some(s) = &saved {
        audit(&app, "mcp.server_saved", s);
    }
    trigger_reload(&app);
    Ok(list)
}

/// 按 id 删除 MCP 服务器（幂等：不存在不报错）。返回删除后的完整列表。
#[tauri::command]
pub fn mcp_server_delete(app: AppHandle, id: String) -> CommandResult<Vec<McpServerConfig>> {
    let (list, removed) = with_locked_config(&app, |cfg| {
        let removed = cfg
            .mcp_servers
            .as_ref()
            .and_then(|l| l.iter().find(|s| s.id == id))
            .cloned();
        remove_from_config(cfg, &id);
        Ok((list_of(cfg), removed))
    })?;
    if let Some(s) = &removed {
        audit(&app, "mcp.server_deleted", s);
    }
    // B4-6：配置删除成功后清机密 blob；清败 WARN 留孤儿（可追溯，重装同 id
    // 概率≈0）——不回滚删除
    if removed.is_some() {
        if let Err(e) = crate::bot::mcp::secrets::purge_server_secrets(&app, &id) {
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Warn,
                "mcp.secret_purge_failed",
                &[("id", id.clone()), ("err", e)],
            );
        }
    }
    trigger_reload(&app);
    Ok(list)
}

/// 启停 MCP 服务器（窄口径：只动 enabled 字段）。返回更新后的完整列表。
#[tauri::command]
pub fn mcp_server_toggle(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> CommandResult<Vec<McpServerConfig>> {
    let (list, toggled) = with_locked_config(&app, |cfg| {
        let ok = set_enabled_in_config(cfg, &id, enabled);
        // B0 评审：id 不存在响亮报错（另一标签页删过的竞态），不静默返回旧列表；
        // DomainRule 而非 Internal（可恢复业务态，前端可按 domain 区分）
        if !ok {
            return Err(crate::error::CommandError::DomainRule {
                domain: "mcp".into(),
                reason: "未找到该 MCP 服务器（可能已被删除），请刷新列表后重试".into(),
            });
        }
        let toggled = cfg
            .mcp_servers
            .as_ref()
            .and_then(|l| l.iter().find(|s| s.id == id))
            .cloned();
        Ok((list_of(cfg), toggled))
    })?;
    if let Some(s) = &toggled {
        audit(&app, "mcp.server_toggled", s);
    }
    trigger_reload(&app);
    Ok(list)
}

/// MCP 服务器状态（mcp_status 命令返回）：配置 + 连接槽合并视图
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    /// "connected" | "down" | "absent"（absent = 未启用或尚未轮到连接）
    pub state: String,
    pub error: Option<String>,
    pub tool_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolBrief {
    pub name: String,
    pub description: Option<String>,
}

/// 设置页读 MCP 状态：配置全量（含禁用）× 连接槽位合并。
/// 同步命令：只做内存合并（槽位是 std Mutex 短临界区），不触发连接。
#[tauri::command]
pub fn mcp_status(app: AppHandle) -> CommandResult<Vec<McpServerStatus>> {
    let cfg = crate::bot::config::io::load_config(&app);
    let manager = shared();
    let mut out = Vec::new();
    for s in cfg.mcp_servers.unwrap_or_default() {
        let (state, error, tool_count) = {
            let st = manager.status_of(&s.id);
            (st.state, st.error, st.tool_count)
        };
        let transport = s.transport_kind().as_str().to_string();
        out.push(McpServerStatus {
            id: s.id,
            name: s.name,
            transport,
            enabled: s.enabled,
            state,
            error,
            tool_count,
        });
    }
    Ok(out)
}

/// 单服务器已发现工具清单（设置页「查看工具」展开用）：只读连接槽的缓存快照，
/// 不触发连接（未连接 → 空列表）。
#[tauri::command]
pub fn mcp_server_tools(app: AppHandle, id: String) -> CommandResult<Vec<McpToolBrief>> {
    Ok(shared()
        .tools_of(&id)
        .into_iter()
        .map(|t| McpToolBrief {
            name: t.name.to_string(),
            description: t.description.as_ref().map(|d| d.to_string()),
        })
        .collect())
}
