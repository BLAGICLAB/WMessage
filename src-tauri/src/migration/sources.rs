//! 扫描整理来源表：读写 + 校验 + 地盘归属判定。
//!
//! 源表存数据目录 `cleanup-sources.json`；文件缺失 = 首装合法态 → 默认仅桌面源。
//! 桌面源路径永不落盘——每次运行经已知文件夹 API 解析（Windows 重定向/OneDrive
//! 劫持/显示名本地化都由该 API 归一），落盘的绝对路径反而会过期。

use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::db;
use crate::error::CommandError;

use super::types::{ScanSource, SourcesFile};

pub(crate) const SOURCES_FILE: &str = "cleanup-sources.json";

pub(crate) fn sources_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join(SOURCES_FILE)
}

pub fn load_sources(app: &AppHandle) -> Result<SourcesFile, CommandError> {
    let path = sources_path(app);
    match load_sources_from(&path) {
        Ok(s) => Ok(s),
        Err(e) => {
            super::ops::log_line(app, &e.to_string());
            Err(e)
        }
    }
}

/// 源表读取内核（纯函数供单测）。文件不存在 → 默认仅桌面源（扫描制的出厂形态）；
/// 存在但读不了（权限/IO）→ IoError；解析失败（损坏）→ DomainRule——坏文件不得被
/// 空源表静默顶替，否则后续 save 会以空表覆写用户配置（与 load_rules_from 同口径）。
pub(crate) fn load_sources_from(path: &Path) -> Result<SourcesFile, CommandError> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourcesFile {
                sources: vec![ScanSource::default()],
            })
        }
        Err(e) => {
            return Err(CommandError::IoError(format!(
                "来源表读取失败（{}）：{e}。请检查文件权限后重试",
                path.display()
            )))
        }
    };
    serde_json::from_str::<SourcesFile>(&text).map_err(|e| CommandError::DomainRule {
        domain: "migration".to_string(),
        reason: format!(
            "来源表解析失败（{}）：{e}。请修复或删除该文件后重试；应用不会以空源表静默替代",
            path.display()
        ),
    })
}

pub(crate) fn save_sources(app: &AppHandle, sources: &SourcesFile) -> Result<(), String> {
    save_sources_to(&sources_path(app), sources)
}

/// 原子写（复用 db::atomic_write，同规则表模式）——崩溃只留 tmp 残件
pub(crate) fn save_sources_to(path: &Path, sources: &SourcesFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(sources).map_err(|e| e.to_string())?;
    db::atomic_write(path, &text)
}

/// 校验源表：kind 合法；桌面源至多一条且不带路径；自定义源路径绝对、目录存在、
/// 互不重复（小写比较）；归档根相对（拒绝 `..`/绝对/根式）对所有源生效。
pub(crate) fn validate_sources(sources: &SourcesFile) -> Result<(), String> {
    let mut seen_custom: Vec<String> = Vec::new();
    let mut desktop_count = 0usize;
    for (i, s) in sources.sources.iter().enumerate() {
        if s.kind != "desktop" && s.kind != "custom" {
            return Err(format!(
                "第 {} 条来源类型非法：{}（仅支持 desktop / custom）",
                i + 1,
                s.kind
            ));
        }
        super::ops::reject_parent_dir_components(Path::new(&s.archive_root), &s.archive_root)?;
        let root = Path::new(&s.archive_root);
        if root.is_absolute() || root.has_root() {
            return Err(format!(
                "第 {} 条来源的归档根必须是相对路径：{}",
                i + 1,
                s.archive_root
            ));
        }
        if s.kind == "desktop" {
            desktop_count += 1;
            if !s.path.trim().is_empty() {
                return Err("桌面来源不带自定义路径（运行时解析真实桌面）".to_string());
            }
            continue;
        }
        let p = Path::new(&s.path);
        if s.path.trim().is_empty() || !p.is_absolute() {
            return Err(format!(
                "第 {} 条自定义来源必须是绝对路径：{}",
                i + 1,
                s.path
            ));
        }
        if !p.is_dir() {
            return Err(format!("第 {} 条自定义来源目录不存在：{}", i + 1, s.path));
        }
        let lower = s.path.to_lowercase();
        if seen_custom.contains(&lower) {
            return Err(format!("自定义来源重复：{}", s.path));
        }
        seen_custom.push(lower);
    }
    if desktop_count > 1 {
        return Err(format!("桌面来源至多一条，现有 {desktop_count} 条"));
    }
    Ok(())
}

/// 源的扫描根（地盘）：桌面预设 → 运行时解析真实桌面；自定义 → 存储路径。
/// Err = 该源当前不可用（桌面解析失败 / 自定义目录不存在）。
pub(crate) fn source_scan_root(app: &AppHandle, source: &ScanSource) -> Result<PathBuf, String> {
    match source.kind.as_str() {
        "desktop" => app
            .path()
            .desktop_dir()
            .map_err(|e| format!("无法定位桌面目录：{e}")),
        "custom" => {
            let p = PathBuf::from(&source.path);
            if p.is_dir() {
                Ok(p)
            } else {
                Err(format!("扫描源目录不存在：{}", source.path))
            }
        }
        other => Err(format!("未知来源类型：{other}")),
    }
}

/// 源的规则归档基准 = 扫描根 + 归档根（空 = 扫描根本身）。
/// 调用方保证 source 已过校验（archive_root 相对），此处只做拼接。
pub(crate) fn source_rule_base(source: &ScanSource, scan_root: &Path) -> PathBuf {
    if source.archive_root.trim().is_empty() {
        scan_root.to_path_buf()
    } else {
        scan_root.join(&source.archive_root)
    }
}

/// 地盘归属（纯函数供单测）：src 落在哪个候选根内，返回最长组件级前缀命中的下标
/// （嵌套地盘最深者胜）。比较统一小写——Windows 路径大小写不敏感，绑定/选择器/
/// API 三方拼出的路径 casing 可能不一致；组件级比较不吃字符串前缀陷阱
/// （`D:\arch` 不是 `D:\a` 内部）。
pub(crate) fn longest_containing_root(src: &Path, roots: &[PathBuf]) -> Option<usize> {
    let src_lower = PathBuf::from(src.to_string_lossy().to_lowercase());
    let mut best: Option<(usize, usize)> = None;
    for (i, r) in roots.iter().enumerate() {
        let r_lower = PathBuf::from(r.to_string_lossy().to_lowercase());
        if src_lower.starts_with(&r_lower) {
            let depth = r_lower.components().count();
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((i, depth));
            }
        }
    }
    best.map(|(i, _)| i)
}

/// 绑定文件的规则归档基准：落在哪个启用源的地盘就用该源的归档根；
/// 都不在 → 桌面（任意地址绑定物的兜底归位，与存量规则表兼容）。
pub(crate) fn resolve_rule_base(
    app: &AppHandle,
    src: &Path,
    sources: &SourcesFile,
) -> Result<PathBuf, String> {
    let desktop = app
        .path()
        .desktop_dir()
        .map_err(|e| format!("无法定位桌面目录：{e}"))?;
    let roots: Vec<PathBuf> = sources
        .sources
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| source_scan_root(app, s).ok())
        .collect();
    match longest_containing_root(src, &roots) {
        Some(i) => Ok(roots[i].clone()),
        None => Ok(desktop),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-sources-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 文件不存在 → 默认仅桌面源（enabled）
    #[test]
    fn load_missing_returns_default_desktop() {
        let p = tmp_dir("missing").join("cleanup-sources.json");
        let s = load_sources_from(&p).unwrap();
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.sources[0].kind, "desktop");
        assert!(s.sources[0].enabled);
    }

    /// 解析失败 → DomainRule 响亮失败，不被空表顶替
    #[test]
    fn load_corrupt_is_domain_error() {
        let dir = tmp_dir("corrupt");
        let p = dir.join("cleanup-sources.json");
        std::fs::write(&p, "{oops").unwrap();
        let err = load_sources_from(&p).unwrap_err();
        assert!(err.to_string().contains("来源表解析失败"), "got: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 往返：save → load 原样（camelCase 落盘）
    #[test]
    fn save_then_load_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let p = dir.join("cleanup-sources.json");
        let file = SourcesFile {
            sources: vec![
                ScanSource::default(),
                ScanSource {
                    kind: "custom".into(),
                    path: dir.to_string_lossy().to_string(),
                    archive_root: "sorted".into(),
                    enabled: false,
                },
            ],
        };
        save_sources_to(&p, &file).unwrap();
        let loaded = load_sources_from(&p).unwrap();
        assert_eq!(loaded, file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_rejects_bad_sources() {
        let ok_custom = |path: &str| ScanSource {
            kind: "custom".into(),
            path: path.to_string(),
            archive_root: String::new(),
            enabled: true,
        };
        // 自定义源路径相对 → 拒
        let err = validate_sources(&SourcesFile {
            sources: vec![ScanSource::default(), ok_custom("relative/dir")],
        })
        .unwrap_err();
        assert!(err.contains("绝对路径"), "got: {err}");
        // 桌面源带路径 → 拒
        let err = validate_sources(&SourcesFile {
            sources: vec![ScanSource {
                path: "C:\\x".into(),
                ..ScanSource::default()
            }],
        })
        .unwrap_err();
        assert!(err.contains("桌面来源"), "got: {err}");
        // 桌面源重复 → 拒
        let err = validate_sources(&SourcesFile {
            sources: vec![ScanSource::default(), ScanSource::default()],
        })
        .unwrap_err();
        assert!(err.contains("至多一条"), "got: {err}");
        // 归档根带 `..` → 拒
        let err = validate_sources(&SourcesFile {
            sources: vec![ScanSource {
                archive_root: "../escape".into(),
                ..ScanSource::default()
            }],
        })
        .unwrap_err();
        assert!(err.contains(".."), "got: {err}");
        // 自定义源重复（同一路径出现两次；目录须真实存在才能走到去重检查）→ 拒
        let dir = tmp_dir("dup");
        let dup_path = dir.to_string_lossy().to_string();
        let err = validate_sources(&SourcesFile {
            sources: vec![
                ScanSource::default(),
                ok_custom(&dup_path),
                ok_custom(&dup_path),
            ],
        })
        .unwrap_err();
        assert!(err.contains("重复"), "got: {err}");
        std::fs::remove_dir_all(&dir).ok();
        // 未知 kind → 拒
        let err = validate_sources(&SourcesFile {
            sources: vec![ScanSource {
                kind: "onedrive".into(),
                ..ScanSource::default()
            }],
        })
        .unwrap_err();
        assert!(err.contains("类型非法"), "got: {err}");
    }

    #[test]
    fn validate_allows_desktop_only_and_rooted_custom() {
        // 仅桌面源（出厂形态）合法
        validate_sources(&SourcesFile {
            sources: vec![ScanSource::default()],
        })
        .unwrap();
        // 自定义源带相对归档根合法（目录须真实存在）
        let dir = tmp_dir("rooted");
        validate_sources(&SourcesFile {
            sources: vec![
                ScanSource::default(),
                ScanSource {
                    kind: "custom".into(),
                    path: dir.to_string_lossy().to_string(),
                    archive_root: "sorted/{year}".into(),
                    enabled: true,
                },
            ],
        })
        .unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn source_rule_base_joins_or_identity() {
        let root = Path::new("/base");
        assert_eq!(
            source_rule_base(&ScanSource::default(), root),
            PathBuf::from("/base"),
            "空归档根 = 扫描根本身"
        );
        let custom = ScanSource {
            kind: "custom".into(),
            path: String::new(),
            archive_root: "sorted".into(),
            enabled: true,
        };
        assert_eq!(
            source_rule_base(&custom, root),
            PathBuf::from("/base/sorted")
        );
    }

    #[test]
    fn longest_root_wins_and_case_insensitive() {
        let outer = PathBuf::from("/data/downloads");
        let inner = PathBuf::from("/data/downloads/keep");
        let roots = vec![outer.clone(), inner.clone()];
        // 深层目录命中更长前缀
        assert_eq!(
            longest_containing_root(Path::new("/data/downloads/keep/a.txt"), &roots),
            Some(1)
        );
        // 浅层命中外层
        assert_eq!(
            longest_containing_root(Path::new("/data/downloads/a.txt"), &roots),
            Some(0)
        );
        // 大小写不一致（Windows 拼写差异）照常命中
        assert_eq!(
            longest_containing_root(Path::new("/DATA/DOWNLOADS/a.txt"), &roots),
            Some(0)
        );
        // 字符串前缀陷阱：/data/downloads2 不属于 /data/downloads
        assert_eq!(
            longest_containing_root(Path::new("/data/downloads2/a.txt"), &roots),
            None
        );
        // 完全无关 → None
        assert_eq!(
            longest_containing_root(Path::new("/etc/passwd"), &roots),
            None
        );
    }
}
