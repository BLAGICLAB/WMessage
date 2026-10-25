//! HTTP 传输的 URL 安全闸：字面量级 + DNS 级 + 钉 IP。
//! 全部用例不依赖外网：IP 字面量的解析由 getaddrinfo 本地完成，
//! 钉 IP 用 .invalid 域名（RFC 2606，真实 DNS 必解析失败）。
use super::*;

fn no_headers() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::new()
}

/// 字面量级闸：内网 / 环回 / 链路本地地址在发请求前就拒。
#[tokio::test]
async fn private_literal_url_is_rejected_before_any_request() {
    for u in [
        "http://127.0.0.1:8080/mcp",
        "http://169.254.169.254/latest/meta-data",
        "http://localhost:3000/mcp",
        "http://10.0.0.5/mcp",
    ] {
        let err = http_client_for_mcp(u, &no_headers())
            .await
            .expect_err("内网 URL 必须被 URL 安全闸拒掉");
        assert!(err.contains("URL 安全闸"), "{err}");
    }
}

/// 公网 IP 字面量：字面量闸放行 → DNS 级闸也放行 → 客户端建得出来。
/// IP 字面量的解析由 getaddrinfo 本地完成（不查外网 DNS）。
#[tokio::test]
async fn public_literal_ip_passes_both_gates() {
    assert!(
        http_client_for_mcp("http://93.184.216.34/mcp", &no_headers())
            .await
            .is_ok(),
        "公网 IP 字面量应通过两道闸"
    );
}

/// 钉 IP 真的生效：.invalid 域名钉到本地回环端口，能连通即证明 reqwest
/// 没有二次解析（DNS TOCTOU 窗口已关）；Host 头仍是原域名。
#[tokio::test]
async fn pinned_client_does_not_resolve_again() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let got_host = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let got_host2 = got_host.clone();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if let Some(line) = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("host:"))
            {
                *got_host2.lock().unwrap() = line[5..].trim().to_string();
            }
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            );
        }
    });
    let url = url::Url::parse(&format!("http://mcp-pinned.invalid:{port}/")).unwrap();
    let pinned = [std::net::SocketAddr::from(([127, 0, 0, 1], 80))];
    let client = http_client_pinned("mcp-pinned.invalid", &pinned, &no_headers())
        .expect("钉住 client 构建失败");
    let resp = client
        .get(url)
        .send()
        .await
        .expect("钉住 127.0.0.1 后请求应成功（.invalid 真实 DNS 必失败，无二次解析）");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        got_host.lock().unwrap().as_str(),
        format!("mcp-pinned.invalid:{port}"),
        "钉 IP 不改 Host 头"
    );
}

/// HTTP 分支必须走 http_client_for_mcp（字面量闸 + DNS 闸 + 钉 IP 三合一）。
/// 连接路径要真连 MCP 服务器，纯函数单测够不到，故以源码文本钉住「分支里
/// 只有这一个闸入口」——否则哪天有人加回裸的字面量级闸，两道闸就只剩一道。
///
/// 扫描源是 manager.rs 的生产段：测试块拆成独立文件后，生产代码是
/// `mod send_probe;` 声明**之前**的部分（测试模块声明在文件末尾）。
#[test]
fn http_branch_goes_through_the_two_gate_client() {
    let full = include_str!("../manager.rs");
    let src = full
        .split_once("mod send_probe;")
        .expect("manager.rs 应有 send_probe 测试模块")
        .0;
    let body = src
        .split_once("McpTransport::Http => {")
        .expect("找不到 HTTP 分支")
        .1
        .split("\n            }")
        .next()
        .expect("HTTP 分支应能截取");
    assert!(
        body.contains("http_client_for_mcp(&url, &cfg.headers)"),
        "HTTP 分支必须走两道闸的客户端构造"
    );
    assert!(
        !body.contains("http_url_is_public"),
        "HTTP 分支不得再单独调字面量级闸——统一在 http_client_for_mcp 里"
    );
}
