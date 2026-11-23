// 测试：Err("...".into()) 逃生舱改走专用 CommandError 变体

use super::*;

/// bot_chat 的开关守卫：bot_get_enabled == false 时必须映射到 BotDisabled 专用变体
/// （不是 From<&str> 兜底的 Internal）
#[test]
fn bot_disabled_maps_to_dedicated_variant() {
    let err = require_bot_enabled(false).expect_err("关闭时应返回 Err");
    assert_eq!(err, CommandError::BotDisabled, "应为 BotDisabled 专用变体");
    assert_eq!(err.code(), crate::error::CommandErrorCode::BotDisabled);
    assert!(
        err.is_recoverable(),
        "BotDisabled 应可恢复（引导去设置页开启）"
    );
    assert!(err.message().contains("机器人聊天已关闭"));
    assert!(require_bot_enabled(true).is_ok(), "开启时应通过");
}

// bot_compact 的 key 守卫已收编进 keyring::read_llm_auth（空 key 语义唯一化：
// 空串 = 免鉴权厂商跳过鉴权头，其余空 key 在 read_llm_auth 内收敛成
// ApiKeyMissing），本文件不再单测已删除的 require_api_key。
