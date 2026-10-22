//! apps runner：跑 `{data_dir}/apps/<id>/<script_path>`，抓 `##RESULT##` 哨兵
//!
//! 与 `py::run_python` 区别：
//! - `run_python` 接脚本源码串、cwd 在 temp run 目录；本 runner 接落盘 app.py
//!   路径、cwd 在 `{data_dir}/apps/<id>/`（脚本能直接读 data/ input/）
//! - 哨兵行 `##RESULT## {...}` 单独解析为结构化 summary（程序页用）
//! - 产物路径固定为 `{data_dir}/apps/<id>/output/<run_id>/`
//!
//! 资源限额与并发闸：继承 `py::run_python` 的同款语义（通过它走）
//! 详细设计：`docs/APPS-DESIGN-2026-10-15.md` §3-4 与 `docs/APP-DEV-CONVENTIONS.md`

use std::time::Instant;

use serde::Serialize;
use tauri::AppHandle;

use crate::bot_slash::StopToken;
use crate::db;
use crate::db::apps::{App, AppRun};
use crate::py::runtime::{self, PyRunResult};

pub mod commands;
pub mod manage;

/// runner 结果（含哨兵解析 + 产物路径）。serde camelCase 出给前端
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppRunResult {
    pub run_id: i64,
    pub app_id: String,
    pub output_dir: String,
    pub result_json: Option<serde_json::Value>,
    /// 哨兵解析的 summary 字符串（无哨兵时 = result_json 序列化）
    pub summary: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub duration_ms: u128,
}

/// app 目录路径（与 `db::paths::app_dir_of` 一致来源）——程序页/命令共用
pub fn app_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>, app_id: &str) -> std::path::PathBuf {
    db::paths::app_dir_of(app, app_id)
}

/// 哨兵行匹配：stdout/stderr 中形如 `##RESULT## {...}` 的最后一行 JSON
const RESULT_SENTINEL: &str = "##RESULT## ";

fn parse_result_sentinel(stdout: &str) -> Option<serde_json::Value> {
    stdout.lines().rev().find_map(|line| {
        let trimmed = line.trim_start();
        trimmed
            .strip_prefix(RESULT_SENTINEL)
            .and_then(|rest| serde_json::from_str::<serde_json::Value>(rest.trim()).ok())
    })
}

/// 脚本相对路径闸：契约上 `script_path` 是 app 目录下的单个文件名
/// （M1 恒 `app.py`）。这里只放行「一个普通文件名」——绝对路径、
/// `..` 穿越、带分隔符的下钻全拒，否则 run/preflight 会把 app 目录外的
/// 任意文件读走当脚本执行
fn script_rel_path(script_rel: &str) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, Path};
    let p = Path::new(script_rel);
    let ok = !script_rel.is_empty()
        && !p.is_absolute()
        && p.components().count() == 1
        && matches!(p.components().next(), Some(Component::Normal(_)));
    if !ok {
        return Err(format!(
            "脚本路径非法：{script_rel}（只能是 app 目录下的单个文件名，如 app.py）"
        ));
    }
    Ok(p.to_path_buf())
}

/// 跑一个 app。脚本路径相对 app 目录（M1 恒为 `app.py`）
/// 产物目录 = `{app_dir}/output/<run_id>/`，由本函数创建并经 argv[2] 传给脚本
/// 入参 params 是表单模式 JSON 串（写进脚本进程 cwd 的 `params.json`）；文件夹模式传 None
/// 入参 timeout_secs 是本次运行的时长上限（秒）：调用方显式值 > 程序声明值 > 内置默认；
/// 最终值由 `py::runtime::resolve_timeout` 钳到全局天花板
pub fn run(
    app: &App,
    script_rel: &str,
    params: Option<&str>,
    timeout_secs: Option<u64>,
    app_handle: &AppHandle,
    stop: Option<&StopToken>,
) -> Result<AppRunResult, String> {
    let now_ms = db::now_ms();
    let dir = app_dir(app_handle, &app.id);
    let script_abs = dir.join(script_rel_path(script_rel)?);
    if !script_abs.exists() {
        return Err(format!(
            "脚本文件不存在：{}（app_dir={}）",
            script_abs.display(),
            dir.display()
        ));
    }
    // 新运行开始前先扫陈旧行：上一轮崩在写回之前的运行行会永远停在 running
    sweep_stale_runs(app_handle);
    let run_id = {
        let conn = db::open_db(app_handle).map_err(|e| e.to_string())?;
        db::record_app_run(&conn, &app.id, now_ms, "running").map_err(|e| e.to_string())?
    };
    let started = Instant::now();
    // 产物目录先建实：脚本经 argv[2] 直接写这里，不用自己拼 run_id（它无从得知）
    let output_dir = dir.join("output").join(run_id.to_string());
    // 失败路径必须把行落回终态：停在 running 的行在程序页上永远是「运行中」，
    // 用户以为程序还在跑，实际早就没影了。
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        mark_run_failed(
            app_handle,
            run_id,
            started.elapsed().as_millis() as i64,
            "failed",
            &format!("建产物目录失败：{e}"),
        );
        return Err(format!("建产物目录失败：{e}"));
    }
    let script_text = match std::fs::read_to_string(&script_abs) {
        Ok(t) => t,
        Err(e) => {
            mark_run_failed(
                app_handle,
                run_id,
                started.elapsed().as_millis() as i64,
                "failed",
                &format!("读脚本失败：{e}"),
            );
            return Err(format!("读脚本失败：{e}"));
        }
    };
    let params_owned = params.map(|s| s.to_string());
    let params_for_py = params_owned.as_deref();
    // 契约：argv[1] = app 目录绝对路径，argv[2] = 本次运行产物目录。
    // run_python 的子进程 cwd 在临时 run 目录（脚本源码被拷过去执行），
    // 脚本一律用 argv[1] 解析 data/ input/，产物写 argv[2]
    let app_dir_arg = vec![
        dir.to_string_lossy().to_string(),
        output_dir.to_string_lossy().to_string(),
    ];
    let PyRunResult {
        stdout,
        stderr,
        exit_code,
        truncated,
        ..
    } = runtime::run_python(
        app_handle,
        &script_text,
        params_for_py,
        &app_dir_arg,
        timeout_secs,
        stop,
    )
    .map_err(|e| {
        // 如实上报：超时 / 停止 / 启动失败是三回事——超时那次程序明明跑起来了，
        // 一律记成「运行启动失败」就是说谎。状态与文案都按失败种类分。
        let (status, summary) = failed_run_report(&e);
        mark_run_failed(
            app_handle,
            run_id,
            // 失败也记真实耗时：被超时杀掉的这次跑了多久，是用户最该看见的数字
            started.elapsed().as_millis() as i64,
            status,
            &summary,
        );
        summary
    })?;
    let duration_ms = started.elapsed().as_millis();
    let result_json = parse_result_sentinel(&stdout);
    let status = if exit_code == Some(0) { "ok" } else { "failed" };
    let summary = match &result_json {
        Some(v) => v
            .as_object()
            .and_then(|o| o.get("summary"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(v.to_string())),
        None => None,
    };
    // 规范 §4.4「stderr 写 summary」：失败且无哨兵摘要时取 stderr 末个非空行，
    // 否则历史行与通知正文是空白，用户看不到失败原因
    let summary = if status == "failed" {
        summary.or_else(|| last_stderr_line(&stderr))
    } else {
        summary
    };
    // M2 摘要截断（`docs/APP-DEV-CONVENTIONS.md` §5）：>300 字符截断 + 标记，
    // 指回 output/<run_id>/ 看完整产物；库、通知、程序页历史同源生效
    let summary = bound_summary(summary, run_id);
    {
        let conn = db::open_db(app_handle).map_err(|e| e.to_string())?;
        if let Err(e) = db::finish_app_run(
            &conn,
            run_id,
            now_ms.saturating_add(duration_ms as i64),
            duration_ms as i64,
            status,
            summary.as_deref(),
        ) {
            // 行留在 running：清扫会按时间阈值把它标成失败（用户看到「中断」而不是「还在跑」）
            eprintln!("[apps] 回写运行结果失败，行暂留 running（清扫兜底）：{e}");
            return Err(e.to_string());
        }
    }
    Ok(AppRunResult {
        run_id,
        app_id: app.id.clone(),
        output_dir: output_dir.to_string_lossy().to_string(),
        result_json,
        summary,
        stdout,
        stderr,
        exit_code,
        truncated,
        duration_ms,
    })
}

/// 失败态回写（DB 层，纯函数便于单测）：把 running 行改成终态。
fn finish_run_failed(
    conn: &rusqlite::Connection,
    run_id: i64,
    duration_ms: i64,
    now_ms: i64,
    status: &str,
    summary: &str,
) -> Result<(), String> {
    db::finish_app_run(conn, run_id, now_ms, duration_ms, status, Some(summary))
}

/// 回写失败态并留痕：所有失败路径都要走这里——
/// 停在 running 的行在程序页上永远是「运行中」，用户以为程序还在跑。
/// DB 写失败不吞（留痕）：行会由清扫兜底标成失败。
fn mark_run_failed(
    app_handle: &AppHandle,
    run_id: i64,
    duration_ms: i64,
    status: &str,
    summary: &str,
) {
    let conn = match db::open_db(app_handle) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[apps] 打开库失败，运行行暂留 running（清扫兜底）：{e}");
            return;
        }
    };
    if let Err(e) = finish_run_failed(&conn, run_id, duration_ms, db::now_ms(), status, summary) {
        eprintln!("[apps] 回写失败态失败，运行行暂留 running（清扫兜底）：{e}");
    }
}

/// 陈旧 running 行的判定阈值：超过这个时长仍停在 running 的行，
/// 它的进程早就不在了（崩在写回之前）——按中断处理。
/// 取 py::runtime::MAX_TIMEOUT_SECS（30 分钟）的四倍：再长的运行也早该结束了。
const STALE_RUN_AGE_MS: i64 = 2 * 60 * 60 * 1000;

/// 清扫陈旧的 running 行（僵尸行，纯函数便于单测）：
/// 进程崩在写回之前时，行会永远停在 running，程序页上显示「运行中」。
/// 只动 started_at 早于阈值的 running 行；已终态的行不碰。
fn sweep_stale_runs_conn(
    conn: &rusqlite::Connection,
    now_ms: i64,
    age_ms: i64,
) -> Result<usize, String> {
    let cutoff = now_ms - age_ms;
    conn.execute(
        "UPDATE app_runs
            SET finished_at = ?1, duration_ms = ?1 - started_at, status = 'failed', summary = ?2
          WHERE status = 'running' AND started_at < ?3",
        rusqlite::params![now_ms, "进程中断（未回写结果）", cutoff],
    )
    .map_err(|e| e.to_string())
}

/// 清扫入口（开库 + 留痕）。调用点：程序页历史读取前、新运行开始前——
/// 程序页的历史列表是这些行的唯一读出口，扫完用户看到的就是真实状态。
pub(crate) fn sweep_stale_runs(app_handle: &AppHandle) {
    let conn = match db::open_db(app_handle) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[apps] 清扫陈旧运行行失败（开库失败）：{e}");
            return;
        }
    };
    match sweep_stale_runs_conn(&conn, db::now_ms(), STALE_RUN_AGE_MS) {
        Ok(n) if n > 0 => eprintln!("[apps] 清扫陈旧运行行：{n} 行由 running 标成失败"),
        Ok(_) => {}
        Err(e) => eprintln!("[apps] 清扫陈旧运行行失败：{e}"),
    }
}

/// 失败种类 → （app_runs 状态, 人读摘要）。
///
/// 超时 / 停止 / 启动失败是三回事：超时那次程序明明跑起来了，
/// 一律记成「运行启动失败」就是说谎（用户在历史里看到的是「启动失败」，
/// 实际是跑了半小时被时长上限砍掉的）。
pub fn failed_run_report(e: &runtime::PyRunError) -> (&'static str, String) {
    match e.kind {
        runtime::PyFailKind::StartFailed => ("failed", format!("运行启动失败：{}", e.msg)),
        runtime::PyFailKind::Timeout => ("timeout", e.msg.clone()),
        runtime::PyFailKind::Stopped => ("stopped", e.msg.clone()),
        runtime::PyFailKind::Runtime => ("failed", e.msg.clone()),
    }
}

pub fn recent_runs(
    app_handle: &AppHandle,
    app_id: &str,
    limit: i64,
) -> Result<Vec<AppRun>, String> {
    // 读之前先扫陈旧行：崩在写回之前的运行行会永远停在 running，
    // 程序页显示成「运行中」——用户看到的必须是真实状态
    sweep_stale_runs(app_handle);
    let conn = db::open_db(app_handle).map_err(|e| e.to_string())?;
    db::list_app_runs(&conn, app_id, limit)
}

/// summary 字符上限：超限截断 + 标记（M2，规范 §5「摘要截断」）
pub const SUMMARY_MAX_CHARS: usize = 300;

/// 失败运行无哨兵摘要时，取 stderr 末个非空行（脚本的「❌+中文原因」约定写在这）
fn last_stderr_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

fn bound_summary(summary: Option<String>, run_id: i64) -> Option<String> {
    summary.map(|s| {
        if s.chars().count() <= SUMMARY_MAX_CHARS {
            return s;
        }
        let head: String = s.chars().take(SUMMARY_MAX_CHARS).collect();
        format!("{head}……（摘要超 {SUMMARY_MAX_CHARS} 字符已截断，完整产物见 output/{run_id}/）")
    })
}

pub fn preflight_script<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    app: &App,
    script_rel: &str,
) -> Result<(), String> {
    let p = app_dir(app_handle, &app.id).join(script_rel_path(script_rel)?);
    let meta = std::fs::metadata(&p).map_err(|e| format!("脚本 {} 不存在：{e}", p.display()))?;
    if meta.len() == 0 {
        return Err(format!("脚本 {} 为空", p.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::runtime::{PyFailKind, PyRunError};

    #[test]
    fn parse_result_sentinel_basic() {
        let s = "1\n2\n##RESULT## {\"a\":1, \"summary\":\"ok\"}\n";
        let v = parse_result_sentinel(s).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["summary"], "ok");
    }

    #[test]
    fn parse_result_sentinel_uses_last_match() {
        let s = "##RESULT## {\"a\":1}\nmid\n##RESULT## {\"a\":2}\n";
        let v = parse_result_sentinel(s).unwrap();
        assert_eq!(v["a"], 2);
    }

    #[test]
    fn parse_result_sentinel_handles_prefix_whitespace() {
        let s = "  ##RESULT## {\"a\":3}\n";
        let v = parse_result_sentinel(s).unwrap();
        assert_eq!(v["a"], 3);
    }

    #[test]
    fn parse_result_sentinel_no_match() {
        assert!(parse_result_sentinel("hello\nworld\n").is_none());
    }

    #[test]
    fn parse_result_sentinel_invalid_json_keeps_none() {
        assert!(parse_result_sentinel("##RESULT## not json\n").is_none());
    }

    #[test]
    fn script_rel_accepts_plain_file_name() {
        assert_eq!(
            script_rel_path("app.py").unwrap(),
            std::path::PathBuf::from("app.py")
        );
        assert_eq!(
            script_rel_path("a b.py").unwrap(),
            std::path::PathBuf::from("a b.py")
        );
    }

    #[test]
    fn script_rel_rejects_traversal_absolute_and_nested() {
        for bad in [
            "",
            "..",
            "../evil.py",
            "../../evil.py",
            "sub/app.py",
            "./app.py",
            "/etc/passwd",
            "/tmp/evil.py",
        ] {
            assert!(
                script_rel_path(bad).is_err(),
                "路径 {bad:?} 应被拒（否则 run/preflight 会读走 app 目录外的文件）"
            );
        }
    }

    #[test]
    fn last_stderr_line_takes_last_non_empty_and_skips_trailing_noise() {
        assert_eq!(
            last_stderr_line("Traceback …\n❌ input/本月.xlsx 不存在\n"),
            Some("❌ input/本月.xlsx 不存在".into())
        );
        assert_eq!(last_stderr_line("\n \n"), None);
        assert_eq!(last_stderr_line(""), None);
    }

    #[test]
    fn bound_summary_passes_short_and_truncates_long_with_pointer() {
        assert_eq!(bound_summary(Some("短摘要".into()), 7).unwrap(), "短摘要");
        assert!(bound_summary(None, 7).is_none());
        let long = "长".repeat(SUMMARY_MAX_CHARS + 50);
        let got = bound_summary(Some(long), 42).unwrap();
        assert!(got.contains("已截断"), "{got}");
        assert!(got.contains("output/42/"), "{got}");
        // 截断后总长有界：300 正文 + 稳定后缀
        assert!(
            got.chars().count() < SUMMARY_MAX_CHARS + 80,
            "{}",
            got.chars().count()
        );
    }
    /// 运行记录必须按失败种类如实上报：超时不是「启动失败」
    #[test]
    fn failed_run_report_maps_kind_to_status_and_summary() {
        let timeout = PyRunError {
            kind: PyFailKind::Timeout,
            msg: "执行超时（1800s）已强制终止".into(),
        };
        let (status, summary) = failed_run_report(&timeout);
        assert_eq!(status, "timeout");
        assert_eq!(summary, "执行超时（1800s）已强制终止");
        assert!(
            !summary.contains("启动失败"),
            "超时的文案不得说成启动失败：{summary}"
        );

        let stopped = PyRunError {
            kind: PyFailKind::Stopped,
            msg: "已停止".into(),
        };
        assert_eq!(failed_run_report(&stopped).0, "stopped");

        let start_failed = PyRunError {
            kind: PyFailKind::StartFailed,
            msg: "启动 Python 失败：No such file or directory".into(),
        };
        let (status, summary) = failed_run_report(&start_failed);
        assert_eq!(status, "failed");
        assert!(summary.contains("启动失败"), "{summary}");

        let runtime_err = PyRunError {
            kind: PyFailKind::Runtime,
            msg: "子进程异常终止（被信号杀死，无退出码）".into(),
        };
        assert_eq!(failed_run_report(&runtime_err).0, "failed");
    }

    // ── 运行行必须落终态（僵尸 running 行）──

    fn runs_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::apps::APP_RUNS_DDL).unwrap();
        conn
    }

    fn insert_run(conn: &rusqlite::Connection, id: i64, started_at: i64, status: &str) {
        conn.execute(
            "INSERT INTO app_runs (id, app_id, started_at, status) VALUES (?1, 'app1', ?2, ?3)",
            rusqlite::params![id, started_at, status],
        )
        .unwrap();
    }

    fn run_status(conn: &rusqlite::Connection, id: i64) -> (String, Option<i64>) {
        conn.query_row(
            "SELECT status, finished_at FROM app_runs WHERE id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .unwrap()
    }

    /// 失败回写：行必须从 running 落成终态（否则程序页永远显示「运行中」）。
    #[test]
    fn finish_run_failed_marks_terminal_status() {
        let conn = runs_db();
        insert_run(&conn, 1, 1000, "running");
        finish_run_failed(&conn, 1, 250, 2000, "failed", "建产物目录失败：权限").unwrap();
        let (status, finished_at) = run_status(&conn, 1);
        assert_eq!(status, "failed");
        assert_eq!(finished_at, Some(2000));
    }

    /// 清扫：只把过了阈值的 running 行标失败；刚起的行与已终态的行不碰。
    #[test]
    fn sweep_stale_runs_only_marks_old_running_rows() {
        let conn = runs_db();
        let now = 10_000_000i64;
        insert_run(&conn, 1, now - 5_000, "running"); // 刚起：仍在跑
        insert_run(&conn, 2, now - 3 * 60 * 60 * 1000, "running"); // 陈旧：僵尸行
        insert_run(&conn, 3, now - 3 * 60 * 60 * 1000, "ok"); // 已终态
        let n = sweep_stale_runs_conn(&conn, now, 60 * 60 * 1000).unwrap();
        assert_eq!(n, 1, "只应扫掉 1 个僵尸行");
        assert_eq!(run_status(&conn, 1).0, "running");
        assert_eq!(run_status(&conn, 2).0, "failed");
        assert_eq!(run_status(&conn, 3).0, "ok");
        // 僵尸行的耗时按「跑到清扫时」算
        let dur: i64 = conn
            .query_row("SELECT duration_ms FROM app_runs WHERE id = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(dur, 3 * 60 * 60 * 1000);
    }
}
