//! stdio MCP 子进程传输：读侧带行长上限（外部服务器不能靠一条长行撑爆宿主）。
//!
//! 为什么不用 rmcp 自带的 `TokioChildProcess`：它的读侧
//! （rmcp `transport/async_rw.rs` 的 `read_until(b'\n', &mut line_buf)`）
//! **没有行长上限**——`line_buf` 一直追加到遇见换行符为止。外部 MCP 服务器
//! （第三方 npm 包，不可信）打印一条不带换行的长行（进度条 / base64 / 崩溃
//! dump）→ 宿主内存无界增长 → 宿主 OOM。rmcp 的 codec 虽有 `max_length`，
//! 但它只用于**写侧**，读侧不生效。
//!
//! 这里改用 rmcp 的 `AsyncRwTransport`（同一个 crate；`transport-child-process`
//! 特性已隐含开启 `transport-async-rw`，**不引新依赖、不 vendor 整包**），
//! 读侧套一层 [`LineCappedReader`]：**每行最多转发 `cap` 字节**，超出的部分
//! 直接丢弃（不缓冲），换行符照常转发 → rmcp 拿到一条被截短的行，按它既有的
//! 口径「非法 JSON 逐行忽略」跳过，会话继续，内存有界。
//!
//! 子进程生命周期仍由本模块托管，与 rmcp 原实现同口径：传输关闭时先关
//! stdin（让服务器看见 EOF 自己退），给 3s 优雅退出窗口，超时强杀；传输被
//! 丢弃时同步强杀，不留孤儿进程。

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::Transport;
use rmcp::RoleClient;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// stdout 单行上限（字节）：超出的部分丢弃，该行按「解析失败」被 rmcp 忽略。
pub const MAX_STDOUT_LINE: usize = 1024 * 1024;

/// stderr 单行保留上限（字节）：**只留尾部**（超长行的末尾通常才是真正的
/// 错误信息；前面的进度条/base64 没有诊断价值）。
pub const MAX_STDERR_LINE: usize = 4000;

/// 关闭传输时给子进程的优雅退出窗口（秒）：关 stdin 后等它自己退。
const GRACEFUL_SHUTDOWN_SECS: u64 = 3;

/// 行长上限读侧：每行最多转发 `cap` 字节，超出部分丢弃（不缓冲、不分配）。
///
/// 丢弃而不是报错，是为了保住会话：服务器刷一条超长行后**仍能正常应答**，
/// 只是那条行本身被跳过。整条连接因此不会因为外部服务器的一次坏输出而断。
pub struct LineCappedReader<R> {
    inner: R,
    cap: usize,
    /// 当前行已转发的字节数（不含换行符）
    line_used: usize,
    /// 当前行已超限：后续字节丢弃直到换行
    dropping: bool,
    /// 已处理、待转发的字节
    pending: Vec<u8>,
    /// 是否已报过「超长行」提示（每连接只报一次，免得被刷屏）
    reported: bool,
}

impl<R> LineCappedReader<R> {
    pub fn new(inner: R, cap: usize) -> Self {
        Self {
            inner,
            cap,
            line_used: 0,
            dropping: false,
            pending: Vec::new(),
            reported: false,
        }
    }

    /// 把刚读到的一块字节过一遍行长闸：超限部分丢弃，换行符照常转发。
    fn ingest(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' {
                self.pending.push(b);
                self.line_used = 0;
                self.dropping = false;
                continue;
            }
            if self.dropping {
                continue;
            }
            if self.line_used < self.cap {
                self.pending.push(b);
                self.line_used += 1;
            } else {
                self.dropping = true;
                if !self.reported {
                    self.reported = true;
                    eprintln!(
                        "[mcp] 服务器输出超长行（>{} 字节）已截断——该行被跳过，后续消息照常处理",
                        self.cap
                    );
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for LineCappedReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        loop {
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            if !me.pending.is_empty() {
                let n = me.pending.len().min(buf.remaining());
                buf.put_slice(&me.pending[..n]);
                me.pending.drain(..n);
                return Poll::Ready(Ok(()));
            }
            let mut chunk = [0u8; 8192];
            let mut rb = ReadBuf::new(&mut chunk);
            match Pin::new(&mut me.inner).poll_read(cx, &mut rb) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let filled = rb.filled().len();
                    if filled == 0 {
                        // EOF：没有待转发字节 = 真的读完了
                        return Poll::Ready(Ok(()));
                    }
                    me.ingest(&chunk[..filled]);
                }
            }
        }
    }
}

/// stdio 子进程传输：读侧带行长上限 + 子进程生命周期托管。
pub struct CappedChildProcess {
    /// 子进程句柄（None = 已收尾）
    child: Option<Child>,
    inner: AsyncRwTransport<RoleClient, LineCappedReader<ChildStdout>, ChildStdin>,
}

impl CappedChildProcess {
    /// 用已 spawn 的子进程与其管道搭传输。`stdout` / `stdin` 必须来自
    /// `Stdio::piped()` 的子进程。
    pub fn new(child: Child, stdout: ChildStdout, stdin: ChildStdin, cap: usize) -> Self {
        let reader = LineCappedReader::new(stdout, cap);
        let inner = AsyncRwTransport::new_client(reader, stdin);
        Self {
            child: Some(child),
            inner,
        }
    }
}

impl Transport<RoleClient> for CappedChildProcess {
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        self.inner.receive()
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // 先关 stdin：MCP 服务器看见 EOF 会自己退（rmcp 原实现同口径）
        self.inner.close().await?;
        if let Some(mut child) = self.child.take() {
            match tokio::time::timeout(Duration::from_secs(GRACEFUL_SHUTDOWN_SECS), child.wait())
                .await
            {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
        Ok(())
    }
}

impl Drop for CappedChildProcess {
    fn drop(&mut self) {
        // 传输被丢弃（服务异常退出 / 槽位清理）时同步强杀，不留孤儿进程。
        // 用 start_kill（同步 SIGKILL）而不是 rmcp 的 tokio::spawn(kill)：
        // 后者在没有运行时的 Drop 里会 panic。
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

/// stderr 后台收割：按字节块读，**不整行分配**；每行只留尾部
/// [`MAX_STDERR_LINE`] 字节，进环形缓冲（满则挤掉最老的一行）。
pub async fn harvest_stderr<R: AsyncRead + Unpin>(
    stderr: R,
    ring: Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
) {
    let mut reader = stderr;
    let mut chunk = vec![0u8; 8192];
    // 行缓冲用双端队列：超限时丢最老的一个字节是 O(1)，
    // 用 Vec + drain 会让一条超长行退化成 O(n²)
    let mut line: VecDeque<u8> = VecDeque::new();
    let mut truncated = false;
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &chunk[..n] {
            if b == b'\n' {
                push_line(&mut line, &ring, max_lines, truncated);
                line.clear();
                truncated = false;
            } else {
                line.push_back(b);
                if line.len() > MAX_STDERR_LINE {
                    line.pop_front();
                    truncated = true;
                }
            }
        }
    }
    // 进程退出时最后一行可能没有换行符
    if !line.is_empty() {
        push_line(&mut line, &ring, max_lines, truncated);
    }
}

/// 一行进环形缓冲（超限行加 '…' 前缀标明被截过）。
fn push_line(
    line: &mut VecDeque<u8>,
    ring: &Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
    truncated: bool,
) {
    let bytes = line.make_contiguous();
    let mut s = String::from_utf8_lossy(bytes).into_owned();
    if truncated {
        s.insert(0, '…');
    }
    let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
    if ring.len() >= max_lines {
        ring.pop_front();
    }
    ring.push_back(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// 一条不带换行的超长行：读侧只能转发 cap 字节，其余丢弃。
    /// 这就是「外部服务器一条长行撑爆宿主内存」被堵住的那一刀。
    #[tokio::test]
    async fn long_line_without_newline_is_truncated() {
        let data = vec![b'x'; 100_000];
        let mut reader = LineCappedReader::new(&data[..], 1024);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out.len(), 1024, "超长行只能转发 cap 字节");
        assert!(out.iter().all(|&b| b == b'x'));
    }

    /// 正常行原样转发；超长行被截断后，后续行照常送达（会话不断）。
    #[tokio::test]
    async fn short_lines_survive_a_truncated_neighbour() {
        let mut data = Vec::new();
        data.extend_from_slice(b"{\"id\":1}\n");
        data.extend(vec![b'x'; 5000]);
        data.push(b'\n');
        data.extend_from_slice(b"{\"id\":2}\n");
        let mut reader = LineCappedReader::new(&data[..], 1024);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "{\"id\":1}");
        assert_eq!(lines[1].len(), 1024, "超长行截到 cap 字节");
        assert_eq!(lines[2], "{\"id\":2}");
    }

    /// stderr 侧：超长行只留尾部，且不整行分配（内存有界）。
    #[tokio::test]
    async fn stderr_keeps_only_the_tail_of_a_long_line() {
        let (mut w, r) = tokio::io::duplex(1024);
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let h = tokio::spawn(harvest_stderr(r, ring.clone(), 20));
        w.write_all(&vec![b'z'; 100_000]).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        w.write_all(b"boom\n").await.unwrap();
        drop(w);
        h.await.unwrap();
        let ring = ring.lock().unwrap();
        assert_eq!(ring.len(), 2);
        assert!(ring[1].contains("boom"));
        assert!(ring[0].starts_with('…'), "截过的行要标出来");
        assert!(ring[0].ends_with('z'), "留的是尾部");
        assert!(
            ring[0].len() <= MAX_STDERR_LINE + 4,
            "行长度必须有界：{}",
            ring[0].len()
        );
    }

    /// 环形缓冲满了挤掉最老的一行。
    #[tokio::test]
    async fn stderr_ring_keeps_the_newest_lines() {
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        for i in 0..5 {
            let mut line = VecDeque::new();
            line.extend(format!("line{i}").bytes());
            push_line(&mut line, &ring, 3, false);
        }
        let ring = ring.lock().unwrap();
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.front().unwrap(), "line2");
        assert_eq!(ring.back().unwrap(), "line4");
    }
}
