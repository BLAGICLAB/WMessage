//! models.dev 同步：拉取 `https://models.dev/api.json` → 解析 → 增量写双表。
//!
//! 语义蓝本：`model-meta-service/main.py` —— 只覆盖 `source='models_dev'` 的行，
//! 用户自建（`user_custom`）永不动；网络/解析失败返回 Err，由调用方降级
//! （启动后台同步记审计不阻断；手动同步命令转成 ok:false 返回前端）。
//!
//! 离线种子：`resources/models-dev/snapshot.json` 随二进制内置（include_str!），
//! meta_provider 空表时落库兜底——首次安装即离线、或网络全程不可达时，设置页
//! 厂商网格与「从模型库添加」按钮开箱可用；在线同步成功后照常刷新覆盖。
//! 快照刷新走 `regen_snapshot_from_live`（#[ignore] 手动测试，需网络）。

use super::{first_char_upper, MetaModel, MetaProvider};

/// models.dev 全量元数据 API
pub const MODELS_DEV_API: &str = "https://models.dev/api.json";
/// 离线种子快照（编译期编进二进制；刷新见 regen_snapshot_from_live）。
/// 只含 parse_models_dev 消费的字段（provider 的 name/api，model 的
/// name + limit.context），格式与线上 api.json 同构。
pub(crate) const SEED_JSON: &str = include_str!("../../resources/models-dev/snapshot.json");
/// models.dev 官方 logo 约定
const LOGO_URL: &str = "https://models.dev/logos/{}.svg";
/// 同步来的服务商兜底色：按 key 散列取色，稳定且可区分
const PALETTE: [&str; 8] = [
    "#0ea5e9", "#8b5cf6", "#f59e0b", "#10b981", "#ef4444", "#ec4899", "#6366f1", "#14b8a6",
];
/// 手动同步会回源 models.dev（大 JSON，实测量级 ~6MB），给整请求级超时
///（reqwest 的 request timeout 从建连算到正文读完）。慢路由下 30s 可能连
/// 头带体都拉不完——半路被掐的现象与连不上同源不同层，放宽到 60s。
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 同步重试：初试 + 2 次重试，间隔指数退避（失败多为路由抖动，立即重试
/// 大概率同路；间隔拉开给路由切换留时间）
const SYNC_ATTEMPTS: usize = 3;
const SYNC_RETRY_DELAYS: [std::time::Duration; 2] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(3),
];

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

/// reqwest 错误的顶层文案只有 "error sending request"，DNS 失败/TLS 拒绝/
/// 连接被重置全藏在 source 链里；审计与前端错误文案都要全链才可诊断。
/// 深度上限 8：防御个别 Error 实现的环状 source。
fn err_chain(e: &impl std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    let mut depth = 0;
    while let (Some(cause), true) = (src, depth < 8) {
        s.push('：');
        s.push_str(&cause.to_string());
        src = cause.source();
        depth += 1;
    }
    s
}

/// 拉取 api.json 原文（解析与网络分离，解析可纯单测）。
/// 响应体 16MB 上限：api.json 正常量级几 MB，超限视为异常/劫持响应直接拒绝
///（防无限 body 把进程内存打爆）。
/// 失败按 attempts 重试（间隔取 delays，长度 = attempts-1）；全部失败后返回
/// 末次错误并注明重试次数——错误文案会原样展示到前端按钮旁。
async fn fetch_models_dev(
    url: &str,
    attempts: usize,
    delays: &[std::time::Duration],
) -> Result<String, String> {
    const MAX_BODY: usize = 16 * 1024 * 1024;
    let mut last_err = String::new();
    for attempt in 0..attempts {
        if attempt > 0 {
            tokio::time::sleep(delays[attempt - 1]).await;
        }
        match async {
            let resp = crate::bot::shared_llm_client()
                .get(url)
                .timeout(SYNC_TIMEOUT)
                .send()
                .await
                .map_err(|e| format!("models.dev 请求失败：{}", err_chain(&e)))?;
            let resp = resp
                .error_for_status()
                .map_err(|e| format!("models.dev HTTP 错误：{}", err_chain(&e)))?;
            let mut buf = Vec::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                let chunk =
                    chunk.map_err(|e| format!("models.dev 读取响应失败：{}", err_chain(&e)))?;
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
        .await
        {
            Ok(body) => return Ok(body),
            Err(e) => last_err = e,
        }
    }
    Err(format!(
        "{last_err}（重试 {} 次后仍失败）",
        attempts.saturating_sub(1)
    ))
}

/// 可测内核：URL/重试参数可注入（坏 URL → Err 且不落库；测试传 attempts=1
/// 避开生产退避等待）；正式路径走 `sync_models_dev`
pub(crate) async fn sync_models_dev_with_url<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
) -> Result<SyncStats, String> {
    sync_models_dev_inner(app, url, SYNC_ATTEMPTS, &SYNC_RETRY_DELAYS).await
}

pub(crate) async fn sync_models_dev_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
    attempts: usize,
    delays: &[std::time::Duration],
) -> Result<SyncStats, String> {
    let body = fetch_models_dev(url, attempts, delays).await?;
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

/// api.json 形状的 Value → 种子快照：只留 parse_models_dev 消费的字段
///（provider 的 name/api，model 的 name + limit.context）。与解析器严格对偶，
/// 解析器新消费字段时必须同步扩这里，否则种子缺字段、离线兜底降级。
/// 仅测试面使用（regen 快照用），生产路径不引用。
#[cfg(test)]
fn trim_models_dev_value(root: &serde_json::Value) -> Result<serde_json::Value, String> {
    let obj = root
        .as_object()
        .ok_or_else(|| "models.dev 顶层必须是对象".to_string())?;
    let mut out = serde_json::Map::new();
    for (pkey, p) in obj {
        let mut tp = serde_json::Map::new();
        if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
            tp.insert("name".into(), serde_json::json!(name));
        }
        if let Some(api) = p.get("api").and_then(|v| v.as_str()) {
            tp.insert("api".into(), serde_json::json!(api));
        }
        let mut ms = serde_json::Map::new();
        if let Some(models) = p.get("models").and_then(|v| v.as_object()) {
            for (mkey, m) in models {
                let mut tm = serde_json::Map::new();
                if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                    tm.insert("name".into(), serde_json::json!(name));
                }
                if let Some(ctx) = m
                    .get("limit")
                    .and_then(|l| l.get("context"))
                    .and_then(|v| v.as_i64())
                {
                    tm.insert("limit".into(), serde_json::json!({ "context": ctx }));
                }
                ms.insert(mkey.clone(), serde_json::Value::Object(tm));
            }
        }
        tp.insert("models".into(), serde_json::Value::Object(ms));
        out.insert(pkey.clone(), serde_json::Value::Object(tp));
    }
    Ok(serde_json::Value::Object(out))
}

/// 离线种子兜底：meta_provider 空表 → 落随包快照（幂等：非空表不动，在线
/// 同步负责刷新）。返回 Some = 本次落了种子。
/// 残留说明：种子是某时点的 models_dev 数据，在线同步只覆盖同名 key，
/// 上游已下架的 key 会留在库里直到下次全量重建——无害，接受。
pub async fn ensure_seed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<SyncStats>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = crate::db::open_db(&app)?;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM meta_provider", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if n > 0 {
            return Ok(None);
        }
        let (providers, models) = parse_models_dev(SEED_JSON)?;
        let stats = apply_sync(&mut conn, &providers, &models)?;
        Ok(Some(stats))
    })
    .await
    .map_err(|e| format!("meta 种子落库线程 join 失败：{e}"))?
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
    /// 库根本不会被打开写入）。attempts=1 避开生产退避等待。
    #[tokio::test]
    async fn sync_with_bad_url_returns_err() {
        let app = tauri::test::mock_app();
        let r = sync_models_dev_inner(app.handle(), "http://127.0.0.1:1/unreachable", 1, &[]).await;
        let err = r.expect_err("坏 URL 必须返回 Err");
        assert!(
            err.contains("models.dev 请求失败"),
            "错误应指明请求阶段：{err}"
        );
        // 根因链：顶层文案之外必须带上底层原因（连接被拒），否则 DNS/TLS/重置不可分
        assert!(
            err.contains("连接拒绝")
                || err.contains("Connection refused")
                || err.contains("refused"),
            "错误应带 source 链根因：{err}"
        );
    }

    /// 重试耗尽：错误注明重试次数（attempts=初试+N，文案写 N 次）
    #[tokio::test]
    async fn fetch_retry_reports_attempts() {
        let delays = [
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(10),
        ];
        let err = fetch_models_dev("http://127.0.0.1:1/unreachable", 3, &delays)
            .await
            .expect_err("坏 URL 重试耗尽必须 Err");
        assert!(
            err.contains("重试 2 次后仍失败"),
            "错误应注明重试次数：{err}"
        );
        assert!(err.contains("models.dev 请求失败"), "{err}");
    }

    /// err_chain：走 source 链拼根因（用自造错误链验证，深度环有上限保护）
    #[test]
    fn err_chain_walks_source() {
        #[derive(Debug)]
        struct Leaf;
        impl std::fmt::Display for Leaf {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "leaf-cause")
            }
        }
        impl std::error::Error for Leaf {}

        #[derive(Debug)]
        struct Wrap(std::io::Error);
        impl std::fmt::Display for Wrap {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "top-level")
            }
        }
        impl std::error::Error for Wrap {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let e = Wrap(std::io::Error::other(Leaf));
        let s = err_chain(&e);
        assert!(s.contains("top-level"), "{s}");
        assert!(s.contains("leaf-cause"), "{s}");
    }

    /// 种子快照可解析且量级正常（编译期常量，解析失败/量级塌方当场红）
    #[test]
    fn seed_snapshot_parses_with_sane_scale() {
        let (providers, models) = parse_models_dev(SEED_JSON).unwrap();
        assert!(
            providers.len() > 100,
            "种子厂商量级塌方：{}",
            providers.len()
        );
        assert!(models.len() > 1000, "种子模型量级塌方：{}", models.len());
    }

    /// trim 与解析器对偶：SAMPLE 经 trim 重建后仍可被 parse_models_dev 解析，
    /// 且字段一一对应（name/api/models{name,limit.context}）
    #[test]
    fn trim_roundtrip_matches_parser() {
        let root: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let trimmed = trim_models_dev_value(&root).unwrap();
        let s = serde_json::to_string(&trimmed).unwrap();
        let (providers, models) = parse_models_dev(&s).unwrap();
        let expected = parse_models_dev(SAMPLE).unwrap();
        assert_eq!(providers, expected.0, "trim 后 provider 集合必须等价");
        assert_eq!(models, expected.1, "trim 后模型集合必须等价");
        // 种子形状约束：model 只留 name 与 limit.context
        let tp = trimmed
            .get("openai")
            .and_then(|p| p.get("models"))
            .and_then(|m| m.get("gpt-4o"))
            .unwrap();
        assert!(tp.get("name").is_some());
        assert!(tp.get("limit").and_then(|l| l.get("context")).is_some());
        assert!(
            tp.get("limit").and_then(|l| l.get("output")).is_none(),
            "只留 context"
        );
    }

    /// 发版前刷新离线种子快照（需网络，手动跑）：
    /// `cargo test --lib meta:: -- --ignored`
    /// 拉线上 api.json 全量原文 → trim 成种子格式 → 写回 resources 文件。
    /// 快照是编译期 include_str!，刷新后必须重编译才生效。
    #[ignore]
    #[tokio::test]
    async fn regen_snapshot_from_live() {
        let body = fetch_models_dev(MODELS_DEV_API, 1, &[]).await.unwrap();
        let root: serde_json::Value = serde_json::from_str(&body).unwrap();
        let trimmed = trim_models_dev_value(&root).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/models-dev/snapshot.json");
        std::fs::write(&path, serde_json::to_string(&trimmed).unwrap() + "\n").unwrap();
        let t = trimmed.as_object().unwrap();
        println!(
            "snapshot refreshed: {} providers, {} models, {} bytes",
            t.len(),
            trimmed
                .as_object()
                .unwrap()
                .values()
                .map(|p| p
                    .get("models")
                    .and_then(|m| m.as_object())
                    .map_or(0, |m| m.len()))
                .sum::<usize>(),
            std::fs::metadata(&path).unwrap().len()
        );
    }
}
