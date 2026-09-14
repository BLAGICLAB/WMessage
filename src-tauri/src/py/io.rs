//! IO 限长读取 + 线程 join + 截断

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::bot_slash::StopToken;
use crate::py::runtime::OUTPUT_CAP;

pub fn read_capped_drain<R: Read>(src: R, cap: usize) -> (Vec<u8>, bool) {
    let mut probe = src.take(cap as u64 + 1);
    let mut buf = Vec::new();
    let _ = probe.read_to_end(&mut buf);
    let truncated = buf.len() > cap;
    if truncated {
        buf.truncate(cap);
        let _ = std::io::copy(&mut probe.into_inner(), &mut std::io::sink());
    }
    (buf, truncated)
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

pub fn join_reader_threads(
    out_handle: std::thread::JoinHandle<()>,
    err_handle: std::thread::JoinHandle<()>,
    audit: &mut dyn FnMut(&str),
) {
    let deadline = Instant::now() + READER_JOIN_TIMEOUT;
    for (kind, h) in [("stdout", out_handle), ("stderr", err_handle)] {
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
#[allow(dead_code)]
fn _write_unused(_w: &mut dyn Write) {}
