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
    let run_id = {
        let conn = db::open_db(app_handle).map_err(|e| e.to_string())?;
        db::record_app_run(&conn, &app.id, now_ms, "running").map_err(|e| e.to_string())?
    };
    // 产物目录先建实：脚本经 argv[2] 直接写这里，不用自己拼 run_id（它无从得知）
    let output_dir = dir.join("output").join(run_id.to_string());
    std::fs::create_dir_all(&output_dir).map_err(|e| format!("建产物目录失败：{e}"))?;
    let script_text =
        std::fs::read_to_string(&script_abs).map_err(|e| format!("读脚本失败：{e}"))?;
    let params_owned = params.map(|s| s.to_string());
    let params_for_py = params_owned.as_deref();
    let started = Instant::now();
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
        let conn = db::open_db(app_handle).ok();
        if let Some(conn) = conn {
            let _ = db::finish_app_run(
                &conn,
                run_id,
                db::now_ms(),
                0,
                "failed",
                Some(&format!("runner 启动失败：{e}")),
            );
        }
        format!("运行启动失败：{e}")
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
        db::finish_app_run(
            &conn,
            run_id,
            now_ms.saturating_add(duration_ms as i64),
            duration_ms as i64,
            status,
            summary.as_deref(),
        )
        .map_err(|e| e.to_string())?;
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

pub fn recent_runs(
    app_handle: &AppHandle,
    app_id: &str,
    limit: i64,
) -> Result<Vec<AppRun>, String> {
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
}
