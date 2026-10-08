//! models.dev 同步：拉取 `https://models.dev/api.json` → 解析 → 增量写双表。
//!
//! 语义蓝本：`model-meta-service/main.py` —— 只覆盖 `source='models_dev'` 的行，
//! 用户自建（`user_custom`）永不动；网络/解析失败返回 Err，由调用方降级
//! （启动后台同步记审计不阻断；手动同步命令转成 ok:false 返回前端）。

use super::{first_char_upper, MetaModel, MetaProvider};

/// models.dev 全量元数据 API
pub const MODELS_DEV_API: &str = "https://models.dev/api.json";
/// models.dev 官方 logo 约定
const LOGO_URL: &str = "https://models.dev/logos/{}.svg";
/// 同步来的服务商兜底色：按 key 散列取色，稳定且可区分
const PALETTE: [&str; 8] = [
    "#0ea5e9", "#8b5cf6", "#f59e0b", "#10b981", "#ef4444", "#ec4899", "#6366f1", "#14b8a6",
];
/// 手动同步会回源 models.dev（大 JSON），给整请求级超时
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub providers: usize,
    pub models: usize,
}

/// FNV-1a 32bit：跨版本稳定的散列（std DefaultHasher 不承诺跨版本稳定），
/// 用于 provider_key → 兜底色稳定取色
fn stable_hash(s: &str) -> usize {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h as usize
}

/// 解析 models.dev api.json（纯函数，无网络/无库，可单测）。
/// 容错：provider 缺 name/api/models 字段、model 缺 name/limit 字段均可解析。
pub fn parse_models_dev(json: &str) -> Result<(Vec<MetaProvider>, Vec<MetaModel>), String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("models.dev JSON 解析失败：{e}"))?;
    let obj = root
        .as_object()
        .ok_or_else(|| "models.dev 顶层必须是对象".to_string())?;
    let mut providers = Vec::with_capacity(obj.len());
    let mut models = Vec::new();
    for (pkey, p) in obj {
        let name = p
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(pkey)
            .to_string();
        providers.push(MetaProvider {
            provider_key: pkey.clone(),
            provider_name: name.clone(),
            logo_url: Some(LOGO_URL.replace("{}", pkey)),
            fallback_color: PALETTE[stable_hash(pkey) % PALETTE.len()].to_string(),
            fallback_char: first_char_upper(&name),
            default_base_url: p.get("api").and_then(|v| v.as_str()).map(str::to_string),
            timeout: None,
            source: "models_dev".into(),
        });
        if let Some(ms) = p.get("models").and_then(|v| v.as_object()) {
            for (mkey, m) in ms {
                models.push(MetaModel {
                    model_key: format!("{pkey}/{mkey}"),
                    provider_key: pkey.clone(),
                    display_name: m
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(mkey)
                        .to_string(),
                    context_length: m
                        .get("limit")
                        .and_then(|l| l.get("context"))
                        .and_then(|v| v.as_i64()),
                    temperature: None,
                    top_p: None,
                    max_tokens: None,
                    default_system_prompt: None,
                    source: "models_dev".into(),
                });
            }
        }
    }
    Ok((providers, models))
}

/// 解析结果落库（单事务 + DB_WRITE_LOCK 与全库写路径互斥）；
/// 只覆盖 source='models_dev' 行（守卫在 upsert_*_sync 的 SQL 里）
pub fn apply_sync(
    conn: &mut rusqlite::Connection,
    providers: &[MetaProvider],
    models: &[MetaModel],
) -> Result<SyncStats, String> {
    let _g = crate::db::lock_db_write();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for p in providers {
        super::upsert_provider_sync(&tx, p).map_err(|e| e.to_string())?;
    }
    for m in models {
        super::upsert_model_sync(&tx, m).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(SyncStats {
        providers: providers.len(),
        models: models.len(),
    })
}

/// 拉取 api.json 原文（解析与网络分离，解析可纯单测）。
/// 响应体 16MB 上限：api.json 正常量级几 MB，超限视为异常/劫持响应直接拒绝
///（防无限 body 把进程内存打爆）。
async fn fetch_models_dev(url: &str) -> Result<String, String> {
    const MAX_BODY: usize = 16 * 1024 * 1024;
    let resp = crate::bot_model_loop::shared_llm_client()
        .get(url)
        .timeout(SYNC_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("models.dev 请求失败：{e}"))?;
    let resp = resp
        .error_for_status()
        .map_err(|e| format!("models.dev HTTP 错误：{e}"))?;
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|e| format!("models.dev 读取响应失败：{e}"))?;
        if buf.len() + chunk.len() > MAX_BODY {
            return Err(format!(
                "models.dev 响应超过 {}MB 上限，已中止",
                MAX_BODY / 1024 / 1024
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|e| format!("models.dev 响应非 UTF-8：{e}"))
}

/// 可测内核：URL 可注入（坏 URL → Err 且不落库）；正式路径走 `sync_models_dev`
pub(crate) async fn sync_models_dev_with_url<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
) -> Result<SyncStats, String> {
    let body = fetch_models_dev(url).await?;
    let (providers, models) = parse_models_dev(&body)?;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = crate::db::open_db(&app)?;
        apply_sync(&mut conn, &providers, &models)
    })
    .await
    .map_err(|e| format!("meta 同步落库线程 join 失败：{e}"))?
}

/// 正式同步入口：models.dev 正式 URL
pub async fn sync_models_dev<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<SyncStats, String> {
    sync_models_dev_with_url(app, MODELS_DEV_API).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "openai": {
            "name": "OpenAI",
            "api": "https://api.openai.com/v1",
            "models": {
                "gpt-4o": { "name": "GPT-4o", "limit": { "context": 128000 } },
                "gpt-4o-mini": { "limit": { "context": 128000 } }
            }
        },
        "bare": {}
    }"#;

    /// 样本解析：字段映射 + logo 规则 + 兜底字符/取色
    #[test]
    fn parse_sample_maps_fields() {
        let (providers, models) = parse_models_dev(SAMPLE).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(models.len(), 2);

        let openai = providers
            .iter()
            .find(|p| p.provider_key == "openai")
            .unwrap();
        assert_eq!(openai.provider_name, "OpenAI");
        assert_eq!(
            openai.logo_url.as_deref(),
            Some("https://models.dev/logos/openai.svg")
        );
        assert_eq!(
            openai.default_base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(openai.fallback_char, "O");
        assert!(
            PALETTE.contains(&openai.fallback_color.as_str()),
            "兜底色必须取自调色板：{}",
            openai.fallback_color
        );
        assert_eq!(openai.source, "models_dev");

        let gpt4o = models
            .iter()
            .find(|m| m.model_key == "openai/gpt-4o")
            .unwrap();
        assert_eq!(gpt4o.display_name, "GPT-4o");
        assert_eq!(gpt4o.context_length, Some(128000));
        assert_eq!(gpt4o.provider_key, "openai");
        assert_eq!(gpt4o.source, "models_dev");
    }

    /// 容错：缺 name/api/models（provider 级）与缺 name/limit（model 级）
    #[test]
    fn parse_tolerates_missing_fields() {
        let (providers, models) = parse_models_dev(SAMPLE).unwrap();
        let bare = providers.iter().find(|p| p.provider_key == "bare").unwrap();
        assert_eq!(bare.provider_name, "bare", "缺 name 回填 key");
        assert_eq!(bare.default_base_url, None, "缺 api → None");
        assert_eq!(bare.fallback_char, "B");

        let mini = models
            .iter()
            .find(|m| m.model_key == "openai/gpt-4o-mini")
            .unwrap();
        assert_eq!(mini.display_name, "gpt-4o-mini", "缺 name 回填 model key");
        assert_eq!(mini.context_length, Some(128000));
        // bare 没有 models 字段 → 不产生任何模型行
        assert!(!models.iter().any(|m| m.provider_key == "bare"));
    }

    /// 坏 JSON / 顶层非对象 → Err（不同步、不落库由调用方保证）
    #[test]
    fn parse_rejects_bad_json() {
        assert!(parse_models_dev("not json").is_err());
        assert!(parse_models_dev("[1,2]").is_err(), "顶层必须是对象");
    }

    /// 散列取色稳定：同 key 多次取色一致，且在调色板内
    #[test]
    fn stable_hash_picks_palette_color_deterministically() {
        let c1 = PALETTE[stable_hash("openai") % PALETTE.len()];
        let c2 = PALETTE[stable_hash("openai") % PALETTE.len()];
        assert_eq!(c1, c2);
        assert!(PALETTE.contains(&c1));
    }

    /// 落库 + 复同步：models_dev 行被刷新，统计数正确
    #[test]
    fn apply_sync_writes_and_reupdates() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::ensure_meta_tables(&conn).unwrap();
        let (providers, models) = parse_models_dev(SAMPLE).unwrap();

        let stats = apply_sync(&mut conn, &providers, &models).unwrap();
        assert_eq!(
            stats,
            SyncStats {
                providers: 2,
                models: 2
            }
        );
        assert_eq!(super::super::list_providers(&conn).unwrap().len(), 2);

        // 复同步同名 key（改名）→ models_dev 行被更新
        let (mut providers2, _) = parse_models_dev(SAMPLE).unwrap();
        providers2
            .iter_mut()
            .find(|p| p.provider_key == "openai")
            .unwrap()
            .provider_name = "OpenAI v2".into();
        apply_sync(&mut conn, &providers2, &[]).unwrap();
        let p = super::super::query_provider(&conn, "openai")
            .unwrap()
            .unwrap();
        assert_eq!(p.provider_name, "OpenAI v2");
    }

    /// 网络失败路径：注入坏 URL → Err（连接被拒，快速失败；失败在落库之前，
    /// 库根本不会被打开写入）
    #[tokio::test]
    async fn sync_with_bad_url_returns_err() {
        let app = tauri::test::mock_app();
        let r = sync_models_dev_with_url(app.handle(), "http://127.0.0.1:1/unreachable").await;
        let err = r.expect_err("坏 URL 必须返回 Err");
        assert!(
            err.contains("models.dev 请求失败"),
            "错误应指明请求阶段：{err}"
        );
    }
}
