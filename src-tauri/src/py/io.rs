//! IO 限长读取 + 线程 join + 截断

use std::io::Read;
use std::time::{Duration, Instant};

use crate::bot::StopToken;
use crate::py::runtime::OUTPUT_CAP;

/// 返回（buf, truncated, 读取中断错误）。读取中断（子进程被杀/管道断裂）时
/// buf 可能不满且 truncated 探测不到——错误随线程结果回传，由持有 audit
/// 闭包的主线程留痕，避免「假完整」输出静默（eprintln 在 GUI 下无人收）。
pub fn read_capped_drain<R: Read>(src: R, cap: usize) -> (Vec<u8>, bool, Option<std::io::Error>) {
    let mut probe = src.take(cap as u64 + 1);
    let mut buf = Vec::new();
    let read_err = probe.read_to_end(&mut buf).err();
    let truncated = buf.len() > cap;
    if truncated {
        buf.truncate(cap);
        let _ = std::io::copy(&mut probe.into_inner(), &mut std::io::sink());
    }
    (buf, truncated, read_err)
}

pub struct StopReader<R> {
    inner: R,
    stop: Option<StopToken>,
}

impl<R> StopReader<R> {
    pub fn new(inner: R, stop: Option<StopToken>) -> Self {
        Self { inner, stop }
    }
}

impl<R: Read> Read for StopReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.as_ref().is_some_and(|s| s.stopped()) {
            return Ok(0);
        }
        self.inner.read(buf)
    }
}

pub fn drain_output(
    rx: &std::sync::mpsc::Receiver<(&'static str, Vec<u8>, bool)>,
    grace: Duration,
) -> (String, String, bool, bool) {
    let deadline = Instant::now() + grace;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut truncated = false;
    let mut got = 0;
    while got < 2 {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            break;
        }
        match rx.recv_timeout(remain) {
            Ok((kind, buf, tr)) => {
                got += 1;
                truncated |= tr;
                let text = String::from_utf8_lossy(&buf).into_owned();
                if kind == "out" {
                    stdout = text;
                } else {
                    stderr = text;
                }
            }
            Err(_) => break,
        }
    }
    (stdout, stderr, got == 2, truncated)
}

pub const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// reader 线程 join（超时 detach 留痕），并留痕读取中断错误。
pub fn join_reader_threads(
    out_handle: std::thread::JoinHandle<()>,
    err_handle: std::thread::JoinHandle<()>,
    read_errors: (
        &std::sync::Mutex<Option<String>>,
        &std::sync::Mutex<Option<String>>,
    ),
    audit: &mut dyn FnMut(&str),
) {
    let deadline = Instant::now() + READER_JOIN_TIMEOUT;
    for (kind, h, err_slot) in [
        ("stdout", out_handle, read_errors.0),
        ("stderr", err_handle, read_errors.1),
    ] {
        loop {
            if h.is_finished() {
                let _ = h.join();
                break;
            }
            if Instant::now() >= deadline {
                audit(&format!(
                    "run_python err | kind=reader_timeout | reader={kind} | join 超时（reader_leaked），已 detach"
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Ok(g) = err_slot.lock() {
            if let Some(e) = g.as_ref() {
                audit(&format!(
                    "run_python warn | kind=read_interrupted | reader={kind} | 输出读取中断（可能不完整）：{e}"
                ));
            }
        }
    }
}

pub fn truncate_output(s: String) -> String {
    let count = s.chars().count();
    if count <= OUTPUT_CAP {
        s
    } else {
        let mut out: String = s.chars().take(OUTPUT_CAP).collect();
        out.push_str("\n…（输出已截断）");
        out
    }
}

// 抑制 dead_code 警告
