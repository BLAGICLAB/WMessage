//! 治理开关 `evolution.applyPolicy`（ 治理归一）。
//!
//! 二档：
//! - `auto`（默认）：consolidate 后达门槛提案自动落库——引入前行为，零变化；
//! - `confirm`：不再自动落库，全留候选池等决策板人工拨开关生效。
//!
//! 读取走轻量路径（同 shadow::ShadowConfig / activation loader 先例）：
//! bot-config.json 缺文件 / 缺块 / 缺字段 / 非法值 = auto = 现状零变化，
//! 门禁绝不弄挂 consolidate 主链路；只有「文件存在但 JSON 坏掉」留 stderr 一行
//! （手改配置改坏了要可诊断，同 read_memory_control_at 口径）。
//! 写入走 RMW 全程持锁 + atomic_write：
//! 文件缺失从空对象起（设置页首次点档不要求先存过配置）；解析失败拒绝写
//!（防整库覆盖——坏文件上 RMW = 丢用户配置）。

use std::path::Path;
use tauri::AppHandle;

/// 应用策略二档——类型定义已迁策略层（批次 B-4，决策词汇归 strategy 所有），
/// 此处转发保持既有导入路径稳定；配置读写函数仍在上下文层本文件。
pub use crate::evolution::strategy::ApplyPolicy;

/// bot-config.json 里的配置键（evolution 块内）。
pub(crate) const APPLY_POLICY_KEY: &str = "applyPolicy";

/// post_consolidation 分流谓词：auto（或缺省/读不到句柄）= 允许自动落库。
/// `None`（emit 句柄未注册，测试环境）按 auto——治理开关缺位时绝不改变既有行为。
pub fn auto_apply_allowed<R: tauri::Runtime>(app: Option<&AppHandle<R>>) -> bool {
    match app {
        Some(app) => read_apply_policy(app) != ApplyPolicy::Confirm,
        None => true,
    }
}

/// 便捷读取（自动定位 bot-config.json）。
pub fn read_apply_policy<R: tauri::Runtime>(app: &AppHandle<R>) -> ApplyPolicy {
    read_apply_policy_at(&crate::db::paths::data_dir(app).join("bot-config.json"))
}

/// 可测内核（纯路径参数）：缺文件/缺块/缺字段/非法值 → Auto。
pub fn read_apply_policy_at(path: &Path) -> ApplyPolicy {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return ApplyPolicy::Auto;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        // 文件坏掉 ≠ 未配置：留痕（每次 consolidate 一行，低频可接受）
        eprintln!("[evolution] {path:?} JSON 解析失败，applyPolicy 按 auto 放行");
        return ApplyPolicy::Auto;
    };
    match v
        .get("evolution")
        .and_then(|e| e.get(APPLY_POLICY_KEY))
        .and_then(|s| s.as_str())
        .and_then(ApplyPolicy::from_config_str)
    {
        Some(p) => p,
        None => ApplyPolicy::Auto, // 缺块/缺字段/非字符串/非法值 = auto（现状）
    }
}

/// 写互斥：RMW（读→改→写）全程持锁，防并发两次点档后写覆盖先写。
/// 持锁跨阻塞文件 IO 是有意取舍：这是低频设置写入路径，拆锁会破坏
/// 「读到的基线在写回时仍有效」的 RMW 不变式。
static APPLY_POLICY_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 便捷写入（自动定位 bot-config.json）。
pub fn set_apply_policy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    policy: ApplyPolicy,
) -> Result<(), String> {
    set_apply_policy_at(
        &crate::db::paths::data_dir(app).join("bot-config.json"),
        policy,
    )
}

/// 可测内核（纯路径参数）：RMW 只改 evolution.applyPolicy，其余字段原样保留；
/// 文件缺失从 `{}` 起步；JSON 解析失败拒绝写（Err，不覆盖坏文件防丢配置）。
pub fn set_apply_policy_at(path: &Path, policy: ApplyPolicy) -> Result<(), String> {
    let _g = APPLY_POLICY_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] evolution::policy::APPLY_POLICY_WRITE_LOCK: {e:?}");
        e.into_inner()
    });
    let mut v = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|e| format!("{path:?} JSON 解析失败，拒绝写（防覆盖既有配置）：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("读 {path:?} 失败：{e}")),
    };
    let obj = v
        .as_object_mut()
        .ok_or_else(|| format!("{path:?} 顶层非 object，拒绝写"))?;
    let evo = obj
        .entry("evolution".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "evolution 块非 object，拒绝写".to_string())?;
    evo.insert(
        APPLY_POLICY_KEY.to_string(),
        serde_json::Value::String(policy.as_str().to_string()),
    );
    crate::db::paths::atomic_write(
        path,
        &serde_json::to_string_pretty(&v).map_err(|e| format!("序列化：{e}"))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wm-evo-policy-{tag}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("bot-config.json")
    }

    // 读取矩阵：缺什么都是 auto（默认档零变化）

    #[test]
    fn missing_file_reads_auto() {
        assert_eq!(
            read_apply_policy_at(Path::new("/tmp/no-such-bot-config-u20-xyz.json")),
            ApplyPolicy::Auto
        );
    }

    #[test]
    fn missing_block_or_field_reads_auto() {
        let p = tmp_path("missing");
        std::fs::write(&p, r#"{"baseUrl":"https://x/v1"}"#).unwrap();
        assert_eq!(
            read_apply_policy_at(&p),
            ApplyPolicy::Auto,
            "缺 evolution 块"
        );
        std::fs::write(&p, r#"{"evolution":{"shadow":{"enabled":true}}}"#).unwrap();
        assert_eq!(
            read_apply_policy_at(&p),
            ApplyPolicy::Auto,
            "缺 applyPolicy 字段"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn invalid_or_non_string_value_reads_auto() {
        let p = tmp_path("invalid");
        std::fs::write(&p, r#"{"evolution":{"applyPolicy":"bogus"}}"#).unwrap();
        assert_eq!(read_apply_policy_at(&p), ApplyPolicy::Auto, "非法值 = auto");
        std::fs::write(&p, r#"{"evolution":{"applyPolicy":42}}"#).unwrap();
        assert_eq!(
            read_apply_policy_at(&p),
            ApplyPolicy::Auto,
            "非字符串 = auto"
        );
        std::fs::write(&p, "{ broken").unwrap();
        assert_eq!(
            read_apply_policy_at(&p),
            ApplyPolicy::Auto,
            "JSON 坏 = auto"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn explicit_values_roundtrip() {
        assert_eq!(
            ApplyPolicy::from_config_str("auto"),
            Some(ApplyPolicy::Auto)
        );
        assert_eq!(
            ApplyPolicy::from_config_str("confirm"),
            Some(ApplyPolicy::Confirm)
        );
        assert_eq!(ApplyPolicy::from_config_str("Confirm"), None, "大小写敏感");
        let p = tmp_path("roundtrip");
        std::fs::write(&p, r#"{"evolution":{"applyPolicy":"confirm"}}"#).unwrap();
        assert_eq!(read_apply_policy_at(&p), ApplyPolicy::Confirm);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    // 分流谓词

    #[test]
    fn auto_apply_allowed_none_handle_is_true() {
        // 无句柄（测试环境/未注册）= auto = 现状，治理开关缺位不改变行为。
        // Some(handle) 的 confirm 分流在 tests/evolution_gov.rs 端到端覆盖
        //（要写共享 bot-config.json，跨进程用例统一收敛到集成测试单用例）。
        assert!(auto_apply_allowed::<tauri::Wry>(None));
    }

    // 写入

    #[test]
    fn set_creates_missing_file_with_block() {
        let p = tmp_path("create");
        set_apply_policy_at(&p, ApplyPolicy::Confirm).unwrap();
        assert_eq!(read_apply_policy_at(&p), ApplyPolicy::Confirm);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["evolution"]["applyPolicy"], "confirm");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn set_preserves_other_fields() {
        let p = tmp_path("preserve");
        std::fs::write(
            &p,
            r#"{"baseUrl":"https://x/v1","evolution":{"shadow":{"enabled":true},"applyPolicy":"auto"},"memoryControl":{"injectionEnabled":false}}"#,
        )
        .unwrap();
        set_apply_policy_at(&p, ApplyPolicy::Confirm).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["evolution"]["applyPolicy"], "confirm", "目标字段已改");
        assert_eq!(
            v["evolution"]["shadow"]["enabled"], true,
            "同块兄弟字段保留"
        );
        assert_eq!(v["baseUrl"], "https://x/v1", "顶层字段保留");
        assert_eq!(v["memoryControl"]["injectionEnabled"], false, "无关块保留");
        // 覆盖回 auto 也保留
        set_apply_policy_at(&p, ApplyPolicy::Auto).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["evolution"]["applyPolicy"], "auto");
        assert_eq!(v["evolution"]["shadow"]["enabled"], true);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn set_refuses_to_overwrite_corrupt_file() {
        // 坏文件上 RMW = 丢用户配置：拒绝写（fail-closed）
        let p = tmp_path("corrupt");
        std::fs::write(&p, "{ broken json").unwrap();
        let err = set_apply_policy_at(&p, ApplyPolicy::Confirm).unwrap_err();
        assert!(err.contains("拒绝写"), "应拒绝覆盖坏文件：{err}");
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "{ broken json",
            "原文件未被破坏"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn set_rejects_non_object_root() {
        let p = tmp_path("nonobj");
        std::fs::write(&p, "[1,2,3]").unwrap();
        assert!(set_apply_policy_at(&p, ApplyPolicy::Confirm).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
