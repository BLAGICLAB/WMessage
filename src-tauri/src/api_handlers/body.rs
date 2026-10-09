//! 请求体读取：上限 + 总时长 + 滴注防护。
//!
//! - Content-Length 声明即超限 → 预拒（413），不等读完
//! - 总读时长 BODY_READ_DEADLINE 滴注检查，防「每次 read 都按时返回」的 slowloris
//! - 单次 read 系统调用级超时由 vendor patch 的 tiny_http 提供（30s）

use std::io::Read;
use std::time::{Duration, Instant};

use tiny_http::Request;

/// 请求体上限（防内存打爆）
pub(crate) const MAX_BODY_BYTES: u64 = 1_000_000;
/// body 读取总时长上限：vendor patch 的 30s 读超时是
/// 「单次 read 系统调用」级——发完 header 后以 <30s 间隔滴注 body，每次 read 都按时
/// 返回，worker 永久占住并发名额（MAX_WORKERS=64 占满即全员 503）。
/// 分块读循环在每次 read 返回后检查总时长，滴注最迟 35s 被拒（408）。
pub(crate) const BODY_READ_DEADLINE: Duration = Duration::from_secs(35);

/// 请求体读取结果分流：`TooLarge` → 413；`IoFailed` → 408（读超时/连接中断
/// 语义，不得误报成「body 过大」）。
pub(crate) enum BodyRead {
    Ok(String),
    TooLarge,
    IoFailed,
    /// 头部格式非法(多 Content-Length / parse 失败) → 400 Bad Request
    Malformed,
}

/// 读请求体（上限 `MAX_BODY_BYTES`，总时长 `BODY_READ_DEADLINE`）。
/// Content-Length 声明即超限的直接预拒（413），不再读完才判。
pub(crate) fn read_body_limited(req: &mut Request) -> BodyRead {
    // 按 Content-Length 预拒绝——声明 >1MB 的 body 不必读
    // 同时拒绝 RFC 7230 §3.3.2 禁止的多 Content-Length(可能被中间件拼出来)
    // 以及 parse 失败的脏值(原代码 .ok() 静默吞,等价于「承认任何格式」)
    let mut content_length_headers: Vec<&str> = Vec::new();
    for h in req.headers().iter() {
        if h.field.equiv("Content-Length") {
            content_length_headers.push(h.value.as_str());
        }
    }
    if content_length_headers.len() > 1 {
        return BodyRead::Malformed;
    }
    // Transfer-Encoding 出现即拒（含 chunked）：Content-Length 预拒对 TE 请求无效，
    // 本地 API 不需要 TE——拒绝优先于「CL 与 TE 并存时的优先级歧义」（RFC 7230 §3.3.3）
    for h in req.headers().iter() {
        if h.field.equiv("Transfer-Encoding") {
            return BodyRead::Malformed;
        }
    }
    let mut declared_len: Option<u64> = None;
    if let Some(raw) = content_length_headers.first() {
        let declared = match raw.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return BodyRead::Malformed,
        };
        if declared > MAX_BODY_BYTES {
            return BodyRead::TooLarge;
        }
        // 声明长度留给读完后的截断校验：提前 EOF = 请求不完整，
        // 不得把截断 body 当成功回给 handler（JSON 会被静默切半）
        declared_len = Some(declared);
    }
    let deadline = Instant::now() + BODY_READ_DEADLINE;
    let mut buf = Vec::new();
    let mut reader = req.as_reader().take(MAX_BODY_BYTES + 1);
    let mut chunk = [0u8; 8192];
    let mut total_read: u64 = 0;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                total_read += n as u64;
                if buf.len() as u64 > MAX_BODY_BYTES {
                    return BodyRead::TooLarge;
                }
                // 滴注检查：每次 read 返回后看总时长（单次 read 的 30s 超时管不到
                // 「每次都按时返回」的 slowloris，见 BODY_READ_DEADLINE 注释）
                if Instant::now() >= deadline {
                    return BodyRead::IoFailed;
                }
            }
            Err(_) => return BodyRead::IoFailed,
        }
    }
    // 实读字节数 ≠ 声明值（声明少发 = 截断）→ 按连接中断语义回 408
    if let Some(exp) = declared_len {
        if total_read != exp {
            return BodyRead::IoFailed;
        }
    }
    // JSON 边界必须严格 UTF-8：from_utf8_lossy 会把非法字节静默替换成 U+FFFD，
    // 损坏内容入库比当场拒绝更糟 → 归入 Malformed（调用方回 400）
    match String::from_utf8(buf) {
        Ok(s) => BodyRead::Ok(s),
        Err(_) => BodyRead::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 起真实 loopback server 收原始字节请求（tiny_http 的 TestRequest body 是
    /// &'static str，带不了非法 UTF-8 字节）；返回的 server/stream 须活到 body 读完
    fn request_with_raw_body(body: &[u8]) -> (tiny_http::Server, Request, std::net::TcpStream) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        stream
            .write_all(
                format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
            )
            .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        let req = server.recv().unwrap();
        (server, req, stream)
    }

    /// 非法 UTF-8 body → Malformed（调用方回 400），不得 lossy 替换成 U+FFFD 入库
    #[test]
    fn invalid_utf8_body_is_malformed() {
        let (_server, mut req, _stream) = request_with_raw_body(&[0x7b, 0xff, 0x7d]);
        assert!(matches!(read_body_limited(&mut req), BodyRead::Malformed));
    }

    #[test]
    fn valid_utf8_body_reads_back() {
        let (_server, mut req, _stream) = request_with_raw_body(br#"{"a":1}"#);
        match read_body_limited(&mut req) {
            BodyRead::Ok(s) => assert_eq!(s, r#"{"a":1}"#),
            _ => panic!("合法 UTF-8 body 不应被拒"),
        }
    }
}
