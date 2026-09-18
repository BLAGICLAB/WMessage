//! evolution.eval 配置块（bot-config.json）
//!
//! spec 格式：
//!   {
//!     "evolution": {
//!       "eval": {
//!         "eval_set_path": "...",
//!         "feedback_path": "...",
//!         "run_frequency": "daily"
//!       }
//!     }
//!   }
//!
//! 加载器只读 `evolution` 顶层 key；其他顶层 key 透传不解析。
//! 不动 bot-config.json 的其他字段（spec 硬约束 3：不改 JSON 字段）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// evolution.eval 块
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionEvalConfig {
    /// eval 用例 jsonl 路径
    pub eval_set_path: PathBuf,
    /// 反馈 jsonl 路径
    pub feedback_path: PathBuf,
    /// 跑频率：daily / manual / hourly
    pub run_frequency: RunFrequency,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunFrequency {
    Daily,
    Hourly,
    Manual,
}

impl RunFrequency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Hourly => "hourly",
            Self::Manual => "manual",
        }
    }
}

/// 整体 bot-config.json（只关心 evolution 块，其他字段以 Value 透传）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotConfig {
    #[serde(default)]
    pub evolution: Option<EvolutionConfig>,
}

/// evolution 顶层（eval 是其中一个子块；后续 R2-R8 会加 sibling 块）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionConfig {
    #[serde(default)]
    pub eval: Option<EvolutionEvalConfig>,
}

/// 加载 bot-config.json；缺文件 / 缺 evolution.eval 块时 return Err（强制显式）
pub fn load(path: &std::path::Path) -> Result<EvolutionEvalConfig, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;
    let cfg: BotConfig = serde_json::from_str(&raw)
        .map_err(|e| format!("解析 bot-config.json 失败：{e}"))?;
    let evo = cfg
        .evolution
        .ok_or_else(|| "bot-config.json 缺少 evolution 块".to_string())?;
    evo.eval
        .ok_or_else(|| "evolution 块缺少 eval 子块".to_string())
}

/// 默认路径解析（开发模式：项目树 bot-config.json）
pub fn default_config_path() -> PathBuf {
    // 项目根相对 exe 的位置（CLI binary 跑在 src-tauri/ 下，向上两级到项目根）
    PathBuf::from("bot-config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_parses_evolution_eval() {
        let dir = std::env::temp_dir().join(format!("cfg-ok-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("bot-config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &p,
            r#"{
                "evolution": {
                    "eval": {
                        "eval_set_path": "../evolution/eval_set.jsonl",
                        "feedback_path": "/tmp/feedback.jsonl",
                        "run_frequency": "daily"
                    }
                }
            }"#,
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.eval_set_path, PathBuf::from("../evolution/eval_set.jsonl"));
        assert_eq!(cfg.run_frequency, RunFrequency::Daily);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_evolution_returns_err() {
        let dir = std::env::temp_dir().join(format!("cfg-noevo-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("bot-config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, r#"{"foo":"bar"}"#).unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("缺少 evolution"), "应提示缺 evolution：{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_eval_block_returns_err() {
        let dir = std::env::temp_dir().join(format!("cfg-noeval-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("bot-config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, r#"{"evolution":{"foo":"bar"}}"#).unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("缺少 eval"), "应提示缺 eval：{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frequency_strings_locked() {
        assert_eq!(RunFrequency::Daily.as_str(), "daily");
        assert_eq!(RunFrequency::Hourly.as_str(), "hourly");
        assert_eq!(RunFrequency::Manual.as_str(), "manual");
    }
}