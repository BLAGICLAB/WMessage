//! 模型元数据模块（原独立 Python 服务 `model-meta-service` 的内嵌版，语义 1:1）
//!
//! - 双表 `meta_provider` / `meta_model` 存进应用自带 wmessage.db
//!   （DDL 在 `db::migrations::ensure_meta_tables`，open_db 时幂等建表）
//! - models.dev 同步见 `sync` 子模块：`source='models_dev'` 的行可被同步覆盖，
//!   `source='user_custom'` 的用户自建行永不覆盖（ON CONFLICT ... WHERE 守卫）
//! - 对外是 8 个 `meta_*` Tauri 命令（注册见 lib.rs invoke_handler），
//!   字段 snake_case 与原 HTTP 接口一致，前端类型零改动

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

pub mod sync;

/// 未命中时的兜底值（与原 Python 服务一致）
pub const FALLBACK_COLOR: &str = "#666666";
pub const FALLBACK_CHAR: &str = "?";

/// meta_provider 行（同步缓存 + 用户自建服务商）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetaProvider {
    pub provider_key: String,
    pub provider_name: String,
    pub logo_url: Option<String>,
    pub fallback_color: String,
    pub fallback_char: String,
    pub default_base_url: Option<String>,
    pub timeout: Option<i64>,
    pub source: String,
}

/// meta_model 行（by-provider 查询返回的就是这个形状）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetaModel {
    /// 形如 "openai/gpt-4o"
    pub model_key: String,
    pub provider_key: String,
    pub display_name: String,
    pub context_length: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<i64>,
    pub default_system_prompt: Option<String>,
    pub source: String,
}

/// meta_model JOIN meta_provider 的结果（meta_list_models / meta_get_model）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetaModelJoined {
    pub model_key: String,
    pub provider_key: String,
    pub display_name: String,
    pub context_length: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<i64>,
    pub default_system_prompt: Option<String>,
    pub source: String,
    pub provider_name: Option<String>,
    pub logo_url: Option<String>,
    pub fallback_color: Option<String>,
    pub fallback_char: Option<String>,
    pub default_base_url: Option<String>,
}

/// meta_sync_models_dev 命令的返回结构：ok:false 也走 Ok（不抛 Err），
/// 前端「更新模型库」按钮直接按 ok/error 字段展示结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetaSyncResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 取名称首字符并大写（非字母/中文原样返回）；空串给兜底 "?"
pub(crate) fn first_char_upper(name: &str) -> String {
    match name.chars().next() {
        Some(c) => c.to_uppercase().collect(),
        None => FALLBACK_CHAR.into(),
    }
}

/// 服务商未命中兜底（key 原样回填，色/字符走兜底值）
pub fn provider_fallback(key: &str) -> MetaProvider {
    MetaProvider {
        provider_key: key.into(),
        provider_name: key.into(),
        logo_url: None,
        fallback_color: FALLBACK_COLOR.into(),
        fallback_char: FALLBACK_CHAR.into(),
        default_base_url: None,
        timeout: None,
        source: "fallback".into(),
    }
}

/// 模型未命中兜底（与原 Python 服务 model_get 兜底字段一一对应）
pub fn model_fallback(key: &str) -> MetaModelJoined {
    MetaModelJoined {
        model_key: key.into(),
        provider_key: FALLBACK_CHAR.into(),
        display_name: FALLBACK_CHAR.into(),
        context_length: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        default_system_prompt: None,
        source: "fallback".into(),
        provider_name: Some(FALLBACK_CHAR.into()),
        logo_url: None,
        fallback_color: Some(FALLBACK_COLOR.into()),
        fallback_char: Some(FALLBACK_CHAR.into()),
        default_base_url: None,
    }
}

const PROVIDER_COLS: &str =
    "provider_key, provider_name, logo_url, fallback_color, fallback_char, default_base_url, timeout, source";
const MODEL_COLS: &str =
    "model_key, provider_key, display_name, context_length, temperature, top_p, max_tokens, default_system_prompt, source";

fn provider_from_row(r: &rusqlite::Row) -> rusqlite::Result<MetaProvider> {
    Ok(MetaProvider {
        provider_key: r.get(0)?,
        provider_name: r.get(1)?,
        logo_url: r.get(2)?,
        fallback_color: r.get(3)?,
        fallback_char: r.get(4)?,
        default_base_url: r.get(5)?,
        timeout: r.get(6)?,
        source: r.get(7)?,
    })
}

fn model_from_row(r: &rusqlite::Row) -> rusqlite::Result<MetaModel> {
    Ok(MetaModel {
        model_key: r.get(0)?,
        provider_key: r.get(1)?,
        display_name: r.get(2)?,
        context_length: r.get(3)?,
        temperature: r.get(4)?,
        top_p: r.get(5)?,
        max_tokens: r.get(6)?,
        default_system_prompt: r.get(7)?,
        source: r.get(8)?,
    })
}

fn joined_from_row(r: &rusqlite::Row) -> rusqlite::Result<MetaModelJoined> {
    Ok(MetaModelJoined {
        model_key: r.get(0)?,
        provider_key: r.get(1)?,
        display_name: r.get(2)?,
        context_length: r.get(3)?,
        temperature: r.get(4)?,
        top_p: r.get(5)?,
        max_tokens: r.get(6)?,
        default_system_prompt: r.get(7)?,
        source: r.get(8)?,
        provider_name: r.get(9)?,
        logo_url: r.get(10)?,
        fallback_color: r.get(11)?,
        fallback_char: r.get(12)?,
        default_base_url: r.get(13)?,
    })
}

pub fn list_providers(conn: &rusqlite::Connection) -> Result<Vec<MetaProvider>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PROVIDER_COLS} FROM meta_provider ORDER BY provider_name"
    ))?;
    let rows = stmt.query_map([], provider_from_row)?.collect();
    rows
}

pub fn query_provider(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Option<MetaProvider>, rusqlite::Error> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        &format!("SELECT {PROVIDER_COLS} FROM meta_provider WHERE provider_key = ?1"),
        [key],
        provider_from_row,
    )
    .optional()
}

/// 未命中返回兜底 provider（#666666 / "?" / source=fallback），与原服务一致
pub fn get_provider_or_fallback(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<MetaProvider, rusqlite::Error> {
    Ok(query_provider(conn, key)?.unwrap_or_else(|| provider_fallback(key)))
}

pub fn models_by_provider(
    conn: &rusqlite::Connection,
    provider_key: &str,
) -> Result<Vec<MetaModel>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MODEL_COLS} FROM meta_model WHERE provider_key = ?1 ORDER BY display_name"
    ))?;
    let rows = stmt.query_map([provider_key], model_from_row)?.collect();
    rows
}

const JOINED_SELECT: &str =
    "SELECT m.model_key, m.provider_key, m.display_name, m.context_length, \
     m.temperature, m.top_p, m.max_tokens, m.default_system_prompt, m.source, \
     p.provider_name, p.logo_url, p.fallback_color, p.fallback_char, p.default_base_url \
     FROM meta_model m JOIN meta_provider p ON p.provider_key = m.provider_key";

pub fn list_models_joined(
    conn: &rusqlite::Connection,
) -> Result<Vec<MetaModelJoined>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "{JOINED_SELECT} ORDER BY p.provider_name, m.display_name"
    ))?;
    let rows = stmt.query_map([], joined_from_row)?.collect();
    rows
}

pub fn query_model_joined(
    conn: &rusqlite::Connection,
    model_key: &str,
) -> Result<Option<MetaModelJoined>, rusqlite::Error> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        &format!("{JOINED_SELECT} WHERE m.model_key = ?1"),
        [model_key],
        joined_from_row,
    )
    .optional()
}

/// 未命中返回兜底模型（display_name="?" / source=fallback），与原服务一致
pub fn get_model_or_fallback(
    conn: &rusqlite::Connection,
    model_key: &str,
) -> Result<MetaModelJoined, rusqlite::Error> {
    Ok(query_model_joined(conn, model_key)?.unwrap_or_else(|| model_fallback(model_key)))
}

/// 同步写入（source='models_dev'）：ON CONFLICT 只更新 models_dev 行，user_custom 不动。
/// 与原 Python 服务一致：UPDATE 只刷 name/logo/base_url（兜底色/字符入库后保持稳定）
pub fn upsert_provider_sync(
    conn: &rusqlite::Connection,
    p: &MetaProvider,
) -> Result<(), rusqlite::Error> {
    debug_assert!(crate::db::holding_db_write(), "meta 写操作必须持有 db 写锁");
    conn.execute(
        "INSERT INTO meta_provider (provider_key, provider_name, logo_url, fallback_color, fallback_char, default_base_url, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'models_dev')
         ON CONFLICT(provider_key) DO UPDATE SET
             provider_name = excluded.provider_name,
             logo_url = excluded.logo_url,
             default_base_url = excluded.default_base_url
         WHERE meta_provider.source = 'models_dev'",
        rusqlite::params![
            p.provider_key,
            p.provider_name,
            p.logo_url,
            p.fallback_color,
            p.fallback_char,
            p.default_base_url
        ],
    )?;
    Ok(())
}

/// 同步写入（source='models_dev'）：推理参数列不刷（那是用户自定义面），
/// 只刷 provider_key/display_name/context_length
pub fn upsert_model_sync(
    conn: &rusqlite::Connection,
    m: &MetaModel,
) -> Result<(), rusqlite::Error> {
    debug_assert!(crate::db::holding_db_write(), "meta 写操作必须持有 db 写锁");
    conn.execute(
        "INSERT INTO meta_model (model_key, provider_key, display_name, context_length, source)
         VALUES (?1, ?2, ?3, ?4, 'models_dev')
         ON CONFLICT(model_key) DO UPDATE SET
             provider_key = excluded.provider_key,
             display_name = excluded.display_name,
             context_length = excluded.context_length
         WHERE meta_model.source = 'models_dev'",
        rusqlite::params![
            m.model_key,
            m.provider_key,
            m.display_name,
            m.context_length
        ],
    )?;
    Ok(())
}

/// 用户自建/更新服务商（source='user_custom'）：全字段覆盖；
/// fallback_char 为空时取 provider_name 首字符大写
pub fn upsert_provider_user(
    conn: &rusqlite::Connection,
    p: &MetaProvider,
) -> Result<(), rusqlite::Error> {
    debug_assert!(crate::db::holding_db_write(), "meta 写操作必须持有 db 写锁");
    let fallback_char = if p.fallback_char.is_empty() {
        first_char_upper(&p.provider_name)
    } else {
        p.fallback_char.clone()
    };
    conn.execute(
        "INSERT INTO meta_provider (provider_key, provider_name, logo_url, fallback_color, fallback_char, default_base_url, timeout, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'user_custom')
         ON CONFLICT(provider_key) DO UPDATE SET
             provider_name = excluded.provider_name,
             logo_url = excluded.logo_url,
             fallback_color = excluded.fallback_color,
             fallback_char = excluded.fallback_char,
             default_base_url = excluded.default_base_url,
             timeout = excluded.timeout,
             source = 'user_custom'",
        rusqlite::params![
            p.provider_key,
            p.provider_name,
            p.logo_url,
            p.fallback_color,
            fallback_char,
            p.default_base_url,
            p.timeout
        ],
    )?;
    Ok(())
}

/// 用户自建/更新模型（source='user_custom'）：provider 须先存在，否则 Err
pub fn upsert_model_user(conn: &rusqlite::Connection, m: &MetaModel) -> Result<(), String> {
    debug_assert!(crate::db::holding_db_write(), "meta 写操作必须持有 db 写锁");
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta_provider WHERE provider_key = ?1",
            [&m.provider_key],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Err(format!(
            "provider 不存在：{}，请先创建服务商",
            m.provider_key
        ));
    }
    conn.execute(
        "INSERT INTO meta_model (model_key, provider_key, display_name, context_length, temperature, top_p, max_tokens, default_system_prompt, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'user_custom')
         ON CONFLICT(model_key) DO UPDATE SET
             provider_key = excluded.provider_key,
             display_name = excluded.display_name,
             context_length = excluded.context_length,
             temperature = excluded.temperature,
             top_p = excluded.top_p,
             max_tokens = excluded.max_tokens,
             default_system_prompt = excluded.default_system_prompt,
             source = 'user_custom'",
        rusqlite::params![
            m.model_key,
            m.provider_key,
            m.display_name,
            m.context_length,
            m.temperature,
            m.top_p,
            m.max_tokens,
            m.default_system_prompt
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// join 失败（spawn_blocking 线程 panic/取消）的统一映射
fn join_err(e: impl std::fmt::Display) -> CommandError {
    CommandError::Internal(format!("meta 数据库线程 join 失败：{e}"))
}

#[tauri::command]
pub async fn meta_list_providers(app: AppHandle) -> CommandResult<Vec<MetaProvider>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        list_providers(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_get_provider(
    app: AppHandle,
    provider_key: String,
) -> CommandResult<MetaProvider> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        get_provider_or_fallback(&conn, &provider_key).map_err(CommandError::from)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_list_models(app: AppHandle) -> CommandResult<Vec<MetaModelJoined>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        list_models_joined(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_models_by_provider(
    app: AppHandle,
    provider_key: String,
) -> CommandResult<Vec<MetaModel>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        models_by_provider(&conn, &provider_key).map_err(CommandError::from)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_get_model(app: AppHandle, model_key: String) -> CommandResult<MetaModelJoined> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        get_model_or_fallback(&conn, &model_key).map_err(CommandError::from)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_upsert_provider(
    app: AppHandle,
    provider_key: String,
    provider_name: String,
    logo_url: Option<String>,
    fallback_color: Option<String>,
    fallback_char: Option<String>,
    default_base_url: Option<String>,
    timeout: Option<i64>,
) -> CommandResult<MetaProvider> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        let _g = crate::db::lock_db_write();
        let p = MetaProvider {
            provider_key: provider_key.clone(),
            provider_name,
            logo_url,
            fallback_color: fallback_color.unwrap_or_else(|| FALLBACK_COLOR.into()),
            fallback_char: fallback_char.unwrap_or_default(),
            default_base_url,
            timeout,
            source: "user_custom".into(),
        };
        upsert_provider_user(&conn, &p).map_err(CommandError::from)?;
        query_provider(&conn, &provider_key)
            .map_err(CommandError::from)?
            .ok_or_else(|| CommandError::Internal("meta_upsert_provider 写后读不到行".into()))
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn meta_upsert_model(
    app: AppHandle,
    model_key: String,
    provider_key: String,
    display_name: String,
    context_length: Option<i64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<i64>,
    default_system_prompt: Option<String>,
) -> CommandResult<MetaModelJoined> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        let _g = crate::db::lock_db_write();
        let m = MetaModel {
            model_key: model_key.clone(),
            provider_key,
            display_name,
            context_length,
            temperature,
            top_p,
            max_tokens,
            default_system_prompt,
            source: "user_custom".into(),
        };
        upsert_model_user(&conn, &m).map_err(|reason| CommandError::DomainRule {
            domain: "meta".into(),
            reason,
        })?;
        query_model_joined(&conn, &model_key)
            .map_err(CommandError::from)?
            .ok_or_else(|| CommandError::Internal("meta_upsert_model 写后读不到行".into()))
    })
    .await
    .map_err(join_err)?
}

/// 手动触发 models.dev 同步（前端「更新模型库」按钮）：
/// 永远返回 Ok(MetaSyncResult)，失败时 ok:false + error，供按钮直接展示
#[tauri::command]
pub async fn meta_sync_models_dev(app: AppHandle) -> MetaSyncResult {
    match sync::sync_models_dev(&app).await {
        Ok(s) => MetaSyncResult {
            ok: true,
            providers: Some(s.providers),
            models: Some(s.models),
            error: None,
        },
        Err(e) => {
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Warn,
                "meta_sync_models_dev",
                &[("err", e.clone())],
            );
            MetaSyncResult {
                ok: false,
                providers: None,
                models: None,
                error: Some(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存库 + 生产 DDL（ensure_meta_tables 即 open_db 建表路径的单源）
    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::ensure_meta_tables(&conn).unwrap();
        conn
    }

    fn dev_provider(key: &str, name: &str) -> MetaProvider {
        MetaProvider {
            provider_key: key.into(),
            provider_name: name.into(),
            logo_url: Some(format!("https://models.dev/logos/{key}.svg")),
            fallback_color: "#0ea5e9".into(),
            fallback_char: first_char_upper(name),
            default_base_url: Some("https://api.example.com/v1".into()),
            timeout: None,
            source: "models_dev".into(),
        }
    }

    fn dev_model(model_key: &str, provider_key: &str, display: &str) -> MetaModel {
        MetaModel {
            model_key: model_key.into(),
            provider_key: provider_key.into(),
            display_name: display.into(),
            context_length: Some(128000),
            temperature: None,
            top_p: None,
            max_tokens: None,
            default_system_prompt: None,
            source: "models_dev".into(),
        }
    }

    /// models_dev 行可被再次同步更新（name/logo/base_url 刷新）
    #[test]
    fn sync_upsert_updates_models_dev_rows() {
        let _g = crate::db::lock_db_write(); // 写锁契约（debug_assert）
        let conn = mem_conn();
        upsert_provider_sync(&conn, &dev_provider("openai", "OpenAI")).unwrap();
        upsert_provider_sync(&conn, &dev_provider("openai", "OpenAI-New")).unwrap();
        let p = query_provider(&conn, "openai").unwrap().unwrap();
        assert_eq!(p.provider_name, "OpenAI-New");
        assert_eq!(p.source, "models_dev");
    }

    /// user_custom 行不被同步覆盖（ON CONFLICT ... WHERE source='models_dev' 守卫）：
    /// provider 与 model 两侧同语义
    #[test]
    fn sync_upsert_never_overwrites_user_custom() {
        let _g = crate::db::lock_db_write(); // 写锁契约（debug_assert）
        let conn = mem_conn();
        upsert_provider_sync(&conn, &dev_provider("openai", "OpenAI")).unwrap();
        upsert_model_sync(&conn, &dev_model("openai/gpt-4o", "openai", "GPT-4o")).unwrap();

        // 用户接管这两行（source 置为 user_custom）
        let mut my_p = dev_provider("openai", "我的 OpenAI");
        my_p.fallback_color = "#123456".into();
        upsert_provider_user(&conn, &my_p).unwrap();
        let mut my_m = dev_model("openai/gpt-4o", "openai", "我的 GPT");
        my_m.temperature = Some(0.7);
        upsert_model_user(&conn, &my_m).unwrap();

        // 再次同步同名 key → 必须完全不动 user_custom 行
        upsert_provider_sync(&conn, &dev_provider("openai", "OpenAI-Upstream")).unwrap();
        let mut upstream_m = dev_model("openai/gpt-4o", "openai", "GPT-4o Upstream");
        upstream_m.context_length = Some(256000);
        upsert_model_sync(&conn, &upstream_m).unwrap();

        let p = query_provider(&conn, "openai").unwrap().unwrap();
        assert_eq!(
            p.provider_name, "我的 OpenAI",
            "user_custom provider 不得被覆盖"
        );
        assert_eq!(p.source, "user_custom");
        assert_eq!(p.fallback_color, "#123456");
        let m = models_by_provider(&conn, "openai").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].display_name, "我的 GPT",
            "user_custom model 不得被覆盖"
        );
        assert_eq!(
            m[0].context_length,
            Some(128000),
            "context_length 不得被同步刷掉"
        );
        assert_eq!(m[0].temperature, Some(0.7));
        assert_eq!(m[0].source, "user_custom");
    }

    /// 用户自建模型：provider 不存在 → Err（fail-closed，对应原服务 ok:false 语义）
    #[test]
    fn upsert_model_user_rejects_missing_provider() {
        let _g = crate::db::lock_db_write(); // 写锁契约（debug_assert）
        let conn = mem_conn();
        let err = upsert_model_user(&conn, &dev_model("ghost/m", "ghost", "M")).unwrap_err();
        assert!(err.contains("provider 不存在"), "err={err}");
        assert!(models_by_provider(&conn, "ghost").unwrap().is_empty());
    }

    /// 未命中查询走兜底：#666666 / "?" / source=fallback（provider 与 model 两侧）
    #[test]
    fn missing_rows_return_fallback() {
        let conn = mem_conn();
        let p = get_provider_or_fallback(&conn, "nope").unwrap();
        assert_eq!(p.provider_key, "nope");
        assert_eq!(p.provider_name, "nope");
        assert_eq!(p.fallback_color, FALLBACK_COLOR);
        assert_eq!(p.fallback_char, FALLBACK_CHAR);
        assert_eq!(p.source, "fallback");

        let m = get_model_or_fallback(&conn, "nope/x").unwrap();
        assert_eq!(m.model_key, "nope/x");
        assert_eq!(m.display_name, FALLBACK_CHAR);
        assert_eq!(m.fallback_color.as_deref(), Some(FALLBACK_COLOR));
        assert_eq!(m.fallback_char.as_deref(), Some(FALLBACK_CHAR));
        assert_eq!(m.source, "fallback");
    }

    /// JOIN 查询：模型附带 provider 的 logo/兜底字段；upsert 写后读回一致
    #[test]
    fn joined_query_carries_provider_fields() {
        let _g = crate::db::lock_db_write(); // 写锁契约（debug_assert）
        let conn = mem_conn();
        upsert_provider_user(&conn, &dev_provider("acme", "Acme")).unwrap();
        upsert_model_user(&conn, &dev_model("acme/m1", "acme", "M1")).unwrap();
        let all = list_models_joined(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].provider_name.as_deref(), Some("Acme"));
        assert_eq!(
            all[0].logo_url.as_deref(),
            Some("https://models.dev/logos/acme.svg")
        );
        assert_eq!(all[0].source, "user_custom");
        let one = query_model_joined(&conn, "acme/m1").unwrap().unwrap();
        assert_eq!(one.display_name, "M1");
    }

    /// fallback_char 为空时自动取 provider_name 首字符大写
    #[test]
    fn user_provider_fallback_char_defaults_to_name_initial() {
        let _g = crate::db::lock_db_write(); // 写锁契约（debug_assert）
        let conn = mem_conn();
        let mut p = dev_provider("zhipu", "智谱");
        p.fallback_char = String::new(); // 用户没填
        upsert_provider_user(&conn, &p).unwrap();
        let got = query_provider(&conn, "zhipu").unwrap().unwrap();
        assert_eq!(got.fallback_char, "智", "中文名取首字符原样");

        let mut p2 = dev_provider("minimax", "minimax");
        p2.fallback_char = String::new();
        upsert_provider_user(&conn, &p2).unwrap();
        let got2 = query_provider(&conn, "minimax").unwrap().unwrap();
        assert_eq!(got2.fallback_char, "M", "英文名取首字符大写");
    }
}
