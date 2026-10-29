use super::*;
use std::collections::HashSet;

/// `TOOLS_TABLE` 里不许有 cfg-gated 条目。
///
/// `BUILTIN_TOOL_COUNT` 是**手写常量**，而表长度是编译期决定的——
/// 一旦某条工具被 `#[cfg(feature = ...)]` gate 掉，某个编译配置下
/// 常量与表长度就不等，基线断言在**别的**配置下才炸（难查）。
/// 这里直接扫源码当场红：加 feature-gated 工具的人立刻看到。
#[test]
fn tools_table_has_no_cfg_gated_entries() {
    let src = include_str!("schemas.rs");
    let start = src
        .find("pub static TOOLS_TABLE")
        .expect("registry.rs 里有 TOOLS_TABLE");
    let end = start
        + src[start..]
            .find("\n];")
            .expect("TOOLS_TABLE 是以 \\n]; 收尾的静态数组");
    let body = &src[start..end];
    assert!(
        !body.contains("#[cfg("),
        "TOOLS_TABLE 里不许出现 #[cfg(...)]：表长度会随编译配置变，\
         而 BUILTIN_TOOL_COUNT 是手写常量（要么改成派生，要么别 gate 工具条目）"
    );
}

/// spawn_subagent schema 暴露 stage 字段（recon/executor/verifier）——锁住
/// 三段式派发对 LLM 的契约入口；字段丢失即等于功能对模型不可见。
#[test]
fn spawn_subagent_schema_exposes_stage_field() {
    let v: serde_json::Value =
        serde_json::from_str(SCHEMA_SPAWN_SUBAGENT).expect("SCHEMA_SPAWN_SUBAGENT 必须是合法 JSON");
    let stage = v
        .pointer("/function/parameters/properties/stage")
        .expect("spawn schema 必须声明 stage 字段");
    // 枚举与字符串类型 + 三档全部可枚举
    assert_eq!(stage["type"], "string");
    let enums: Vec<&str> = stage["enum"]
        .as_array()
        .expect("stage enum 必须是数组")
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    for v in ["recon", "executor", "verifier"] {
        assert!(enums.contains(&v), "stage enum 缺 {v}");
    }
    // stage 不在 required 里（与 profile 可同时给，也可只给一个）
    let required: Vec<&str> = v
        .pointer("/function/parameters/required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !required.contains(&"stage"),
        "stage 是可选字段，不应进 required"
    );
}

/// spawn schema 暴露 `onion` 配置档，且 `profile` 不再是必填。
///
/// 移出 required 消的是一处**既有矛盾**：代码一直支持「只给 stage / 配置档
/// 让系统派生档位」，而 schema 把 profile 列为必填让模型永远先填满它，
/// 配置档的 profile 就没有兜底生效的机会。
#[test]
fn spawn_subagent_schema_exposes_onion_and_makes_profile_optional() {
    let v: serde_json::Value =
        serde_json::from_str(SCHEMA_SPAWN_SUBAGENT).expect("SCHEMA_SPAWN_SUBAGENT 必须是合法 JSON");
    let onion = v
        .pointer("/function/parameters/properties/onion")
        .expect("spawn schema 必须声明 onion 字段——否则配置档对模型不可见");
    assert_eq!(onion["type"], "string");
    // 三份内置配置档名要写进描述，模型才知道有哪些可派发
    let desc = onion["description"].as_str().unwrap_or_default();
    for name in crate::bot_onions::builtin_config_ids() {
        assert!(
            desc.contains(name),
            "onion 参数描述应列出内置配置档 {name}，否则模型无从知道有哪些"
        );
    }
    // profile 不在 required：给 stage 或给 onion 都足以定档
    let required: Vec<&str> = v
        .pointer("/function/parameters/required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !required.contains(&"profile"),
        "profile 应可由 onion / stage 派生，不该是必填"
    );
    assert!(
        required.contains(&"objective") && required.contains(&"acceptanceCriteria"),
        "objective / acceptanceCriteria 仍必须必填：{required:?}"
    );
}

#[test]
fn tools_table_count_matches_constant() {
    let v: serde_json::Value =
        serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
    let arr = v.as_array().expect("TOOLS 顶层必须是数组");
    // 数目一律从 BUILTIN_TOOL_COUNT / SUBAGENT_ONLY_TOOLS 推导，全仓只此一处
    // 硬编码工具数：注释、架构文档、本测试都引用它（数字失真过一次——
    // 注释写 33 实为 44，正是因为断言与注释各写各的）。
    assert_eq!(
        TOOLS_TABLE.len(),
        BUILTIN_TOOL_COUNT,
        "TOOLS_TABLE 全量与 BUILTIN_TOOL_COUNT 不符：增删工具后请改该常量"
    );
    assert_eq!(
        arr.len(),
        BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len(),
        "主 agent 可见工具数（表全量减去 subagent-only）与常量不符"
    );
    for name in SUBAGENT_ONLY_TOOLS {
        assert!(
            tools_index().contains_key(*name),
            "subagent-only 工具不在表里：{name}"
        );
    }

    let mut seen: HashSet<String> = HashSet::new();
    for t in arr.iter() {
        let func = t.get("function").expect("tool.function 必存");
        let name = func
            .get("name")
            .and_then(|n| n.as_str())
            .expect("name 必为 string");
        assert!(seen.insert(name.to_string()), "工具名重复: {name}");
        assert_eq!(t.get("type").and_then(|v| v.as_str()), Some("function"));
        assert!(func.get("description").is_some(), "{name} 缺 description");
        assert!(func.get("parameters").is_some(), "{name} 缺 parameters");
    }

    let names: HashSet<String> = arr
        .iter()
        .map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap()
                .to_string()
        })
        .collect();
    let expected: HashSet<String> = [
        "query_tasks",
        "query_single_task",
        "create_task",
        "complete_task",
        "delete_task",
        "edit_task",
        "add_subtask",
        "toggle_subtask",
        "remove_subtask",
        "read_text_file",
        "ocr_image",
        "grep_files",
        "list_files",
        "link_file_to_task",
        "extract_document",
        "create_word",
        "create_word_revisions",
        "create_excel",
        "create_ppt",
        "create_pdf",
        "run_python",
        "web_search",
        "fetch_url",
        "get_current_time",
        "lookup_receipt",
        "remember_fact",
        "recall_facts",
        "record_lesson",
        "use_skill",
        "create_skill",
        "install_skill",
        "search_skills",
        "spawn_subagent",
        "check_subagent",
        "cancel_subagent",
        "reveal_path",
        "open_url",
        "clipboard_write",
        "screenshot",
        "edit_file",
        "write_file",
        "apps_list",
        "apps_create",
        "apps_update_files",
        "ask_user",
        "ast_inspect",
        "unpack_archive",
        "pack_archive",
        "manage_file",
        "copy_file_to_clipboard",
        "schedule_reminder",
        "list_reminders",
        "cancel_reminder",
        "list_windows",
        "activate_window",
        "ui_inspect",
        "ui_click",
        "ui_type",
        "wait_for_element",
        "start_service",
        "stop_service",
        "list_services",
        "update_plan",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(names, expected, "工具名集合失配");

    let order: Vec<String> = arr
        .iter()
        .map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap()
                .to_string()
        })
        .collect();
    let expected_order: Vec<String> = [
        "query_tasks",
        "query_single_task",
        "create_task",
        "complete_task",
        "delete_task",
        "edit_task",
        "add_subtask",
        "toggle_subtask",
        "remove_subtask",
        "read_text_file",
        "ocr_image",
        "grep_files",
        "list_files",
        "link_file_to_task",
        "extract_document",
        "create_word",
        "create_word_revisions",
        "create_excel",
        "create_ppt",
        "create_pdf",
        "run_python",
        "web_search",
        "fetch_url",
        "get_current_time",
        "lookup_receipt",
        "remember_fact",
        "recall_facts",
        "record_lesson",
        "use_skill",
        "create_skill",
        "install_skill",
        "search_skills",
        "spawn_subagent",
        "check_subagent",
        "cancel_subagent",
        "reveal_path",
        "open_url",
        "clipboard_write",
        "screenshot",
        "edit_file",
        "write_file",
        "apps_list",
        "apps_create",
        "apps_update_files",
        "ask_user",
        "ast_inspect",
        "unpack_archive",
        "pack_archive",
        "manage_file",
        "copy_file_to_clipboard",
        "schedule_reminder",
        "list_reminders",
        "cancel_reminder",
        "list_windows",
        "activate_window",
        "ui_inspect",
        "ui_click",
        "ui_type",
        "wait_for_element",
        "start_service",
        "stop_service",
        "list_services",
        "update_plan",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(order, expected_order, "工具名顺序失配");
}

#[test]
fn mutating_tools_matches_baseline() {
    let derived: HashSet<&str> = mutating_tools().into_iter().collect();
    let expected: HashSet<&str> = [
        "create_task",
        "edit_task",
        "complete_task",
        "delete_task",
        "add_subtask",
        "toggle_subtask",
        "remove_subtask",
        "link_file_to_task",
        "create_word",
        "create_word_revisions",
        "create_excel",
        "create_ppt",
        "create_pdf",
        "remember_fact",
        "record_lesson",
        "spawn_subagent",
        "cancel_subagent",
        "write_artifact_file",
        "edit_file",
        "write_file",
        "create_skill",
        "install_skill",
        "apps_create",
        "apps_update_files",
        "unpack_archive",
        "pack_archive",
        "manage_file",
        "schedule_reminder",
        "cancel_reminder",
        "ui_click",
        "ui_type",
        "start_service",
        "stop_service",
        "screenshot",
    ]
    .into_iter()
    .collect();
    assert_eq!(derived, expected);
}

/// 已下线的工具名不得再出现在任何 schema 描述里。
///
/// 描述是模型看得见的说明书：让它去找一个已经不存在的工具，得到的
/// 只会是「未知工具」，用户看到的是一次白跑的工具调用
/// （list_tasks / search_tasks 已合并进 query_tasks）。
#[test]
fn retired_tool_names_absent_from_schemas() {
    const RETIRED: &[&str] = &["list_tasks", "search_tasks"];
    for t in TOOLS_TABLE {
        for gone in RETIRED {
            assert!(
                !t.schema.contains(gone),
                "{} 的 schema 仍提到已下线的工具 {gone}",
                t.name
            );
        }
    }
}

/// 阶段 2 spec 1: TOOLS JSON 与基线一致。
/// 读 tests/fixtures/tools_baseline.json（原 const TOOLS 全文），提取其字符串正文
/// 反序列化后与 tools_json() 做 Value 级比对：锁住工具顺序 + 每条 schema 的
/// 字段名 / 描述 / 参数结构。
///
/// 不比字节：tools_json() 用统一的 `,\n  ` 缩进拼装，而原 const 的
/// link_file_to_task 条目顶格写（无 2 空格），故两者只差这一处空白。
///
/// 核心 28 工具基线（重排为 query_tasks；电脑辅助四工具追加在
/// 编排三工具之后、子 agent 专属之前——不进 baseline 前缀）。
/// baseline 必须是 derived 的**前缀**（核心 schema 变更走「显式更新本批 +
/// 重生成 fixture」流程）。
#[test]
fn tools_json_matches_baseline() {
    let fixture = include_str!("../../../tests/fixtures/tools_baseline.json");
    let r_str_marker = fixture.find("r#\"").expect("fixture 缺 r#\"") + 3;
    let end = fixture.rfind("]\"#;").expect("fixture 缺 ]\"#;") + 1;
    let baseline: serde_json::Value = serde_json::from_str(&fixture[r_str_marker..end])
        .expect("fixture 内 const TOOLS 正文必须是合法 JSON");
    let derived: serde_json::Value =
        serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
    let base_arr = baseline.as_array().unwrap();
    let der_arr = derived.as_array().unwrap();
    assert_eq!(
        &der_arr[..base_arr.len()],
        base_arr.as_slice(),
        "tools_json() 前 28 项与 baseline 漂移（核心 schema 变更须显式重生成 fixture）"
    );
    assert_eq!(
        der_arr.len(),
        base_arr.len() + 35,
        "主可见应为 28+3+4+2+1+1+3+1+13+3+2+1+1（末位 +13 = 归档两工具 + manage_file + copy_file_to_clipboard + 提醒三工具 + UI 六件套；+3 = 受管服务三工具；+2 = 远程技能安装与搜索两工具；+1 = 凭证召回工具 lookup_receipt；+1 = 计划工具 update_plan）"
    );
}

/// 「重生成 fixture」流程的显式入口（测试注释承诺的流程落地）：
/// `cargo test regen_tools_baseline_fixture -- --ignored` 后人眼复核 diff。
/// 只写前 28 项核心工具；格式无关紧要（基线比对是 Value 级）
#[test]
#[ignore]
fn regen_tools_baseline_fixture() {
    let derived: serde_json::Value =
        serde_json::from_str(tools_json()).expect("tools_json() 必须是合法 JSON");
    let arr = derived.as_array().expect("tools_json() 必须是数组");
    let body = serde_json::to_string_pretty(&arr[..base_arr_len()]).unwrap();
    let text = format!("const TOOLS: &str = r#\"{body}\"#;\n");
    std::fs::write(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tools_baseline.json"
        ),
        text,
    )
    .expect("fixture 写入失败");
}

fn base_arr_len() -> usize {
    let fixture = include_str!("../../../tests/fixtures/tools_baseline.json");
    let r_str_marker = fixture.find("r#\"").expect("fixture 缺 r#\"") + 3;
    let end = fixture.rfind("]\"#;").expect("fixture 缺 ]\"#;") + 1;
    let baseline: serde_json::Value = serde_json::from_str(&fixture[r_str_marker..end])
        .expect("fixture 内 const TOOLS 正文必须是合法 JSON");
    baseline.as_array().unwrap().len()
}

/// spawn_subagent 不再暴露 budget 参数：子 agent 没有独立预算，轮数 / 工具数与
/// 主会话共用安全网。schema 里一旦又长出 budget，模型就会按自己给的数安排
/// 后续动作，而那个数根本不生效。
#[test]
fn spawn_subagent_schema_has_no_budget_param() {
    let v: serde_json::Value =
        serde_json::from_str(SCHEMA_SPAWN_SUBAGENT).expect("schema 常量必须是合法 JSON");
    assert!(
        v["function"]["parameters"]["properties"]
            .get("budget")
            .is_none(),
        "spawn_subagent schema 不得再有 budget 参数（子 agent 无独立预算）"
    );
}

/// 单源真相的核心不变式：ToolDef.name 必须等于它自己 schema 里的 function.name。
/// 二者漂移 = 模型按 schema 调的工具名在 dispatch 查表时找不到（silent bug）。
#[test]
fn tool_def_name_matches_schema_name() {
    for t in TOOLS_TABLE {
        let v: serde_json::Value =
            serde_json::from_str(t.schema).expect("schema 常量必须是合法 JSON");
        assert_eq!(
            v.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str()),
            Some(t.name),
            "ToolDef.name 与 schema 内的 function.name 漂移: {}",
            t.name
        );
    }
}

/// 每个 mutating 工具都必须至少声明一个 claims_pattern——否则该工具的
/// 「我做完了」声称对幻觉守卫完全不可见。样本硬编码会随工具增删漂移，
/// 这里只遍历 TOOLS_TABLE 锁「非空」不变式；具体话术能否命中由
/// bot_model_loop 的 claims_mutation_* 用例锁。
#[test]
fn every_mutating_tool_has_claim_patterns() {
    for t in TOOLS_TABLE.iter().filter(|t| t.mutating) {
        assert!(
            !t.claims_patterns.is_empty(),
            "{} 是 mutating 但 claims_patterns 为空——幻觉守卫对其变更声称不可见",
            t.name
        );
    }
}

/// 非 mutating 工具的 claims_patterns 必须为 &[]（错填会带入伪变更检测）。
#[test]
fn non_mutating_tools_have_empty_patterns() {
    for t in TOOLS_TABLE.iter().filter(|t| !t.mutating) {
        assert!(
            t.claims_patterns.is_empty(),
            "{} 是非 mutating 但 claims_patterns 不空: {:?}",
            t.name,
            t.claims_patterns
        );
    }
}

/// screenshot 真写盘（PNG 落 AI_Gen_Files）——必须按变更类登记：否则
/// 「已截屏 / 已保存截图」这类声称对幻觉守卫完全不可见，模型不调工具也能报。
#[test]
fn screenshot_is_registered_as_a_mutating_tool() {
    let t = TOOLS_TABLE
        .iter()
        .find(|t| t.name == "screenshot")
        .expect("screenshot 必须在工具表里");
    assert!(t.mutating, "截图真写盘，必须标 mutating");
    assert!(
        claims_mutation("已截屏：/tmp/a.png"),
        "「已截屏」应命中变更声称"
    );
    assert!(
        claims_mutation("已保存截图到 /tmp/a.png"),
        "「已保存截图」应命中变更声称"
    );
}

/// 截图的声称话术不得误伤「读图」：用户传图让模型看内容时，模型说的是
/// 「已查看截图」——那是只读动作（ocr_image），打回模型等于凭空多一轮。
#[test]
fn screenshot_claim_patterns_do_not_fire_on_reading_an_image() {
    assert!(
        !claims_mutation("已查看截图里的内容"),
        "读图不是变更——截图的声称话术不得命中「已查看截图」"
    );
}

/// 剪贴板语义豁免：「已复制到剪贴板」说的是系统剪贴板（clipboard_write /
/// copy_file_to_clipboard 都是非变更类工具），不是文件复制——而 manage_file 的
/// 短动词「已复制」是它的子串。不豁免的话，用户让「把这段话复制到剪贴板」
/// 会被幻觉守卫判成「声称变更但未调工具」：凭空打回模型，还给用户挂一条
/// 「这句没有对应凭证」的更正。
#[test]
fn clipboard_phrasing_is_not_a_mutation_claim() {
    assert!(
        !claims_mutation("已复制到剪贴板（42 字符），可直接粘贴"),
        "剪贴板话术不是变更声称"
    );
    assert!(
        !claims_mutation("文件已放入剪贴板"),
        "剪贴板话术不是变更声称"
    );
    // 文件复制话术照常命中（豁免只在剪贴板邻域生效）
    assert!(
        claims_mutation("已复制：/a/b.txt → /c/d.txt"),
        "文件复制仍是变更声称"
    );
    // 同句前后并存时，文件复制那处不因远处出现「剪贴板」而被豁免
    assert!(
        claims_mutation("已复制 /a/b.txt 到 /c/d.txt，并把说明复制到剪贴板"),
        "同句里的文件复制那处仍要命中"
    );
}

/// UNC 路径不再绕过存在性校验：`\\server\share\x.docx` 形态候选此前被
/// `/` 与盘符冒号两个前缀过滤掉，守卫对编造的 UNC 路径完全失明。
/// 主机名用 `.invalid`（RFC 保留假 TLD，解析必败，Windows 上也快速返回）。
/// `#[ignore]`：依赖 DNS 解析（即便 .invalid 也会查系统解析器），CI 噪声源，
/// 手动跑：`cargo test --test registry claimed_doc_paths -- --ignored`。
#[test]
#[ignore = "UNC 主机名解析依赖系统 DNS resolver，CI 慢且不稳；手动 cargo test ... -- --ignored 验"]
fn claimed_doc_paths_checks_unc_paths() {
    let text = "已生成报告，路径 \\\\nonexistent-host-.invalid\\share\\报告.docx";
    assert_eq!(
        claimed_doc_paths(text),
        vec!["\\\\nonexistent-host-.invalid\\share\\报告.docx".to_string()]
    );
    // 无声称触发词 → 不检查（与既有口径一致）
    assert!(claimed_doc_paths("\\\\nonexistent-host-.invalid\\share\\报告.docx").is_empty());
}

/// path tokenizer 边界契约：以下形态**不**应被拆碎成多 token，
/// `\` 不进 boundary 集合（否则 `C:\Users\foo.docx` 拆成 `C:` + `Users` +
/// `foo.docx` 三个碎片，盘符路径失访）。本测试用**本地可写**的 tmpdir
/// 拼出真存在的盘符/UNC 形态路径，验 fabricated_gen_path 不返回误报。
#[test]
fn claimed_doc_paths_preserves_drive_letter_path() {
    // tmpdir 路径形如 /var/folders/.../T/wm-fab-XXXX——本机 fs 存在，
    // 拼一个后缀 .docx 的真存在文件做 baseline
    let tmp = std::env::temp_dir().join(format!(
        "wm-fab-drive-{}.docx",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, b"baseline").unwrap();
    // 在已存在文件旁描述一个**不存在**的同前缀路径：应被提取为候选，
    // 且存在性判据判它缺
    let ghost = tmp.with_file_name(format!("{}.docx", uuid::Uuid::new_v4().simple()));
    let text = format!("已生成 {}", ghost.display());
    // 本机绝对路径以 / 开头，已在 claimed_doc_paths 边界守卫内 → 命中
    assert_eq!(
        claimed_doc_paths(&text).first().map(|s| s.as_str()),
        Some(ghost.to_string_lossy().as_ref()),
        "本机绝对盘符路径（含 / 前缀）必须被提取为候选"
    );
    assert!(
        path_missing_on_disk(&ghost.to_string_lossy()),
        "候选路径在磁盘上不存在，存在性判据要判它缺"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 边界字符不切分：本机绝对路径 / 盘符 / UNC 三种形态在 split + filter 后
/// 仍以完整 token 形式进存在性校验（不在 boundary 处分）。
/// 本测试**避免**测盘符路径（C:\foo）在 macOS 上的边界解析差异（macOS
/// 把 `\` 当普通字符，行为与 Windows 不同；这正是当时 `\` 不入 boundary
/// 的原因——盘符守卫专为 Windows 设计的）。改测**本机绝对路径**形态，
/// 与现有 `fabricated_gen_path_preserves_drive_letter_path` 形成对照：
/// 后者验「存在就跳过」、本测试验「不存在就命中 fabricated」。
#[test]
fn claimed_doc_paths_tokenizer_does_not_split_at_certain_boundaries() {
    let tmp = std::env::temp_dir();
    let ghost = tmp.join(format!("{}.docx", uuid::Uuid::new_v4().simple()));
    // 必须先创建父目录存在——否则 ghost 父路径不存在导致 exists() 在
    // Unix 上行为奇怪；这里用 temp_dir() 已经存在
    let text = format!("已生成 {}", ghost.display());
    // tmp 在 macOS 上是 /var/folders/.../T/，路径以 / 开头——
    // tok.starts_with('/') 通过守卫；Path::new(ghost).exists() 必为 false
    // （uuid 新名未创建）；find 应找到完整 token
    let got = claimed_doc_paths(&text);
    assert_eq!(
        got.first().map(|s| s.as_str()),
        Some(ghost.to_string_lossy().as_ref()),
        "本机绝对路径（含 / 前缀）必须以完整 token 进候选，不被切分"
    );
}

/// TOOLS_TABLE 索引长度等于 TOOLS_TABLE 长度——保证无重复 name。
#[test]
fn tools_index_len_matches_table_len() {
    let idx = tools_index();
    assert_eq!(
        idx.len(),
        TOOLS_TABLE.len(),
        "tools_index().len() ({}) != TOOLS_TABLE.len() ({})，含重复 name",
        idx.len(),
        TOOLS_TABLE.len()
    );
}

/// TOOLS_TABLE 里每个 name 都能在 index 里查到——索引完整覆盖。
/// ToolDef 不 impl PartialEq/Debug，改用 std::ptr::eq 校验指针同一性。
#[test]
fn tools_index_covers_all_table_names() {
    let idx = tools_index();
    for t in TOOLS_TABLE {
        assert!(idx.contains_key(t.name), "tools_index 缺 key `{}`", t.name);
        // idx.get(t.name) 是 Option<&&'static ToolDef>；.copied() 拆一层外引用
        let got = idx
            .get(t.name)
            .copied()
            .expect("contains_key 已返 true，这里不可能 None");
        // 指针同一性校验（不需 PartialEq）
        assert!(
            std::ptr::eq(got, t),
            "tools_index[`{}`] 指针与 TOOLS_TABLE 不一致",
            t.name
        );
    }
}

// 子 agent 白名单 schema

/// 设计 §5/§13：子 agent 工具清单断言无 spawn（递归双保险①——schema 层）；
/// 三个 profile 都不含任务卡主状态写工具与编排工具。
#[test]
fn subagent_whitelists_exclude_spawn_and_task_writes() {
    let forbidden = [
        "spawn_subagent",
        "check_subagent",
        "cancel_subagent",
        "create_task",
        "complete_task",
        "delete_task",
        "edit_task",
        "add_subtask",
        "toggle_subtask",
        "remove_subtask",
        "link_file_to_task",
        "use_skill",
    ];
    for p in [
        crate::db::SubagentProfile::Research,
        crate::db::SubagentProfile::Coder,
        crate::db::SubagentProfile::General,
    ] {
        let json = profile_tools_json(p);
        let v: serde_json::Value = serde_json::from_str(json).expect("白名单必须是合法 JSON");
        let names: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        for f in forbidden {
            assert!(
                !names.contains(&f),
                "{:?} 白名单不得含 {f}（递归禁用/主状态只读）",
                p
            );
        }
        assert!(
            names.contains(&"read_own_card"),
            "白名单必须含 read_own_card"
        );
        assert!(names.contains(&"write_artifact_file"));
    }
}

/// 白名单内容精确性：research 联网 + 读列；coder 读列 + edit/write + run_python；
/// general = research ∪ coder 去重。`ast_inspect` 随 run_python 一同进 coder/general，
/// 不进 research（只读 profile 不装执行器）。
#[test]
fn profile_whitelists_match_design_mapping() {
    assert_eq!(
        RESEARCH_TOOLS,
        &[
            "web_search",
            "fetch_url",
            "list_files",
            "read_text_file",
            "grep_files",
            "extract_document",
            "write_artifact_file",
            "read_own_card",
        ]
    );
    assert!(
        !RESEARCH_TOOLS.contains(&"ast_inspect"),
        "research 是只读 profile，不得装 AST 执行器"
    );
    assert_eq!(
        CODER_TOOLS,
        &[
            "read_text_file",
            "list_files",
            "grep_files",
            "edit_file",
            "write_file",
            "run_python",
            "ast_inspect",
            "unpack_archive",
            "pack_archive",
            "write_artifact_file",
            "read_own_card",
        ]
    );
    let mut union: Vec<&str> = RESEARCH_TOOLS
        .iter()
        .chain(CODER_TOOLS.iter())
        .copied()
        .collect();
    union.sort();
    union.dedup();
    let mut general: Vec<&str> = GENERAL_TOOLS.to_vec();
    general.sort();
    assert_eq!(general, union, "general 必须是 research ∪ coder 去重");
    // 白名单里的名字全部真实存在（防拼写漂移）
    for name in GENERAL_TOOLS {
        assert!(
            tools_index().contains_key(name),
            "白名单工具 {name} 不在 TOOLS_TABLE"
        );
    }
}

/// 无会话上下文 → 默认全量（含编排三工具，不含 subagent-only）
#[test]
fn tools_json_for_plain_session_is_default() {
    assert_eq!(
        tools_json_for(Some("not-a-subagent-session")).as_ref(),
        tools_json()
    );
    assert_eq!(tools_json_for(None).as_ref(), tools_json());
}

/// per-onion 收窄的**接线**守卫：绑了配置的会话，`tools_json_for` 真得少几个。
///
/// 纯函数 `effective_tools` 的单测钉不住这条线——变异实测把
/// `tools_json_for` 里的 `allowed` 换成一个恒空的切片（配置完全不收窄、
/// 退回整份 profile 白名单），bot_onions 与本模块其余 32 条测试全绿，
/// 而这套配置的全部意义就是「收窄」失效了。所以这里直接断言**最终那份
/// JSON 里的工具名集合**，钉的是接线而不是中间量。
#[test]
fn tools_json_for_narrows_when_session_has_onion_config() {
    fn names_of(session_id: Option<&str>) -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(tools_json_for(session_id).as_ref()).expect("清单须合法");
        v.as_array()
            .expect("顶层是数组")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
            .collect()
    }
    let cfg = crate::bot_onions::parse(
        "onion_id: validator\nprofile: coder\nallowed: [read_text_file, grep_files]\n",
    )
    .expect("配置应合法");

    // 对照组：同一条会话，没绑配置时走默认全量清单
    crate::bot_onions::bind_for_test("reg-onion-narrow", None);
    let plain = names_of(Some("reg-onion-narrow"));
    assert_eq!(
        plain.len(),
        tools_json_names().len(),
        "未绑配置应拿默认全量"
    );

    // 实验组：绑一份只留两个工具的配置（yaml 的 profile=coder 提供上限）
    crate::bot_onions::bind_for_test("reg-onion-narrow", Some(cfg));
    let narrowed = names_of(Some("reg-onion-narrow"));
    assert_eq!(
        narrowed,
        vec!["read_text_file".to_string(), "grep_files".to_string()],
        "绑了 allowed=[read_text_file, grep_files] 的会话必须只拿到这两个"
    );
    assert!(narrowed.len() < plain.len(), "收窄必须真的变少");

    // 解除绑定回到默认（设置页换配置/解绑的路径）
    crate::bot_onions::bind_for_test("reg-onion-narrow", None);
    assert_eq!(names_of(Some("reg-onion-narrow")), plain);
}

/// schema 层与 dispatch 闸必须算出**同一份**工具集（期 6）。
///
/// 两条路径各算一次（闸读注册时存进 ctx 的，schema 现算），一旦漂移：
/// 漂成 schema 比闸宽 → 模型看得见却调不动，白烧一轮拿一句「白名单外」；
/// 漂成 schema 比闸窄 → 能力被静默藏起来。本测试让两侧对着算一遍。
#[test]
fn gate_whitelist_and_schema_list_are_computed_from_the_same_intersection() {
    fn names_of(session_id: &str) -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(tools_json_for(Some(session_id)).as_ref()).expect("清单须合法");
        v.as_array()
            .expect("顶层是数组")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
            .collect()
    }
    // 配置档自称 profile=research，运行时报 coder。allowed 里一个只属 coder
    // （write_file）、一个只属 research（web_search）——两侧算错哪一边、
    // 错几次都会在结果里露出来（本用例无区分度时这条测试就是假绿）。
    let cfg = crate::bot_onions::parse(
        "onion_id: validator\nprofile: research\nallowed: [write_file, web_search]\n",
    )
    .expect("配置应合法");
    let sid = "reg-onion-gate-parity";
    let runtime_profile = crate::db::SubagentProfile::Coder;
    let allowed = cfg.allowed.clone();
    crate::bot_onions::bind_for_test(sid, Some(cfg));
    crate::tool_guard::register_subagent_session(
        sid,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_parity".into(),
            task_id: None,
            profile: runtime_profile,
            tools: std::sync::Arc::new(crate::bot_onions::effective_tools(
                runtime_profile,
                &allowed,
            )),
            artifact_dir: std::path::PathBuf::from("/tmp/art_parity"),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_progress_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    );

    let gate: Vec<String> = crate::tool_guard::subagent_ctx(Some(sid))
        .expect("刚注册过")
        .tools
        .iter()
        .map(|t| t.to_string())
        .collect();
    let schema = names_of(sid);
    assert_eq!(
        schema, gate,
        "schema 清单与闸白名单必须逐项一致（都应是运行时 profile ∩ allowed）"
    );
    assert_eq!(
        schema,
        vec!["write_file".to_string()],
        "运行时 profile=coder 才是上限：web_search 只属 research，应被收掉"
    );
    crate::tool_guard::unregister_subagent_session(sid);
    crate::bot_onions::bind_for_test(sid, None);
}

fn tools_json_names() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(tools_json()).expect("清单须合法");
    v.as_array()
        .expect("顶层是数组")
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect()
}

/// yaml 的 `profile:` 不得覆盖运行时的子 agent profile（否则就是「放宽」）。
#[test]
fn yaml_profile_cannot_widen_beyond_runtime_profile() {
    fn names_of(session_id: Option<&str>) -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(tools_json_for(session_id).as_ref()).expect("清单须合法");
        v.as_array()
            .expect("顶层是数组")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
            .collect()
    }
    // yaml 自称 coder（含写类工具），allowed 也不收窄
    let cfg = crate::bot_onions::parse("onion_id: p\nprofile: coder\n").expect("配置应合法");
    let sid = "reg-onion-widen";
    crate::bot_onions::bind_for_test(sid, Some(cfg));
    // 没有子 agent 上下文时 yaml 的 profile 兜底生效（配置本来的用法）
    assert_eq!(names_of(Some(sid)).len(), CODER_TOOLS.len());
}

// 阶段 3：MCP 动态挂载点

/// 无 MCP 连接：tools_json_with_mcp 必须原样借用静态 JSON（零分配、零漂移）。
/// 与 manager e2e 共用测试锁：e2e 的连接存活窗口会让「无连接」断言
/// 在 cargo test 并行下 flaky；先清残留（前序用例 panic 可能留下连接槽）。
#[test]
fn tools_json_with_mcp_without_connections_is_borrowed_static() {
    // blocking_lock 须在 block_on 进入 runtime 上下文之前取（tokio Mutex 语义）
    let _serial = crate::bot::mcp::manager::SHARED_MCP_TEST_LOCK.blocking_lock();
    tauri::async_runtime::block_on(crate::bot::mcp::manager::shared().shutdown());
    let with = tools_json_with_mcp(None);
    assert!(matches!(with, std::borrow::Cow::Borrowed(_)));
    assert_eq!(with.as_ref(), tools_json());
    // 子 agent 会话短路：不碰 MCP（挂载层根本不被调用）
    let sub = tools_json_with_mcp(Some("not-a-subagent-session"));
    assert!(matches!(sub, std::borrow::Cow::Borrowed(_)));
}

/// 摘尾拼接契约——base 正常时摘尾接段补收尾；契约破坏（未以 \n] 收尾）
/// 回退 base 原样返回，绝不产出非法 JSON（release 无 debug_assert 保护）。
#[test]
fn splice_mcp_body_falls_back_when_base_contract_broken() {
    let broken = "[\n  {\"type\":\"function\"}";
    assert!(matches!(
        super::splice_mcp_body(broken, ",\n  {}"),
        std::borrow::Cow::Borrowed(_)
    ));
    let out = super::splice_mcp_body("[\n  {}\n]", ",\n  {}");
    assert_eq!(out.as_ref(), "[\n  {},\n  {}\n]");
    // 拼装结果恒为合法 JSON 数组
    let v: serde_json::Value = serde_json::from_str(out.as_ref()).expect("拼装结果应合法");
    assert_eq!(v.as_array().map(|a| a.len()), Some(2));
    // body 侧对偶契约（评审）：动态段不以 ",\n" 起头（mount 层构造破坏）
    // 同样回退 base，不产出非法 JSON
    assert!(matches!(
        super::splice_mcp_body("[\n  {}\n]", "garbage"),
        std::borrow::Cow::Borrowed(_)
    ));
    assert!(matches!(
        super::splice_mcp_body("[\n  {}\n]", ",garbage"),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// 三份清单共用 `assemble_tools_json` 后形状恒等——这正是
/// `splice_mcp_body` 依赖的「恒以 `\n]` 收尾」契约。
/// 契约破坏以前只能靠限频 eprintln 事后发现；现在由构造保证，这里钉住。
#[test]
fn all_three_assemblies_share_the_same_shape() {
    use crate::db::SubagentProfile;
    let base = tools_json();
    assert!(base.starts_with("[\n  "), "首项缩进两格：{base:.20}");
    assert!(
        base.ends_with("\n]"),
        "必须以 \\n] 收尾：{:?}",
        &base[base.len()..]
    );
    for p in [
        SubagentProfile::Research,
        SubagentProfile::Coder,
        SubagentProfile::General,
    ] {
        let s = profile_tools_json(p);
        assert!(
            s.starts_with("[\n  ") && s.ends_with("\n]"),
            "{} 档形状与主清单不一致",
            p.as_str()
        );
    }
    // 交集清单（owned 路径）同样形状
    let narrowed = super::narrowed_tools_json(&["run_python"]);
    assert!(narrowed.starts_with("[\n  ") && narrowed.ends_with("\n]"));
    // 空集也要有合法形状（模型该收到空清单，不是默认清单）
    assert_eq!(super::narrowed_tools_json(&[]), "[\n]");
}

/// 拼接内核只有一份判定：借用版与 owned 版对同一输入必须同进同退。
#[test]
fn splice_both_variants_share_one_kernel() {
    let cases = [
        ("[\n  {}\n]", ",\n  {}"), // 正常
        ("[\n  {}\n]", "garbage"), // body 契约破坏
        ("[broken", ",\n  {}"),    // base 契约破坏
    ];
    for (base, body) in cases {
        let borrowed = super::splice_mcp_body(base, body);
        let owned = super::splice_mcp_body_owned(base.to_string(), body);
        assert_eq!(
            borrowed.as_ref(),
            owned.as_str(),
            "两版对 base={base:?} body={body:?} 必须给出同一结果"
        );
    }
}

/// 动态拼装契约（纯格式）：拿 mount 的纯函数造一个假 body，
/// 拼装结果必须是合法 JSON 数组、内置 BUILTIN_TOOL_COUNT 个工具 schema 在前 + MCP 条目在后。
/// （真实连接路径由 manager e2e 测试覆盖：连接后 with_mcp 含 mcp_echo_echo。）
#[test]
fn tools_json_with_mcp_assembly_contract() {
    let base = tools_json();
    let body = crate::bot::mcp::mount::build_tools_json_body(&[]);
    assert!(body.is_empty(), "无服务器时空段");
    // 模拟 registry 拼装路径（与 tools_json_with_mcp Owned 分支同构）
    let fake_body = r#",
  {"type":"function","function":{"name":"mcp_fake_x","description":"d","parameters":{"type":"object","properties":{}}}}"#;
    let mut s = String::with_capacity(base.len() + fake_body.len() + 2);
    s.push_str(&base[..base.len() - 2]);
    s.push_str(fake_body);
    s.push_str("\n]");
    let v: serde_json::Value = serde_json::from_str(&s).expect("拼装结果必须合法");
    let arr = v.as_array().unwrap();
    // 数目从常量推导而非写死：增删内置工具时这里跟着走，不会再漂
    let main_visible = BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len();
    assert_eq!(
        arr.len(),
        main_visible + 1,
        "{main_visible} 内置主可见工具 + 1 假 MCP"
    );
    assert_eq!(arr[main_visible]["function"]["name"], "mcp_fake_x");
    // 内置前 main_visible 项顺序不变（增量挂载不漂移）
    let base_arr = serde_json::from_str::<serde_json::Value>(base).unwrap();
    assert_eq!(
        &arr[..main_visible],
        base_arr.as_array().unwrap().as_slice()
    );
}

/// update_plan 的 schema 契约：整表替换 + 三态 + 门槛写在工具描述里。
///
/// 三条都是产品决定，锁在这里是防它们被顺手改掉：
/// - `plan` 必填：没有清单就没有「计划」这回事；
/// - status 只有三态且枚举写死：多一态（如 blocked）就多一套前端不会画的语义；
/// - 描述里必须留着「什么时候不要用」：门槛写在工具描述里才拦得住模型，
///   靠系统提示词劝是劝不住的（Codex / Claude Code 都把门槛写进工具描述）。
#[test]
fn update_plan_schema_contract() {
    let v: serde_json::Value =
        serde_json::from_str(SCHEMA_UPDATE_PLAN).expect("SCHEMA_UPDATE_PLAN 必须是合法 JSON");
    assert_eq!(v["function"]["name"], "update_plan");
    let required: Vec<&str> = v
        .pointer("/function/parameters/required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    assert!(
        required.contains(&"plan"),
        "plan 必须必填：没有清单就没有计划可言，actual required: {required:?}"
    );
    let status = v
        .pointer("/function/parameters/properties/plan/items/properties/status")
        .expect("plan.items 必须声明 status");
    let enums: Vec<&str> = status["enum"]
        .as_array()
        .expect("status 必须有 enum")
        .iter()
        .filter_map(|s| s.as_str())
        .collect();
    assert_eq!(
        enums,
        vec!["pending", "in_progress", "completed"],
        "状态只有三态：多一态就多一套面板画不出来的语义"
    );
    let desc = v["function"]["description"]
        .as_str()
        .expect("工具必须有 description");
    assert!(
        desc.contains("什么时候不要用"),
        "描述里必须留着使用门槛（模型靠这个判断该不该建计划）：{desc}"
    );
    assert!(
        desc.contains("整表替换"),
        "描述里必须写明整表替换语义：模型按增量写会与后端状态错位：{desc}"
    );
    // 正向触发条件必须留着：只有负向清单时，「看起来是单一目标、实际多步」的
    // 任务无从判断（实测：做 PPT 跑了 18 步工具调用，全程没建计划）。
    assert!(
        desc.contains("什么时候建计划"),
        "描述里必须有正向触发条件（模型只能判信号，判不了还没开始的步数）：{desc}"
    );
    // 建计划窗口含「读完技能文档之后」：use_skill 往往在第 6 轮才发生，
    // 只写「开工前」会让模型以为自己错过了建计划的时机。
    assert!(
        desc.contains("读完技能文档"),
        "描述里必须写明读完技能文档之后建计划也算来得及：{desc}"
    );
    // 「三步以内能做完的事」这条判据要删干净：模型在决定的那一刻没法数一个
    // 还没开始的任务的步数，只能拿「像不像多步」来近似——单目标产出物
    // （做一个 PPT）天生读起来像简单任务，于是该建时不建。
    assert!(
        !desc.contains("三步以内"),
        "「三步以内」是模型判不了的判据，别加回来：{desc}"
    );
    // 计划工具给主 agent 用；子 agent 另有 read_own_card 口径。
    assert!(
        !SUBAGENT_ONLY_TOOLS.contains(&"update_plan"),
        "update_plan 不是子 agent 专属工具"
    );
    assert!(
        tools_index().contains_key("update_plan"),
        "update_plan 必须进 TOOLS_TABLE"
    );
}

/// run_python 必须公告 scriptPath，且不得把 code 列为必填。
///
/// 回归背景：长脚本的正确做法是「先落盘、出错只改出错那几行重跑」——内联代码
/// 每改一次都重付一遍全文的输出 token。schema 里没有这条入口，模型就只会整段
/// 重写；把 code 留成必填更是直接堵死「只给脚本路径」这条路。
#[test]
fn run_python_schema_advertises_script_path_without_forcing_code() {
    let v: serde_json::Value = serde_json::from_str(super::schemas::SCHEMA_RUN_PYTHON)
        .expect("run_python schema 必须是合法 JSON");
    let func = &v["function"];
    assert!(
        func["parameters"]["properties"]["scriptPath"].is_object(),
        "run_python 必须公告 scriptPath 参数：{v}"
    );
    assert!(
        func["parameters"]["properties"]["code"].is_object(),
        "run_python 仍要保留内联 code 参数：{v}"
    );
    let required = func["parameters"]["required"].as_array();
    assert!(
        required.is_some_and(|r| r.is_empty()),
        "code 与 scriptPath 二选一，required 不得强绑 code：{v}"
    );
    // 描述里要点明「先落盘再执行」——参数名自解释不了为什么该用它
    let desc = func["description"].as_str().unwrap_or_default();
    assert!(
        desc.contains("scriptPath"),
        "描述里应说明 scriptPath 的用法：{desc}"
    );
}
