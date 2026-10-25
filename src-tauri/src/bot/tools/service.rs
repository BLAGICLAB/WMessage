//! 受管服务面：start/stop/list 长驻进程 + 集成测试钩子。

use super::*;

// 受管长驻服务（宿主托管的长驻进程：node MCP server / dev server / watcher）

/// start_service：起一个受管长驻进程。
///
/// 进程由**宿主直接 spawn**，不经过 run_python —— 所以不继承沙箱层的
/// RLIMIT_CPU / RLIMIT_AS（前者会 SIGXCPU 杀掉正在构建的 dev server，
/// 后者会让 node 启动即失败）。stdout/stderr 落文件不占管道，停止走降级阶梯。
pub(crate) async fn tool_start_service<R, F>(
    app: &AppHandle<R>,
    args: &str,
    session_id: Option<&str>,
    yolo: bool,
    resolve_cwd: F,
) -> ToolResult
where
    R: tauri::Runtime,
    F: Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<std::path::PathBuf, String>> + Send>,
        > + Send
        + Sync,
{
    let v = parse_args(args);
    let Some(command) = v["command"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("start_service 缺少 command");
    };

    // 闸 1：设置页独立开关（默认关）。yolo 免开关，口径同 run_python。
    if !crate::bot::services::enabled(app) && !yolo {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Warn,
            "service.denied",
            &[
                ("command", command.clone()),
                ("reason", "开关未开启".to_string()),
            ],
        );
        return ToolResult::denied(
            "⚠️ 启动本地服务未开启（设置页「允许机器人启动本地服务」）。如任务确实需要，请向用户说明。",
        );
    }

    // 闸 2：启动器白名单（与 MCP 侧同一份）
    if !crate::bot::mcp::config::STDIO_LAUNCH_ALLOWLIST.contains(&command.as_str()) {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Warn,
            "service.denied",
            &[
                ("command", command.clone()),
                ("reason", "不在启动器白名单内".to_string()),
            ],
        );
        return ToolResult::denied(format!(
            "⚠️ 启动器「{command}」不在白名单内（允许：{}）。",
            crate::bot::mcp::config::STDIO_LAUNCH_ALLOWLIST.join(" / ")
        ));
    }

    // 闸 3：工作目录过文件授权闸（服务的工作目录就是它的读写作用域）
    let cwd = match v["cwd"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => match resolve_cwd(p.to_string()).await {
            Ok(path) => path,
            Err(e) => {
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "service.denied",
                    &[
                        ("command", command.clone()),
                        ("reason", format!("工作目录未获授权：{e}")),
                    ],
                );
                return ToolResult::denied(format!("⚠️ 工作目录未获授权：{e}"));
            }
        },
        None => std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
    };

    let args_list: Vec<String> = v["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let name = v["name"].as_str().unwrap_or_default().to_string();
    let ready_pattern = v["readyPattern"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let ready_timeout_secs = v["readyTimeoutSecs"]
        .as_u64()
        .unwrap_or(crate::bot::services::DEFAULT_READY_TIMEOUT_SECS)
        .min(crate::bot::services::MAX_READY_TIMEOUT_SECS);

    // 闸 4：端口 / 监听地址判据（P2-③）
    //
    // 两层：① spawn 前从命令行 flag 粗解析的早退（**仅诊断级**——模型能写
    // 配置文件、设环境变量、在代码里硬编码端口，静态解析不可靠）；② 运行期
    // 采样（真正兜底，违规 → 杀进程组）。
    //
    // 本平台兜不住时（Windows 采不到运行期监听）**只诊断不拒**——拿 flag
    // 解析冒充 L1 比没有边界更危险：review 会误以为有闸。
    let allow_non_loopback = crate::bot::services::expose_net_allowed(app);
    let precheck_note = match crate::bot::services::precheck_listen(&args_list, allow_non_loopback)
    {
        None => None,
        Some((rule, address, port, message)) => {
            if crate::bot::procs::listening_supported() {
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "service.denied",
                    &[
                        ("command", command.clone()),
                        ("rule", rule.as_str().to_string()),
                        ("address", address.clone()),
                        ("port", port.to_string()),
                        (
                            "action",
                            crate::bot::services::ViolationAction::Rejected
                                .as_str()
                                .to_string(),
                        ),
                        ("reason", message.clone()),
                    ],
                );
                return ToolResult::denied(format!("⚠️ {message}"));
            }
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "service.port_violation_precheck",
                &[
                    ("command", command.clone()),
                    ("rule", rule.as_str().to_string()),
                    ("address", address.clone()),
                    ("port", port.to_string()),
                    ("action", "diagnostic".to_string()),
                    ("reason", message.clone()),
                    ("enforced", "false".to_string()),
                ],
            );
            Some(format!(
                "{message}（注意：本平台采不到运行期监听，端口闸未生效，这条只作诊断）"
            ))
        }
    };

    let req = crate::bot::services::StartRequest {
        command: command.clone(),
        args: args_list,
        cwd,
        name,
        ready_pattern,
        ready_timeout_secs,
        session_id: session_id.map(|s| s.to_string()),
    };
    let handle = app.clone();
    let started = crate::py::document::spawn_blocking_map(move || {
        crate::bot::services::do_start(&handle, &req)
    })
    .await;

    match started {
        Ok(out) => {
            let mut entry = crate::bot::services::list_all(app)
                .into_iter()
                .find(|s| s.id == out.id)
                .map(|s| crate::bot::services::service_to_json(&s, false))
                .unwrap_or_else(|| serde_json::json!({}));
            // 本平台兜不住时的诊断说明（如实告诉模型这条不是边界）
            if let Some(note) = precheck_note {
                entry["precheckNote"] = serde_json::json!(note);
            }
            // 返回紧凑 JSON：Skill DSL 的 ${stepN.serviceId} 要能解析它
            ToolResult::ok(entry.to_string(), Vec::new())
        }
        Err(e) => ToolResult::error(format!("启动失败：{e}"), Vec::new()),
    }
}

/// stop_service：停一个受管服务（降级信号阶梯 + 脱离进程组的子孙回收）。
pub(crate) async fn tool_stop_service<R: tauri::Runtime>(
    app: &AppHandle<R>,
    args: &str,
) -> ToolResult {
    let v = parse_args(args);
    let Some(id) = v["serviceId"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("stop_service 缺少 serviceId");
    };
    let handle = app.clone();
    let target = id.clone();
    let stopped = crate::py::document::spawn_blocking_map(move || {
        crate::bot::services::stop_service(&handle, &target, "tool")
    })
    .await;
    match stopped {
        Ok(o) => {
            let out = serde_json::json!({
                "serviceId": id,
                "stopped": o.stopped,
                "method": o.method,
                "descendantsKilled": o.descendants_killed,
                "exitCode": o.exit_code,
                "elapsedMs": o.elapsed_ms,
            });
            ToolResult::ok(out.to_string(), Vec::new())
        }
        Err(e) => ToolResult::error(format!("停止失败：{e}"), Vec::new()),
    }
}

// 集成测试钩子：tests/ 是独立 crate，够不到 pub(crate) 的工具实现。
// 受管服务的 Skill DSL 全链路 e2e 需要真工具（真闸 + 真进程 + 真登记表）。
// 仅测试使用，生产路径不调。

/// 真 `start_service`（路径闸由调用方注入：生产注入 bot_fs 授权闸，
/// 测试注入固定解析器——授权闸本身在 Wry 层，见调用点注释）。
#[doc(hidden)]
#[cfg(debug_assertions)]
pub async fn test_hook_tool_start_service<R, F>(
    app: &AppHandle<R>,
    args: &str,
    yolo: bool,
    resolve_cwd: F,
) -> ToolResult
where
    R: tauri::Runtime,
    F: Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<std::path::PathBuf, String>> + Send>,
        > + Send
        + Sync,
{
    tool_start_service(app, args, None, yolo, resolve_cwd).await
}

/// 真 `stop_service`
#[doc(hidden)]
#[cfg(debug_assertions)]
pub async fn test_hook_tool_stop_service<R: tauri::Runtime>(
    app: &AppHandle<R>,
    args: &str,
) -> ToolResult {
    tool_stop_service(app, args).await
}

/// 真 `list_services`
#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn test_hook_tool_list_services<R: tauri::Runtime>(app: &AppHandle<R>) -> ToolResult {
    tool_list_services(app)
}

/// list_services：列出宿主托管的受管服务。
pub(crate) fn tool_list_services<R: tauri::Runtime>(app: &AppHandle<R>) -> ToolResult {
    let out = crate::bot::services::services_snapshot_json(app, true);
    ToolResult::ok(out.to_string(), Vec::new())
}
