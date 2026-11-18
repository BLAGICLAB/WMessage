//! ort（onnxruntime）动态库环境预检。
//!
//! ort 的 load-dynamic 初始化是 panic 不是 Err（dylib 加载走 `expect`），失败还会
//! 毒化 ort 的全局 Mutex——此后每一次 ort 调用都 panic（实机形态：`Failed to load
//! ONNX Runtime dylib` → `Mutex poisoned` 风暴，记忆子系统整进程失效）。本模块把
//! 「缺 dll / 缺依赖」拦在触碰 ort 之前：按 ort 的同一套路径解析规则真加载一次
//! 动态库，拿到 Win32 错误码并分类出可操作的降级原因；一旦确认环境不可用即
//! 永久短路。记忆（memory/embed）与 OCR（ocr.rs）两侧共用同一份结论。
//!
//! 非 Windows 目标 ort 是 build 期链接（download-binaries），没有运行期加载风险，
//! 预检恒 Ok；`catch_unwind` 兜底仍生效（拦 ort 内部残余 panic 点）。

/// 路径类型仅被 cfg(windows) 的加载路径与测试构建使用（非 Windows 生产构建无调用方）
#[cfg(any(target_os = "windows", test))]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// ort 环境已确认不可用（预检失败，或 ort 初始化 panic 过一次）。
static ORT_ENV_FAILED: AtomicBool = AtomicBool::new(false);

/// 预检结论缓存：进程内只真加载一次。
static PREFLIGHT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// 环境已判死后的统一降级说明（进引擎状态与诊断面）。
const ENV_FAILED_NOTE: &str =
    "onnxruntime 环境已确认不可用（此前加载失败或崩溃过），已永久降级为关键词模式";

/// 标记环境不可用：之后所有 ort 调用方直接走降级路径，不再触碰 ort——
/// ort 的 OnceLock 在 init 闭包 panic 后保持未初始化，不短路的话每一次
/// 调用都会重新 init、重新 panic、重新毒化。
pub fn mark_env_failed() {
    ORT_ENV_FAILED.store(true, Ordering::Relaxed);
}

/// ort 环境预检：失败返回可操作的降级原因（Win32 错误码 + 指引）。
pub(crate) fn preflight() -> Result<(), String> {
    if ORT_ENV_FAILED.load(Ordering::Relaxed) {
        return Err(ENV_FAILED_NOTE.into());
    }
    match PREFLIGHT.get_or_init(try_load_dylib) {
        Ok(()) => Ok(()),
        Err(reason) => {
            // 预检失败 = 环境问题与具体调用方无关，两侧都别再试
            mark_env_failed();
            Err(reason.clone())
        }
    }
}

/// panic 载荷 → 可读原因（两侧 load_engine 的 catch_unwind 兜底共用）。
pub(crate) fn panic_to_reason(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return format!("onnxruntime 初始化崩溃：{s}");
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return format!("onnxruntime 初始化崩溃：{s}");
    }
    "onnxruntime 初始化崩溃（panic）".to_string()
}

/// 启动自检（幂等）：真加载一次动态库，失败 → 服务告警通知（一次性，新一集
/// 先收上一集）+ 审计留痕；成功 → 收掉历史告警（装好运行库后旧告警不该继续
/// 挂着）。缺件机器上语义记忆静默失效没人知道为什么——这是「缺件时用户知道
/// 怎么办」的那半边；非 Windows 预检恒过，这里只做告警清扫，等于无操作。
pub fn startup_selfcheck<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    const SERVICE_ID: &str = "ort_env";
    const ALERT_TYPE: &str = "dylib";
    match preflight() {
        Ok(()) => {
            let _ = crate::notifications::resolve_service_alert(app, SERVICE_ID, ALERT_TYPE, None);
        }
        Err(reason) => {
            // 新一集先收上一集：条件持续期间角标不涨（服务告警口径）
            let _ = crate::notifications::resolve_service_alert(app, SERVICE_ID, ALERT_TYPE, None);
            let id = format!(
                "ort-env-unavailable-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );
            let payload = serde_json::json!({
                "serviceId": SERVICE_ID,
                "alertType": ALERT_TYPE,
            });
            let _ = crate::notifications::insert_service_alert(
                app,
                &id,
                "语义记忆不可用（onnxruntime 加载失败）",
                &reason,
                &payload,
            );
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "memory.embed_engine_unavailable",
                "reason" => reason,
                "phase" => "startup"
            );
        }
    }
}

/// ort 运行时的 dylib 路径解析规则（复刻同源，保证预检与 ort 走同一条路）：
/// `ORT_DYLIB_PATH` 优先；相对名先按 exe 同目录提升为绝对路径（文件在场才提升），
/// 否则原样交给加载器按标准搜索序解析。
/// 非 Windows 只在测试构建编译（生产没有调用方）。
#[cfg(any(target_os = "windows", test))]
fn resolve_dylib_path(env: Option<&str>, exe_dir: Option<&Path>, default_name: &str) -> PathBuf {
    let name = env
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_name);
    let path = Path::new(name);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(dir) = exe_dir {
        let candidate = dir.join(path);
        if candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

/// 从错误链里取 Win32 原生错误码（libloading 的包装层不固定，逐层爬 source）。
/// 非 Windows 只在测试构建编译（生产没有调用方）。
#[cfg(any(target_os = "windows", test))]
fn error_code_from_chain(e: &(dyn std::error::Error + 'static)) -> Option<i32> {
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(ioe) = err.downcast_ref::<std::io::Error>() {
            return ioe.raw_os_error();
        }
        cur = err.source();
    }
    None
}

/// Windows：真加载一次（libloading 与 ort load-dynamic 同一加载语义），
/// 失败分类成可操作原因；成功即卸载，让 ort 走自己的加载路径（引用计数，无副作用）。
#[cfg(target_os = "windows")]
fn try_load_dylib() -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let env_val = std::env::var("ORT_DYLIB_PATH").ok();
    let path = resolve_dylib_path(env_val.as_deref(), exe_dir.as_deref(), "onnxruntime.dll");
    // SAFETY: 只加载随安装包分发的 onnxruntime.dll（路径按 ort 同一套规则解析），
    // 与 ort 自身加载的是同一份二进制；加载失败走 Err 分支，不触碰 ort 的全局状态
    // （真加载由 ort 在自己的 OnceLock 里再做一次）。
    let loaded = unsafe { libloading::Library::new(&path) };
    match loaded {
        Ok(lib) => {
            drop(lib);
            Ok(())
        }
        Err(e) => Err(load_failure_reason(&e, &path)),
    }
}

/// 非 Windows：ort 是 build 期链接，没有运行期加载风险。
#[cfg(not(target_os = "windows"))]
fn try_load_dylib() -> Result<(), String> {
    Ok(())
}

/// Win32 错误码 → 可操作的原因。
///
/// 126（ERROR_MOD_NOT_FOUND）= 模块**或其依赖**找不到：dll 在场（路径已按
/// exe 同目录提升成功）时 126 意味着依赖缺失——onnxruntime.dll 的 PE 导入表
/// 依赖 MSVC 运行库四件（VCRUNTIME140 / MSVCP140 系），NuGet 包不含它们，
/// 目标机没装 VC++ Redistributable 时 dll 躺在安装目录里也照样 126。
#[cfg(target_os = "windows")]
fn load_failure_reason(e: &libloading::Error, path: &Path) -> String {
    match error_code_from_chain(e) {
        Some(126) => format!(
            "onnxruntime.dll 无法加载（Win32 错误码 126，依赖缺失）：{} 在安装目录里，但缺少 VC++ 2015-2022 x64 运行库（VCRUNTIME140 / MSVCP140 系），请安装 Microsoft Visual C++ Redistributable",
            path.display()
        ),
        Some(2) | Some(3) => format!(
            "onnxruntime.dll 未找到：{}（应随安装包分发在安装目录）",
            path.display()
        ),
        Some(c) => format!("onnxruntime.dll 加载失败（Win32 错误码 {c}）：{e}"),
        None => format!("onnxruntime.dll 加载失败：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(p: &str) -> Option<PathBuf> {
        Some(PathBuf::from(p))
    }

    /// 环境变量绝对路径原样采用（不参与提升）
    #[test]
    fn resolve_prefers_absolute_env_path() {
        let got = resolve_dylib_path(
            Some("C:\\ort\\custom.dll"),
            dir("C:\\app").as_deref(),
            "onnxruntime.dll",
        );
        assert_eq!(got, PathBuf::from("C:\\ort\\custom.dll"));
    }

    /// 相对名在 exe 同目录命中 → 提升为绝对路径（与 ort 的提升规则同源）
    #[test]
    fn resolve_promotes_relative_name_from_exe_dir() {
        let tmp = std::env::temp_dir().join(format!("ort_env_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp).unwrap();
        let probe = tmp.join("onnxruntime.dll");
        std::fs::write(&probe, b"x").unwrap();
        let got = resolve_dylib_path(None, Some(tmp.as_path()), "onnxruntime.dll");
        assert_eq!(got, probe);
        std::fs::remove_file(&probe).ok();
    }

    /// exe 同目录没有 → 原样交加载器（标准搜索序里 exe 目录仍最优先）
    #[test]
    fn resolve_falls_back_to_raw_name_when_not_beside_exe() {
        let got = resolve_dylib_path(
            None,
            dir("/nonexistent-exe-dir").as_deref(),
            "onnxruntime.dll",
        );
        assert_eq!(got, PathBuf::from("onnxruntime.dll"));
    }

    /// Win32 错误码从 source 链里爬出来（包装层形状不固定）
    #[test]
    fn error_code_extracted_from_source_chain() {
        let io = std::io::Error::from_raw_os_error(126);
        assert_eq!(error_code_from_chain(&io), Some(126));
        let direct = std::io::Error::from_raw_os_error(2);
        assert_eq!(error_code_from_chain(&direct), Some(2));
        // 有链但无码：io::Error 包字符串，raw_os_error 为 None
        let wrapped = std::io::Error::other("no native code");
        assert_eq!(error_code_from_chain(&wrapped), None);
    }

    /// panic 载荷 → 可读原因（&str / String / 其他三态）
    #[test]
    fn panic_payload_converted_to_reason() {
        let s: Box<dyn std::any::Any + Send> = Box::new("dylib expect 炸了");
        assert!(panic_to_reason(&s).contains("dylib expect 炸了"));
        let owned: Box<dyn std::any::Any + Send> = Box::new(String::from("Owned panic"));
        assert!(panic_to_reason(&owned).contains("Owned panic"));
        let other: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert!(panic_to_reason(&other).contains("panic"));
    }
}
