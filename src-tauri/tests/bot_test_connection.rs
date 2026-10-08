//! bot_test_connection 集成测试（2026-10-02 厂商详情页「测试连接」）
//!
//! 仿 llm_integration.rs 的 mock server 模式：tests/mock_llm.rs 本地 HTTP server
//! + 真实 reqwest，直打可测内核 `bot::config::commands::probe_connection`
//!（不依赖 Tauri runtime / keyring）。
//!
//! 覆盖：200 → ok:true + status；401 → ok:false + status:401；
//! 连接失败（端口无人监听）→ ok:false + error 消息；anthropic 协议变体同路径。

mod mock_llm_re_export {
    include!("mock_llm.rs");
}

use mock_llm_re_export::{MockBehavior, MockLlmServer};
use wmessage_lib::bot::config::commands::probe_connection;

#[tokio::test]
async fn connection_test_ok_on_2xx() {
    let server = MockLlmServer::start();
    // 队列空 → 默认 TextReply 200（探测只判 HTTP 状态，不解析 body）
    let r = probe_connection(&reqwest::Client::new(), &server.base_url, "openai", "k").await;
    assert!(r.ok, "2xx 应 ok:true：{r:?}");
    assert_eq!(r.status, Some(200));
    assert_eq!(r.error, None);
    assert_eq!(server.request_count(), 1);
    // 探测打 GET {base}/models 探活路径
    let paths = server.request_paths();
    assert!(
        paths[0].contains("GET /v1/models"),
        "应打 /models 探活路径：{paths:?}"
    );
}

#[tokio::test]
async fn connection_test_401_reports_status() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        401,
        r#"{"error":{"message":"Invalid API key"}}"#.into(),
    ));
    let r = probe_connection(&reqwest::Client::new(), &server.base_url, "openai", "bad").await;
    assert!(!r.ok, "401 应 ok:false：{r:?}");
    assert_eq!(r.status, Some(401));
    assert_eq!(r.error, None, "HTTP 状态失败不带网络错误消息");
}

#[tokio::test]
async fn connection_test_anthropic_format_same_path() {
    // anthropic 协议 + base 已含 /v1：直接拼 /models；鉴权头差异在 apply_anthropic_auth
    //（mock server 不记录 header，头构造由 bot_anthropic 模块内单测覆盖）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        403,
        r#"{"error":"forbidden"}"#.into(),
    ));
    let r = probe_connection(&reqwest::Client::new(), &server.base_url, "anthropic", "k").await;
    assert!(!r.ok, "403 应 ok:false：{r:?}");
    assert_eq!(r.status, Some(403));
    let paths = server.request_paths();
    assert!(
        paths[0].contains("GET /v1/models"),
        "anthropic 打 /v1/models 探活路径：{paths:?}"
    );
}

#[tokio::test]
async fn connection_test_anthropic_base_without_v1_gets_v1_prefix() {
    // 回归（审计 P0）：anthropic base 不带 /v1（预设 https://api.anthropic.com）
    // 时探测必须补 /v1，否则打 /models → 404 → 厂商永远进不了 verified_vendors
    let server = MockLlmServer::start();
    let base = server.base_url.trim_end_matches("/v1").to_string();
    let r = probe_connection(&reqwest::Client::new(), &base, "anthropic", "k").await;
    assert!(r.ok, "默认 200 应 ok:true：{r:?}");
    let paths = server.request_paths();
    assert!(
        paths[0].contains("GET /v1/models"),
        "base 不带 /v1 应补前缀：{paths:?}"
    );
}

#[tokio::test]
async fn connection_test_endpoint_suffix_stripped() {
    // 回归（ocr high）：误把完整端点粘进 Base URL 时先剥尾段再拼探测路径——
    // anthropic …/v1/messages 不得叠成 /v1/messages/v1/models，
    // openai …/chat/completions 不得叠成 /chat/completions/models
    let server = MockLlmServer::start();
    let r = probe_connection(
        &reqwest::Client::new(),
        &format!("{}/messages", server.base_url),
        "anthropic",
        "k",
    )
    .await;
    assert!(r.ok, "{r:?}");
    let r = probe_connection(
        &reqwest::Client::new(),
        &format!("{}/chat/completions", server.base_url),
        "openai",
        "k",
    )
    .await;
    assert!(r.ok, "{r:?}");
    let paths = server.request_paths();
    assert!(
        paths.iter().all(|p| p.contains("GET /v1/models")),
        "两次探测都应打 /v1/models：{paths:?}"
    );
}

#[tokio::test]
async fn connection_test_unreachable_reports_error() {
    // 探 127.0.0.1:1（tcpmux 保留端口，无监听由地址本身保证 refused）——
    // 弃用 bind:0→drop 方案：端口释放后可能被并行测试抢占，TOCTOU 竞口
    let base = "http://127.0.0.1:1/v1";
    let r = probe_connection(&reqwest::Client::new(), base, "openai", "k").await;
    assert!(!r.ok, "连不上应 ok:false：{r:?}");
    assert_eq!(r.status, None, "网络错误无 HTTP 状态");
    assert!(
        r.error.as_deref().is_some_and(|e| !e.is_empty()),
        "网络错误应带消息：{r:?}"
    );
}
