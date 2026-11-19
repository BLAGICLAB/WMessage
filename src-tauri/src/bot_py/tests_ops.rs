use super::*;
use crate::audit::escape_for_log;
use crate::bot::constants::PY_MAX_TIMEOUT_SECS;
use std::time::{Duration, Instant};

// ── 退出清理注册表（kill_py_children 整树杀 + 守卫注销）──

#[cfg(unix)]
#[test]
fn kill_py_children_kills_registered_process_group() {
    // 模拟生产 spawn（process_group(0) 自成组首）：注册后按 pid 整组强杀，
    // 子进程必须已退出（不得留孤儿）；守卫 Drop 后注册表必须清空
    use std::os::unix::process::CommandExt;
    let mut cmd = silent_cmd("sleep");
    cmd.arg("30");
    cmd.process_group(0);
    let mut child = cmd.spawn().unwrap();
    let pid = child.id();
    {
        let _guard = ChildRegGuard::register(&child);
        assert!(
            crate::db::lock::lock_or_recover(py_children(), "bot_py::tests py_children")
                .contains(&pid),
            "注册后必须能查到 pid"
        );
        assert_eq!(kill_py_children(&[pid]), 1, "应杀掉 1 个进程组");
        let status = child.wait().unwrap();
        assert!(!status.success(), "子进程应已被强杀");
    }
    assert!(
        !crate::db::lock::lock_or_recover(py_children(), "bot_py::tests py_children")
            .contains(&pid),
        "守卫 Drop 后必须注销（防 pid 复用误杀）"
    );
    // 已退出的 pid 再杀：组不存在按未杀计，不 panic
    assert_eq!(kill_py_children(&[pid]), 0);
}

// ── 残留目录清扫（启动时清 py-runs 超龄目录）──

#[test]
fn sweep_stale_py_runs_removes_only_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("py-runs");
    let stale = root.join("stale1");
    let fresh = root.join("fresh1");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::write(root.join("stray.txt"), b"x").unwrap();
    let now = std::time::SystemTime::now();
    // max_age=0 → 所有目录视为过期，全删（非目录文件不动）
    let removed = sweep_stale_py_runs_in(&root, Duration::ZERO, now);
    assert_eq!(removed, 2);
    assert!(!stale.exists() && !fresh.exists());
    assert!(root.join("stray.txt").exists(), "非目录文件不应被清扫");
    // 重建 fresh：max_age=1h → 刚建的目录必须保留
    std::fs::create_dir_all(&fresh).unwrap();
    let removed = sweep_stale_py_runs_in(&root, Duration::from_secs(3600), now);
    assert_eq!(removed, 0);
    assert!(fresh.exists());
    // root 不存在时静默返回 0
    assert_eq!(
        sweep_stale_py_runs_in(&tmp.path().join("no-such-dir"), Duration::ZERO, now),
        0
    );
}

// ── 运行目录产物回收（harvest_run_outputs）──

#[test]
fn harvest_moves_new_files_and_skips_harness_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run");
    let gen = tmp.path().join("gen");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&gen).unwrap();
    std::fs::write(dir.join("run.py"), b"x").unwrap();
    std::fs::write(dir.join("script.py"), b"y").unwrap();
    std::fs::write(dir.join("params.json"), b"{}").unwrap();
    std::fs::write(dir.join("report.docx"), b"a").unwrap();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out").join("data.csv"), b"b").unwrap();
    // gen 里已有同名文件 → 搬入必须加 (1) 序号，不覆盖
    std::fs::write(gen.join("report.docx"), b"old").unwrap();
    let mut lines: Vec<String> = Vec::new();
    let moved = harvest_run_outputs(&dir, &gen, &mut |l: &str| lines.push(l.to_string()));
    // 脚本正文也不搬：搬进 AI_Gen_Files 只会给用户塞一堆看不懂的 .py 垃圾
    assert!(
        dir.join("run.py").exists()
            && dir.join("script.py").exists()
            && dir.join("params.json").exists()
    );
    assert!(!dir.join("report.docx").exists() && !dir.join("out").exists());
    assert_eq!(std::fs::read(gen.join("report.docx")).unwrap(), b"old");
    assert_eq!(std::fs::read(gen.join("report (1).docx")).unwrap(), b"a");
    assert_eq!(
        std::fs::read(gen.join("out").join("data.csv")).unwrap(),
        b"b"
    );
    assert!(
        lines.iter().any(|l| l.contains("harvested=2")),
        "缺回收审计行: {lines:?}"
    );
    // 回报给调用方的只有**文件**：目录搬过去是搬了个文件夹，凭证给不了内容哈希
    assert_eq!(
        moved,
        vec![gen.join("report (1).docx").to_string_lossy().into_owned()],
        "回报的产物清单不对：{moved:?}"
    );
}

#[test]
fn dedup_dest_handles_dirs_and_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
    std::fs::create_dir_all(dir.join("d")).unwrap();
    assert_eq!(
        dedup_dest(dir, std::ffi::OsStr::new("a.txt")),
        dir.join("a (2).txt")
    );
    assert_eq!(
        dedup_dest(dir, std::ffi::OsStr::new("d")),
        dir.join("d (1)")
    );
    assert_eq!(
        dedup_dest(dir, std::ffi::OsStr::new("b.md")),
        dir.join("b.md")
    );
}

// ── 探测缓存（连续调用只探测一次；探测本身带超时）──

#[test]
fn cached_python_probes_only_once() {
    invalidate_python_cache();
    let _ = cached_python(); // 首次：真正探测
    let n1 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
    let a = cached_python();
    let b = cached_python();
    let n2 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(a, b, "缓存结果应稳定");
    assert_eq!(n2, n1, "缓存命中后不得重复 spawn 探测");
}

#[cfg(unix)]
#[test]
fn probe_version_times_out_on_hanging_shim() {
    // 卡死的 shim（sleep 远探测超时）必须在超时内按失败返回，不得挂住
    let start = Instant::now();
    let ok = probe_version_ok_with("sleep", &["10"], Duration::from_millis(300));
    assert!(!ok);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "探测挂死了：{:?}",
        start.elapsed()
    );
}

#[test]
fn spawn_not_found_flagged_for_cache_retry() {
    // 无效路径 spawn → RunFail.spawn_not_found=true（run_python 据此作废缓存重试）
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-notfound");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
    let r = run_python_at(
        "/nonexistent/python-zzz",
        Some("run.py"),
        &dir,
        None,
        &[],
        Some(1),
        PY_MAX_TIMEOUT_SECS,
        &mut |_| {},
        None,
    );
    match r {
        Err(f) => assert!(f.spawn_not_found),
        Ok(_) => panic!("无效 python 路径不应成功"),
    }
}

// ── /stop 中断在途执行（停止令牌注入轮询循环）──

#[test]
fn run_python_at_stop_token_interrupts_promptly() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-stop");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.py"), "import time\ntime.sleep(30)\n").unwrap();
    let guard = crate::bot_slash::StopGuard::new(&guard_test_handle(), false, None);
    let token = guard.token();
    let mut lines: Vec<String> = Vec::new();
    let start = Instant::now();
    // 50ms 后置位停止标志（模拟 /stop）；timeout 给足 60s，
    // 若停止检查失效就要等满 60s 才返回 —— 用耗时断言区分
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            guard.force_stop();
        });
        let r = run_python_at(
            &py,
            Some("run.py"),
            &dir,
            None,
            &[],
            Some(60),
            PY_MAX_TIMEOUT_SECS,
            &mut |l: &str| lines.push(l.to_string()),
            Some(&token),
        );
        let e = match r {
            Err(f) => f.msg,
            Ok(_) => panic!("被停止的脚本不应成功返回"),
        };
        assert!(e.contains("已停止"), "got: {e}");
    });
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "/stop 后应立即中断，实际耗时 {:?}",
        start.elapsed()
    );
    assert!(
        lines.iter().any(|l| l.contains("kind=stopped")),
        "缺 stopped 审计行: {lines:?}"
    );
    // 停止 = 脚本跑过了（可能跑了一半）→ 目录留着给模型读回
    assert!(dir.exists(), "停止路径应保留运行目录（脚本留在里面）");
}

// ── strip_known_ext（扩展名剥离大小写不敏感）──

#[test]
fn strip_known_ext_case_insensitive() {
    // 「周报.DOCX」+ docx → 剥掉后 gen_out_path 产出「周报.docx」而非「周报.DOCX.docx」
    assert_eq!(strip_known_ext("周报.DOCX", "docx"), "周报");
    assert_eq!(strip_known_ext("a.docx", "docx"), "a");
    assert_eq!(strip_known_ext("b.Docx", "docx"), "b");
    // 不匹配：无扩展名 / 别的扩展名 / 仅后缀相似（非 .ext），一律原样
    assert_eq!(strip_known_ext("报告", "docx"), "报告");
    assert_eq!(strip_known_ext("a.txt", "docx"), "a.txt");
    assert_eq!(strip_known_ext("adocx", "docx"), "adocx");
    assert_eq!(strip_known_ext("a.docx.docx", "docx"), "a.docx");
}

// ── escape_for_log（剥换行/管道符，防伪造日志行）──

#[test]
fn escape_for_log_strips_newlines_and_pipes() {
    // 验收用例："a\nb| c" → "a\\nb||  c"
    assert_eq!(escape_for_log("a\nb| c", 300), "a\\nb||  c");
    assert_eq!(escape_for_log("x\ry", 300), "x\\ry");
    assert_eq!(escape_for_log("plain", 300), "plain");
    // 伪造前缀注入：剥完后无法再伪装成行首分隔符
    assert_eq!(
        escape_for_log("evil\n[2026-01-01 00:00:00] INFO | fake", 300),
        "evil\\n[2026-01-01 00:00:00] INFO ||  fake"
    );
}

#[test]
fn escape_for_log_truncates_after_escape() {
    let long = "x".repeat(400);
    let out = escape_for_log(&long, 300);
    assert_eq!(out.chars().count(), 301);
    assert!(out.ends_with('…'));
}

// ── 并发闸门有界并发 2，长任务不再把队列堵死）──

#[test]
fn py_run_gate_caps_concurrent_runs() {
    use crate::py::runtime::{gate_test_lock, py_gate_acquire_for_test, PY_RUN_MAX_CONCURRENT};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CUR: AtomicUsize = AtomicUsize::new(0);
    static MAX: AtomicUsize = AtomicUsize::new(0);
    // 许可池是进程级全局：别的用例同时拿许可会让这里量到的并发数偏小
    let _serial = gate_test_lock();
    let mut joins = Vec::new();
    // 10 个并发请求同时抢许可，临界区内 sleep 100ms 放大竞争窗口
    for _ in 0..10 {
        joins.push(std::thread::spawn(|| {
            let _g = py_gate_acquire_for_test();
            let cur = CUR.fetch_add(1, Ordering::SeqCst) + 1;
            MAX.fetch_max(cur, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            CUR.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for j in joins {
        j.join().unwrap();
    }
    let max_seen = MAX.load(Ordering::SeqCst);
    assert!(
        max_seen <= PY_RUN_MAX_CONCURRENT,
        "闸门内并发数不得超过 {PY_RUN_MAX_CONCURRENT}（实测 {max_seen}）"
    );
    // 评审 M 采纳收紧：10 线程 × 100ms 临界区在 2 许可下几乎必然观察到
    // 满载并发——闸门若退化回串行（max==1）此处响亮失败
    assert_eq!(
        max_seen, PY_RUN_MAX_CONCURRENT,
        "应观察到满载并发 {PY_RUN_MAX_CONCURRENT}（实测 {max_seen}）"
    );
}

// ── spawn_blocking_map（doc_* async 命令不得把阻塞压在 runtime worker 上）──

#[test]
fn spawn_blocking_map_ok_passthrough() {
    let r = tauri::async_runtime::block_on(spawn_blocking_map(|| Ok::<_, String>(42)));
    assert_eq!(r.unwrap(), 42);
}

#[test]
fn spawn_blocking_map_err_passthrough() {
    let r: Result<i32, String> =
        tauri::async_runtime::block_on(spawn_blocking_map(|| Err("业务错误".to_string())));
    assert_eq!(r.unwrap_err(), "业务错误");
}

#[test]
fn spawn_blocking_map_panic_mapped_to_err() {
    // 闭包 panic → JoinError → 映射为错误字符串，而不是扩散到调用方
    let r: Result<(), String> =
        tauri::async_runtime::block_on(spawn_blocking_map(|| panic!("boom")));
    let e = r.unwrap_err();
    assert!(e.starts_with("执行线程异常"), "got: {e}");
}

// ── py_audit_to（py_audit 必须与 write_event/audit_log 共用 BOT_LOG_LOCK）──

#[test]
fn py_audit_to_writes_wellformed_line() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("bot.log");
    py_audit_to(&p, "py_exec | script: hello");
    let content = std::fs::read_to_string(&p).unwrap();
    let line = content.trim_end();
    assert!(
        line.starts_with('[') && line.contains("] py_exec | script: hello"),
        "got: {line}"
    );
}

#[test]
fn py_audit_to_concurrent_no_interleaved_lines() {
    // 8 线程 × 100 行并发写：上锁后每行必须完整（无撕裂/合并），行数精确
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("bot.log");
    let mut joins = Vec::new();
    for t in 0..8 {
        let p = p.clone();
        joins.push(std::thread::spawn(move || {
            for i in 0..100 {
                py_audit_to(&p, &format!("marker-{t}-{i}-{}", "x".repeat(64)));
            }
        }));
    }
    for j in joins {
        j.join().unwrap();
    }
    let content = std::fs::read_to_string(&p).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 800, "行数不符（可能有错行合并/撕裂）");
    for line in lines {
        assert!(
            line.starts_with("[20") && line.ends_with(&"x".repeat(64)),
            "错行: {line}"
        );
    }
}

// ── doc_* command 结构化错误迁移 ──

/// 空细节不拼悬空冒号：脚本失败但无 stderr 输出时，必须明说「没有原因」
/// 而不是产出「提取失败：」这种看起来被截断的无因错误
#[test]
fn f2_script_fail_err_empty_stderr_says_no_reason() {
    let err = script_fail_err("提取失败", "  \n");
    let msg = err.message();
    assert!(
        msg.contains("脚本未输出失败原因"),
        "空 stderr 必须显式说明：{msg}"
    );
}

/// doc_extract 错误路径：对话框未选中文件 → CommandError::Internal（code 稳定）
#[test]
fn f2_resolve_doc_path_cancel_returns_internal() {
    let err = resolve_doc_path(None).unwrap_err();
    assert_eq!(err.code(), crate::error::CommandErrorCode::Internal);
    assert!(err.message().contains("用户取消了选择"));
}

/// doc_extract happy path（纯函数层）：选中文件 → 路径透传
#[test]
fn f2_resolve_doc_path_picked_passes_through() {
    let p = resolve_doc_path(Some("/tmp/a.docx".into())).unwrap();
    assert_eq!(p, "/tmp/a.docx");
}

/// 6 个 doc_* command 的脚本失败分支：统一 Internal 变体，stderr trim 后入 message
#[test]
fn f2_script_fail_err_covers_all_doc_commands() {
    for (cmd, what) in [
        ("doc_extract", "提取失败"),
        ("doc_make_word", "生成 Word 失败"),
        ("doc_make_word_revisions", "生成修订版 Word 失败"),
        ("doc_make_excel", "生成 Excel 失败"),
        ("doc_make_pdf", "生成 PDF 失败"),
        ("doc_make_ppt", "生成 PPT 失败"),
    ] {
        let err = script_fail_err(what, "  boom\n");
        assert_eq!(
            err.code(),
            crate::error::CommandErrorCode::Internal,
            "{cmd} 错误 code 应为 INTERNAL"
        );
        assert!(
            err.message().contains(what) && err.message().contains("boom"),
            "{cmd} message 应含操作名 + stderr：{}",
            err.message()
        );
        assert!(!err.is_recoverable(), "{cmd} Internal 不可重试");
    }
}

/// doc_make_* happy path（共享逻辑层）：输出路径落在目标目录 + 扩展名正确
#[test]
fn f2_gen_out_path_in_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("AI_Gen_Files");
    let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
    assert!(out.starts_with(dir.to_str().unwrap()));
    assert!(out.ends_with("周报.docx"), "out={out}");
    assert!(dir.is_dir(), "create_dir_all 应已建目录");
}

/// 同名不覆盖：已存在 周报.docx → 产出 周报 (1).docx
#[test]
fn f2_gen_out_path_in_conflict_appends_sequence() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("AI_Gen_Files");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("周报.docx"), b"x").unwrap();
    let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
    assert!(out.ends_with("周报 (1).docx"), "out={out}");
}

/// 防路径穿越：../../etc/evil → 只取 basename
#[test]
fn f2_gen_out_path_in_strips_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("AI_Gen_Files");
    let out = gen_out_path_in(&dir, Some("../../etc/evil"), "xlsx").unwrap();
    assert!(out.ends_with("evil.xlsx"), "out={out}");
    assert!(!out.contains("etc"), "out={out}");
}

/// doc_make_* 错误路径（共享逻辑层）：AI_Gen_Files 被文件占用 →
/// create_dir_all 失败 → CommandError::IoError（不再走 String 逃生舱）
#[test]
fn f2_gen_out_path_in_io_error_maps_to_io_error_variant() {
    let tmp = tempfile::tempdir().unwrap();
    let file_as_dir = tmp.path().join("AI_Gen_Files");
    std::fs::write(&file_as_dir, b"not a dir").unwrap();
    let err = gen_out_path_in(&file_as_dir, Some("x"), "pdf").unwrap_err();
    assert_eq!(err.code(), crate::error::CommandErrorCode::IoError);
    assert!(!err.is_recoverable());
}

/// Windows 子进程直拉 Python 时强制 UTF-8 IO encoding，
/// 否则 stdout 默认 cp936，Rust 端按 UTF-8 解码看到乱码。
#[cfg(windows)]
#[test]
fn silent_cmd_on_windows_sets_python_utf8_env() {
    let cmd = silent_cmd("python");
    let mut found_io = false;
    let mut found_utf8 = false;
    // get_envs 的项是 (键, Option<值>)：None = 「从环境里继承」，
    // 只看显式设过的那些（filter_map 把 None 滤掉）
    for (k, v) in cmd.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))) {
        if k.to_str() == Some("PYTHONIOENCODING") && v.to_str() == Some("utf-8") {
            found_io = true;
        }
        if k.to_str() == Some("PYTHONUTF8") && v.to_str() == Some("1") {
            found_utf8 = true;
        }
    }
    assert!(found_io, "PYTHONIOENCODING=utf-8 未设置");
    assert!(found_utf8, "PYTHONUTF8=1 未设置");
}
