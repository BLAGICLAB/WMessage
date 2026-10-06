//! Python / .NET 环境探测 + 缓存

use std::process::Stdio;
use std::time::{Duration, Instant};
use tauri::AppHandle;

use crate::error::CommandError;
use crate::error::CommandResult;
use serde::Serialize;

/// 带超时的版本探测：PATH 里的 python 可能是损坏 shim，
/// 无超时的 `.output()` 会把探测本身卡死；超过 3s 不退出就杀掉按失败处理
fn probe_version_ok(program: &str, args: &[&str]) -> bool {
    probe_version_ok_with(program, args, Duration::from_secs(3))
}

pub fn probe_version_ok_with(program: &str, args: &[&str], timeout: Duration) -> bool {
    let mut child = match crate::py::runtime::silent_cmd(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// 检测本机 Python（macOS/Linux: python3/python；Windows: python/python3/py -3）
/// 每个候选最多 3s（探测超时），卡死的 shim 直接跳过
pub fn detect_python() -> Option<String> {
    #[cfg(windows)]
    let candidates: &[&str] = &["python", "python3"];
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let candidates: &[&str] = &["python3", "python"];
    // macOS GUI（Finder 双击）启动 PATH 极简（/usr/bin:/bin:…），
    // brew 装的 python3 探测不到 → 补固定路径候选（不存在由 3s 探针超时跳过）
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "python3",
        "python",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
    ];
    for c in candidates {
        if probe_version_ok(c, &["--version"]) {
            return Some(c.to_string());
        }
    }
    // Windows 兜底：py 启动器
    #[cfg(windows)]
    {
        if probe_version_ok("py", &["-3", "--version"]) {
            return Some("py".to_string());
        }
    }
    None
}

/// 探测结果缓存：None=未探测；Some(inner)=已探测（inner 为 None 表示本机无 Python）。
/// 否则每次 run_python 都要 spawn 1-3 次 `python --version`，启动延迟 + 资源浪费。
pub static PY_CACHE: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

pub static PY_PROBE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn cached_python() -> Option<String> {
    {
        let g = PY_CACHE.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] py::env::PY_CACHE: {e:?}");
            e.into_inner()
        });
        if let Some(cached) = &*g {
            return cached.clone();
        }
    }
    // 探测前先放锁：探测要 spawn 最多 4-5 个子进程（每个 3s 超时上限），
    // 持锁探测会让这段时间内所有 cached_python 调用方白等一轮
    PY_PROBE_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let detected = detect_python();
    let mut g = PY_CACHE.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] py::env::PY_CACHE: {e:?}");
        e.into_inner()
    });
    // 竞态兜底：别的线程可能已在我们探测期间填好缓存，直接复用其结果
    if let Some(cached) = &*g {
        return cached.clone();
    }
    *g = Some(detected.clone());
    detected
}

pub fn invalidate_python_cache() {
    *PY_CACHE.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] py::env::PY_CACHE: {e:?}");
        e.into_inner()
    }) = None;
}

/// 检测本机 dotnet 运行时（修订版 Word 的 .NET 生成路径前置条件）：
/// `dotnet --version` 探测（复用 3s 超时探针，卡死的 shim 直接跳过）
pub fn detect_dotnet() -> Option<String> {
    // macOS GUI 启动 PATH 极简，补官方/brew 固定安装路径
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "dotnet",
        "/usr/local/share/dotnet/dotnet",
        "/opt/homebrew/bin/dotnet",
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates: &[&str] = &["dotnet"];
    for c in candidates {
        if probe_version_ok(c, &["--version"]) {
            return Some(c.to_string());
        }
    }
    None
}

pub static DOTNET_CACHE: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

pub fn cached_dotnet() -> Option<String> {
    {
        let g = DOTNET_CACHE.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] py::env::DOTNET_CACHE: {e:?}");
            e.into_inner()
        });
        if let Some(cached) = &*g {
            return cached.clone();
        }
    }
    // 同 cached_python：探测不持锁，填缓存前复查竞态
    let detected = detect_dotnet();
    let mut g = DOTNET_CACHE.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] py::env::DOTNET_CACHE: {e:?}");
        e.into_inner()
    });
    if let Some(cached) = &*g {
        return cached.clone();
    }
    *g = Some(detected.clone());
    detected
}

/// 定位 .NET 修订工具入口：(程序, 入口 dll 参数)。
pub fn dotnet_revisions_entry() -> Option<(String, Option<String>)> {
    const REL_DLL: &str = "wm-docx-revisions.dll";
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    {
        #[cfg(windows)]
        let tool_exe = exe_dir.join("dotnet").join("wm-docx-revisions.exe");
        #[cfg(not(windows))]
        let tool_exe = exe_dir.join("dotnet").join("wm-docx-revisions");
        if tool_exe.is_file() {
            return Some((tool_exe.to_string_lossy().into_owned(), None));
        }
        let dll = exe_dir.join("dotnet").join(REL_DLL);
        if dll.is_file() {
            return cached_dotnet().map(|d| (d, Some(dll.to_string_lossy().into_owned())));
        }
    }
    #[cfg(debug_assertions)]
    {
        let dll = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("dotnet/WmDocxRevisions/bin/Release/net8.0")
            .join(REL_DLL);
        if dll.is_file() {
            return cached_dotnet().map(|d| (d, Some(dll.to_string_lossy().into_owned())));
        }
    }
    None
}

/// 跑 .NET 修订工具：与 Python 同一执行内核。
/// 返回 None = dotnet 或工具不可用（调用方回退 Python 脚本路径）。
pub fn run_dotnet_revisions(
    app: &AppHandle,
    input_json: &str,
) -> Option<Result<crate::py::runtime::PyRunResult, String>> {
    let (prog, entry) = dotnet_revisions_entry()?;
    let dir = crate::py::runtime::py_runs_root().join(uuid::Uuid::new_v4().simple().to_string());
    if let Err(e) = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join("params.json"), input_json))
    {
        crate::py::audit::py_audit(
            app,
            &format!("doc_revisions_dotnet err | kind=setup_fail | {e}"),
        );
        return None;
    }
    let gen = crate::db::gen_dir(app).ok();
    let mut audit_sink = |line: &str| crate::py::audit::py_audit(app, line);
    let args = vec!["params.json".to_string()];
    Some(
        crate::py::runtime::run_python_at(
            &prog,
            entry.as_deref(),
            &dir,
            gen.as_deref(),
            &args,
            Some(120),
            &mut audit_sink,
            None,
        )
        .map_err(|f| f.msg),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyEnv {
    pub available: bool,
    pub python: String,
    pub version: String,
    pub libs: Vec<String>,
}

/// 带超时且捕获 stdout 的探测（py_env_check_blocking 专用，3s 上限同 probe_version_ok）：
/// 坏 shim 会把无超时的 `.output()` 永久挂死，把整个检测后台线程卡住。
/// 输出量有界（版本串 / 5 行模块探测，远小于管道缓冲），子进程退出后一次性读完即可。
fn probe_output_with_timeout(program: &str, args: &[&str]) -> Option<String> {
    let mut child = crate::py::runtime::silent_cmd(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            // 仅成功退出才取输出（与原 `.output()` + status.success() 语义一致）
            Ok(Some(status)) if status.success() => {
                let mut out = String::new();
                use std::io::Read;
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                return Some(out);
            }
            Ok(Some(_)) => return None,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(3) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// 同步检测核心（后台线程运行）
pub fn py_env_check_blocking() -> PyEnv {
    let Some(py) = detect_python() else {
        return PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        };
    };
    let version = probe_output_with_timeout(&py, &["--version"])
        .map(|o| o.trim().to_string())
        .unwrap_or_default();
    let probe = r#"import importlib
for m in ["openpyxl", "docx", "pptx", "pypdf", "reportlab"]:
    try:
        importlib.import_module(m)
        print(f"{m}:OK")
    except Exception:
        print(f"{m}:缺")
"#;
    let mut libs = Vec::new();
    if let Some(out) = probe_output_with_timeout(&py, &["-c", probe]) {
        for line in out.lines() {
            if line.contains(':') {
                libs.push(line.to_string());
            }
        }
    }
    PyEnv {
        available: true,
        python: py,
        version,
        libs,
    }
}

// 保留 CommandError / CommandResult 引用以便未来 env.rs 中错误路径使用
#[allow(dead_code)]
fn _unused_imports(err: CommandError) -> CommandResult<()> {
    Err(err)
}
