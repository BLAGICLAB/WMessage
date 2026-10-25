//! 词元统计聚合面：按日 / 按模型 / 按模型×按日。
//!
//! 从 `db/trace.rs` 拆出。本模块与 `crate::db::trace` **同层**，
//! 两者的 `super::` 同指 `crate::db`——所以搬移不改任何路径语义。

use serde::Serialize;

use super::trace::{FileChangeRow, SpanRow, TraceRow};

/// 单次执行的完整痕迹（trace 摘要 + 全部 span + 全部文件变更；diff 已按 MAX_DIFF_LINES 钳）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceDetail {
    #[serde(flatten)]
    pub trace: TraceRow,
    pub spans: Vec<SpanRow>,
    pub file_changes: Vec<FileChangeRow>,
}

/// 词元统计单日聚合（设置页「词元统计」卡数据源， 实装消费）。
/// 升序返回、窗口内缺日补零（趋势图/热力图 x 轴必须连续）；
/// 执行次数只计已收尾行（running 僵尸不算一次执行，token 仍计入）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    /// 本地日期 YYYY-MM-DD
    pub day: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// 缓存读取 / 写入 token（Anthropic 计价档）。旧行与 OpenAI 协议为 0。
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub runs: i64,
    pub tool_calls: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）：窗口 = today 往前 days 天（含两端）。
pub fn usage_stats_daily_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageDay>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    // 本地午夜落在 DST 空洞/时区数据缺失时响亮报错——静默回退 0 会扫全表
    // 且把窗口外总量错画进图表
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .ok_or_else(|| {
            format!("无法把起始日期 {start_date} 解析为本地时间（时区数据缺失或 DST 空洞）")
        })?;
    let mut stmt = conn
        .prepare(
            "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day,
                    SUM(prompt_tokens), SUM(completion_tokens),
                    SUM(cache_read_tokens), SUM(cache_write_tokens),
                    SUM(CASE WHEN finished_at IS NOT NULL THEN 1 ELSE 0 END),
                    SUM(tool_calls)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY day",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                r.get::<_, Option<i64>>(6)?.unwrap_or(0),
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut by_day: std::collections::BTreeMap<String, [i64; 6]> =
        std::collections::BTreeMap::new();
    for r in rows {
        let (day, p, c, cr, cw, runs, tools) = r.map_err(|e| e.to_string())?;
        by_day.insert(day, [p, c, cr, cw, runs, tools]);
    }
    let mut out = Vec::with_capacity(days as usize);
    let mut d = start_date;
    while d <= today_local {
        let key = d.format("%Y-%m-%d").to_string();
        let v = by_day.remove(&key).unwrap_or([0; 6]);
        out.push(UsageDay {
            day: key,
            prompt_tokens: v[0],
            completion_tokens: v[1],
            cache_read_tokens: v[2],
            cache_write_tokens: v[3],
            runs: v[4],
            tool_calls: v[5],
        });
        d += chrono::Duration::days(1);
    }
    Ok(out)
}

/// 词元统计按模型聚合（「模型用量」榜数据源）：只计已收尾行（与 daily 口径一致），
/// 按四档 token 合计降序（缓存两档计入排序但单列展示，理由见 UsageDay 的字段注释）；
/// 旧行/未发请求 model 为 NULL，由前端显示「未知模型」。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageByModel {
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub runs: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）
pub fn usage_stats_by_model_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageByModel>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    // 本地午夜落在 DST 空洞/时区数据缺失时响亮报错——静默回退 0 会扫全表
    // 且把窗口外总量错画进图表
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .ok_or_else(|| {
            format!("无法把起始日期 {start_date} 解析为本地时间（时区数据缺失或 DST 空洞）")
        })?;
    let mut stmt = conn
        .prepare(
            "SELECT model, SUM(prompt_tokens), SUM(completion_tokens),
                    SUM(cache_read_tokens), SUM(cache_write_tokens),
                    SUM(CASE WHEN finished_at IS NOT NULL THEN 1 ELSE 0 END)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY model
             ORDER BY SUM(prompt_tokens + completion_tokens) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok(UsageByModel {
                model: r.get(0)?,
                prompt_tokens: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                completion_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                cache_read_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                cache_write_tokens: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                runs: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 词元统计按模型×按日聚合（「每日 Token 趋势图」按模型分线数据源）：
/// 只返回有数据的 (day, model) 组合（缺日由前端以 0 补齐），day 升序。
/// 只计已收尾行口径与 daily/by_model 一致；model NULL 保留（前端显示「未知模型」）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageDayModel {
    /// 本地日期 YYYY-MM-DD
    pub day: String,
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）
pub fn usage_stats_daily_by_model_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageDayModel>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    // 本地午夜落在 DST 空洞/时区数据缺失时响亮报错——静默回退 0 会扫全表
    // 且把窗口外总量错画进图表
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .ok_or_else(|| {
            format!("无法把起始日期 {start_date} 解析为本地时间（时区数据缺失或 DST 空洞）")
        })?;
    let mut stmt = conn
        .prepare(
            "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day, model,
                    SUM(prompt_tokens), SUM(completion_tokens),
                    SUM(cache_read_tokens), SUM(cache_write_tokens)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY day, model ORDER BY day ASC, model ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok(UsageDayModel {
                day: r.get(0)?,
                model: r.get(1)?,
                prompt_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                completion_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                cache_read_tokens: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                cache_write_tokens: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
