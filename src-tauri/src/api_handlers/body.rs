//! 请求体读取：上限 + 总时长 + 滴注防护。
//!
//! - Content-Length 声明即超限 → 预拒（413），不等读完
//! - 总读时长 BODY_READ_DEADLINE 滴注检查，防「每次 read 都按时返回」的 slowloris
//! - 单次 read 系统调用级超时由 vendor patch 的 tiny_http 提供（30s）

use std::io::{Cursor, Read};
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
}

/// 读请求体（上限 `MAX_BODY_BYTES`，总时长 `BODY_READ_DEADLINE`）。
/// Content-Length 声明即超限的直接预拒（413），不再读完才判。
pub(crate) fn read_body_limited(req: &mut Request) -> BodyRead {
    // 按 Content-Length 预拒绝——声明 >1MB 的 body 不必读
    if let Some(declared) = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Length"))
        .and_then(|h| h.value.as_str().parse::<u64>().ok())
    {
        if declared > MAX_BODY_BYTES {
            return BodyRead::TooLarge;
        }
    }
    let deadline = Instant::now() + BODY_READ_DEADLINE;
    let mut buf = Vec::new();
    let mut reader = req.as_reader().take(MAX_BODY_BYTES + 1);
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
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
    BodyRead::Ok(String::from_utf8_lossy(&buf).into_owned())
}

// 公开 Response 辅助类型让调用方签名简短
pub type JsonResponse = tiny_http::Response<Cursor<Vec<u8>>>;
