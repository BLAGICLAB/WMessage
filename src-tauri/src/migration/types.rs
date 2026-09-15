//! 迁移模块公共数据类型：规则表 / 报告 / 状态 / journal 条目。
//! 全部为纯数据 + Default 派生；无 IO、无锁。

use serde::{Deserialize, Serialize};

/// B1: 文件迁移操作日志记录（防 lost-update / 孤儿文件）。
/// 原来 file-move 成功但 db_upsert 失败 → 下一轮"源已消失"逻辑会解绑 file_path，
/// 附件链接永久丢失。本表记录「正在进行」的操作，启动时 replay 修复 DB。
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub id: i64,
    pub op: String, // 'move' | 'delete'
    pub src: String,
    pub dst: Option<String>,
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRule {
    pub id: String,
    pub enabled: bool,
    pub keywords: Vec<String>,
    #[serde(default = "default_action")]
    pub action: String, // "move" | "delete"
    #[serde(default)]
    pub archive_dir: String,
}

pub(crate) fn default_action() -> String {
    "move".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    #[serde(default = "default_version")]
    pub version: u32,
    pub rules: Vec<MigrationRule>,
}

pub(crate) fn default_version() -> u32 {
    1
}

impl Default for RulesFile {
    fn default() -> Self {
        Self {
            version: 1,
            rules: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MigrationReport {
    pub ts: i64,
    /// 本轮自动归档的任务数（完成满 7 天 → archived=true）
    pub archived: usize,
    /// 移动归档成功数
    pub moved: usize,
    pub deleted: usize,
    /// 跳过数（源缺失/冲突/权限/无匹配等）
    pub skipped: usize,
    /// 本轮日志行
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationStatus {
    pub rules_count: usize,
    pub poll_interval_secs: u64,
}
