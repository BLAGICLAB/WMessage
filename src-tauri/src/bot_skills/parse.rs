pub(crate) const SKILL_NAME_CHARS_OK: fn(char) -> bool = |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';

/// SKILL.md 完整元数据（Skill 运行模型 v1.0 字段集，全部带默认值）
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub risk_level: String, // low | medium | high
    pub mode: String,       // auto | interactive
    pub max_steps: usize,
    pub timeout_secs: u64,
    pub rollback: String, // none | auto
    pub enabled: bool,
    /// 是否支持暂停后断点续跑（false 时暂停即终止：确认完成当前动作后技能结束）
    pub resumable: bool,
    pub intents: Vec<String>,
    /// 多步 Skill 自报的对话轮数上限（2026-08-20）；
    /// None → 运行时 fallback bot_model_loop::DEFAULT_MAX_ROUNDS（20）。
    /// 解析时 clamp 到 1..=60（现有 Skill 未声明该字段 → None，不受影响）。
    pub max_rounds: Option<usize>,
}

impl Default for SkillMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            risk_level: "medium".into(),
            mode: "interactive".into(),
            max_steps: 8,
            timeout_secs: 180,
            rollback: "none".into(),
            enabled: true,
            resumable: false,
            intents: Vec::new(),
            max_rounds: None,
        }
    }
}

pub(crate) fn parse_frontmatter(text: &str, dir_name: &str) -> (String, String) {
    let meta = parse_meta(text, dir_name);
    (meta.name, meta.description)
}

/// 解析完整元数据（调度器用）：缺失字段走默认值；非法值回退默认
pub fn parse_meta(text: &str, dir_name: &str) -> SkillMeta {
    let mut m = SkillMeta::default();
    let body = text.strip_prefix("---").unwrap_or(text);
    let Some(end) = body.find("\n---") else {
        m.name = dir_name.to_string();
        return m;
    };
    let fm = &body[..end];
    let fm_lines: Vec<&str> = fm.lines().collect();
    let mut li = 0;
    while li < fm_lines.len() {
        let line = fm_lines[li].trim();
        li += 1;
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "name" => m.name = v.to_string(),
            "description" => m.description = v.to_string(),
            "risk_level" => {
                let v2 = v.to_ascii_lowercase();
                m.risk_level = if matches!(v2.as_str(), "low" | "medium" | "high") {
                    v2
                } else {
                    "medium".into()
                }
            }
            "mode" => {
                let v2 = v.to_ascii_lowercase();
                m.mode = if matches!(v2.as_str(), "auto" | "interactive") {
                    v2
                } else {
                    "interactive".into()
                }
            }
            "max_steps" => {
                if let Ok(n) = v.parse::<usize>() {
                    m.max_steps = n.clamp(1, 20);
                }
            }
            "timeout_secs" => {
                if let Ok(n) = v.parse::<u64>() {
                    m.timeout_secs = n.clamp(10, 600);
                }
            }
            "max_rounds" => {
                if let Ok(n) = v.parse::<usize>() {
                    m.max_rounds = Some(n.clamp(1, 60));
                }
            }
            "rollback" => {
                let v2 = v.to_ascii_lowercase();
                m.rollback = if matches!(v2.as_str(), "none" | "auto") {
                    v2
                } else {
                    "none".into()
                }
            }
            "enabled" => {
                m.enabled = !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no");
            }
            "resumable" => {
                m.resumable =
                    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes");
            }
            "intents" => {
                let inner = v.trim();
                if inner.is_empty() {
                    // 多行 YAML 列表（2026-08-19 动态路由）：intents: 后随 `- ` 行逐条收取。
                    // 模式含逗号（如量词 {0,15}）必须用此形式——单行逗号分隔会切碎量词。
                    while li < fm_lines.len() {
                        let t = fm_lines[li].trim();
                        let Some(item) = t.strip_prefix("- ") else {
                            break;
                        };
                        let item = item.trim().trim_matches('"').trim_matches('\'');
                        if !item.is_empty() {
                            m.intents.push(item.to_string());
                        }
                        li += 1;
                    }
                } else {
                    // 单行：["a","b"] 或逗号分隔 "a,b"（模式内不得含逗号，含逗号请用多行列表）
                    m.intents = inner
                        .trim_matches(['[', ']'])
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
            _ => {}
        }
    }
    if m.name.is_empty() || !m.name.chars().all(SKILL_NAME_CHARS_OK) {
        m.name = dir_name.to_string();
    }
    if m.name.is_empty() {
        m.name = dir_name.to_string();
    }
    // 风险推导模式（显式 mode 优先）：high/medium → interactive；low → auto
    // 元数据里 mode 未显式标记时按风险推导：这里无法区分"显式"与否（默认已是 interactive），
    // 保持解析值即可；high 风险强制 interactive（安全兜底）
    if m.risk_level == "high" {
        m.mode = "interactive".into();
    }
    m
}

// ─────────────────────── DSL 解析 + 调度器（Phase 1 2026-08-17 23:15）───────────────────────

/// Skill 步骤（DSL 解析后的结构，Phase 1）
///
/// Markdown DSL 格式（仅 `mode: "auto"` 的 Skill 走调度器）：
///   ## Step N: 标题
///   tool_name({"arg": "value"})
///   ## Rollback
///   undo_tool({"arg": "value"})
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillStep {
    /// 1-based 步骤序号（用于错误信息 / 回滚顺序）
    pub index: usize,
    pub title: String,
    /// 工具名（必须存在于 TOOLS 白名单）
    pub tool_name: String,
    /// JSON 参数串（直接透传给 `bot::execute_tool`）
    pub args_json: String,
}

/// 剥离 YAML frontmatter（`---` ... `---`），返回正文部分。
fn strip_frontmatter(text: &str) -> &str {
    if !text.starts_with("---") {
        return text;
    }
    let body = &text[3..];
    match body.find("\n---") {
        Some(end) => body[end + 4..].trim_start_matches('\n'),
        None => text,
    }
}

/// 解析 `## Step N: 标题` 或 `## Step N 标题` → `(序号, 标题)`
fn parse_step_heading(line: &str) -> Option<(usize, String)> {
    let rest = line.trim().strip_prefix("## Step ")?;
    let (num_str, title) = if let Some(colon) = rest.find(':') {
        (&rest[..colon], rest[colon + 1..].trim())
    } else if let Some(space) = rest.find(' ') {
        (&rest[..space], rest[space + 1..].trim())
    } else {
        (rest, "")
    };
    let num: usize = num_str.trim().parse().ok()?;
    Some((num, title.to_string()))
}

/// 解析一行工具调用：`tool_name({"arg": "value"})` → `(name, args)`
/// 约束：单行、结尾 `)`、name 为 ASCII 字母数字下划线；args 可含任意字符（含嵌套括号）
fn parse_tool_call(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if !line.ends_with(')') {
        return None;
    }
    let open = line.find('(')?;
    let name = line[..open].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let args = &line[open + 1..line.len() - 1];
    Some((name.to_string(), args.to_string()))
}

/// 解析 Skill body 为 (主步骤, 回滚步骤)。
/// - 空 body / 无 step → `(Vec::new(), Vec::new())`
/// - 解析失败 → Err
pub fn parse_skill_steps(body: &str) -> Result<(Vec<SkillStep>, Vec<SkillStep>), String> {
    let body = strip_frontmatter(body);
    let mut steps: Vec<SkillStep> = Vec::new();
    let mut rollback: Vec<SkillStep> = Vec::new();
    let mut current: Option<SkillStep> = None;
    let mut current_rollback: Option<SkillStep> = None;
    let mut mode: &str = "step"; // "step" | "rollback"

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((idx, title)) = parse_step_heading(line) {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            if let Some(r) = current_rollback.take() {
                rollback.push(r);
            }
            current = Some(SkillStep {
                index: idx,
                title,
                tool_name: String::new(),
                args_json: String::new(),
            });
            mode = "step";
        } else if line == "## Rollback" || line.starts_with("## Rollback ") {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            current_rollback = Some(SkillStep {
                index: 0,
                title: "rollback".into(),
                tool_name: String::new(),
                args_json: String::new(),
            });
            mode = "rollback";
        } else if line.starts_with('#') {
            continue;
        } else if let Some((name, args)) = parse_tool_call(line) {
            let target = if mode == "step" {
                &mut current
            } else {
                &mut current_rollback
            };
            if let Some(s) = target.as_mut() {
                s.tool_name = name;
                s.args_json = args;
            }
        }
    }
    if let Some(s) = current {
        steps.push(s);
    }
    if let Some(r) = current_rollback {
        rollback.push(r);
    }
    Ok((steps, rollback))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_skills::{extract_task_id, substitute_vars, CompletedStep};

    // ── 元数据解析 ──

    #[test]
    fn meta_parses_full_fields() {
        let text = "---\nname: daily-archive\ndescription: 汇总归档\nrisk_level: medium\nmode: interactive\nmax_steps: 6\ntimeout_secs: 120\nrollback: auto\nenabled: true\nintents: [\"日报\",\"归档\"]\n---\n# 步骤\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(m.name, "daily-archive");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.mode, "interactive");
        assert_eq!(m.max_steps, 6);
        assert_eq!(m.timeout_secs, 120);
        assert_eq!(m.rollback, "auto");
        assert!(m.enabled);
        assert_eq!(m.intents, vec!["日报", "归档"]);
    }

    #[test]
    fn meta_intents_multiline_list_preserves_quantifier_commas() {
        // 2026-08-19 动态路由：intents 多行 YAML 列表形式。
        // 关键：模式内的量词逗号（{0,15}）不得被切碎——单行逗号分隔形式做不到，所以路由模式用多行列表
        let text = "---\nname: minimax-docx\ndescription: d\ntriggers:\n  - Word\n  - 文档\nintents:\n  - '(?i)(润色|修订).{0,15}(word|文档)'\n  - '(?is)(润色|修订)[\\s\\S]*\\.docx?'\n---\n# body\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(
            m.intents,
            vec![
                r"(?i)(润色|修订).{0,15}(word|文档)",
                r"(?is)(润色|修订)[\s\S]*\.docx?",
            ]
        );
        // triggers 列表行不得混入 intents（intents 之前的 `- ` 行不属于本字段）
        assert!(!m.intents.iter().any(|i| i == "Word"));
    }

    #[test]
    fn meta_intents_empty_without_declaration() {
        // 未声明 intents → 空（动态路由下该技能不产生路由条目）
        let m = parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert!(m.intents.is_empty());
    }

    #[test]
    fn meta_defaults_and_clamps() {
        let m = parse_meta("没有 frontmatter", "dir-x");
        assert_eq!(m.name, "dir-x");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.max_steps, 8);
        assert_eq!(m.timeout_secs, 180);
        assert!(!m.rollback.eq("auto"));

        // 非法值回退 + 越界 clamp
        let t = "---\nrisk_level: dangerous\nmode: auto\nmax_steps: 999\ntimeout_secs: 1\n---\n";
        let m2 = parse_meta(t, "d");
        assert_eq!(m2.risk_level, "medium"); // 非法回退
        assert_eq!(m2.mode, "auto"); // 合法保留
        assert_eq!(m2.max_steps, 20); // clamp 上限
        assert_eq!(m2.timeout_secs, 10); // clamp 下限
    }

    #[test]
    fn meta_high_risk_forces_interactive() {
        let t = "---\nrisk_level: high\nmode: auto\n---\n";
        let m = parse_meta(t, "d");
        assert_eq!(m.mode, "interactive"); // 安全兜底：high 强制人机协同
    }

    #[test]
    fn meta_disabled_flag() {
        let t = "---\nenabled: false\n---\n";
        let m = parse_meta(t, "d");
        assert!(!m.enabled);
    }

    #[test]
    fn meta_parses_resumable() {
        let t = "---\nresumable: true\n---\n";
        let m = parse_meta(t, "d");
        assert!(m.resumable);
        let t2 = "---\nresumable: false\n---\n";
        assert!(!parse_meta(t2, "d").resumable);
        assert!(!parse_meta("无 frontmatter", "d").resumable); // 默认 false
    }

    #[test]
    fn frontmatter_parses_name_and_description() {
        let text = "---\nname: ppt-pro\ndescription: \"PPT 制作规范\"\n---\n# 正文\n步骤一\n";
        let (n, d) = parse_frontmatter(text, "fallback");
        assert_eq!(n, "ppt-pro");
        assert_eq!(d, "PPT 制作规范");
    }

    #[test]
    fn frontmatter_missing_falls_back_to_dir_name() {
        let (n, d) = parse_frontmatter("没有 frontmatter", "dir-x");
        assert_eq!(n, "dir-x");
        assert_eq!(d, "");
    }

    #[test]
    fn frontmatter_invalid_name_falls_back() {
        let text = "---\nname: \"../evil\"\ndescription: x\n---\n";
        let (n, _) = parse_frontmatter(text, "dir-x");
        assert_eq!(n, "dir-x"); // 非法名（含路径字符）回退目录名
    }

    #[test]
    fn read_skill_rejects_path_traversal() {
        // 直接验证 name 校验函数逻辑（无需 AppHandle）
        let ok = |s: &str| !s.is_empty() && s.chars().all(SKILL_NAME_CHARS_OK);
        assert!(ok("ppt-pro"));
        assert!(ok("minimax_docx"));
        assert!(!ok("../evil"));
        assert!(!ok("a/b"));
        assert!(!ok(""));
    }

    // ── DSL 解析（Phase 1 2026-08-17 23:15） ──

    #[test]
    fn dsl_parse_empty_body() {
        let (steps, rollback) = parse_skill_steps("").unwrap();
        assert!(steps.is_empty());
        assert!(rollback.is_empty());
    }

    #[test]
    fn dsl_parse_single_step() {
        let body = "## Step 1: 列出任务\nlist_tasks({})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(rb.is_empty());
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].title, "列出任务");
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
    }

    #[test]
    fn dsl_parse_multiple_steps_preserves_order() {
        let body = "## Step 1: 第一步\nfoo({})\n\n## Step 2: 第二步\nbar({\"x\": \"y\"})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].tool_name, "foo");
        assert_eq!(steps[1].index, 2);
        assert_eq!(steps[1].tool_name, "bar");
        assert_eq!(steps[1].args_json, "{\"x\": \"y\"}");
    }

    #[test]
    fn dsl_parse_with_rollback_section() {
        let body = "## Step 1: 行动\ndo({\"k\": \"v\"})\n\n## Rollback\nundo({\"k\": \"v\"})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(rb.len(), 1);
        assert_eq!(rb[0].tool_name, "undo");
        assert_eq!(rb[0].args_json, "{\"k\": \"v\"}");
    }

    #[test]
    fn dsl_parse_strips_frontmatter() {
        let body = "---\nname: test\ndescription: 解析测试\n---\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_ignores_free_text_comments() {
        // 自由文字 / 注释行应被忽略，不破坏解析
        let body = "# 标题\n\n这是自由文字说明\n\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_tool_call_rejects_bad_name() {
        // 工具名含非法字符（不是 ASCII 字母数字下划线）应被拒
        assert!(parse_tool_call("foo-bar({})").is_none());
        assert!(parse_tool_call("foo bar({})").is_none());
        // 不以 ) 结尾也应被拒
        assert!(parse_tool_call("foo({}").is_none());
        assert!(parse_tool_call("foo{}").is_none());
        // 合法 name（含数字）应通过
        assert!(parse_tool_call("foo123({})").is_some());
    }

    // ── Phase 3 样板 Skill 端到端 DSL 验证（2026-08-18 05:30） ──

    /// 样板 SKILL.md 的 body（与 target/debug/skills/minimax-task-archive-demo/SKILL.md 同源）
    const DEMO_BODY: &str = "---\n\
name: minimax-task-archive-demo\n\
description: Phase 3 样板\n\
---\n\
# 任务归档演示\n\
\n\
演示 2 步 DSL。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn demo_skill_parses_two_steps_no_rollback() {
        // 样板 Skill 应被解析为 2 个 step，无 rollback 段
        let (steps, rollback) = parse_skill_steps(DEMO_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(rollback.is_empty());
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn demo_skill_substitution_chain_resolves_step1_id() {
        // 模拟调度器跑完 Step 1 → Step 2 之前的 ctx 状态：
        // Step 1 工具返回文本含 UUID → extract_task_id 拿到 id → push 进 ctx
        // Step 2 执行前调 substitute_vars → ${step1.id} 应被替换成真实 UUID
        let step1_text = "当前 3 张任务卡：\n\
- 7c9e6679-7425-40de-944b-e07fc1f90ae7 买牛奶\n\
- 11111111-2222-3333-4444-555555555555 写报告\n\
- 66666666-7777-8888-9999-000000000000 周会准备";
        let step1_uuid = extract_task_id(step1_text).unwrap();
        assert_eq!(step1_uuid, "7c9e6679-7425-40de-944b-e07fc1f90ae7");

        let ctx = vec![CompletedStep {
            index: 1,
            title: "列出当前所有任务".into(),
            result: step1_text.into(),
            id: Some(step1_uuid.clone()),
            // Phase 4 第 2 项：parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        // Step 2 的 args_json 模板（与 SKILL.md 一致）
        let step2_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(step2_args, &ctx);
        assert_eq!(resolved, format!(r#"{{"id": "{}"}}"#, step1_uuid));
    }

    // ── Phase 3 第二步（2026-08-18 05:33）：v2 升级样板 ──

    /// v2 SKILL.md 的 body（与 target/debug/skills/minimax-task-summary-v2/SKILL.md 同源）
    const V2_BODY: &str = "---\n\
name: minimax-task-summary-v2\n\
description: v2 升级\n\
rollback: auto\n\
---\n\
# 任务汇总 v2\n\
\n\
v2 相对 v1 改进：加 rollback 段。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n\
\n\
## Rollback\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn v2_parses_two_steps_one_rollback() {
        // v2 应被解析为 2 step + 1 rollback step
        let (steps, rollback) = parse_skill_steps(V2_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(rollback.len(), 1);
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
        // rollback 段也是 query_single_task，与主 step 复用同样变量替换路径
        assert_eq!(rollback[0].tool_name, "query_single_task");
        assert_eq!(rollback[0].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn v2_rollback_step_args_also_go_through_substitute_vars() {
        // rollback 段 args 也走变量替换（Phase 2 已接入 run_skill_scheduler）
        // 模拟 Step 1 跑完后，回滚段用 ctx 里的 id 替换 ${step1.id}
        let ctx = vec![CompletedStep {
            index: 1,
            title: "list".into(),
            result: "r".into(),
            id: Some("uuid-99".into()),
            // Phase 4 第 2 项：parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        let rb_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(rb_args, &ctx);
        assert_eq!(resolved, r#"{"id": "uuid-99"}"#);
    }

    // ── Phase 3 第三步（2026-08-18 06:10）：扫所有 Skill 目录通用验证 ──

    /// 扫 target/debug/skills/ 下所有 SKILL.md，验证 parse_skill_steps 通过
    /// 且至少 1 step，rollback 段（若有）也合法。
    /// 不依赖具体某个 Skill 文件——sub-agent 写完任意 Skill 后，这个测试自动覆盖。
    #[test]
    fn scan_all_skills_in_debug_dir_parse_correctly() {
        let skill_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("skills");
        let mut found = 0;
        let read_dir = match std::fs::read_dir(&skill_dir) {
            Ok(d) => d,
            Err(_) => {
                // 目录不存在不报错（开发期可能未初始化）
                eprintln!("skill dir 不存在，跳过扫瞄：{}", skill_dir.display());
                return;
            }
        };
        for entry in read_dir.flatten() {
            let skill_md = entry.path().join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let body = std::fs::read_to_string(&skill_md)
                .unwrap_or_else(|e| panic!("read {} failed: {}", skill_md.display(), e));
            // 2026-08-19：interactive 技能（如 minimax-docx）没有 DSL Step 段——
            // 它由 LLM 驱动、不进调度器，DSL 校验只针对 mode: auto 的技能
            if parse_meta(&body, "smoke").mode != "auto" {
                continue;
            }
            let (steps, rollback) = parse_skill_steps(&body).unwrap_or_else(|e| {
                panic!(
                    "parse_skill_steps failed for {}: {}",
                    entry.path().display(),
                    e
                )
            });
            assert!(!steps.is_empty(), "{} 没有 step", entry.path().display());
            for (i, step) in steps.iter().enumerate() {
                assert!(
                    !step.tool_name.is_empty(),
                    "{} step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            for (i, rb) in rollback.iter().enumerate() {
                assert!(
                    !rb.tool_name.is_empty(),
                    "{} rollback step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            found += 1;
        }
        // 不强制 Skill 数 — 任意现存 Skill 都应被正确解析
        if found == 0 {
            // 目录存在但为空（cargo clean 误删 dev mock / dev 首次未 init）—— graceful skip
            eprintln!("skill dir 为空，跳过扫瞄：{}", skill_dir.display());
            return;
        }
        eprintln!("扫瞄 {} 个 Skill，全部解析通过", found);
    }

}
