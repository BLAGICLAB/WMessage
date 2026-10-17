//!  L3 沙箱层 · kill_switch（spec ）
//!
//! 两开关：
//! - all_auto_apply: 全关自动应用（仍可手动/人工批准）
//! - disable_notification: 关掉 apply 完成通知
//!
//! 原来的第三个开关 `shadow_only`（只跑影子轨、不真实写入）随影子轨一起删除：
//! 影子轨在当前配置下从不执行 evaluate（恒 Allow，不阻断任何东西），它留着的
//! 唯一作用是把「不写入」这件事说两遍。
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
    pub disable_notification: bool,
}

impl KillSwitch {
    /// 三开关全关（默认）
    pub fn default_off() -> Self {
        Self::default()
    }

    /// 是否允许自动应用（apply.rs 入口检查）
    pub fn should_auto_apply(&self) -> bool {
        !self.all_auto_apply
    }

    /// 是否因 kill switch 跳过了真实写入（apply.rs 入口检查）
    pub fn should_skip_apply(&self) -> bool {
        self.all_auto_apply
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
/// 4. 缺 evolution / 缺 kill_switch → 返 Err（强制显式，调用方按全关处理）
pub fn load_from_file(path: &Path) -> Result<KillSwitch, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("解析 {path:?} 失败：{e}"))?;
    let evo = v
        .get("evolution")
        .ok_or_else(|| "bot-config.json 缺少 evolution 块".to_string())?;
    let kill_switch = evo
        .get("kill_switch")
        .ok_or_else(|| "evolution 块缺少 kill_switch 子块".to_string())?;
    serde_json::from_value(kill_switch.clone()).map_err(|e| format!("kill_switch 解析失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_off_all_false() {
        let k = KillSwitch::default_off();
        assert!(!k.all_auto_apply);
        assert!(!k.disable_notification);
        assert!(k.should_auto_apply());
        assert!(!k.should_skip_apply());
        assert!(!k.should_disable_notification());
    }

    /// 全关自动应用 = 不写长期记忆，但仍可手动/人工批准
    #[test]
    fn all_auto_apply_blocks_real_write() {
        let k = KillSwitch {
            all_auto_apply: true,
            disable_notification: false,
        };
        assert!(!k.should_auto_apply());
        assert!(k.should_skip_apply());
    }

    #[test]
    fn missing_block_is_err_and_caller_falls_back_to_default_off() {
        let dir = std::env::temp_dir().join(format!(
            "wm-evo-kill-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"baseUrl":"https://x/v1"}"#).unwrap();
        assert!(
            load_from_file(&p).is_err(),
            "缺 evolution/kill_switch 块必须显式报错，让调用方走 default_off"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 落盘值直通：两个开关各自独立生效，不做隐含归一
    #[test]
    fn roundtrip_both_switches() {
        let dir = std::env::temp_dir().join(format!(
            "wm-evo-kill-rt-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(
            &p,
            r#"{"evolution":{"kill_switch":{"all_auto_apply":true,"disable_notification":true}}}"#,
        )
        .unwrap();
        let k = load_from_file(&p).expect("应能读出");
        assert!(k.all_auto_apply);
        assert!(k.disable_notification);
        assert!(!k.should_auto_apply());
        assert!(k.should_disable_notification());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
