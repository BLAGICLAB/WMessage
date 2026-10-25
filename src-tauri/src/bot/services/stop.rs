//! 停止面：降级信号阶梯、子孙回收与退出确认。

use super::*;

// ── 停止 ──

/// 停止一个服务：降级信号阶梯 + 脱离进程组的子孙回收。
///
/// 阻塞：含等待窗口，必须挪到 blocking 线程。
///
/// **只报「停掉了」的前提是进程真的退了**：等不到退出就把状态放回原样
/// （服务留在活动集合里，退出时还会再收一次），并返回一条明确错误——
/// 写 `Exited` 等于告诉退出清理「这个不用收了」，而它明明还在跑。
pub(crate) fn stop_entry<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    reason: &str,
) -> Result<StopOutcome, String> {
    let started = Instant::now();
    let (pid, pgid, stdout_path, stderr_path, id, prev_state) = {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        if g.state == ServiceState::Exited || g.state == ServiceState::Failed {
            return Ok(StopOutcome {
                stopped: false,
                method: "already_exited".to_string(),
                descendants_killed: 0,
                exit_code: g.exit_code,
                elapsed_ms: 0,
            });
        }
        let prev = g.state;
        g.state = ServiceState::Stopping;
        g.stop_reason = Some(reason.to_string());
        (
            g.pid,
            g.pgid,
            g.stdout_path.clone(),
            g.stderr_path.clone(),
            g.id.clone(),
            prev,
        )
    };

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.stop_requested",
        &[
            ("service_id", id.clone()),
            ("reason", reason.to_string()),
            ("pid", pid.to_string()),
        ],
    );

    // 停止阶梯 + 子孙回收：走公共进程树模块（受管服务层与 MCP stdio 共用一套）。
    // 等待口径传「登记表状态」——watcher 的 wait() 是权威（进程真退了状态才变）。
    let tree = match crate::bot::procs::stop_tree(pid, pgid, |grace| wait_state_exit(entry, grace))
    {
        Ok(tree) => tree,
        Err(e) => {
            // 本平台没有停止手段：状态放回原样，服务继续留在活动集合里
            restore_state(entry, prev_state);
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.stop_unsupported",
                &[
                    ("service_id", id.clone()),
                    ("reason", e.clone()),
                    ("pid", pid.to_string()),
                ],
            );
            return Err(format!("{e}（无法停止受管服务）"));
        }
    };
    let method = tree.method;
    let killed = tree.descendants_killed;

    // 没有进程可停（pid 0 占位条目 / pid 1）：这个服务本来就没在跑，
    // 记成已退出是如实的。spawn 窗口里真的起来的那一个，由 do_start 侧兜底收掉。
    if method == "no_process" {
        {
            let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
            g.state = ServiceState::Exited;
            g.exited_at_ms = Some(crate::db::now_ms());
        }
        crate::audit::write_event(
            app,
            AuditLevel::Info,
            "service.stopped",
            &[
                ("service_id", id.clone()),
                ("method", method.clone()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ("descendants_killed", "0".to_string()),
            ],
        );
        return Ok(StopOutcome {
            stopped: true,
            method,
            descendants_killed: 0,
            exit_code: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }

    for (p, old, new) in &tree.reused_skipped {
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.pid_reuse_skipped",
            &[
                ("service_id", id.clone()),
                ("pid", p.to_string()),
                ("old_starttime", old.clone()),
                ("new_starttime", new.clone()),
            ],
        );
    }

    // 等 watcher 收尸，拿到退出码
    let exited = wait_state_exit(entry, Duration::from_millis(STOP_EXIT_WAIT_MS));
    let (exit_code, state) = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        (g.exit_code, g.state)
    };
    if !exited {
        // 进程还活着：状态放回原样（不许写 Exited），并明确报失败
        restore_state(entry, prev_state);
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.stop_failed",
            &[
                ("service_id", id.clone()),
                ("reason", "进程仍在运行".to_string()),
                ("method", method.clone()),
                ("pid", pid.to_string()),
            ],
        );
        return Err(format!("进程仍在运行（pid={pid}），停止未生效"));
    }
    if state != ServiceState::Exited {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ServiceState::Exited;
        g.exited_at_ms = Some(crate::db::now_ms());
    }
    // 服务停了 → 看门狗助手收掉（别留一个空转的 sh 进程）
    watchdog::disarm(&id);

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.stopped",
        &[
            ("service_id", id.clone()),
            ("method", method.clone()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ("descendants_killed", killed.to_string()),
        ],
    );

    // 日志截断（长跑服务的日志会无限涨）
    truncate_stream(app, &id, "stdout", &stdout_path);
    truncate_stream(app, &id, "stderr", &stderr_path);

    Ok(StopOutcome {
        stopped: true,
        method,
        descendants_killed: killed,
        exit_code,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 停止失败时把状态放回停止前的那个。
///
/// 关键：失败的服务必须留在活动集合里（`is_active`）——写 `Exited` 等于
/// 告诉退出清理「这个不用收了」，而它明明还在跑。
///
/// 只在状态还是 `Stopping` 时回退：watcher 若在这期间把状态转成了终态
/// （进程确实退了），那就不该再把它说成「还在跑」。
fn restore_state(entry: &ServiceEntry, prev: ServiceState) {
    let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
    if g.state == ServiceState::Stopping {
        g.state = prev;
    }
}

/// 等服务状态转为终态。返回是否已终止。
fn wait_state_exit(entry: &ServiceEntry, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let state = entry
            .lock()
            .map(|s| s.state)
            .unwrap_or(ServiceState::Failed);
        if state == ServiceState::Exited || state == ServiceState::Failed {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(crate::bot::procs::ALIVE_POLL_MS));
    }
}

/// 停止一个服务（按 id）。
pub(crate) fn stop_service<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    reason: &str,
) -> Result<StopOutcome, String> {
    let table = crate::app_state::services(app);
    let entry = {
        let guard = lock_table(&table);
        guard.get(id).cloned()
    };
    let Some(entry) = entry else {
        return Err(format!("找不到服务「{id}」，用 list_services 看现存服务"));
    };
    stop_entry(app, &entry, reason)
}

/// 退出清理：停掉所有仍在跑的服务。并发执行，总时长受单个服务的阶梯约束。
pub(crate) fn stop_all<R: tauri::Runtime>(app: &AppHandle<R>) -> usize {
    let table = crate::app_state::services(app);
    let entries: Vec<ServiceEntry> = {
        let guard = lock_table(&table);
        guard
            .values()
            .filter(|e| e.lock().map(|s| s.state.is_active()).unwrap_or(false))
            .cloned()
            .collect()
    };
    if entries.is_empty() {
        return 0;
    }
    let mut stopped = 0usize;
    for entry in entries {
        // 失败由 stop_entry 自己写审计（service.stop_failed / service.stop_unsupported）
        if let Ok(o) = stop_entry(app, &entry, "app_exit") {
            if o.stopped {
                stopped += 1;
            }
        }
    }
    // 清掉日志目录（退出即收干净；诊断信息已在审计与 stderr 尾部里）
    let root = services_root();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let _ = std::fs::remove_dir(&root);
    stopped
}
