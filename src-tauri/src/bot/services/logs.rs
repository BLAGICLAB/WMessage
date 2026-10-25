//! 日志与输出上限面：服务日志文件读写、输出截断、上限巡检。

use super::*;

// ── 日志文件 ──

/// 服务日志根目录（与 `py_runs_root` 同款：系统 temp 下按功能分目录）。
pub(crate) fn services_root() -> PathBuf {
    std::env::temp_dir().join("wmessage-services")
}

/// 建服务的工作目录（stdout.log / stderr.log 落这里）。
pub(super) fn make_service_dir(id: &str) -> Result<PathBuf, String> {
    let dir = services_root().join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建服务目录失败：{e}"))?;
    Ok(dir)
}

/// 以 **append 模式**打开日志文件。
///
/// 必须 append：子进程持有同一个 fd，`O_APPEND` 让它每次写都落到**当前文件末尾**。
/// 宿主截断后（§ truncate_output_file）子进程续写会接在新末尾，而不是按旧偏移
/// 写出"前面一段空洞"的稀疏文件。
pub(super) fn open_append(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开日志文件失败（{}）：{e}", path.display()))
}

/// 读文件末尾 `n` 字节。
pub(super) fn read_tail_bytes(path: &Path, n: usize) -> Vec<u8> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let len = meta.len();
    if len == 0 {
        return Vec::new();
    }
    let start = len.saturating_sub(n as u64);
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    use std::io::{Read, Seek, SeekFrom};
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    buf
}

/// 读文件末尾若干行（最多 `n` 行）。
pub(crate) fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let bytes = read_tail_bytes(path, 8 * 1024);
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}

/// 截断日志文件到 `cap` 以内，只保留尾部。
///
/// 返回被丢弃的字节数。
///
/// 三个 fd 的开法必须一致（都是 append）：
/// - 子进程侧：append（唯一真相源，持续写）
/// - 宿主读侧：只读
/// - 宿主写侧：**append**（普通写会从偏移 0 覆盖，把刚写回的尾部和子进程并发写冲掉）
///
/// 已知代价：`set_len(0)` 与写回之间有几毫秒窗口，窗口内子进程的写可能丢失或
/// 顺序错乱。日志尾部本就是"最近发生了什么"的近似，丢几毫秒不影响诊断；
/// 关键信息（启动横幅 / 监听地址）由 `ready_snapshot` 单独固化。
pub(super) fn truncate_output_file(path: &Path, cap: u64) -> Result<u64, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读日志元数据失败：{e}"))?;
    let len = meta.len();
    if len <= cap {
        return Ok(0);
    }
    let keep = (cap / 2) as usize;
    let tail = read_tail_bytes(path, keep);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("打开日志文件写失败：{e}"))?;
    f.set_len(0).map_err(|e| format!("截断日志文件失败：{e}"))?;
    drop(f);
    let mut w = open_append(path)?;
    use std::io::Write;
    w.write_all(&tail)
        .map_err(|e| format!("写回日志尾部失败：{e}"))?;
    Ok(len - tail.len() as u64)
}
/// 截断一个日志流到上限，超限才记审计。
pub(super) fn truncate_stream<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    stream: &str,
    path: &Path,
) {
    let Ok(dropped) = truncate_output_file(path, OUTPUT_CAP_BYTES) else {
        return;
    };
    if dropped == 0 {
        return;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.output_truncated",
        &[
            ("service_id", id.to_string()),
            ("stream", stream.to_string()),
            ("bytes", dropped.to_string()),
            ("cap", OUTPUT_CAP_BYTES.to_string()),
        ],
    );
}

/// 巡检所有服务的日志：超限的截到上限（只留最近输出）。
pub(crate) fn enforce_output_caps<R: tauri::Runtime>(app: &AppHandle<R>) {
    for s in list_all(app) {
        truncate_stream(app, &s.id, "stdout", &s.stdout_path);
        truncate_stream(app, &s.id, "stderr", &s.stderr_path);
    }
}
/// 启动输出上限巡检：每 30s 一轮（App 启动时调用）。
///
/// 周期取 30s：长跑服务的日志增长与 CPU 跑满都是「分钟级」的，
/// 巡检太密只是白跑 IO。
/// 单轮失败不杀循环：截断失败只丢日志，不影响服务本身。
pub fn start_output_sweeper<R: tauri::Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(OUTPUT_SWEEP_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            sweep_round(&app);
        }
    });
}
