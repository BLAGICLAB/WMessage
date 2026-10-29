//! `bot::params` 的单元测试（从 params.rs 拆出）。
//!
//! 拆分的理由：params.rs 同时装着参数表与三档预算画像，两块加起来超了
//! 1500 行的文件上限。测试搬出去后，生产代码与测试各自独立演进。

use super::*;
use crate::bot::config::types::BotConfig;

/// 每条参数都必须能被表里自带的 resolver 解析出真值。
///
/// 这条测试替代原来的兜底臂：解析曾经是一个独立的 `match key`，兜底是
/// `other => (format!("?{other}"), "hardcoded")` —— 表里加了一条 key 而忘了
/// 加 match 臂时，不编译失败、不测试失败，只在设置页显示字面量
/// `?subagent.foo`。解析收进表之后，漏解析根本构造不出来。
#[test]
fn every_param_resolves_to_a_real_value() {
    for d in PARAMS_TABLE {
        let (value, source) = (d.resolve)(&empty_cfg());
        assert!(!value.is_empty(), "{} 解析出空值", d.key);
        assert!(
            !value.starts_with('?'),
            "{} 落进了兜底（key 与 resolver 对不上）",
            d.key
        );
        assert!(
            matches!(source, "config" | "default" | "hardcoded"),
            "{} 来源非法：{source}",
            d.key
        );
        assert_eq!(
            d.editable,
            source != "hardcoded",
            "{}：可编辑性与来源标注不一致（editable={} source={source}）",
            d.key,
            d.editable
        );
    }
}

/// key 不得重复：设置页按 key 定位，重复会静默遮蔽前一条。
#[test]
fn param_keys_are_unique() {
    let mut keys: Vec<&str> = PARAMS_TABLE.iter().map(|d| d.key).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(before, keys.len(), "参数表里有重复 key");
}

fn empty_cfg() -> BotConfig {
    BotConfig {
        base_url: String::new(),
        model: String::new(),
        api_key: None,
        bypass_llm_on_pre_step_hit: true,
        allowed_dirs: Vec::new(),
        tavily_key: None,
        tavily_enabled: None,
        brave_key: None,
        brave_enabled: None,
        python_timeout_secs: None,
        perm_mode: None,
        api_provider: None,
        max_tokens: None,
        reasoning_effort: None,
        max_function_calls: None,
        final_selfcheck_round_gap: None,
        selfcheck_model_id: None,
        guard_claim_keywords: None,
        verifier_retry_max: None,
        archive_after_days: None,
        models_by_provider: None,
        active_model_id: None,
        ui_font_size: None,
        memory_consolidation: None,
        memory_control: None,
        memory_tuning: None,
        mcp_servers: None,
        disabled_vendors: Vec::new(),
        verified_vendors: Vec::new(),
        schema_version: 1,
        task_mode: None,
        subagent_max_tool_calls: None,
        max_rounds: None,
        history_truncate_target_pct: None,
        reasoning_echo_enabled: None,
        in_turn_context_edit: None,
        history_budget_chars: None,
        tool_rules: None,
        subagent_max_turns: None,
        search_max_results: None,
        max_tool_output_bytes: None,
        ask_budget: None,
    }
}

/// 硬编码只读区每行的展示值必须等于代码里真正生效的那个常量。
///
/// 这些行的 resolve 取的是真常量（MAX_GLOBAL_RUNNING / MAX_TURNS_HARD_CAP /
/// clamp 上界 / SEARCH_OUTPUT_CAP_BYTES / 流式超时 / 重试两常量），而 `default`
/// 现在是**从 resolver 派生**的（空配置下的输出）——只读区两者恒等。
/// 这里逐条把派生值与常量双向钉死：任一侧单独改动而不改另一侧即红。
#[test]
fn hardcoded_rows_track_runtime_constants() {
    let views = effective_params(&empty_cfg());
    // 取一行的展示值，并断言生效值与 default 同源（只读区两者恒等）
    let shown = |k: &str| -> String {
        let v = views.iter().find(|v| v.key == k).unwrap();
        assert_eq!(v.source, "hardcoded", "{k} 应属硬编码只读区");
        assert_eq!(v.value, v.default, "{k}: 生效值与 default 不同源");
        v.default.clone()
    };
    assert_eq!(
        shown("subagent.max_running"),
        crate::bot::constants::SUBAGENT_MAX_GLOBAL_RUNNING.to_string()
    );
    assert_eq!(
        shown("subagent.per_session_running"),
        crate::bot::constants::SUBAGENT_MAX_PER_SESSION_RUNNING.to_string()
    );
    assert_eq!(
        shown("subagent.turns_hard_cap"),
        crate::db::MAX_TURNS_HARD_CAP.to_string()
    );
    assert_eq!(
        shown("search.count_cap"),
        SEARCH_MAX_RESULTS_CAP.to_string(),
        "搜索条数硬顶须与 clamp_search_max_results 上界同源"
    );
    assert_eq!(
        shown("search.output_bytes"),
        crate::bot_web::SEARCH_OUTPUT_CAP_BYTES.to_string()
    );
    assert_eq!(
        shown("llm.stream_idle_secs"),
        crate::bot_model_loop::STREAM_CHUNK_IDLE_TIMEOUT
            .as_secs()
            .to_string()
    );
    assert_eq!(
        shown("llm.retry"),
        format!(
            "{}次/{}s",
            crate::bot_model_loop::MAX_LLM_ATTEMPTS,
            crate::bot_model_loop::LLM_RETRY_DELAY.as_millis() as f64 / 1000.0
        )
    );
    // 扫描间隔 / 痕迹三表的两个钳制 + 保留期：这四条原先是表里的裸字面量
    // （"30" / "16384" / "2000" / "30"），与真常量恰好相等纯属手写对了——
    // 真常量一改，表里的旧数会静默留着（没有任何测试把两者绑住）。
    // 现在 resolve 直接取常量，测试再钉一层。
    assert_eq!(
        shown("schedule.scan_secs"),
        crate::bot_scheduler::SCHED_TICK_SECS.to_string()
    );
    assert_eq!(
        shown("trace.span_text_max"),
        crate::db::trace::SPAN_TEXT_MAX.to_string()
    );
    assert_eq!(
        shown("trace.diff_max_lines"),
        crate::db::trace::MAX_DIFF_LINES.to_string()
    );
    assert_eq!(
        shown("trace.retention_days"),
        crate::db::trace::TRACE_RETENTION_DAYS.to_string()
    );
    // 记忆抽取限频：设置页文案里的「≥N 分钟一次」引用的就是这个常量
    assert_eq!(
        shown("memory.extract_min_interval_secs"),
        crate::memory::extract::EXTRACT_MIN_INTERVAL_SECS.to_string()
    );
    // 「每会话并发」此前只在 max_running 的 hint 里以文字出现，表里查不到；
    // 现在它有自己的行，且展示值即派发闸真正认的那个数
    assert!(PARAMS_TABLE
        .iter()
        .any(|d| d.key == "subagent.per_session_running"));
}

/// 工具调用熔断上界单源：子 agent 预算解析与模型循环派生链末端同一个数。
///
/// 两处曾各写一份 `clamp(1, 500)`，其中薄壳那份还会被绕过（测试直驱 core
/// 传 deps.fuse_cap_override 时不经薄壳）——同一条边界抄两份，改一处漏一处
/// 不编译失败也不测试失败。
#[test]
fn max_function_calls_cap_is_single_sourced() {
    let mut cfg = empty_cfg();
    cfg.subagent_max_tool_calls = Some(u32::MAX);
    assert_eq!(
        resolve_subagent_budget(&cfg).max_tool_calls,
        MAX_FUNCTION_CALLS_CAP,
        "预算侧须钳到 MAX_FUNCTION_CALLS_CAP"
    );
    assert_eq!(clamp_max_function_calls(0), 1, "0 次调用无意义，钳到 1");
    assert_eq!(clamp_max_function_calls(30), 30, "区间内原样放行");
    // 模型循环消费端（bot_model_loop::fuse_cap_for）用的是同一个常量的
    // usize 投影——两边改数必须同时，否则一个按 500 记账一个按 999 放行
    assert_eq!(
        MAX_FUNCTION_CALLS_CAP as usize,
        crate::bot_model_loop::fuse_cap_upper_bound()
    );
}

/// verifier 重试预算：默认值与钳制边界单源，写侧落盘与读侧提示词同口径。
#[test]
fn resolve_verifier_retry_max_single_source() {
    assert_eq!(resolve_verifier_retry_max(None), 1, "未配置 = 默认 1");
    assert_eq!(resolve_verifier_retry_max(Some(0)), 0, "0 = 不重试，合法");
    assert_eq!(
        resolve_verifier_retry_max(Some(99)),
        VERIFIER_RETRY_MAX_CAP,
        "越界钳到上限"
    );
    assert_eq!(
        clamp_verifier_retry_max(u32::MAX),
        VERIFIER_RETRY_MAX_CAP,
        "写侧与读侧共用同一钳制"
    );
}

/// 表完整性：key 唯一 + 每项 category/hint 非空 + 行数下限（防漏登记）
#[test]
fn params_table_keys_unique_and_populated() {
    let mut seen = std::collections::HashSet::new();
    for d in PARAMS_TABLE {
        assert!(seen.insert(d.key), "重复 key：{}", d.key);
        assert!(!d.label.is_empty() && !d.hint.is_empty() && !d.category.is_empty());
    }
    assert!(
        PARAMS_TABLE.len() >= 20,
        "参数表 {} 项，疑似漏登记",
        PARAMS_TABLE.len()
    );
}

/// 参数表里的默认展示值必须等于代码里真正生效的那个数。
///
/// `tools.max_output_bytes` 的 `default` 曾写「不截断」，而空配置下 resolver
/// 返 30000、模型循环按自己那份字面量走——表里自相矛盾，且
/// 没有任何测试把这两个数字绑在一起。这条把该绑的都绑上：改任一侧不同步即红。
///
/// 现在 `default` 由 resolver 派生（不再是手写字面量），但「派生的结果等于
/// 运行时常量」这件事仍要钉住——派生走的是 resolver 路径，运行时走的是
/// 模型循环里的常量，两条路必须落在同一个数上。
#[test]
fn params_table_defaults_match_runtime_constants() {
    let views = effective_params(&empty_cfg());
    let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
    assert_eq!(
        by_key("tools.max_output_bytes").default,
        crate::bot::params::BALANCED_PRESET
            .tool_output_bytes
            .to_string(),
        "参数表展示的默认截断字节数与档位预设不符"
    );
    // 其余可编辑项：展示的默认 = 运行时内置默认常量
    assert_eq!(
        by_key("loop.max_rounds").default,
        DEFAULT_MAX_ROUNDS.to_string()
    );
    assert_eq!(
        by_key("loop.max_function_calls").default,
        crate::bot::params::budget_share(
            BALANCED_PRESET.global_tool_calls,
            MAIN_SESSION_BUDGET_PCT,
        )
        .to_string(),
        "主会话工具调用默认 = 均衡档全局池的 60%"
    );
    assert_eq!(
        by_key("loop.history_budget_chars").default,
        DEFAULT_HISTORY_BUDGET_CHARS.to_string()
    );
    assert_eq!(
        by_key("subagent.max_turns").default,
        DEFAULT_SUBAGENT_MAX_TURNS.to_string()
    );
    assert_eq!(
        by_key("search.max_results").default,
        DEFAULT_SEARCH_MAX_RESULTS.to_string()
    );
    // 全局 max_tokens：默认 32768（后端早已从 8192 提到 32K），表里若还写
    // 8192 就是页面与运行时各说各话
    assert_eq!(
        by_key("model.max_tokens").default,
        crate::bot::config::schema::DEFAULT_MAX_TOKENS.to_string()
    );
    // 越界配置：展示值必须已经是钳过的生效值，不能是用户填的原值
    let mut cfg = empty_cfg();
    cfg.max_tool_output_bytes = Some(MAX_TOOL_OUTPUT_BYTES_CAP + 100_000);
    let views = effective_params(&cfg);
    let v = views
        .iter()
        .find(|v| v.key == "tools.max_output_bytes")
        .unwrap();
    assert_eq!(v.value, MAX_TOOL_OUTPUT_BYTES_CAP.to_string());
}

/// 可填范围（min/max）必须等于实际生效的钳制边界。
///
/// 设置页把这两个数直接显示成「可填 X–Y」——它跟真正生效的钳制不是同一个
/// 数的话，页面就在骗人（用户填 999，运行时按 200 走，页面却说可填到 999）。
#[test]
fn editable_rows_ranges_match_clamp_bounds() {
    let views = effective_params(&empty_cfg());
    let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
    let range = |k: &str| -> (Option<u64>, Option<u64>) {
        let v = by_key(k);
        (v.min, v.max)
    };
    assert_eq!(
        range("loop.max_rounds"),
        (Some(MAX_ROUNDS_MIN as u64), Some(MAX_ROUNDS_CAP as u64))
    );
    assert_eq!(
        range("loop.max_function_calls"),
        (
            Some(MAX_FUNCTION_CALLS_MIN as u64),
            Some(MAX_FUNCTION_CALLS_CAP as u64)
        )
    );
    assert_eq!(
        range("loop.history_budget_chars"),
        (
            Some(HISTORY_BUDGET_MIN as u64),
            Some(HISTORY_BUDGET_MAX as u64)
        )
    );
    assert_eq!(
        range("subagent.max_turns"),
        (
            Some(crate::db::MIN_SUBAGENT_TURNS as u64),
            Some(crate::db::MAX_TURNS_HARD_CAP as u64)
        )
    );
    assert_eq!(
        range("search.max_results"),
        (
            Some(SEARCH_MAX_RESULTS_MIN as u64),
            Some(SEARCH_MAX_RESULTS_CAP as u64)
        )
    );
    assert_eq!(
        range("tools.ask_budget"),
        (Some(ASK_BUDGET_MIN as u64), Some(ASK_BUDGET_MAX as u64))
    );
    // 全局 max_tokens：页面显示的「可填 X–Y」要与写侧/读侧同一个边界
    assert_eq!(
        range("model.max_tokens"),
        (
            Some(crate::bot::config::schema::MIN_MAX_TOKENS as u64),
            Some(crate::bot::config::schema::MAX_MAX_TOKENS as u64)
        )
    );
    // 痕迹保留期：手动清理的输入框公示「1–365」，与实际删的范围同源
    assert_eq!(
        range("trace.retention_days"),
        (
            Some(crate::db::trace::TRACE_RETENTION_MIN_DAYS as u64),
            Some(crate::db::trace::TRACE_RETENTION_MAX_DAYS as u64)
        )
    );
    // 工具输出截断：0 是合法值（显式关闭截断），不是下界 1
    assert_eq!(
        range("tools.max_output_bytes"),
        (Some(0), Some(MAX_TOOL_OUTPUT_BYTES_CAP as u64))
    );
    // 范围两端都要真的被钳制函数认（否则展示与生效又是一回事）
    assert_eq!(clamp_max_rounds(MAX_ROUNDS_MIN), MAX_ROUNDS_MIN);
    assert_eq!(clamp_max_rounds(MAX_ROUNDS_CAP), MAX_ROUNDS_CAP);
    assert_eq!(
        clamp_max_function_calls(MAX_FUNCTION_CALLS_CAP),
        MAX_FUNCTION_CALLS_CAP
    );
    assert_eq!(clamp_ask_budget(ASK_BUDGET_MAX), ASK_BUDGET_MAX);
    assert_eq!(
        clamp_search_max_results(SEARCH_MAX_RESULTS_CAP),
        SEARCH_MAX_RESULTS_CAP
    );
}

/// 空配置 → 全部默认来源（可编辑区）；只读区恒 hardcoded
#[test]
fn effective_params_defaults_and_hardcoded() {
    let views = effective_params(&empty_cfg());
    let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
    // 均衡档全局池 100 回合 → 主会话 60% = 60 轮
    assert_eq!(by_key("loop.max_rounds").value, "60");
    assert_eq!(by_key("loop.max_rounds").source, "default");
    assert_eq!(by_key("schedule.scan_secs").source, "hardcoded");
    assert!(!by_key("schedule.scan_secs").editable);
    assert_eq!(views.len(), PARAMS_TABLE.len());
}

/// 配置覆盖 → source=config 且值跟随；钳制在 resolve_* 生效
#[test]
fn effective_params_reflect_config_and_clamp() {
    let mut cfg = empty_cfg();
    cfg.max_rounds = Some(999);
    cfg.subagent_max_turns = Some(1);
    cfg.search_max_results = Some(99);
    let views = effective_params(&cfg);
    let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
    assert_eq!(
        by_key("loop.max_rounds").value,
        "500",
        "表展示生效值：999 已被 resolve 钳到全局回合硬顶 500"
    );
    assert_eq!(by_key("loop.max_rounds").source, "config");
    // resolve_* 才是行为入口：钳 5..=500
    assert_eq!(resolve_max_rounds_cfg(&cfg), 500);
    assert_eq!(resolve_subagent_budget(&cfg).max_turns, 1);
    assert_eq!(
        by_key("search.max_results").value,
        "10",
        "表展示生效值：99 已钳到硬顶 10"
    );
    assert_eq!(resolve_search_max_results(&cfg), 10, "钳到硬顶 10");
    // 非法推理强度：下游静默回退 medium，展示同步
    cfg.reasoning_effort = Some("foobar".into());
    let views = effective_params(&cfg);
    let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
    assert_eq!(
        by_key("model.reasoning_effort").value,
        "medium",
        "非法档位展示回退后的生效值"
    );
}

/// resolve 链：子 agent 预算两项生效 + 各自钳制；工具调用走**独立键**
/// `subagent_max_tool_calls`（主会话 60% / 子 agent 20%，同一池的两份配额）
#[test]
fn resolve_subagent_budget_independent_fields() {
    let mut cfg = empty_cfg();
    cfg.subagent_max_tool_calls = Some(9999);
    let b = resolve_subagent_budget(&cfg);
    assert_eq!(b.max_turns, DEFAULT_SUBAGENT_MAX_TURNS);
    assert_eq!(
        b.max_tool_calls, 1200,
        "子 agent 工具数独立于主会话，钳 1..=全局工具池硬顶 1200"
    );
}

/// resolve 链：未配置 max_function_calls 时子 agent 工具调用落内置默认 100
#[test]
fn resolve_subagent_budget_tool_calls_default() {
    let b = resolve_subagent_budget(&empty_cfg());
    assert_eq!(b.max_tool_calls, DEFAULT_SUBAGENT_MAX_TOOL_CALLS);
}

/// resolve 链：历史预算钳 20K..=500K
#[test]
fn resolve_history_budget_clamps() {
    let mut cfg = empty_cfg();
    cfg.history_budget_chars = Some(100);
    assert_eq!(resolve_history_budget_chars(&cfg), HISTORY_BUDGET_MIN);
    cfg.history_budget_chars = Some(999_999_999);
    assert_eq!(resolve_history_budget_chars(&cfg), HISTORY_BUDGET_MAX);
    cfg.history_budget_chars = None;
    assert_eq!(
        resolve_history_budget_chars(&cfg),
        DEFAULT_HISTORY_BUDGET_CHARS
    );
}

/// 思考段回灌开关：只有 0/1 两态，别的值一律夹到 1；未配置 = 关。
#[test]
fn reasoning_echo_is_two_state_and_defaults_off() {
    assert_eq!(reasoning_echo_enabled_value(None), 0);
    assert!(!reasoning_echo_on(None));
    assert_eq!(reasoning_echo_enabled_value(Some(0)), 0);
    assert!(!reasoning_echo_on(Some(0)));
    assert_eq!(reasoning_echo_enabled_value(Some(1)), 1);
    assert!(reasoning_echo_on(Some(1)));
    // 越界值夹到上界（当作开），不是夹成关——写侧已钳过，这里是读侧兜底
    assert_eq!(reasoning_echo_enabled_value(Some(9)), 1);
}

/// 思考段回灌行的可填范围是 0–1，与钳制边界同源。
#[test]
fn reasoning_echo_param_row_range_matches_clamp() {
    let views = effective_params(&empty_cfg());
    let v = views
        .iter()
        .find(|v| v.key == "loop.reasoning_echo_enabled")
        .expect("参数表应有思考段回灌这一条");
    assert_eq!(v.min, Some(0));
    assert_eq!(v.max, Some(REASONING_ECHO_ENABLED_MAX as u64));
    assert!(v.editable);
    assert_eq!(v.default, "0", "内置默认是关");
}

// ── 三档任务模式 ──

/// 三档的六项画像必须与定稿文档逐个对齐。
///
/// 这条是「定稿总表」与代码之间的**对拍闸**：文档改数而代码没改（或反之）
/// 即红。数值写死在这里是刻意的——它就是那份文档的可执行副本。
#[test]
fn mode_presets_match_the_spec_table() {
    assert_eq!(
        LIGHT_PRESET,
        ModePreset {
            global_turns: 60,
            global_tool_calls: 150,
            history_budget_chars: 80_000,
            max_tokens: 8_192,
            tool_output_bytes: 64 * 1024,
        },
        "轻量档与定稿表不符"
    );
    assert_eq!(
        BALANCED_PRESET,
        ModePreset {
            global_turns: 100,
            global_tool_calls: 250,
            history_budget_chars: 100_000,
            max_tokens: 16_384,
            tool_output_bytes: 128 * 1024,
        },
        "均衡档（默认）与定稿表不符"
    );
    assert_eq!(
        DEEP_PRESET,
        ModePreset {
            global_turns: 250,
            global_tool_calls: 700,
            history_budget_chars: 300_000,
            max_tokens: 32_768,
            tool_output_bytes: 200 * 1024,
        },
        "深度档与定稿表不符"
    );
}

/// 全局硬顶三档共用：回合 500 / 工具 1200。
#[test]
fn global_caps_are_shared_by_all_modes() {
    assert_eq!(GLOBAL_TURNS_CAP, 500);
    assert_eq!(GLOBAL_TOOL_CALLS_CAP, 1_200);
    // 每档的预设都必须在硬顶之下
    for mode in [TaskMode::Light, TaskMode::Balanced, TaskMode::Deep] {
        let p = preset_for(mode);
        assert!(
            p.global_turns <= GLOBAL_TURNS_CAP,
            "{} 回合超硬顶",
            mode.as_str()
        );
        assert!(
            p.global_tool_calls <= GLOBAL_TOOL_CALLS_CAP,
            "{} 工具数超硬顶",
            mode.as_str()
        );
    }
}

/// 档位解析：非法字面量回落默认档（均衡）。
#[test]
fn task_mode_falls_back_to_balanced_on_garbage() {
    assert_eq!(TaskMode::from_cfg(None), TaskMode::Balanced);
    assert_eq!(TaskMode::from_cfg(Some("light")), TaskMode::Light);
    assert_eq!(TaskMode::from_cfg(Some("deep")), TaskMode::Deep);
    assert_eq!(TaskMode::from_cfg(Some("balanced")), TaskMode::Balanced);
    assert_eq!(TaskMode::from_cfg(Some("LIGHT")), TaskMode::Light);
    assert_eq!(
        TaskMode::from_cfg(Some("nonsense")),
        TaskMode::Balanced,
        "非法档位必须回落默认档，不能崩也不能默默取别的档"
    );
}

/// 分配比例：主会话 60% / 单个子 agent 20% / 单个技能 15%，合计 95%。
#[test]
fn budget_allocation_ratios_are_pinned() {
    assert_eq!(MAIN_SESSION_BUDGET_PCT, 60);
    assert_eq!(SUBAGENT_BUDGET_PCT, 20);
    assert_eq!(SKILL_BUDGET_PCT, 15);
    assert_eq!(
        MAIN_SESSION_BUDGET_PCT + SUBAGENT_BUDGET_PCT + SKILL_BUDGET_PCT,
        95,
        "三者合计 95%，留 5% 余量"
    );
}

/// 档位切换真的改生效值：轻量 < 均衡 < 深度。
#[test]
fn switching_mode_changes_effective_budgets() {
    let mut cfg = empty_cfg();
    let mut seen = Vec::new();
    for mode in ["light", "balanced", "deep"] {
        cfg.task_mode = Some(mode.into());
        seen.push((
            resolve_max_rounds_cfg(&cfg),
            resolve_main_tool_calls(&cfg),
            resolve_history_budget_chars(&cfg),
            resolve_max_tokens(&cfg),
        ));
    }
    for i in 0..seen.len() - 1 {
        assert!(seen[i].0 < seen[i + 1].0, "回合数未随档位递增：{:?}", seen);
        assert!(seen[i].1 < seen[i + 1].1, "工具数未随档位递增：{:?}", seen);
        assert!(
            seen[i].2 < seen[i + 1].2,
            "历史预算未随档位递增：{:?}",
            seen
        );
        assert!(
            seen[i].3 < seen[i + 1].3,
            "输出上限未随档位递增：{:?}",
            seen
        );
    }
    // 均衡档（默认）：回合 60 / 工具 150 / 历史 100K / 输出 16384
    assert_eq!(seen[1].0, 60);
    assert_eq!(seen[1].1, 150);
    assert_eq!(seen[1].2, 100_000);
    assert_eq!(seen[1].3, 16_384);
}

/// 高级覆盖优先于档位预设：用户手填的值是他明确要的，档位只是默认值。
#[test]
fn advanced_override_wins_over_mode_preset() {
    let mut cfg = empty_cfg();
    cfg.task_mode = Some("light".into());
    cfg.max_rounds = Some(77);
    cfg.max_function_calls = Some(88);
    cfg.history_budget_chars = Some(99_000);
    cfg.max_tokens = Some(4_096);
    assert_eq!(resolve_max_rounds_cfg(&cfg), 77);
    assert_eq!(resolve_main_tool_calls(&cfg), 88);
    assert_eq!(resolve_history_budget_chars(&cfg), 99_000);
    assert_eq!(resolve_max_tokens(&cfg), 4_096);
}

/// 子 agent 与主会话是同一池的两份配额：主 60% / 子 20%，互不占用。
#[test]
fn main_and_subagent_take_separate_shares() {
    let mut cfg = empty_cfg();
    cfg.task_mode = Some("balanced".into());
    let b = resolve_subagent_budget(&cfg);
    assert_eq!(resolve_max_rounds_cfg(&cfg), 60, "主会话拿 60%");
    assert_eq!(b.max_turns, 20, "子 agent 拿 20%");
    assert_eq!(resolve_main_tool_calls(&cfg), 150, "主会话工具数拿 60%");
    assert_eq!(b.max_tool_calls, 50, "子 agent 工具数拿 20%");
}
