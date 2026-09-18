//! R3 L3 沙箱层 · kill_switch（spec R3）
//!
//! 三开关：
//! - all_auto_apply: 全关自动应用（仍可手动/人工批准）
//! - shadow_only: 只跑 shadow，不实际写入 mem_items
//! - disable_notification: 关掉 apply 完成通知
//!
//! 优先级：kill_switch > 策略分层 > 默认
//! 「立即生效」= 每次调用现读，无缓存

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct KillSwitch {
    #[serde(default)]
    pub all_auto_apply: bool,
    #[serde(default)]
    pub shadow_only: bool,
    #[serde(default)]
    pub disable_notification: bool,
}

impl KillSwitch {
    /// 三开关全关（默认）
    pub fn default_off() -> Self {
        Self::default()
    }

    /// 全开（紧急 kill）
    pub fn all_on() -> Self {
        Self {
            all_auto_apply: true,
            shadow_only: true,
            disable_notification: true,
        }
    }

    /// 是否允许自动应用（apply.rs 入口检查）
    ///
    /// 优先级：
    /// - all_auto_apply=true → 全关
    /// - 否则 → 允许
    pub fn should_auto_apply(&self) -> bool {
        !self.all_auto_apply
    }

    /// 是否只跑 shadow（apply.rs 入口检查）
    ///
    /// 优先级：
    /// - shadow_only=true → 只 shadow
    /// - all_auto_apply=true → 也只 shadow（隐含）
    /// - 否则 → 正常 apply
    pub fn should_shadow_only(&self) -> bool {
        self.shadow_only || self.all_auto_apply
    }

    /// 是否禁用 apply 完成通知
    pub fn should_disable_notification(&self) -> bool {
        self.disable_notification
    }
}

/// 全局默认（所有开关 false）
pub fn default_off() -> KillSwitch {
    KillSwitch::default_off()
}

/// 从 bot-config.json 加载 kill_switch 块
///
/// 读取路径：
/// 1. 读 {path} 整体 JSON
/// 2. 提取 evolution.kill_switch 子树
/// 3. 反序列化为 KillSwitch（缺字段用默认 false）
/// 4. 缺 evolution / 缺 kill_switch → 返 Err（强制显式）
pub fn load_from_file(path: &Path) -> Result<KillSwitch, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("解析 {path:?} 失败：{e}"))?;
    let evo = v
        .get("evolution")
        .ok_or_else(|| "bot-config.json 缺少 evolution 块".to_string())?;
    let kill_switch = evo
        .get("kill_switch")
        .ok_or_else(|| "evolution 块缺少 kill_switch 子块".to_string())?;
    serde_json::from_value(kill_switch.clone())
        .map_err(|e| format!("kill_switch 解析失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_off_all_false() {
        let k = KillSwitch::default_off();
        assert!(!k.all_auto_apply);
        assert!(!k.shadow_only);
        assert!(!k.disable_notification);
        assert!(k.should_auto_apply());
        assert!(!k.should_shadow_only());
        assert!(!k.should_disable_notification());
    }

    #[test]
    fn all_on_disables_everything() {
        let k = KillSwitch::all_on();
        assert!(!k.should_auto_apply());
        assert!(k.should_shadow_only());
        assert!(k.should_disable_notification());
    }

    #[test]
    fn shadow_only_implies_no_real_apply() {
        let k = KillSwitch {
            all_auto_apply: false,
            shadow_only: true,
            disable_notification: false,
        };
        // shadow_only=true → should_auto_apply=false（因为要 shadow）
        // 但 should_auto_apply 只看 all_auto_apply
        // 实际语义：apply.rs 应先看 should_shadow_only
        assert!(k.should_shadow_only());
        assert!(!k.should_disable_notification());
    }

    #[test]
    fn all_auto_apply_implies_shadow_only() {
        // 硬约束语义：all_auto_apply=true 隐含 shadow_only
        let k = KillSwitch {
            all_auto_apply: true,
            shadow_only: false,
            disable_notification: false,
        };
        assert!(k.should_shadow_only(), "all_auto_apply=true 应隐含 shadow_only");
    }

    #[test]
    fn load_parses_kill_switch_block() {
        let dir = std::env::temp_dir().join(format!("ks-ok-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(
            &p,
            r#"{
                "evolution": {
                    "eval": {
                        "eval_set_path": "x",
                        "feedback_path": "y",
                        "run_frequency": "daily"
                    },
                    "kill_switch": {
                        "all_auto_apply": true,
                        "shadow_only": false,
                        "disable_notification": false
                    }
                }
            }"#,
        )
        .unwrap();
        let k = load_from_file(&p).unwrap();
        assert!(k.all_auto_apply);
        assert!(!k.shadow_only);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_kill_switch_block_errors() {
        let dir = std::env::temp_dir().join(format!("ks-no-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"eval":{"eval_set_path":"x","feedback_path":"y","run_frequency":"daily"}}}"#).unwrap();
        let e = load_from_file(&p).unwrap_err();
        assert!(e.contains("kill_switch"), "应提示缺 kill_switch：{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_evolution_block_errors() {
        let dir = std::env::temp_dir().join(format!("ks-ne-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"foo":"bar"}"#).unwrap();
        let e = load_from_file(&p).unwrap_err();
        assert!(e.contains("evolution"), "应提示缺 evolution：{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_partial_kill_switch_uses_defaults() {
        // 只给 all_auto_apply，shadow_only / disable_notification 用默认 false
        let dir = std::env::temp_dir().join(format!("ks-p-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(
            &p,
            r#"{"evolution":{"kill_switch":{"all_auto_apply":true}}}"#,
        )
        .unwrap();
        let k = load_from_file(&p).unwrap();
        assert!(k.all_auto_apply);
        assert!(!k.shadow_only);
        assert!(!k.disable_notification);
        let _ = std::fs::remove_dir_all(&dir);
    }
}