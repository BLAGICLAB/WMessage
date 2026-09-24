//! Bot 配置 + 审计底座（阶段 2 拆分后位置）。
//!
//! 阶段 1：原 bot.rs 行 55–1193 全部配置/keyring/审计/constants/check_len 代码
//! 搬入 `bot/config.rs`；阶段 2（本次 run5）：`bot/config.rs` 1923 行 → 7 子模块。
//!
//! 公开路径由 `bot.rs` 的 `pub use config::{...}` 块统一再导出：
//! 所有外部调用走 `crate::bot::<item>`，不直接访问 `crate::bot::config::<item>`。
//! 因此子模块边界不破坏外部接口——facade 模式。
//!
//! 模块分层：
//! - `types`    核心数据类型（KeySlot/BotConfig/ModelEntry/ModelsByProvider/
//!              ActiveModelId/ApiProvider/PermMode/BotConfigView）
//!              + 12 个 MAX_* 常量 + check_len + keyring service 名
//! - `schema`   双协议模型迁移（migrate_legacy_models/derive_default_label/
//!              derive_legacy_fields_from_active）+ max_tokens 解析
//!              + bot-config.json schema 版本号迁移（migrate_config_value/
//!              migrate_config_file/migrate_bot_config_schema）
//! - `keyring`  keyring 后端探测（System / PlaintextFile）+ 按后端分发
//!              read/has/write/delete + 3 个 KeySlot 顶层入口
//!              + System 后端 v0→v1 service 迁移 + 降级明文回迁
//! - `io`       bot-config.json 读写（load_config/write_bot_config_file/
//!              add_allowed_dir/update_config_file/read_bypass_llm_switch/
//!              base_url_is_safe）+ 老版本明文 key 迁移
//!              （migrate_legacy_key/migrate_search_keys/migrate_search_key_slot）
//! - `audit`    bot.log 审计写入（audit_log/audit_log_hook/append_bot_log_line）
//!              + escape_for_log/truncate_for_log + read_log_tail
//! - `commands` 4 个 tauri command（bot_get_config/bot_set_config/
//!              bot_clear_api_key/bot_log_read）+ perm_mode helper

pub mod audit;
pub mod commands;
pub mod io;
pub mod keyring;
pub mod schema;
pub mod types;

// ──────────────────── bot.rs facade re-exports ────────────────────
// bot.rs 的 `pub use config::{...}` 块需要这些符号在 crate::bot::config::* 路径可见。
// 子模块内符号分别 re-export 到此，供 facade 一一加载，保持外部路径 crate::bot::X 不变。

pub use audit::{audit_log, audit_log_hook, bot_log_read};
pub use commands::{bot_clear_api_key, bot_get_config, bot_set_config, perm_mode};
pub use io::{config_path, migrate_legacy_key, migrate_search_keys, read_bypass_llm_switch};
pub use keyring::{has_api_key, has_search_key, read_api_key, read_search_key, write_search_key};
pub use schema::{
    migrate_bot_config_schema, resolve_max_tokens, DEFAULT_MAX_TOKENS, MAX_MAX_TOKENS,
    MIN_MAX_TOKENS,
};
pub use types::{
    check_len, ActiveModelId, ApiProvider, BotConfig, BotConfigView, KeySlot, ModelEntry,
    ModelsByProvider, PermMode, BOT_CONFIG_SCHEMA_VERSION, KEYRING_SERVICE, KEYRING_USER, MAX_DUE,
    MAX_KEYWORD, MAX_NOTE, MAX_SUBTASK_TEXT, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE,
};

// pub(crate) 项：bot.rs 的 `pub(crate) use config::{...}` 块需要。
pub(crate) use audit::{escape_for_log, truncate_for_log};
pub(crate) use io::{
    add_allowed_dir, base_url_is_safe, load_config, migrate_search_key_slot, update_config_file,
    write_bot_config_file,
};

// tauri 命令宏生成物（__cmd__* / __tauri_command_name_*）：在 commands.rs 里
// #[tauri::command] 函数旁边自动生成；同样 re-export 以让 bot.rs 的 facade 一行不动。
pub use commands::{
    __cmd__bot_clear_api_key, __cmd__bot_get_config, __cmd__bot_set_config,
    __tauri_command_name_bot_clear_api_key, __tauri_command_name_bot_get_config,
    __tauri_command_name_bot_set_config,
};
// bot_log_read 是 #[tauri::command] 但住在 audit.rs（同模块同生命周期），
// 宏生成物也在 audit 里。__cmd__bot_log_read 与 __tauri_command_name_bot_log_read
// 因此走 audit 子模块 re-export。
pub use audit::{__cmd__bot_log_read, __tauri_command_name_bot_log_read};

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    // 跨 6 个子模块的集成测试，super::* 只拿 mod.rs 顶层项，所以这里显式列每个子模块的入口。
    use super::*;
    use crate::bot::config::audit::{escape_for_log, read_log_tail, truncate_for_log};
    use crate::bot::config::io::{
        add_allowed_dir, base_url_is_safe, load_config, migrate_legacy_key,
        migrate_search_key_slot, read_bypass_llm_switch_at, update_config_file,
        write_bot_config_file,
    };
    use crate::bot::config::keyring::{
        backend_for, classify_get_password, classify_has_key, delete_api_key_at, has_api_key,
        has_api_key_at, has_key_of_slot, key_backend, plaintext_key_path_for, read_api_key,
        read_api_key_at, read_search_key, secret_service_available_with, write_api_key_at,
        write_key_of_slot, write_search_key, KeyBackend,
    };
    use crate::bot::config::schema::{
        derive_legacy_fields_from_active, migrate_bot_config_schema, migrate_config_file,
        migrate_legacy_models, resolve_max_tokens, SCHEMA_VERSION_KEY,
    };
    use crate::bot::config::schema::{DEFAULT_MAX_TOKENS, MAX_MAX_TOKENS, MIN_MAX_TOKENS};
    use crate::bot::config::types::{
        check_len, ActiveModelId, ApiProvider, BotConfig, KeySlot, ModelEntry, ModelsByProvider,
        PermMode, BOT_CONFIG_SCHEMA_VERSION, KEYRING_SERVICE, LEGACY_KEYRING_SERVICE, MAX_DUE,
        MAX_KEYWORD, MAX_NOTE, MAX_SUBTASK_TEXT, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE,
    };

    // ────────── bot_config_tests（基础类型 + 默认 + 老配置） ──────────

    #[test]
    fn default_has_bypass_llm_on_pre_step_hit_true() {
        let cfg = BotConfig::default();
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "Default impl 应默认开启新行为（bypass=true）"
        );
    }

    #[test]
    fn old_config_without_bypass_field_deserializes_to_true() {
        // 模拟老用户 bot-config.json 没有 bypass_llm_on_pre_step_hit 字段
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig =
            serde_json::from_str(raw).expect("老配置应通过 struct 级 #[serde(default)] 兼容");
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "老配置缺字段应默认 true（开启新行为，保证 release 不破现有用户）"
        );
    }

    #[test]
    fn perm_mode_defaults_to_ask() {
        // 老配置缺 permMode 字段 → None → Ask（默认：弹授权而非硬拒）
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(PermMode::from_cfg(cfg.perm_mode.as_deref()), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(None), PermMode::Ask);
    }

    #[test]
    fn perm_mode_parses_known_values_and_falls_back() {
        assert_eq!(PermMode::from_cfg(Some("strict")), PermMode::Strict);
        assert_eq!(PermMode::from_cfg(Some("ask")), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(Some("yolo")), PermMode::Yolo);
        // 非法值/空白回退 Ask（安全默认偏严一侧的可用形态）
        assert_eq!(
            PermMode::from_cfg(Some("YOLO ")),
            PermMode::Ask,
            "大小写不识别，回退 Ask"
        );
        assert_eq!(PermMode::from_cfg(Some("garbage")), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(Some("")), PermMode::Ask);
        // as_str 往返
        assert_eq!(PermMode::Strict.as_str(), "strict");
        assert_eq!(PermMode::Ask.as_str(), "ask");
        assert_eq!(PermMode::Yolo.as_str(), "yolo");
    }

    #[test]
    fn api_provider_defaults_to_openai_and_falls_back() {
        // 老配置缺 apiProvider 字段 → None → Openai（Anthropic 兼容模式对
        // 老配置零影响）；非法值/空白防御回退 Openai（与 PermMode::from_cfg 同风格）
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            ApiProvider::from_cfg(cfg.api_provider.as_deref()),
            ApiProvider::Openai
        );
        assert_eq!(ApiProvider::from_cfg(None), ApiProvider::Openai);
        assert_eq!(ApiProvider::from_cfg(Some("openai")), ApiProvider::Openai);
        assert_eq!(
            ApiProvider::from_cfg(Some("anthropic")),
            ApiProvider::Anthropic
        );
        assert_eq!(
            ApiProvider::from_cfg(Some("Anthropic")),
            ApiProvider::Openai,
            "大小写不识别，回退 Openai"
        );
        assert_eq!(ApiProvider::from_cfg(Some("garbage")), ApiProvider::Openai);
        assert_eq!(ApiProvider::Openai.as_str(), "openai");
        assert_eq!(ApiProvider::Anthropic.as_str(), "anthropic");
    }

    #[test]
    fn max_tokens_default_and_clamped() {
        assert_eq!(
            resolve_max_tokens(None),
            DEFAULT_MAX_TOKENS,
            "None = 默认 8192"
        );
        assert_eq!(resolve_max_tokens(Some(4096)), 4096);
        assert_eq!(
            resolve_max_tokens(Some(1)),
            MIN_MAX_TOKENS,
            "低于下限钳 256"
        );
        assert_eq!(
            resolve_max_tokens(Some(999_999)),
            MAX_MAX_TOKENS,
            "高于上限钳 200000"
        );
    }

    // ── bot-config.json schema 迁移 ──

    #[test]
    fn config_schema_migration_fills_missing_version_and_preserves_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        std::fs::write(
            &p,
            r#"{"baseUrl":"https://api.example.com/v1","model":"m","apiKey":"***","allowedDirs":["/a"]}"#,
        )
        .unwrap();
        let got = migrate_config_file(&p).unwrap();
        assert_eq!(got, Some((0, BOT_CONFIG_SCHEMA_VERSION)), "缺字段必须补写");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            v[SCHEMA_VERSION_KEY],
            serde_json::json!(BOT_CONFIG_SCHEMA_VERSION)
        );
        // 明文 key / 其他字段原样保留：迁移只管版本号，清明文是 migrate_legacy_key 的职责
        assert_eq!(v["apiKey"], serde_json::json!("***"));
        assert_eq!(v["allowedDirs"][0], serde_json::json!("/a"));
    }

    #[test]
    fn config_schema_migration_is_idempotent_and_skips_future_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        // 已是当前版本 → 不再写回（load_config 热路径不产生无谓写盘）
        std::fs::write(
            &p,
            format!(r#"{{"{SCHEMA_VERSION_KEY}":{BOT_CONFIG_SCHEMA_VERSION}}}"#),
        )
        .unwrap();
        assert_eq!(migrate_config_file(&p).unwrap(), None);
        // 未来版本（用户装过更新版后回退）→ 不降级、不动文件
        std::fs::write(&p, format!(r#"{{"{SCHEMA_VERSION_KEY}":999}}"#)).unwrap();
        assert_eq!(migrate_config_file(&p).unwrap(), None);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v[SCHEMA_VERSION_KEY], serde_json::json!(999));
    }

    #[test]
    fn config_schema_migration_tolerates_missing_or_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(migrate_config_file(&missing).unwrap(), None);
        // JSON 损坏 / 非对象：不在这里纠错（读取侧回默认），更不能 panic
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        assert_eq!(migrate_config_file(&broken).unwrap(), None);
        let arr = dir.path().join("array.json");
        std::fs::write(&arr, "[1,2,3]").unwrap();
        assert_eq!(migrate_config_file(&arr).unwrap(), None);
    }

    #[test]
    fn bot_config_default_and_legacy_deserialize_carry_current_version() {
        assert_eq!(
            BotConfig::default().schema_version,
            BOT_CONFIG_SCHEMA_VERSION
        );
        // 老配置（无 schemaVersion 字段）：字段级 default 兜底为当前版本
        let cfg: BotConfig =
            serde_json::from_str(r#"{"baseUrl":"https://api.example.com/v1"}"#).unwrap();
        assert_eq!(cfg.schema_version, BOT_CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn keyring_service_names_are_versioned() {
        // 协议锁：service 名变更必须同步「新 + 旧」两个常量，否则存量用户 key 读不到
        assert_eq!(KEYRING_SERVICE, "wmessage.bot.v1");
        assert_eq!(LEGACY_KEYRING_SERVICE, "wmessage_bot");
    }

    /// F-1 的 bypass 开关语义：只有显式 false 才关，其余（文件缺失 / 读失败 /
    /// JSON 损坏 / 缺字段）一律 true——默认走 bypass（命中 Skill 后不再多烧一次
    /// 外层 LLM）。走可测内核（纯路径参数），不碰 mock app 的共享数据目录。
    #[test]
    fn bypass_llm_switch_defaults_true_and_honors_explicit_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        // 文件不存在 → true
        assert!(read_bypass_llm_switch_at(&p), "文件缺失应默认 bypass");
        // 缺字段（老配置）→ true
        std::fs::write(&p, r#"{"baseUrl":"https://api.example.com/v1"}"#).unwrap();
        assert!(read_bypass_llm_switch_at(&p), "缺字段应默认 bypass");
        // 显式 true / false → 跟随配置
        std::fs::write(&p, r#"{"bypassLlmOnPreStepHit":true}"#).unwrap();
        assert!(read_bypass_llm_switch_at(&p));
        std::fs::write(&p, r#"{"bypassLlmOnPreStepHit":false}"#).unwrap();
        assert!(!read_bypass_llm_switch_at(&p), "显式 false 必须关掉 bypass");
        // JSON 损坏 → true（聊天入口不能被坏配置挡住）
        std::fs::write(&p, "{not json").unwrap();
        assert!(
            read_bypass_llm_switch_at(&p),
            "损坏配置应默认 bypass 而非 panic"
        );
    }

    // ────────── keyring 错误分类纯函数单测 ──────────

    /// 真实 keychain 在测试环境不可用，用构造的 keyring::Error 注入故障。
    fn platform_failure() -> ::keyring::Error {
        ::keyring::Error::PlatformFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "keychain locked",
        )))
    }

    #[test]
    fn read_api_key_keyring_failure_maps_to_keyring_error_variant() {
        // keyring 故障（钥匙串锁定/权限拒绝）→ KeyringError 结构化变体，不走 String 逃生舱
        let err = classify_get_password(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::KeyringError);
        assert!(err.message().contains("读取 API Key 失败"));
        // error.rs 定义：KeyringError recoverable=false（用户需先解锁 keychain，重试无意义）
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_api_key_success_passes_value_through() {
        let key = classify_get_password(Ok("sk-test".into())).unwrap();
        assert_eq!(key, "sk-test");
    }

    #[test]
    fn has_api_key_no_entry_is_ok_false() {
        // 「key 不存在」不是故障：Ok(false)，前端显示「未配置 API Key」
        let r = classify_has_key(Err(::keyring::Error::NoEntry)).unwrap();
        assert!(!r);
    }

    #[test]
    fn has_api_key_keyring_failure_not_swallowed_to_false() {
        // keyring 真实故障不得吞成 false（「反复填 key 仍失败无提示」假象）
        let err = classify_has_key(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::KeyringError);
        assert!(err.message().contains("检查 API Key 失败"));
    }

    #[test]
    fn has_api_key_present_is_ok_true() {
        assert!(classify_has_key(Ok("sk-test".into())).unwrap());
    }

    // ────────── Linux secret-service 探测 + 降级明文文件后端 ──────────

    #[test]
    fn backend_for_selects_plaintext_when_secret_service_down() {
        assert_eq!(backend_for(true), KeyBackend::System);
        assert_eq!(backend_for(false), KeyBackend::PlaintextFile);
    }

    #[test]
    fn secret_service_probe_dbus_addr_or_bus_socket() {
        assert!(
            secret_service_available_with(true, None),
            "有 dbus 地址即可用"
        );
        assert!(
            !secret_service_available_with(false, None),
            "无任何线索 = 不可用"
        );
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().to_str().unwrap();
        assert!(
            !secret_service_available_with(false, Some(d)),
            "XDG_RUNTIME_DIR 无 bus socket = 不可用"
        );
        std::fs::write(tmp.path().join("bus"), b"").unwrap();
        assert!(
            secret_service_available_with(false, Some(d)),
            "XDG_RUNTIME_DIR/bus 存在即可用"
        );
    }

    #[test]
    fn plaintext_backend_roundtrip_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("bot-api-key.txt");
        // has：文件缺失 → Ok(false)（对齐 NoEntry 语义，不吞错）
        assert!(!has_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap());
        // read：缺失 → KeyringError（与 System 路径 NoEntry 同 code，前端 hint 一致）
        let e = read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap_err();
        assert_eq!(e.code(), crate::error::CommandErrorCode::KeyringError);
        // write → 文件落盘 + Unix 0600（与 api-token.txt 同策略）
        write_api_key_at(KeyBackend::PlaintextFile, &f, "***", KeySlot::Llm).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600,
                "降级 key 文件必须 0600"
            );
        }
        assert_eq!(
            read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap(),
            "***"
        );
        assert!(has_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap());
        // delete：删后不存在；再删幂等 Ok
        delete_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap();
        assert!(!f.exists());
        delete_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap();
    }

    #[test]
    fn plaintext_write_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("nested").join("bot-api-key.txt");
        write_api_key_at(KeyBackend::PlaintextFile, &f, "sk-x", KeySlot::Llm).unwrap();
        assert_eq!(
            read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap(),
            "sk-x"
        );
    }

    #[test]
    fn fallback_warn_audit_lands_in_bot_log() {
        // 降级告警：WARN 级别 + 事件名 + reason 落 bot.log（write_warn_audit_to）
        let tmp = tempfile::tempdir().unwrap();
        crate::audit::write_warn_audit_to(
            tmp.path(),
            "keyring_fallback_plaintext",
            &[("reason", "secret-service 不可用")],
        );
        let log = std::fs::read_to_string(tmp.path().join("bot.log")).unwrap();
        assert!(
            log.contains("WARN | keyring_fallback_plaintext"),
            "缺 WARN 审计行: {log:?}"
        );
        assert!(log.contains("reason=secret-service 不可用"), "got: {log:?}");
    }

    // ────────── 搜索 key 进 keyring ──────────

    #[test]
    fn key_slot_keyring_user_and_filename_distinct() {
        // 三个用途的 keyring 条目名 / 降级文件名必须互不相同，且 LLM 保持既有值不变
        assert_eq!(KeySlot::Llm.keyring_user(), "api-key");
        assert_eq!(KeySlot::Llm.plaintext_filename(), "bot-api-key.txt");
        assert_eq!(KeySlot::Tavily.keyring_user(), "tavily_api_key");
        assert_eq!(KeySlot::Tavily.plaintext_filename(), "bot-tavily-key.txt");
        assert_eq!(KeySlot::Brave.keyring_user(), "brave_api_key");
        assert_eq!(KeySlot::Brave.plaintext_filename(), "bot-brave-key.txt");
    }

    #[test]
    fn search_slot_plaintext_backend_roundtrip() {
        // Tavily/Brave 槽位走同一套后端分发（PlaintextFile 注入路径可测）
        let tmp = tempfile::tempdir().unwrap();
        for slot in [KeySlot::Tavily, KeySlot::Brave] {
            let f = tmp.path().join(slot.plaintext_filename());
            assert!(!has_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap());
            write_api_key_at(KeyBackend::PlaintextFile, &f, "tvly-x", slot).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "搜索 key 降级文件同样必须 0600"
                );
            }
            assert_eq!(
                read_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap(),
                "tvly-x"
            );
            assert!(has_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap());
            // 覆盖写
            write_api_key_at(KeyBackend::PlaintextFile, &f, "tvly-y", slot).unwrap();
            assert_eq!(
                read_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap(),
                "tvly-y"
            );
        }
    }

    #[test]
    fn migrate_slot_migrates_plaintext_to_store() {
        let mut written: Vec<String> = Vec::new();
        let (remaining, migrated) = migrate_search_key_slot(Some("tvly-plain"), false, &mut |k| {
            written.push(k.to_string());
            Ok(())
        });
        assert_eq!(written, vec!["tvly-plain"], "应写入 keyring");
        assert_eq!(remaining, None, "写成功 → 配置字段清掉明文");
        assert!(migrated, "应记迁移");
    }

    #[test]
    fn migrate_slot_does_not_overwrite_existing_store_value() {
        // keyring 已有值（用户可能在别处更新过）→ 不覆盖，直接清明文
        let mut written: Vec<String> = Vec::new();
        let (remaining, migrated) =
            migrate_search_key_slot(Some("tvly-old-plain"), true, &mut |k| {
                written.push(k.to_string());
                Ok(())
            });
        assert!(written.is_empty(), "keyring 已有值时不得覆盖写入");
        assert_eq!(remaining, None, "明文仍应清掉（目标 = 文件不留明文）");
        assert!(!migrated, "未发生写入不算迁移（不记迁移审计）");
    }

    #[test]
    fn migrate_slot_write_failure_keeps_plaintext() {
        // keyring 写失败 → 保留文件明文，下次再试（数据保留优先）
        let (remaining, migrated) = migrate_search_key_slot(Some("tvly-plain"), false, &mut |_| {
            Err("keychain locked".to_string())
        });
        assert_eq!(remaining.as_deref(), Some("tvly-plain"), "写失败保留明文");
        assert!(!migrated);
    }

    #[test]
    fn migrate_slot_no_plaintext_is_idempotent() {
        let (r1, m1) = migrate_search_key_slot(None, false, &mut |_| Ok(()));
        assert_eq!((r1, m1), (None, false));
        // 空串/空白视同无明文
        let (r2, m2) = migrate_search_key_slot(Some("  "), false, &mut |_| Ok(()));
        assert_eq!((r2, m2), (None, false));
    }

    #[test]
    fn write_bot_config_file_strips_all_key_fields() {
        // bot_set_config 双保险回归：前端误把 key 塞进 config 对象也不落明文
        let tmp = tempfile::tempdir().unwrap();
        let cfg = BotConfig {
            api_key: Some("***".into()),
            tavily_key: Some("tvly-plain".into()),
            brave_key: Some("bsa-plain".into()),
            ..Default::default()
        };
        write_bot_config_file(tmp.path(), cfg).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("bot-config.json")).unwrap();
        assert!(!raw.contains("***"), "LLM key 不得落盘：{raw}");
        assert!(!raw.contains("tvly-plain"), "Tavily key 不得落盘：{raw}");
        assert!(!raw.contains("bsa-plain"), "Brave key 不得落盘：{raw}");
        // 字段本身也应消失（skip_serializing_if + 强制 None）
        let saved: BotConfig = serde_json::from_str(&raw).unwrap();
        assert!(saved.api_key.is_none() && saved.tavily_key.is_none() && saved.brave_key.is_none());
    }

    // ────────── bot_log_read ──────────

    #[test]
    fn read_log_tail_missing_file_returns_placeholder() {
        // 文件不存在 = 日志确实没东西 → Ok 占位文案（前端按「暂无日志」显示）
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        assert_eq!(read_log_tail(&p, None).unwrap(), "（暂无日志）");
    }

    #[test]
    fn read_log_tail_io_error_not_swallowed() {
        // 路径是目录 → read_to_string 失败（非 NotFound）→ Err(IoError)，不静默吞
        let tmp = tempfile::tempdir().unwrap();
        let err = read_log_tail(tmp.path(), None).unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::IoError);
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_log_tail_returns_reversed_tail_with_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        std::fs::write(&p, "l1\nl2\nl3\n").unwrap();
        assert_eq!(read_log_tail(&p, Some(2)).unwrap(), "l3\nl2");
        assert_eq!(read_log_tail(&p, None).unwrap(), "l3\nl2\nl1");
    }

    #[test]
    fn bot_log_read_fail_audit_line_written() {
        // ERROR 审计走 write_error_audit（泛型 Runtime，mock_app 跑同一条生产代码路径）。
        // generic_log_dir 探针命中测试二进制旁目录（target/debug/deps），读完即删。
        // 删除/读全量共享 bot.log，必须持测试串行锁（与 audit 模块同名用例互斥）
        let _serial = crate::audit::BOT_LOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let app = tauri::test::mock_app();
        crate::audit::write_error_audit(app.handle(), "bot_log_read_fail", &[("err", "boom")]);
        let exe = std::env::current_exe().unwrap();
        let log = exe.parent().unwrap().join("bot.log");
        let content = std::fs::read_to_string(&log).unwrap();
        assert!(
            content.contains("bot_log_read_fail"),
            "ERROR 审计行应出现: {content}"
        );
        assert!(content.contains("boom"), "审计行应含 err 信息: {content}");
        let _ = std::fs::remove_file(&log);
    }

    // ────────── audit_log 写盘内核 ──────────

    /// 不依赖 Tauri AppHandle，验证分页格式化（位置行 + 截断续读提示 + offset 切片）。
    /// audit_log 写盘内核——成功带 [ts] 前缀落行；
    /// 失败 eprintln 带 path + 返回 false（对齐 audit::append_line 的做法），不静默不 panic
    #[test]
    fn append_bot_log_line_ok_writes_with_ts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot.log");
        assert!(super::audit::append_bot_log_line(&p, "hello"));
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(
            content.ends_with("] hello\n"),
            "应带 [ts] 前缀落行：{content}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn append_bot_log_line_readonly_dir_fails_visible() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let p = ro.join("bot.log");
        // 写失败：返回 false + eprintln 带 path（不能 if let Ok 全静默，丢日志零痕迹）
        assert!(!super::audit::append_bot_log_line(&p, "x"));
        assert!(!p.exists());
        // 恢复权限让 tempdir 清理不掉链子
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // ────────── base_url 安全判定 ──────────

    #[test]
    fn base_url_safety_classification() {
        // 安全：https
        assert!(base_url_is_safe("https://api.deepseek.com/v1"));
        // 安全：空（未配置，无可 warn）
        assert!(base_url_is_safe(""));
        assert!(base_url_is_safe("   "));
        // 安全：回环（本地推理服务合法场景）
        assert!(base_url_is_safe("http://localhost:11434/v1"));
        assert!(base_url_is_safe("http://127.0.0.1:8000/v1"));
        assert!(base_url_is_safe("http://[::1]:8080"));
        // 不安全：http 公网 / 内网
        assert!(!base_url_is_safe("http://api.example.com/v1"));
        assert!(!base_url_is_safe("http://192.168.1.10:8000/v1"));
        assert!(!base_url_is_safe("ftp://example.com"));
        // 不安全：前缀匹配时代的绕过变体（host 精确判定后全拒）
        assert!(!base_url_is_safe("http://localhost.evil.com"));
        assert!(!base_url_is_safe("http://127.0.0.1.evil.com"));
        assert!(!base_url_is_safe("http://[::1].evil.com"));
        // 安全：userinfo 不影响连接目标——host 解析后确为回环
        assert!(base_url_is_safe("http://attacker.com@127.0.0.1"));
    }
}

// 抑制 unused 警告：escape_for_log / truncate_for_log 跨模块被外部 bot_chat / bot_model_loop
// 仍通过 bot.rs 的 `pub use config::{...}` 路径使用，本模块测试不直接覆盖
// （truncate_for_log 是 escape_for_log 的别名）。
#[allow(dead_code)]
fn _unused_marker(_s: &str) -> String {
    audit::escape_for_log("", 0)
}
