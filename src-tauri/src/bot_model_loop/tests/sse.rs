//! 模型循环单元测试（sse 桶）。
//!
//! 覆盖读流侧的一条边界：用户叫停后到达的在途数据不再推流，但正文照旧记账。

use super::*;
use crate::bot_model_loop::{LlmHttp, ModelLoopDeps};
use crate::bot_slash::StopGuard;
use std::sync::{Arc, Mutex};

#[cfg(test)]
mod stop_emit_gate_tests {
    use super::*;

    /// 一块 OpenAI 形状的 SSE 数据（正文增量）。
    const CHUNK: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n";

    fn llm_http() -> LlmHttp {
        LlmHttp {
            client: reqwest::Client::new(),
            base_url: "http://127.0.0.1:1".into(),
            api_key: "k".into(),
            model: "m".into(),
            provider: crate::bot::ApiProvider::Openai,
            max_tokens: 1024,
            reasoning: crate::bot::reasoning::ReasoningWire::None,
            temperature: None,
            top_p: None,
            system_prompt: None,
        }
    }

    /// 用户叫停后到达的那一块数据：不再往前端推流式增量，但正文照旧记账。
    ///
    /// 回归背景（真机 2026-10-26）：读流循环只在**每轮开头**查一次停止标志，
    /// 用户按停止时往往正卡在 `stream.next()` 上——在途的那块数据到了照样
    /// 被解析、被推出。那段迟到增量到达时回合往往已经收尾，前端据此新开
    /// 一个没人收的「正在思考」气泡，活体指示永远转。
    #[tokio::test]
    async fn stopped_flag_suppresses_delta_emit_but_keeps_text() {
        let app = tauri::test::mock_app();
        let stop = StopGuard::new(app.handle(), true, Some("s1".to_string()));
        // body 的第一次 poll 就置停止标志：在途数据到达的同时用户按了停止
        let token = stop.token();
        let body = reqwest::Body::wrap_stream(futures_util::stream::once(async move {
            token.stop();
            Ok::<_, std::io::Error>(CHUNK.to_string())
        }));
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .body(body)
                .expect("应能造出响应"),
        );

        let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = emitted.clone();
        let emit = move |ev: &str, _payload: serde_json::Value| {
            sink.lock().expect("锁不该中毒").push(ev.to_string());
        };
        let audit = |_: crate::audit::AuditLevel, _: &str, _: Vec<(&'static str, String)>| {};
        let skill_finish = |_: bool, _: &str| String::new();
        let no_skill_run = |_: Option<&str>| None;
        let on_round = |_: usize| {};
        let no_keywords: Vec<String> = Vec::new();
        let deps = ModelLoopDeps {
            fuse_cap_override: None,
            emit: &emit,
            audit: &audit,
            skill_finish: &skill_finish,
            active_skill_run: &no_skill_run,
            conv_sidecar_dir: None,
            final_selfcheck_round_gap: Some(0),
            final_selfcheck_http: None,
            on_round: &on_round,
            turn_status: None,
            session_tools: None,
            guard_claim_keywords: &no_keywords,
            file_receipts: None,
        };

        let sse = match read_sse_stream(&llm_http(), resp, &stop, &deps, false, false).await {
            Ok(s) => s,
            Err(f) => panic!("流读取不该失败：{}", f.err),
        };
        assert!(sse.stopped, "用户叫停后这一轮必须标记为 stopped");
        // 闸只挡「推流」，不挡「记账」：正文照旧进最终答复（否则答复丢字）
        assert!(
            sse.final_text.contains("你好"),
            "停止后正文仍要记账：{:?}",
            sse.final_text
        );
        let events = emitted.lock().expect("锁不该中毒").clone();
        assert!(
            !events.iter().any(|e| e == "bot-chat-delta"),
            "停止后不得再推流式增量（前端据此会开一个没人收的活体气泡）：{events:?}"
        );
    }

    /// 没叫停时推流照旧（闸不能误伤正常回合）。
    #[tokio::test]
    async fn running_stream_still_emits_delta() {
        let app = tauri::test::mock_app();
        let stop = StopGuard::new(app.handle(), true, Some("s1".to_string()));
        let body = reqwest::Body::wrap_stream(futures_util::stream::once(async {
            Ok::<_, std::io::Error>(CHUNK.to_string())
        }));
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .body(body)
                .expect("应能造出响应"),
        );

        let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = emitted.clone();
        let emit = move |ev: &str, _payload: serde_json::Value| {
            sink.lock().expect("锁不该中毒").push(ev.to_string());
        };
        let audit = |_: crate::audit::AuditLevel, _: &str, _: Vec<(&'static str, String)>| {};
        let skill_finish = |_: bool, _: &str| String::new();
        let no_skill_run = |_: Option<&str>| None;
        let on_round = |_: usize| {};
        let no_keywords: Vec<String> = Vec::new();
        let deps = ModelLoopDeps {
            fuse_cap_override: None,
            emit: &emit,
            audit: &audit,
            skill_finish: &skill_finish,
            active_skill_run: &no_skill_run,
            conv_sidecar_dir: None,
            final_selfcheck_round_gap: Some(0),
            final_selfcheck_http: None,
            on_round: &on_round,
            turn_status: None,
            session_tools: None,
            guard_claim_keywords: &no_keywords,
            file_receipts: None,
        };

        let sse = match read_sse_stream(&llm_http(), resp, &stop, &deps, false, false).await {
            Ok(s) => s,
            Err(f) => panic!("流读取不该失败：{}", f.err),
        };
        assert!(!sse.stopped, "没叫停就不该标记 stopped");
        let events = emitted.lock().expect("锁不该中毒").clone();
        assert!(
            events.iter().any(|e| e == "bot-chat-delta"),
            "正常回合必须照旧推流：{events:?}"
        );
    }
}
