//! 启动面：受管服务的 spawn、就绪等待与失败归因。

use super::*;

// ── 启动 ──

/// 白名单校验 + 逐臂字面量构造。
///
/// 逐臂构造意味着源码层面不存在"运行时变量 → 进程名"的路径；
/// 白名单是 MCP 侧同一份 `STDIO_LAUNCH_ALLOWLIST`（单一事实源）。
pub(super) fn build_command(command: &str, args: &[String], cwd: &Path) -> Result<Command, String> {
    if !STDIO_LAUNCH_ALLOWLIST.contains(&command) {
        return Err(format!(
            "命令「{command}」不在启动器白名单内（允许：{}）",
            STDIO_LAUNCH_ALLOWLIST.join(" / ")
        ));
    }
    let mut cmd = match command {
        "npx" => Command::new("npx"),
        "bunx" => Command::new("bunx"),
        "uvx" => Command::new("uvx"),
        "pipx" => Command::new("pipx"),
        "node" => Command::new("node"),
        "deno" => Command::new("deno"),
        "python" => Command::new("python"),
        "python3" => Command::new("python3"),
        "docker" => Command::new("docker"),
        "podman" => Command::new("podman"),
        other => {
            return Err(format!(
                "命令「{other}」不在启动器白名单内（允许：{}）",
                STDIO_LAUNCH_ALLOWLIST.join(" / ")
            ))
        }
    };
    cmd.args(args).current_dir(cwd);
    Ok(cmd)
}

/// 起一个服务并登记。**阻塞**：含就绪等待，必须挪到 blocking 线程。
#[allow(clippy::too_many_lines)]
pub(crate) fn do_start<R: tauri::Runtime>(
    app: &AppHandle<R>,
    req: &StartRequest,
) -> Result<StartOutcome, String> {
    let id = format!("svc_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    // 白名单校验先做：命令不合法时连目录都不建，更不会占名额
    let mut cmd = build_command(&req.command, &req.args, &req.cwd)?;
    let dir = make_service_dir(&id)?;
    let stdout_path = dir.join("stdout.log");
    let stderr_path = dir.join("stderr.log");
    let stdout_file = open_append(&stdout_path)?;
    let stderr_file = open_append(&stderr_path)?;

    // 名额预留：检查与占位在**同一把锁**里做完——先查后插会让并发启动
    // 各自看到「还有空位」，结果起出超过上限的服务。占位条目状态是
    // Starting（算活跃），spawn 失败时删掉。
    let table = crate::app_state::services(app);
    let entry: ServiceEntry = Arc::new(Mutex::new(ManagedService::new(
        id.clone(),
        req.name.clone(),
        req.command.clone(),
        req.args.clone(),
        req.cwd.clone(),
        0,
        0,
        stdout_path.clone(),
        stderr_path.clone(),
        req.session_id.clone(),
    )));
    {
        let mut guard = lock_table(&table);
        let active = guard
            .values()
            .filter(|e| e.lock().map(|s| s.state.is_active()).unwrap_or(false))
            .count();
        if active >= MAX_CONCURRENT_SERVICES {
            // 没起成东西，目录也别留
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "受管服务数已达上限（{MAX_CONCURRENT_SERVICES} 个），请先 stop 掉不再用的服务"
            ));
        }
        guard.insert(id.clone(), entry.clone());
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 自成进程组：整组发信号用（组杀/组信号），且组外逃逸的子孙由 ppid 树补
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::bot::procs::CREATE_NO_WINDOW);
    }

    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // 撤掉预留名额（spawn 失败 = 什么都没起）
            lock_table(&table).remove(&id);
            let _ = std::fs::remove_dir_all(&dir);
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.start_failed",
                &[
                    ("command", req.command.clone()),
                    ("reason", format!("spawn 失败：{e}")),
                ],
            );
            return Err(format!("启动「{}」失败：{e}", req.command));
        }
    };
    let pid = child.id();
    let pgid = pid as i32;
    {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.pid = pid;
        g.pgid = pgid;
    }

    // spawn 窗口里可能已经被 stop 过（那时 pid 还是 0，登记表记的是「没有进程」，
    // stop 如实标了已退出）。现在进程真的起来了，但那条 stop 已经「完成」——
    // 不收掉它，它就是宿主管不到的孤儿。必须立刻杀掉，并让这次启动失败。
    let stopped_during_spawn = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        !matches!(g.state, ServiceState::Starting)
    };
    if stopped_during_spawn {
        // 走同一套停止阶梯（含子孙回收）——刚起来的进程也可能已经 fork 了
        let _ = crate::bot::procs::stop_tree(pid, pgid, |_| {
            child.try_wait().map(|s| s.is_some()).unwrap_or(false)
        });
        let _ = child.wait();
        lock_table(&table).remove(&id);
        let _ = std::fs::remove_dir_all(&dir);
        return Err("服务在启动过程中已被停止".to_string());
    }

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.started",
        &[
            ("service_id", id.clone()),
            ("command", req.command.clone()),
            (
                "args_preview",
                crate::bot::truncate_for_log(&req.args.join(" "), 400),
            ),
            ("cwd", req.cwd.to_string_lossy().to_string()),
            ("pid", pid.to_string()),
        ],
    );

    // 宿主死亡看门狗：宿主非正常死亡（崩溃 / 被强杀 / OOM）时，服务进程组必须
    // 跟着走——否则留下的是宿主管不到的孤儿（还占着端口，下次启动直接撞端口闸）。
    // 装不上不拦启动：看门狗是兜底，不是服务能不能跑的前提。
    watchdog::attach(&id, pgid, &child);

    // watcher 线程：wait() 回收子进程并把状态转 Exited。不同步 wait 会留僵尸。
    {
        let entry = entry.clone();
        let app = app.clone();
        let watch_id = id.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            let mut guard = entry.lock().unwrap_or_else(|e| e.into_inner());
            guard.state = ServiceState::Exited;
            guard.exited_at_ms = Some(crate::db::now_ms());
            guard.exit_code = status.ok().and_then(|s| s.code());
            // 服务自己退了 → 看门狗助手没有存在的意义了，收掉（别留空转的 sh）
            watchdog::disarm(&watch_id);
            let exit_code = guard.exit_code;
            let uptime = guard.uptime_ms();
            // 是宿主停的（还是它自己退的）——退出码单独一个数分不出来：
            // Windows 上 `taskkill /F` 的退出码就是 1，和「服务自己挂了」撞脸。
            let stop_reason = guard.stop_reason.clone();
            drop(guard);
            crate::audit::write_event(
                &app,
                AuditLevel::Info,
                "service.exited",
                &exited_event_kv(&watch_id, pid, exit_code, uptime, stop_reason),
            );
        });
    }

    // 启动期加密采样监听（0/0.5/1/2/5/10s）：dev server 起来到 bind 落定可能
    // 隔好几秒，30s 一轮的巡检在这个窗口里会漏。发现违规立即终止，不等窗口。
    spawn_startup_listening_watch(
        app,
        entry.clone(),
        reserved_ports(),
        expose_net_allowed(app),
    );

    // 就绪判定
    let outcome = match &req.ready_pattern {
        Some(pattern) => wait_ready_pattern(app, &entry, pattern, req.ready_timeout_secs, &id),
        None => {
            std::thread::sleep(Duration::from_millis(LIVENESS_CONFIRM_MS));
            let alive = entry
                .lock()
                .map(|s| s.state == ServiceState::Starting)
                .unwrap_or(false);
            if alive {
                ReadyOutcome::Ready
            } else {
                ReadyOutcome::Exited
            }
        }
    };

    if outcome != ReadyOutcome::Ready {
        // 就绪超时 / 提前退出：主动收掉这个服务，别留半死不活的东西
        let reason = "ready_timeout";
        // 先判定、后停止：停了之后状态必然是 Exited、退出码必然是停止手段的
        // （Windows 上 taskkill /F = 1），那时候再读状态就归因错了——
        // 「我们超时杀的」会被说成「它启动后立即退出（退出码 1）」。
        // 停止的**真实结果**也要进文案：停不掉就不能说「已停止」。
        let mut stop_note = String::from("已停止");
        let snapshot = resolve_ready_failure(&entry, || match stop_entry(app, &entry, reason) {
            Ok(o) if o.stopped => {}
            Ok(o) if o.method == "already_exited" => stop_note = "已退出".to_string(),
            Ok(o) => {
                stop_note = format!("停止未确认（method={}，进程可能仍在运行）", o.method);
            }
            Err(e) => {
                stop_note = format!("停止失败：{e}（进程仍在运行）");
                // 停不掉也要看得见：审计里有 service.stop_failed，这里再留一行诊断
                eprintln!("[bot::services] 就绪超时的服务收不掉：{e}");
            }
        });
        let stderr_tail = tail_lines(&stderr_path, TAIL_LINES);
        let stdout_tail = tail_lines(&stdout_path, TAIL_LINES);
        let mut detail = String::new();
        if !stdout_tail.is_empty() {
            detail.push_str("stdout: ");
            detail.push_str(&stdout_tail.join(" | "));
        }
        if !stderr_tail.is_empty() {
            if !detail.is_empty() {
                detail.push('；');
            }
            detail.push_str("stderr: ");
            detail.push_str(&stderr_tail.join(" | "));
        }
        // 端口闸违规由巡检线程落：取最新的（违规致死优先于「没就绪」——
        // 模型要拿到规则、地址和改绑示例才可能下一轮改对）。
        let violation = entry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_violation
            .clone();
        let failure = classify_ready_failure(
            &outcome,
            &snapshot,
            violation.as_ref(),
            req.ready_timeout_secs,
            &stop_note,
        );
        let msg = ready_failure_message(&req.command, &failure, &detail);
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.start_failed",
            &[
                ("service_id", id.clone()),
                ("command", req.command.clone()),
                ("reason", msg.clone()),
            ],
        );
        return Err(msg);
    }

    // 就绪：固化 stdout 尾部快照（不参与后续截断）+ 记审计
    let ready_snapshot = tail_text(&stdout_path, READY_SNAPSHOT_BYTES);
    {
        let mut guard = entry.lock().unwrap_or_else(|e| e.into_inner());
        guard.ready_snapshot = Some(ready_snapshot.clone());
        guard.state = ServiceState::Running;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.ready",
        &[
            ("service_id", id.clone()),
            ("pid", pid.to_string()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ("stdout_head", head_chars(&ready_snapshot, 1024)),
            ("stdout_tail", tail_chars(&ready_snapshot, 1024)),
        ],
    );

    Ok(StartOutcome { id })
}

/// 就绪判定的结果。
///
/// 「进程自己退了」与「等超时了」必须分开：两者的根因不同，模型拿到
/// 「启动后立即退出」会去查「这个命令为什么自己挂了」（改命令行、改依赖），
/// 而真相常常是「readyPattern 没匹配上 / 输出没 flush」——诊断方向完全相反。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadyOutcome {
    /// stdout 出现匹配正则 → 就绪
    Ready,
    /// 进程自己退了（watcher 已记下它自己的退出码）
    Exited,
    /// 到点还没就绪，进程还活着
    TimedOut,
    /// readyPattern 不是合法正则（压根没开始等）
    BadPattern(String),
}

/// 就绪失败的**停止前快照**。
///
/// 必须在停止之前取：停了之后状态必然是 Exited、退出码必然是停止手段的
/// （Windows 上 `taskkill /F` 的退出码就是 1），那时候读到的一切都归因错了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadyFailureSnapshot {
    /// 停止之前它是不是已经自己退了
    pub(super) exited_early: bool,
    /// 它自己退时的退出码（停止之前取的才是它自己的）
    pub(super) exit_code: Option<i32>,
}

/// 就绪失败的归类（决定给模型看的文案与根因）。
#[derive(Debug, Clone)]
pub(super) enum ReadyFailure {
    /// 端口闸违规致死
    Violation(Violation),
    /// readyPattern 不是合法正则
    BadPattern(String),
    /// 进程自己退了（退出码是它自己的）
    ExitedEarly { exit_code: Option<i32> },
    /// 等超时了（是我们停的）——`stop_note` 说的是停止的**真实结果**
    TimedOut {
        timeout_secs: u64,
        stop_note: String,
    },
}

/// 取停止前的状态快照（纯读，不改状态）。
fn ready_failure_snapshot(entry: &ServiceEntry) -> ReadyFailureSnapshot {
    let g = entry.lock().unwrap_or_else(|e| e.into_inner());
    ReadyFailureSnapshot {
        exited_early: g.state == ServiceState::Exited,
        exit_code: g.exit_code,
    }
}

/// 就绪失败的两步：**先判定、后停止**（顺序是语义的一部分）。
///
/// 停了之后状态必然变 Exited、退出码必然变成停止手段的——那时候再读状态，
/// 「我们超时杀的」就会被说成「它启动后立即退出（退出码 1）」。
pub(super) fn resolve_ready_failure<F: FnOnce()>(
    entry: &ServiceEntry,
    stop: F,
) -> ReadyFailureSnapshot {
    let snapshot = ready_failure_snapshot(entry);
    stop();
    snapshot
}

/// 就绪失败的归类（纯函数，供单测钉住）。
///
/// 优先级：端口闸违规 > readyPattern 非法 > 进程自己退 > 等超时。
pub(super) fn classify_ready_failure(
    outcome: &ReadyOutcome,
    snapshot: &ReadyFailureSnapshot,
    violation: Option<&Violation>,
    timeout_secs: u64,
    stop_note: &str,
) -> ReadyFailure {
    if let Some(v) = violation {
        return ReadyFailure::Violation(v.clone());
    }
    match outcome {
        ReadyOutcome::BadPattern(e) => ReadyFailure::BadPattern(e.clone()),
        // 进程自己退了：退出码取停止前的快照（不是我们 kill 出来的那个）
        _ if snapshot.exited_early => ReadyFailure::ExitedEarly {
            exit_code: snapshot.exit_code,
        },
        _ => ReadyFailure::TimedOut {
            timeout_secs,
            stop_note: stop_note.to_string(),
        },
    }
}

/// 就绪失败的文案（纯函数，供单测钉住）。
///
/// 「等超时」与「进程自己退」是两种病历：前者说「你的 readyPattern 没匹配上
/// （输出多半没 flush）」，后者才报它自己的退出码。两者混起来，模型会被引到
/// 错的方向上重试好几轮。
pub(super) fn ready_failure_message(command: &str, failure: &ReadyFailure, detail: &str) -> String {
    match failure {
        ReadyFailure::Violation(v) => {
            let mut m = format!(
                "服务「{command}」已被端口闸终止：{}",
                violation_message(v.rule, &v.address, v.port)
            );
            if !detail.is_empty() {
                m.push(' ');
                m.push_str(detail);
            }
            m
        }
        ReadyFailure::BadPattern(e) => {
            format!("服务「{command}」的 readyPattern 不是合法正则：{e}。已停止。{detail}")
        }
        ReadyFailure::ExitedEarly { exit_code } => match exit_code {
            Some(c) => format!("服务「{command}」启动后立即退出（退出码 {c}）。{detail}"),
            None => format!("服务「{command}」启动后立即退出。{detail}"),
        },
        ReadyFailure::TimedOut {
            timeout_secs,
            stop_note,
        } => format!("服务「{command}」在 {timeout_secs}s 内未就绪，{stop_note}。{detail}"),
    }
}

/// `service.exited` 的 kv（纯函数，供测试钉住）。
///
/// **必须带 stop_reason**：退出码单独一个数分不出根因——Windows 上
/// `taskkill /F` 的退出码是 1，和「服务自己挂了」撞脸；stop_reason=none
/// 才是「它自己退的」。
pub(super) fn exited_event_kv(
    service_id: &str,
    pid: u32,
    exit_code: Option<i32>,
    uptime_ms: i64,
    stop_reason: Option<String>,
) -> Vec<(&'static str, String)> {
    vec![
        ("service_id", service_id.to_string()),
        ("pid", pid.to_string()),
        (
            "exit_code",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
        ("uptime_ms", uptime_ms.to_string()),
        (
            "stop_reason",
            stop_reason.unwrap_or_else(|| "none".to_string()),
        ),
    ]
}

/// 就绪等待：stdout 出现匹配正则即算就绪。
fn wait_ready_pattern<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    pattern: &str,
    timeout_secs: u64,
    id: &str,
) -> ReadyOutcome {
    let re = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.start_failed",
                &[
                    ("service_id", id.to_string()),
                    ("reason", format!("readyPattern 不是合法正则：{e}")),
                ],
            );
            return ReadyOutcome::BadPattern(e.to_string());
        }
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let stdout_path = entry
            .lock()
            .map(|s| s.stdout_path.clone())
            .unwrap_or_default();
        let text =
            crate::bot::services::logs::decode_log_bytes(&read_tail_bytes(&stdout_path, 64 * 1024));
        if re.is_match(&text) {
            return ReadyOutcome::Ready;
        }
        let state = entry
            .lock()
            .map(|s| s.state)
            .unwrap_or(ServiceState::Failed);
        if state == ServiceState::Exited || state == ServiceState::Failed {
            return ReadyOutcome::Exited;
        }
        if Instant::now() >= deadline {
            return ReadyOutcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 读文件末尾 `n` 字节并转成文本（尾部可能是半个 UTF-8 字符，替换掉即可）。
fn tail_text(path: &Path, n: usize) -> String {
    crate::bot::services::logs::decode_log_bytes(&read_tail_bytes(path, n))
}

/// 取文本头部 n 个字符（按字符切，不切碎 UTF-8）。
pub(super) fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 取文本尾部 n 个字符（按字符切，不切碎 UTF-8）。
pub(super) fn tail_chars(s: &str, n: usize) -> String {
    let len = s.chars().count();
    s.chars().skip(len.saturating_sub(n)).collect()
}
