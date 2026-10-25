//! 技能解析的纯函数与集成测试。

use super::*;
use crate::bot_skills::{extract_task_id, substitute_vars, CompletedStep};

// ── 元数据解析 ──

#[test]
fn scope_session_key_parses_into_meta() {
    let m = parse_meta(
        "---\nname: a\ndescription: d\nscope: session\n---\n正文",
        "",
    );
    assert!(m.session_scoped, "scope: session 必须解析成会话档");
    let m2 = parse_meta("---\nname: a\ndescription: d\n---\n正文", "");
    assert!(!m2.session_scoped, "未声明 scope = 全局档（默认）");
    let m3 = parse_meta("---\nname: a\ndescription: d\nscope: global\n---\n正文", "");
    assert!(!m3.session_scoped, "scope: global 显式全局，不是会话档");
}

#[test]
fn force_session_scope_inserts_when_absent() {
    let src = "---\nname: a\ndescription: d\nintents:\n  - 润色文档\n---\n正文";
    let out = force_frontmatter_session_scope(src).expect("应注入成功");
    assert!(
        out.contains("scope: session"),
        "注入后的 frontmatter 必须含 scope: session"
    );
    let m = parse_meta(&out, "");
    assert!(m.session_scoped, "注入后必须能被解析器读回会话档");
    assert!(
        m.intents.contains(&"润色文档".to_string()),
        "注入不得劈坏多行 intents 块：{out}"
    );
}

#[test]
fn force_session_scope_overrides_any_existing_value() {
    let src = "---\nname: a\ndescription: d\nscope: global\n---\n正文";
    let out = force_frontmatter_session_scope(src).expect("应改写成功");
    assert!(
        !out.contains("scope: global"),
        "模型写的 scope 值必须被覆盖（会话档不是模型可自选项）"
    );
    assert!(parse_meta(&out, "").session_scoped);
}

#[test]
fn force_session_scope_rejects_frontmatterless_text() {
    assert!(
        force_frontmatter_session_scope("没有 frontmatter 的正文").is_none(),
        "无 frontmatter 无法标记，宁可失败也不静默放行"
    );
}

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
    // intents 多行 YAML 列表形式。
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
    assert_eq!(m.max_steps, 20); // 默认 = clamp 上限
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
fn meta_invalid_mode_does_not_block_low_auto_promotion() {
    // 非法 mode 按未声明处理：low 未显式声明 → auto 提升不被吞
    let m = parse_meta("---\nmode: bg\nrisk_level: low\n---\n", "d");
    assert_eq!(m.mode, "auto");
    // 合法显式声明保留（不受提升逻辑改写）
    let m2 = parse_meta("---\nmode: interactive\nrisk_level: low\n---\n", "d");
    assert_eq!(m2.mode, "interactive");
    let m3 = parse_meta("---\nmode: auto\nrisk_level: low\n---\n", "d");
    assert_eq!(m3.mode, "auto");
}

#[test]
fn meta_disabled_flag() {
    let t = "---\nenabled: false\n---\n";
    let m = parse_meta(t, "d");
    assert!(!m.enabled);
}

// ── enabled 键改写（设置页拨杆的落盘内核） ──

/// 改写后必须能被解析器读回目标值——这是「拨杆点了没反应」的唯一防线。
#[test]
fn set_enabled_flips_declared_value_and_keeps_other_keys() {
    let src = "---\nname: demo\ndescription: 演示\nenabled: true\nmode: auto\n---\n正文\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    let m = parse_meta(&out, "demo");
    assert!(!m.enabled, "关闭后解析器必须读到 false");
    assert_eq!(m.name, "demo");
    assert_eq!(m.description, "演示");
    assert_eq!(m.mode, "auto", "其他键不得被改写波及");
    assert!(out.contains("正文"), "正文必须原样保留");
}

#[test]
fn set_enabled_inserts_key_when_absent() {
    let src = "---\nname: demo\ndescription: 演示\n---\n正文\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(!parse_meta(&out, "demo").enabled);
    assert!(out.contains("enabled: false"), "应插入 enabled 键");
    assert_eq!(parse_meta(&out, "demo").name, "demo");
}

/// 未声明 enabled 的技能默认是启用的（parse_meta 默认 true），
/// 插入不能把「默认启用」误改成「显式禁用」。
#[test]
fn set_enabled_inserts_true_when_turning_on_undeclared_skill() {
    let src = "---\nname: demo\n---\n";
    let out = set_frontmatter_enabled(src, true).expect("应可改写");
    assert!(parse_meta(&out, "demo").enabled);
    assert!(out.contains("enabled: true"));
}

/// 完全没有 frontmatter 的文件：补一个只含 enabled 的 frontmatter，
/// 其余解析结果（name 回退目录名 / description 空）保持不变。
#[test]
fn set_enabled_creates_frontmatter_when_absent() {
    let src = "# 标题\n正文\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    let m = parse_meta(&out, "demo-dir");
    assert!(!m.enabled);
    assert_eq!(m.name, "demo-dir", "name 仍回退目录名");
    assert!(m.description.is_empty(), "description 仍为空");
    assert!(out.contains("# 标题"), "正文必须原样保留");
}

/// BOM 必须保留：带 BOM 的文件 frontmatter 会整体静默丢失（既有解析行为），
/// 写入侧不该顺手把文件的编码改了。
#[test]
fn set_enabled_preserves_bom() {
    let src = "\u{feff}---\nname: demo\nenabled: true\n---\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(out.starts_with('\u{feff}'), "BOM 必须保留");
    assert!(
        !parse_meta(&out, "demo").enabled,
        "BOM 存在时解析器仍须读到 false"
    );
}

/// CRLF 行尾：改写不得把 \r\n 变成 \n（Windows 上 SKILL.md 是 CRLF 的）。
#[test]
fn set_enabled_preserves_crlf() {
    let src = "---\r\nname: demo\r\nenabled: true\r\n---\r\n正文\r\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(out.contains("\r\n"), "CRLF 行尾必须保留");
    assert!(!out.contains("\n\n"), "不得混入裸 LF");
    assert!(!parse_meta(&out, "demo").enabled);
}

/// 解析器不剥行尾注释（`enabled: false  # 关闭原因` 的值不是 "false"
/// → 会被读成启用）。整行替换把值定死，不留注释。
#[test]
fn set_enabled_normalizes_line_with_trailing_comment() {
    let src = "---\nname: demo\nenabled: true  # 默认开\n---\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(
        !parse_meta(&out, "demo").enabled,
        "带注释的行必须读到 false"
    );
    assert!(!out.contains("# 默认开"), "整行替换不保留行尾注释");
}

/// 多行块（intents / params / allowed-tools）的续行不是顶层键——
/// 把块内的 `- enabled: xxx` 当成 enabled 键改掉 = 改坏了技能。
#[test]
fn set_enabled_does_not_touch_multiline_block_items() {
    let src = "---\nname: demo\nallowed-tools:\n  - enabled\n  - read_file\n---\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(out.contains("- enabled"), "块内条目不得被改写");
    assert!(out.contains("- read_file"), "块内条目不得被改写");
    assert!(!parse_meta(&out, "demo").enabled, "仍须写入顶层 enabled");
}

/// 同名键后者覆盖前者（parse_meta 逐行赋值）→ 改最后一个才与生效值对齐。
/// 用缩进区分两个同名键：改错了第一个，结果是「两个 false」；
/// 改对了最后一个，结果是「缩进的 true + 平的 false」。
#[test]
fn set_enabled_targets_the_last_declared_key() {
    let src = "---\nname: demo\n  enabled: true\nenabled: false\n---\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(
        out.contains("  enabled: true\nenabled: false"),
        "只应改最后一个 enabled 键（前者原样保留），got:\n{out}"
    );
    assert!(!parse_meta(&out, "demo").enabled, "最后一个键决定生效值");
}

/// 缩进保留：frontmatter 里的缩进风格不该被改写抹掉。
#[test]
fn set_enabled_preserves_indentation() {
    let src = "---\nname: demo\n  enabled: true\n---\n";
    let out = set_frontmatter_enabled(src, false).expect("应可改写");
    assert!(out.contains("  enabled: false"), "缩进必须保留");
    assert!(!parse_meta(&out, "demo").enabled);
}

/// frontmatter_enabled_declared：区分「没声明」与「声明为 true」
/// （内置技能物化时据此判断用户是不是手动关过它）。
#[test]
fn frontmatter_enabled_declared_distinguishes_absent_from_true() {
    assert_eq!(frontmatter_enabled_declared("---\nname: x\n---\n"), None);
    assert_eq!(
        frontmatter_enabled_declared("---\nname: x\nenabled: true\n---\n"),
        Some(true)
    );
    assert_eq!(
        frontmatter_enabled_declared("---\nname: x\nenabled: false\n---\n"),
        Some(false)
    );
    assert_eq!(
        frontmatter_enabled_declared("---\nname: x\nenabled: 0\n---\n"),
        Some(false),
        "0 / no / false 三种写法都算关闭（与 parse_meta 同口径）"
    );
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

// ── ：allowed-tools / version / params 解析 ──

#[test]
fn meta_allowed_tools_single_line_and_multiline() {
    let single = "---\nallowed-tools: read_text_file, grep_files\n---\n";
    let m = parse_meta(single, "d");
    assert_eq!(m.allowed_tools, vec!["read_text_file", "grep_files"]);
    let multi = "---\nallowed-tools:\n  - read_text_file\n  - edit_file\n---\n";
    let m2 = parse_meta(multi, "d");
    assert_eq!(m2.allowed_tools, vec!["read_text_file", "edit_file"]);
    // 未声明 = 空 = 不限制（向后兼容）
    assert!(parse_meta("---\nname: x\n---\n", "x")
        .allowed_tools
        .is_empty());
}

#[test]
fn meta_version_optional() {
    let m = parse_meta("---\nversion: 2.1.0\n---\n", "d");
    assert_eq!(m.version.as_deref(), Some("2.1.0"));
    assert!(parse_meta("---\nname: x\n---\n", "x").version.is_none());
}

#[test]
fn meta_params_multiline_with_required_and_default() {
    let text = "---\nname: code-fix\nparams:\n  - file: 目标文件绝对路径（必填）\n  - lang: 语言（默认 rust）\n  - note: 备注\n---\nbody\n";
    let m = parse_meta(text, "d");
    assert_eq!(m.params.len(), 3);
    assert_eq!(m.params[0].key, "file");
    assert!(m.params[0].required, "file 应为必填");
    assert_eq!(m.params[0].desc, "目标文件绝对路径");
    assert_eq!(m.params[1].key, "lang");
    assert!(!m.params[1].required);
    assert_eq!(m.params[1].default.as_deref(), Some("rust"));
    assert_eq!(m.params[1].desc, "语言");
    assert_eq!(m.params[2].key, "note");
    assert!(!m.params[2].required);
    assert!(m.params[2].default.is_none());
}

#[test]
fn meta_params_invalid_key_line_skipped() {
    // key 含非法字符的行跳过（不中断其余参数解析）
    let text = "---\nparams:\n  - bad-key!: x\n  - ok_field: 正常（必填）\n---\n";
    let m = parse_meta(text, "d");
    assert_eq!(m.params.len(), 1);
    assert_eq!(m.params[0].key, "ok_field");
}

#[test]
fn meta_params_half_width_parens_no_panic() {
    // 半角括号字节宽与全角不同（1 vs 3）：切片边界必须按命中字符实际宽度推进，
    // 之前硬编码全角宽度会把边界踩进多字节字符中间直接 panic
    let text = "---\nparams:\n  - lang: 语言 (默认 rust)\n---\n";
    let m = parse_meta(text, "d");
    assert_eq!(m.params.len(), 1);
    assert_eq!(m.params[0].default.as_deref(), Some("rust"));
    assert_eq!(m.params[0].desc, "语言");
}

#[test]
fn meta_params_plain_paren_with_moji_not_default() {
    // 说明文字里恰好含「默认」二字的普通括号注释不是默认值标记，不提取也不剥
    let text = "---\nparams:\n  - mode: 选择模式（本 skill 默认行为）\n---\n";
    let m = parse_meta(text, "d");
    assert_eq!(m.params.len(), 1);
    assert!(m.params[0].default.is_none());
    assert_eq!(m.params[0].desc, "选择模式（本 skill 默认行为）");
}

#[test]
fn frontmatter_parses_name_and_description() {
    let text = "---\nname: ppt-pro\ndescription: \"PPT 制作规范\"\n---\n# 正文\n步骤一\n";
    let m = parse_meta(text, "fallback");
    assert_eq!(m.name, "ppt-pro");
    assert_eq!(m.description, "PPT 制作规范");
}

#[test]
fn frontmatter_missing_falls_back_to_dir_name() {
    let m = parse_meta("没有 frontmatter", "dir-x");
    assert_eq!(m.name, "dir-x");
    assert_eq!(m.description, "");
}

#[test]
fn frontmatter_invalid_name_falls_back() {
    let text = "---\nname: \"../evil\"\ndescription: x\n---\n";
    let m = parse_meta(text, "dir-x");
    assert_eq!(m.name, "dir-x"); // 非法名（含路径字符）回退目录名
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

// ── DSL 解析 ──

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

// ── 样板 Skill 端到端 DSL 验证 ──

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
        // parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
        parsed: None,
    }];
    // Step 2 的 args_json 模板（与 SKILL.md 一致）
    let step2_args = r#"{"id": "${step1.id}"}"#;
    let resolved = substitute_vars(step2_args, &ctx);
    assert_eq!(resolved, format!(r#"{{"id": "{}"}}"#, step1_uuid));
}

// ── v2 升级样板 ──

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
    // rollback 段 args 也走变量替换
    // 模拟 Step 1 跑完后，回滚段用 ctx 里的 id 替换 ${step1.id}
    let ctx = vec![CompletedStep {
        index: 1,
        title: "list".into(),
        result: "r".into(),
        id: Some("uuid-99".into()),
        // parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
        parsed: None,
    }];
    let rb_args = r#"{"id": "${step1.id}"}"#;
    let resolved = substitute_vars(rb_args, &ctx);
    assert_eq!(resolved, r#"{"id": "uuid-99"}"#);
}

// ── 扫所有 Skill 目录通用验证 ──

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
        // interactive 技能（如 minimax-docx）没有 DSL Step 段——
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

/// `rollback: auto` 但正文没有 `## Rollback` 段 → 降为 `none`。
///
/// SKILL-DSL 承诺过这个降级，实现此前没做：声明了 auto 却一节回滚都没有，
/// 读起来却像「会回滚」。
#[test]
fn rollback_auto_without_a_rollback_section_downgrades_to_none() {
    let m = parse_meta("---\nrollback: auto\n---\n# 步骤\n", "d");
    assert_eq!(m.rollback, "auto", "frontmatter 单独解析时不该改判");

    // 正文没有回滚段 → 降为 none
    let downgraded = normalize_rollback_meta(m.clone(), "## Step 1: a\nquery_tasks({})\n");
    assert_eq!(downgraded.rollback, "none", "没有回滚段可跑 = 没有回滚");

    // 有回滚段 → 保持 auto
    let kept = normalize_rollback_meta(
        m.clone(),
        "## Step 1: a\nquery_tasks({})\n\n## Rollback\ndelete_task({})\n",
    );
    assert_eq!(kept.rollback, "auto");

    // 中文标题的段落也认（判据单源在 is_rollback_heading）
    let kept_cn = normalize_rollback_meta(
        m,
        "## Step 1: a\nquery_tasks({})\n\n## 回滚\ndelete_task({})\n",
    );
    assert_eq!(kept_cn.rollback, "auto");
}
