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
//! 权限模型：显式确认在前端——添加/启用前 McpPanel 完整展示
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
    // MCP 机密迁移（本写路径最可能首次触达老明文配置；失败中止本次写
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
    // trim 重名键闸在 normalize 之前：原始键一旦被 normalize 合并，冲突就看不见了
    crate::bot::mcp::config::find_trim_collisions(&server).map_err(|reason| {
        CommandError::InvalidArgument {
            field: "env/headers".into(),
            value: String::new(),
            reason,
        }
    })?;
    let mut server = normalize_server(server);
    if server.id.trim().is_empty() {
        server.id = uuid::Uuid::new_v4().simple().to_string();
    }
    let server_id = server.id.clone();
    let app_for_blob = app.clone();
    let (list, saved) = with_locked_config(&app, |cfg| {
        // **先校验后落机密**：校验失败时已存机密原封不动。旧次序是先写机密
        // 再校验，校验失败就是「保存报错，但机密已被清空/替换」。
        upsert_in_config(cfg, server.clone())?;
        // 机密落 keyring/降级文件——**锁内、迁移之后**（评审 CRITICAL②：
        // 若在锁外先写，紧随其后的迁移会读文件里的旧明文同 id 覆盖刚写的新值）。
        // 写败 → f 返 Err → 配置文件不落盘（闭包返回 Err 时外层直接短路）
        crate::bot::mcp::secrets::store_server_secrets(
            &app_for_blob,
            &server_id,
            &server.env,
            &server.headers,
        )
        .map_err(CommandError::KeyringError)?;
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
    // 内置条目（Playwright）不可删：启动时缺失会补写回来，删了等于没删，
    // 用户看到的却是「我明明删掉了又冒出来」。停用走 mcp_server_toggle。
    if super::builtin::is_builtin_id(&id) {
        return Err(crate::error::CommandError::DomainRule {
            domain: "mcp".into(),
            reason: "内置 MCP 服务器不可删除，可在设置页关闭它的开关来停用".into(),
        });
    }
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
    // 配置删除成功后清机密 blob；清败 WARN 留孤儿（可追溯，重装同 id
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
    // 幂等 no-op（id 已不存在，如陈旧标签页重试）不触发重载：免得白拆白建全部连接
    if removed.is_some() {
        trigger_reload(&app);
    }
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
        //  评审：id 不存在响亮报错（另一标签页删过的竞态），不静默返回旧列表；
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
    /// 内置条目（不可删除，只能开关）；前端据此隐藏删除按钮
    pub builtin: bool,
    /// 本地子进程 pid（stdio 连接才有；HTTP 连接没有子进程）
    pub pid: Option<u32>,
    /// 常驻内存（MB，整棵进程树合计）；未连接时没有
    pub rss_mb: Option<f64>,
    /// CPU 占用率（百分比，整棵进程树合计）；第一次采样没有前值时没有
    pub cpu_pct: Option<f64>,
    /// 累计 CPU 时间（秒）
    pub cpu_seconds: Option<f64>,
    /// 子进程已运行秒数
    pub uptime_secs: Option<u64>,
    /// 子进程树在监听的 socket（TCP LISTEN / UDP 已绑定；整棵树合计）
    pub listening: Vec<ListeningBrief>,
    /// 本平台能不能采到监听 socket（Windows 不能）。false = 面板要说明
    /// 「本平台不支持采集」——采空不等于没人监听，那是**没判**。
    pub listening_supported: bool,
    /// 本平台能不能采到进程资源占用（Windows 上进程快照层退化，采不到）。
    /// 同上：false 时内存 / CPU 恒空，不是「这进程没占资源」。
    pub resource_supported: bool,
}

/// 一个监听 socket（面板展示用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningBrief {
    /// 协议名（TCP / UDP）
    pub protocol: String,
    /// 监听地址（`127.0.0.1` / `::1` / `0.0.0.0` / `::`）
    pub address: String,
    /// 端口
    pub port: u16,
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
pub async fn mcp_status(app: AppHandle) -> CommandResult<Vec<McpServerStatus>> {
    let cfg = crate::bot::config::io::load_config(&app);
    let manager = shared();
    // 资源采样可能要拍一张全机 ps（~50ms）——挪到 blocking 线程，别卡主线程
    let children = manager.connected_children();
    let pids: Vec<u32> = children.values().map(|(pid, _)| *pid).collect();
    let sampled = tauri::async_runtime::spawn_blocking(move || manager.sample_children(&pids))
        .await
        .unwrap_or_default();
    let now_ms = crate::db::now_ms();
    // 平台采集能力：采不到时面板必须说明「没判」，不能把空值显示成「没占用」
    let listening_supported = crate::bot::procs::listening_supported();
    let resource_supported = crate::bot::procs::snapshot_supported();
    let mut out = Vec::new();
    for s in cfg.mcp_servers.unwrap_or_default() {
        let (state, error, tool_count) = {
            let st = manager.status_of(&s.id);
            (st.state, st.error, st.tool_count)
        };
        let transport = s.transport_kind().as_str().to_string();
        let builtin = super::builtin::is_builtin_id(&s.id);
        let (pid, started_at_ms) = children.get(&s.id).copied().unwrap_or((0, 0));
        let resource = if pid == 0 {
            None
        } else {
            sampled.get(&pid).cloned()
        };
        let listening = resource
            .as_ref()
            .map(|r| {
                r.listening
                    .iter()
                    .map(|l| ListeningBrief {
                        protocol: l.protocol.as_str().to_string(),
                        address: l.address.clone(),
                        port: l.port,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(McpServerStatus {
            id: s.id,
            name: s.name,
            transport,
            enabled: s.enabled,
            state,
            error,
            tool_count,
            builtin,
            pid: if pid == 0 { None } else { Some(pid) },
            rss_mb: resource
                .as_ref()
                .map(|r| (r.rss_bytes as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0),
            cpu_pct: resource.as_ref().and_then(|r| r.cpu_pct),
            cpu_seconds: resource
                .as_ref()
                .map(|r| (r.cpu_seconds * 10.0).round() / 10.0),
            uptime_secs: if pid == 0 {
                None
            } else {
                Some(((now_ms - started_at_ms).max(0) / 1000) as u64)
            },
            listening,
            listening_supported,
            resource_supported,
        });
    }
    Ok(out)
}

/// 单台服务器的机密脱敏视图（设置页编辑回填用）：键名 + 掩码值，明文不出宿主。
///
/// 前端拿不到明文（`skip_serializing`），但用户要看得见「哪些变量已设置」
/// 并能逐项增删——于是下发脱敏视图，保存时掩码值原样回传即表示「保留原值」。
#[tauri::command]
pub fn mcp_server_secrets_masked(
    app: AppHandle,
    id: String,
) -> CommandResult<crate::bot::mcp::secrets::McpSecretsMasked> {
    let secrets = crate::bot::mcp::secrets::read_server_secrets(&app, &id)
        .map_err(CommandError::KeyringError)?
        .unwrap_or_default();
    let masked = crate::bot::mcp::secrets::masked(&secrets);
    Ok(crate::bot::mcp::secrets::McpSecretsMasked {
        mask: crate::bot::mcp::secrets::MASK.to_string(),
        env: masked.env,
        headers: masked.headers,
    })
}

/// 单服务器已发现工具清单（设置页「查看工具」展开用）：只读连接槽的缓存快照，
/// 不触发连接（未连接 → 空列表）。
#[tauri::command]
pub fn mcp_server_tools(_app: AppHandle, id: String) -> CommandResult<Vec<McpToolBrief>> {
    Ok(shared()
        .tools_of(&id)
        .into_iter()
        .map(|t| McpToolBrief {
            name: t.name.to_string(),
            description: t.description.as_ref().map(|d| d.to_string()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约：面板按 camelCase 键读这两个能力标志。字段改名 / 漏填都会让
    /// 「本平台不支持采集」的提示静默消失——前端拿到 undefined 一律当「支持」，
    /// 于是采不到又被显示成「没占用」（**没判**被显示成**判了没事**）。
    #[test]
    fn status_serializes_platform_capability_flags() {
        let st = McpServerStatus {
            id: "srv1".into(),
            name: "fs".into(),
            transport: "stdio".into(),
            enabled: true,
            state: "connected".into(),
            error: None,
            tool_count: 3,
            builtin: false,
            pid: Some(42),
            rss_mb: Some(1.0),
            cpu_pct: Some(2.0),
            cpu_seconds: Some(3.0),
            uptime_secs: Some(4),
            listening: Vec::new(),
            listening_supported: false,
            resource_supported: false,
        };
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!(
            v.get("listeningSupported"),
            Some(&serde_json::Value::Bool(false)),
            "listeningSupported 键名/类型不能变（前端按它显示「本平台不支持采集」）"
        );
        assert_eq!(
            v.get("resourceSupported"),
            Some(&serde_json::Value::Bool(false)),
            "resourceSupported 键名/类型不能变（前端按它显示「本平台不支持采集」）"
        );
    }
}
