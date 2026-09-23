//! 规则表读写 + CSV 解析 + 路径常量。
//!
//! 规则存于数据目录 `cleanup-rules.json`；模版 / 导入 CSV 由前端触发。
//! 日志路径同处便于 ops::log_line 复用。

use std::fs;
use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::db;

use super::types::{MigrationRule, RulesFile};

pub(crate) const RULES_FILE: &str = "cleanup-rules.json";
pub(crate) const LOG_FILE: &str = "migration.log";

pub(crate) fn rules_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join(RULES_FILE)
}

pub(crate) fn log_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join(LOG_FILE)
}

pub fn load_rules(app: &AppHandle) -> RulesFile {
    match fs::read_to_string(rules_path(app)) {
        Ok(text) => match serde_json::from_str::<RulesFile>(&text) {
            Ok(r) => r,
            Err(e) => {
                crate::migration::ops::log_line(
                    app,
                    &format!("规则文件解析失败（按空规则处理）：{e}"),
                );
                RulesFile::default()
            }
        },
        Err(_) => RulesFile::default(),
    }
}

pub(crate) fn save_rules(app: &AppHandle, rules: &RulesFile) -> Result<(), String> {
    save_rules_to(&rules_path(app), rules)
}

/// P2-8：原子写（复用 NEW-B-6 db::atomic_write，同 D3 profile 模式）——
/// 崩溃在写中途只留 tmp 残件，rules.json 要么旧完整版要么新完整版，不留半截。
/// 抽 path 参数便于单测（AppHandle 无法单测构造）。
pub(crate) fn save_rules_to(path: &Path, rules: &RulesFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    crate::db::atomic_write(path, &text)
}

/// 校验规则：action 合法；move 必须有归档目录；关键字至少一个非空
pub fn validate_rules(rules: &RulesFile) -> Result<(), String> {
    for (i, r) in rules.rules.iter().enumerate() {
        if r.action != "move" && r.action != "delete" {
            return Err(format!(
                "第 {} 条规则动作非法：{}（仅支持 move / delete）",
                i + 1,
                r.action
            ));
        }
        if r.action == "move" && r.archive_dir.trim().is_empty() {
            return Err(format!("第 {} 条规则是移动归档，归档目录不能为空", i + 1));
        }
        if r.action == "move" {
            // 与 resolve_archive_dir 同一校验：导入期早错（run 期 ops 侧还有一道）
            super::ops::reject_parent_dir_components(
                std::path::Path::new(&r.archive_dir),
                &r.archive_dir,
            )?;
        }
        if !r.keywords.iter().any(|k| !k.trim().is_empty()) {
            return Err(format!("第 {} 条规则缺少文件名关键字", i + 1));
        }
    }
    Ok(())
}

/// CSV 规则模版文本（UTF-8 BOM：Excel/WPS 双击打开中文不乱码）。
/// 列：启用 | 文件名关键字 | 动作 | 归档目录
/// 关键字多个用中文逗号「，」分隔；启用填 是/否；动作填 移动归档/删除文件
pub(crate) fn template_csv() -> String {
    // 规则顺序即优先级：靠前的行先匹配（同名文件命中第一条规则）
    "\u{feff}启用,文件名关键字,动作,归档目录\n     是,工资，报销,移动归档,工资/{year}\n     否,临时,删除文件,\n"
        .to_string()
}

/// 解析 CSV 规则表文本（自动识别表头列；编码层已由调用方处理）
pub(crate) fn parse_rules_csv(text: &str) -> Result<RulesFile, crate::error::CommandError> {
    use std::io::Cursor;
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(Cursor::new(text.trim_start_matches('\u{feff}').as_bytes()));
    let headers = rdr
        .headers()
        .map_err(|e| format!("CSV 表头解析失败：{e}"))?
        .clone();
    // 表头定位：按列别名集精确匹配（Trim::All 已修边）。contains 宽容匹配会把
    // 「子关键字」错绑到「关键字」列、多列同 needle 时静默取首列——精确匹配 +
    // 重复命中报错（MI-08）。
    let find_col = |aliases: &[&str]| -> Result<Option<usize>, crate::error::CommandError> {
        let mut hits = headers
            .iter()
            .enumerate()
            .filter(|(_, h)| aliases.contains(h));
        let first = hits.next().map(|(i, _)| i);
        if hits.next().is_some() {
            return Err(crate::error::CommandError::DomainRule {
                domain: "migration".to_string(),
                reason: format!(
                    "CSV 表头含重复列（{}），无法确定目标列",
                    aliases.join(" / ")
                ),
            });
        }
        Ok(first)
    };
    let (Some(ci_enable), Some(ci_kw), Some(ci_action), Some(ci_dir)) = (
        find_col(&["启用"])?,
        find_col(&["文件名关键字", "关键字"])?,
        find_col(&["动作"])?,
        find_col(&["归档目录", "目录"])?,
    ) else {
        return Err(crate::error::CommandError::DomainRule {
            domain: "migration".to_string(),
            reason: "CSV 需包含四列表头：启用 / 文件名关键字 / 动作 / 归档目录".to_string(),
        });
    };
    let mut rules: Vec<MigrationRule> = Vec::new();
    for (ri, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| format!("第 {} 行解析失败：{e}", ri + 2))?;
        let get = |i: usize| rec.get(i).unwrap_or("").trim().to_string();
        let kw_raw = get(ci_kw);
        let action_raw = get(ci_action);
        if kw_raw.is_empty() {
            continue; // 空行/无关键字行跳过
        }
        let action = match action_raw.as_str() {
            "移动归档" | "移动" | "move" | "Move" => "move".to_string(),
            "删除文件" | "删除" | "delete" | "Delete" => "delete".to_string(),
            other if other.is_empty() => {
                return Err(crate::error::CommandError::DomainRule {
                    domain: "csv".to_string(),
                    reason: format!("第 {} 行动作为空（应填 移动归档 或 删除文件）", ri + 2),
                })
            }
            other => {
                return Err(crate::error::CommandError::DomainRule {
                    domain: "csv".to_string(),
                    reason: format!(
                        "第 {} 行动作「{}」无效（应填 移动归档 或 删除文件）",
                        ri + 2,
                        other
                    ),
                })
            }
        };
        let enabled = match get(ci_enable).as_str() {
            "是" | "true" | "True" | "TRUE" | "1" => true,
            "否" | "false" | "False" | "FALSE" | "0" => false,
            other => {
                return Err(crate::error::CommandError::DomainRule {
                    domain: "csv".to_string(),
                    reason: format!("第 {} 行启用值「{}」无效（应填 是 或 否）", ri + 2, other),
                })
            }
        };
        let keywords: Vec<String> = kw_raw
            .split([',', '，', '、', ';', '；'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if keywords.is_empty() {
            continue;
        }
        rules.push(MigrationRule {
            id: uuid::Uuid::new_v4().to_string(),
            enabled,
            keywords,
            action: action.clone(),
            archive_dir: if action == "move" {
                get(ci_dir)
            } else {
                String::new()
            },
        });
    }
    if rules.is_empty() {
        return Err(crate::error::CommandError::DomainRule {
            domain: "migration".to_string(),
            reason: "CSV 中没有解析出任何规则".to_string(),
        });
    }
    Ok(RulesFile { version: 1, rules })
}
