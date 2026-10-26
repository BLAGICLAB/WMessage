//! create_skill 相关：准入核 / 落盘 / 工具口。
//!
//! 从 manage.rs 切出。`use super::*` 拿父模块（manage）的 import——
//! SkillMeta / CommandError / AppHandle 等都在那里。

use super::*;
use crate::bot_skills::runtime;

/// SKILL.md 正文字符上限（对齐业界 SKILL.md ≤500 行/≈5K tokens 规范的宽松裕度）
const CREATE_MAX_SKILL_CHARS: usize = 64 * 1024;
/// 单附件上限；附件个数上限；附件总字节上限
const CREATE_MAX_FILE_BYTES: usize = 200 * 1024;
const CREATE_MAX_FILES: usize = 10;
const CREATE_MAX_TOTAL_BYTES: usize = 1024 * 1024;

/// 创建校验的元数据部分（纯逻辑可测）：description 必填（技能清单与路由展示必需）
/// + intents/description 危险关键词预检（与运行时 preflight 同一黑名单——把拒绝
/// 提前到落盘前，而非装上后每次启动才炸）+ intents 宽度闸（装上即进进程级路由表，
/// 过宽模式会把此后每条用户消息都劫持进该技能）。
/// 三个装入入口（create_skill / install_skill / 设置页导入）共用的准入核：
/// description 必填、intents/description 不命中危险关键词、intents 每条模式
/// 可编译且不过宽、auto 模式必须有可执行的 Step 块。
pub fn validate_created_meta(meta: &SkillMeta, content: &str) -> Result<(), String> {
    if meta.description.trim().is_empty() {
        return Err("frontmatter 缺少 description（技能清单与路由展示必需）".into());
    }
    let hay = format!(
        "{} {}",
        meta.description.to_lowercase(),
        meta.intents.join(" ")
    );
    if let Some(hit) = runtime::INTENT_BLACKLIST
        .iter()
        .find(|b| hay.contains(&b.to_lowercase()))
    {
        return Err(format!(
            "intents/description 命中危险关键词「{hit}」，拒绝创建"
        ));
    }
    for p in &meta.intents {
        crate::intent_router::check_intent_pattern(p)
            .map_err(|reason| format!("intents「{p}」{reason}"))?;
    }
    validate_auto_mode_steps(meta, content)
}

/// auto = 命中后不经 LLM 直接执行步骤，正文必须解析得出可执行的 Step 块，
/// 否则装上即首跑必败（调度器 DSL 解析为空当场终止）。解析口径与运行时
/// 一致：正文按 MAX_SKILL_BODY 截断后走 parse_skill_steps。interactive 的
/// 正文是给 LLM 的参考文档，不受此限。
fn validate_auto_mode_steps(meta: &SkillMeta, content: &str) -> Result<(), String> {
    if meta.mode != "auto" {
        return Ok(());
    }
    let body: String = content
        .chars()
        .take(crate::bot_skills::state::MAX_SKILL_BODY)
        .collect();
    match parse_skill_steps(&body) {
        Ok((steps, _)) if !steps.is_empty() => Ok(()),
        Ok(_) => Err(
            "mode=auto 要求正文含「## Step N:」加工具调用行形式的可执行步骤块，当前正文一个都没有（散文式流程请改用 mode: interactive）".into(),
        ),
        Err(e) => Err(format!(
            "mode=auto 的步骤块解析失败：{e}；请修正「## Step N:」格式，或改用 mode: interactive"
        )),
    }
}

/// 待创建技能的元数据核（纯逻辑、可测）：解析 frontmatter + 名称一致性 +
/// 准入核。名称一致性：frontmatter name 必须与技能名一致——路由/清单用
/// meta.name，磁盘目录用参数名，失配会产出「路由指 frontmatter 名、目录叫参数名」
/// 的僵尸技能（load/delete 都按目录名找）。
fn prepare_created_meta(name: &str, content: &str) -> Result<SkillMeta, String> {
    let meta = parse_meta(content, name);
    if meta.name != name {
        return Err(format!(
            "SKILL.md frontmatter name「{}」与技能名「{name}」不一致——frontmatter name 必须与技能名完全一致",
            meta.name
        ));
    }
    validate_created_meta(&meta, content)?;
    Ok(meta)
}

/// 落盘核（同步、可测）：name 须已过 validate_skill_name；content 为完整 SKILL.md
/// 文本；files 为相对技能目录的附件（path, content）。排他占位 + 失败清理，
/// 与 skills_import 同一模式。返回写入的附件个数。
pub(crate) fn create_skill_at(
    dir: &std::path::Path,
    name: &str,
    content: &str,
    files: &[(String, String)],
) -> Result<usize, CommandError> {
    if content.chars().count() > CREATE_MAX_SKILL_CHARS {
        return Err(CommandError::InvalidArgument {
            field: "content".into(),
            value: String::new(),
            reason: format!("SKILL.md 超过 {CREATE_MAX_SKILL_CHARS} 字符上限"),
        });
    }
    // frontmatter name 必须与目录名一致 + 准入核（description 必填 / 危险词 /
    // intents 宽度闸）：解析与校验口径单源，工具层与落盘核共用
    prepare_created_meta(name, content).map_err(|reason| CommandError::InvalidArgument {
        field: "content".into(),
        value: String::new(),
        reason,
    })?;
    if files.len() > CREATE_MAX_FILES {
        return Err(CommandError::InvalidArgument {
            field: "files".into(),
            value: files.len().to_string(),
            reason: format!("附件超过 {CREATE_MAX_FILES} 个上限"),
        });
    }
    let mut total = content.len();
    let mut cleaned: Vec<(std::path::PathBuf, &str)> = Vec::new();
    for (p, c) in files {
        let pb = std::path::Path::new(p);
        if pb.is_absolute() || p.contains("..") {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "附件路径必须为技能目录内的相对路径".into(),
            });
        }
        // 逐组件字符集校验（字母/数字/点/-/_，禁空组件）：穿越与怪名都在这里挡下
        for comp in pb.components() {
            let s = match comp {
                std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
                _ => {
                    return Err(CommandError::InvalidArgument {
                        field: "files.path".into(),
                        value: p.clone(),
                        reason: "附件路径含非法组件（绝对路径/./.. 等）".into(),
                    })
                }
            };
            if s.is_empty()
                || !s
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                || s == "."
                || s == ".."
            {
                return Err(CommandError::InvalidArgument {
                    field: "files.path".into(),
                    value: p.clone(),
                    reason: format!("附件路径组件「{s}」含不允许的字符"),
                });
            }
        }
        if pb
            .file_name()
            .map(|f| f.to_string_lossy().eq_ignore_ascii_case("skill.md"))
            .unwrap_or(false)
        {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "SKILL.md 由 content 参数提供，附件不得覆盖".into(),
            });
        }
        if c.len() > CREATE_MAX_FILE_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files.content".into(),
                value: p.clone(),
                reason: format!("单附件超过 {} 字节上限", CREATE_MAX_FILE_BYTES),
            });
        }
        total += c.len();
        if total > CREATE_MAX_TOTAL_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files".into(),
                value: p.clone(),
                reason: format!("附件总大小超过 {} 字节上限", CREATE_MAX_TOTAL_BYTES),
            });
        }
        cleaned.push((pb.to_path_buf(), c.as_str()));
    }

    // 排他占位（同 skills_import：并发双建只有一个成功）
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let dest = dir.join(name);
    std::fs::create_dir(&dest).map_err(|_| CommandError::SkillLoadFailed {
        name: name.to_string(),
        reason: "同名技能已存在：如需覆盖请先删除".into(),
    })?;
    let write_all = || -> Result<(), CommandError> {
        std::fs::write(dest.join("SKILL.md"), content)
            .map_err(|e| CommandError::IoError(e.to_string()))?;
        for (p, c) in &cleaned {
            let target = dest.join(p);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CommandError::IoError(e.to_string()))?;
            }
            std::fs::write(target, c).map_err(|e| CommandError::IoError(e.to_string()))?;
        }
        Ok(())
    };
    if let Err(e) = write_all() {
        // 创建已失败，回滚（清理半拷贝）再失败时两层信息都要让调用方看到——
        // 静默吞掉回滚失败会留下堵死重试的残骸且无迹可查
        if let Err(rb) = std::fs::remove_dir_all(&dest) {
            return Err(CommandError::IoError(format!(
                "{e}（回滚清理目录 {} 也失败：{rb}，请手动删除后重试）",
                dest.display()
            )));
        }
        return Err(e);
    }
    Ok(cleaned.len())
}

/// 机器人自建技能：把对话中沉淀的可复用流程固化为 SKILL.md（可带脚本/模板附件），
/// 创建即生效（路由重建）。与 skills_import 同一校验核/排他落盘/路由重建口径；
/// intents 危险词在创建时预检（同运行时 preflight 黑名单），并过 intents 宽度闸
/// （装上即进程级路由，过宽模式不得进）。
///
/// 落盘前有确认窗：技能装上即进**进程级**路由表，此后命中的用户消息会被直接
/// 送进该技能（正文进系统提示；mode=auto 时整段 DSL 无 LLM 参与地执行）。
/// 与 install_skill 同一姿态——「从公网下载要确认，本地写一个不要」不是设计。
pub async fn tool_create_skill(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let v = crate::bot::parse_args(args);
    let Some(name) = v["name"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("create_skill 缺少 name（技能名：字母/数字/-/_）".to_string());
    };
    if let Err(reason) = validate_skill_name(&name) {
        return ToolResult::denied(format!("create_skill：{reason}"));
    }
    let Some(content) = v["content"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied(
            "create_skill 缺少 content（完整 SKILL.md 文本，frontmatter 必含 description）"
                .to_string(),
        );
    };
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(arr) = v["files"].as_array() {
        for f in arr {
            match (f["path"].as_str(), f["content"].as_str()) {
                (Some(p), Some(c)) => files.push((p.to_string(), c.to_string())),
                _ => {
                    return ToolResult::denied("files 条目必须同时包含 path 与 content".to_string())
                }
            }
        }
    }

    // 准入核先行（解析 + 名称一致 + 危险词 + intents 宽度闸）：不合格的技能
    // 不该占用用户一次确认
    //
    // 口径：这些分支**工具根本没执行**（没落盘、没进路由表）——与工具层其余
    // 分支一致（缺 title、空 query、开关未开都是 denied），也与 install_skill
    // 对同一准入核的口径一致（那边是 denied）。走 error 会让「被闸拦下」记成
    // ERROR 进 error.log——被拒是预期内控制流，不是故障。
    let meta = match prepare_created_meta(&name, &content) {
        Ok(m) => m,
        Err(reason) => return ToolResult::denied(format!("create_skill：{reason}")),
    };
    // 会话档强制注入：模型 content 里写什么 scope 都会被覆盖成 session。
    // 工具来源的技能只在本会话路由（本会话立即生效、跨会话不劫持、重启即失效），
    // 想提升为全局由用户改 frontmatter 或走设置页导入——这层不由模型自选。
    // frontmatter 缺席在此分支实际到不了（准入核已验 description 在场），按
    // 「工具没执行 = denied」口径收口。
    let content = match crate::bot_skills::parse::force_frontmatter_session_scope(&content) {
        Some(c) => c,
        None => {
            return ToolResult::denied(
                "create_skill：技能内容没有可用的 frontmatter，无法标记会话档，未创建".to_string(),
            )
        }
    };
    // 确认窗展示 intents 与 mode：这两项决定「建成后哪些话会被路由进来」和
    // 「命中后是否绕开 LLM 直接执行」——不给用户看就等于让用户盲批
    let confirm_text = format!(
        "创建技能「{name}」并在**本会话**立即生效（仅本会话路由，重启后不再自动触发；如需长期启用请到设置页改技能的 scope）。触发词 intents：{}；执行模式 mode：{}（auto = 命中后不经 LLM 复核直接执行技能步骤）。",
        if meta.intents.is_empty() {
            "未声明（不会自动触发）".to_string()
        } else {
            meta.intents.join("、")
        },
        meta.mode
    );
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "create_skill",
        &confirm_text,
        interactive,
        session_id,
    )
    .await;
    if !approved {
        return ToolResult::denied("用户拒绝了创建，技能未创建".to_string());
    }

    let dir = skills_dir(app);
    let dir_log = dir.display().to_string();
    crate::bot::audit_log(
        app,
        &format!(
            "skill_create | {} | files: {}",
            crate::bot::escape_for_log(&name, 100),
            files.len()
        ),
    );
    let name_for_task = name.clone();
    let created = crate::py::document::spawn_blocking_map(move || {
        create_skill_at(&dir, &name_for_task, &content, &files).map_err(|e| e.to_string())
    })
    .await;
    match created {
        Ok(file_count) => {
            // 全局重建刷新既有技能的规则；新技能因 scope: session 被上面
            // intent_rules_from_dirs 的过滤跳过——它只经下面的会话档进路由。
            rebuild_intent_routes(app);
            // 会话档注册：本会话立即生效。session 缺席时不注册（技能在场但
            // 不可路由，等用户改 scope 显式提升）——None 会变成全局档，那是
            // 本设计明确要防的形态。
            if let Some(sid) = session_id {
                let mut patterns = Vec::new();
                if let Some(p) = crate::intent_router::explicit_name_pattern(&name) {
                    patterns.push(p);
                }
                patterns.extend(meta.intents.iter().cloned());
                if !patterns.is_empty() {
                    crate::intent_router::add_session_route(crate::intent_router::IntentRule {
                        patterns,
                        skill_name: name.clone(),
                        session: Some(sid.to_string()),
                    });
                }
            }
            ToolResult::ok(
                format!(
                    "技能「{name}」已创建，在本会话立即生效（仅本会话路由，重启后不再自动触发；如需长期启用请在设置页把该技能的 scope 改为全局）。附件 {file_count} 个，技能目录：{dir_log}。用户在本会话说出该技能 intents 关键词即会进入该技能流程。"
                ),
                Vec::new(),
            )
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "skill_create_failed | {} | {}",
                    crate::bot::escape_for_log(&name, 100),
                    crate::bot::escape_for_log(&e, 200)
                ),
            );
            ToolResult::error(format!("创建失败：{e}"), Vec::new())
        }
    }
}
