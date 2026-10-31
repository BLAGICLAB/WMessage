use super::*;

/// L5 接线守卫：钩子层必须**真被 `execute_tool_impl` 调到**，且排在
/// 「工具已执行」之后、「审计 preview 与结果回灌」之前。
///
/// 变异实测把 dispatch 里那段调用整段删掉（工具结果再也过不了钩子）时，
/// hooks 模块 17 条 + registry 与 bot_onions 全部 57 条测试照样全绿——
/// 它们验的是注册表的行为，验不到 dispatch 有没有接线。所以这里钉住
/// 「语句存在 + 三点相对位置」，钉的是接线而不是中间量。
#[test]
fn post_tool_layer_is_wired_between_execution_and_audit() {
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    let body = body
        .split_once("async fn execute_tool_impl(")
        .expect("找不到 execute_tool_impl")
        .1
        .split("\n}\n")
        .next()
        .expect("execute_tool_impl 应以顶格 } 收尾");
    let exec = body.find("(t.call)(&ctx, args)").expect("找不到工具执行点");
    let hook = body
        .find("run_post_tool(&call, &mut text)")
        .expect("execute_tool_impl 必须调用 post_tool 钩子层：结果回灌前必须过钩子");
    // 「tool.return」在函数里有好几处（三个早退出口 + 末尾那次）——锚点取
    // **最后一次**：末尾那次才是工具执行完之后的审计，前几处在早退分支里、
    // 排在钩子之前，拿来当锚点会把顺序判反。
    let audit = body.rfind("\"tool.return\"").expect("找不到工具返回审计");
    let refill = body
        .find("skill_on_step_post")
        .expect("找不到结果回灌/技能后置钩子");
    assert!(
        exec < hook,
        "钩子必须在工具执行之后（拿得到结果才谈得上改写）"
    );
    assert!(
        hook < audit,
        "钩子必须在审计 preview 之前：否则记的是一份模型从没见过的文本"
    );
    assert!(
        hook < refill,
        "钩子必须在结果回灌之前：否则改写不会到达模型"
    );
}

/// 主聊天路径的 panic 隔离守卫：工具 future 必须包 catch_unwind。
///
/// 工具 panic → future unwind → 这一轮对话整轮丢失（无部分输出、无错误提示），
/// 前端停在「正在思考」。子 agent runner 早有同样的隔离
/// （`bot_orchestrator::run_subagent`），主链路必须有——**同一份工具代码
/// 不该有两种健壮性**。hooks 扩展点三层都做了 fail-open，被扩展的本体
/// 反而没有。
#[test]
fn tool_execution_is_panic_isolated() {
    let src = include_str!("../dispatch.rs");
    let body = src
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    // 内置工具与 MCP 兜底路由都要走隔离层。
    // 判据按「调用点之前有没有出现过隔离层的名字」——不按整行文本：
    // rustfmt 一换行整行匹配就失效，而失效的守卫往往是被改断言绕过的。
    // （用 rfind 而不是定长字节窗口：窗口偏移可能落在多字节字符中间。）
    let call_pos = body.find("(t.call)(&ctx, args)").expect("找不到工具执行点");
    assert!(
        body[..call_pos].rfind("run_tool_panic_isolated").is_some(),
        "内置工具执行必须走 panic 隔离层"
    );
    let mcp_pos = body.find("execute_mcp_tool").expect("找不到 MCP 兜底路由");
    assert!(
        body[..mcp_pos].rfind("run_tool_panic_isolated").is_some(),
        "MCP 兜底路由也要走 panic 隔离层"
    );
    // 隔离层本身必须真的 catch，且把 panic 归一成失败（不是成功调用）
    // 匹配函数名不带括号：泛型化后名字后面是 `<R, F>`，带括号就匹配不到。
    let iso = body
        .split_once("fn run_tool_panic_isolated")
        .expect("找不到 panic 隔离层")
        .1;
    assert!(
        iso.contains("catch_unwind"),
        "隔离层必须真的 catch_unwind（panic 要归一成普通失败回灌给模型）"
    );
    assert!(
        iso.contains("ToolResult::error"),
        "panic 必须归一成失败——算成成功调用会污染幻觉守卫的成功凭证表"
    );
}

/// panic 的**行为**测试：工具炸了不许把这一轮带走。
///
/// 归一成 `Error` 而不是「成功」：成功凭证表会被污染（幻觉守卫查不到
/// 真实成功记录，反过来给模型背书）。
#[tokio::test]
async fn panic_in_tool_is_normalized_into_an_error_result() {
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let fut = async move {
        // black_box 挡住编译期折叠，编译器才不会把另一支判成不可达
        if std::hint::black_box(true) {
            panicking_tool()
        } else {
            crate::bot::registry::ToolResult::ok("没炸", Vec::new())
        }
    };
    let r = super::run_tool_panic_isolated(&handle, "boom_tool", fut).await;
    assert_eq!(
        r.status,
        crate::bot::registry::ToolStatus::Error,
        "panic 必须归一成失败，不能算成成功调用"
    );
    assert!(
        r.text.contains("工具执行异常"),
        "失败文案要说清是异常中断：{}",
        r.text
    );
}

/// 一个只会炸的工具（模拟工具内部 panic）。
fn panicking_tool() -> crate::bot::registry::ToolResult {
    panic!("工具内部炸了");
}

fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
}

fn keys(kv: &[(&'static str, String)]) -> Vec<&'static str> {
    kv.iter().map(|(k, _)| *k).collect()
}

#[test]
fn hook_interrupt_balances_tool_call_with_same_shape_as_existing_early_returns() {
    // 配平形状必须与既有两处 early-return 分支同口径（统计面板按
    // tool.call / tool.return 配对，不配平 = 悬挂调用）。
    let trace = ToolCallTrace {
        turn: Some(4),
        tool_call_id: Some("call_hk".into()),
    };
    let evs = hook_interrupt_events("write_file", "本轮不允许写盘", 7, Some("sess-9"), &trace);
    let names: Vec<&str> = evs.iter().map(|(_, e, _)| *e).collect();
    assert_eq!(names, ["pre_tool.hook_interrupt", "tool.return"]);
    assert!(evs
        .iter()
        .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
    assert_eq!(kv_get(&evs[0].2, "reason"), Some("本轮不允许写盘"));
    // 与 early_return_events 的 tool.return 同键集：tool / reason / exit_code /
    // duration_ms + trace 段。
    let baseline = early_return_events(
        "write_file",
        "denied_by_rule",
        7,
        None,
        Some("sess-9"),
        &trace,
    );
    let baseline_ret = &baseline[1].2;
    let ret = &evs[1].2;
    assert_eq!(kv_get(ret, "exit_code"), Some("none"));
    assert_eq!(kv_get(ret, "duration_ms"), Some("7"));
    assert_eq!(
        kv_get(ret, "reason"),
        Some("pre_tool_hook_interrupt"),
        "reason 要能区分拒绝来源（规则表 vs 钩子）"
    );
    let mut a = keys(baseline_ret);
    let mut b = keys(ret);
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b, "tool.return 的 kv 键集必须与既有 early-return 一致");
    // 第三处早退（子 agent 白名单）原先是手写字面量，与另两处没有任何
    // 机制保证一致。现在三处都走 tool_return_kv 一致性由构造保证，
    // 本断言只钉住「没有哪一处绕开它自己手搓 kv」。
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    // 每处早退分支：从「事件名」到下一个 return 之间那段——手搓 kv 就在
    // 这段里（键集只由 tool_return_kv 决定）。
    for (event, label) in [
        ("subagent_whitelist_deny\",", "子 agent 白名单"),
        ("tool_args_invalid_json", "入参 JSON 非法"),
    ] {
        let branch = body
            .split_once(event)
            .unwrap_or_else(|| panic!("找不到{label}早退分支"))
            .1
            .split("return crate::bot::registry::ToolResult::denied(")
            .next()
            .expect("早退分支应在此处返回");
        assert!(
            branch.contains("tool_return_kv("),
            "{label}分支必须走 tool_return_kv 构造 tool.return：{branch}"
        );
        assert!(
            !branch.contains("exit_code"),
            "{label}分支不得自己手搓 tool.return 的 kv（键集只由 tool_return_kv 决定）：{branch}"
        );
    }
    assert_eq!(kv_get(ret, "turn"), Some("4"));
    assert_eq!(kv_get(ret, "tool_call_id"), Some("call_hk"));
    assert_eq!(kv_get(ret, "session_id"), Some("sess-9"));
}

#[test]
fn subagent_whitelist_gate_judges_on_the_intersection() {
    // 接线守卫：闸的判据是 ctx.tools（profile ∩ per-onion allowed），
    // 不是 profile 白名单。只收窄 schema 不收窄闸等于没收窄——模型看不见
    // 的工具仍能硬猜名字调通配置档明令禁用的能力。
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    let gate = body
        .split_once("if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id) {")
        .expect("找不到子 agent 白名单闸的入口")
        .1
        .split("subagent_whitelist_deny\",")
        .next()
        .expect("闸分支应一直到 deny 事件");
    assert!(
        gate.contains(concat!("ctx.tools.", "contains(&name)")),
        "闸的判据必须是 ctx.tools（交集后的全集）：{gate}"
    );
    assert!(
        !gate.contains(concat!("profile_whitelist(", "ctx.profile)")),
        "闸不得退回 profile 白名单（那样 allowed 收窄形同虚设）：{gate}"
    );
}

/// 收尾窗放行清单的漂移守卫：每个 profile 都必须有可用的写产物工具与
/// read_own_card——清单里出现 profile 白名单没有的工具名，收尾窗就对该
/// profile 整体关闭（写了也过不了白名单闸），触顶等于零产出路径复活。
#[test]
fn wrap_up_tools_exist_in_every_profile_whitelist() {
    use crate::bot::constants::WRAP_UP_TOOLS;
    for profile in [
        crate::db::SubagentProfile::Research,
        crate::db::SubagentProfile::Coder,
        crate::db::SubagentProfile::General,
    ] {
        let whitelist = crate::bot::registry::profile_whitelist(profile);
        assert!(
            whitelist.contains(&"read_own_card"),
            "{:?} 缺 read_own_card：收尾窗内模型将无法重读契约",
            profile
        );
        let has_writer = WRAP_UP_TOOLS
            .iter()
            .any(|t| *t != "read_own_card" && whitelist.contains(t));
        assert!(
            has_writer,
            "{:?} 无任何收尾窗写产物工具，触顶后零产出路径复活",
            profile
        );
    }
}

/// 子 agent 路径不得再有自己的工具预算闸：阶梯判定单点在模型循环层
/// （fuse::gate_call），dispatch 只留执行计数仪表。两处各算一份会分叉——
/// 一处按 95% 开收尾窗、另一处按 100% 拒读，模型看到的是两套互相打架的提示。
#[test]
fn dispatch_has_no_budget_gate_of_its_own() {
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    assert!(
        !body.contains("fetch_update"),
        "dispatch 不得再做预算 acquire（阶梯在模型循环层）"
    );
    assert!(
        !body.contains("WRAP_UP_TOOLS"),
        "dispatch 不得再判收尾窗白名单（判据单点在 fuse::gate_call）"
    );
    assert!(
        body.contains("used_tool_calls\n            .fetch_add(1,"),
        "执行计数仪表必须保留（read_own_card 余量与收尾触达判定读它）"
    );
}

/// 动作流接线守卫：call 相位 1 处（白名单后）+ return 相位 2 处
/// （坏入参拒 / 正常执行完）。
/// 少一处，前端那行动作就永远挂着「进行中」或整条不显示。
#[test]
fn subagent_activity_emits_cover_call_and_all_return_paths() {
    let production = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    assert_eq!(
        production.matches("emit_subagent_activity(").count(),
        4,
        "动作流 emit 应为 4 处（1 定义 + 1 call + 2 return）——少了某条拒绝路径就没有结局"
    );
}

#[test]
fn hook_interrupt_text_is_classified_as_tool_failure() {
    // ⚠️ 前缀缺失时 Skill DSL 调度器会把拒跑记成步骤成功（scheduler 只看
    // tool_call_failed 判定），拒跑必须被判失败。
    let msg = format!("⚠️ {}", "本轮不允许写盘");
    assert!(
        crate::audit::tool_call_failed("write_file", &msg),
        "钩子拒绝文案必须被判失败：{msg}"
    );
}

/// 门禁拒绝的文案必须同时满足两件事：status=Denied（成败判定）+
/// 文本可被 classify_tool_failure 认出子类（error_class 细分 + 转换历史
/// 消息的 Anthropic 层靠它设 is_error）。
///
/// 这条测试盯的是实际发出去的文案。历史上规则 deny / 用户拒绝 / 白名单外
/// 这几条既不带「⚠️」也不带「失败：」，classify 全部落空 → 返回 None
/// → Skill DSL 调度器把被拒步骤记成成功、trace span 记 ok=true。
#[test]
fn every_gate_denial_is_denied_status_and_classifiable_text() {
    use crate::bot::registry::{ToolResult, ToolStatus};
    // 与 execute_tool_impl 里的门禁早退点一一对应（rendered 文案）
    let cases: Vec<(&str, ToolResult)> = vec![
        (
            "pre_tool 钩子拒绝",
            ToolResult::denied("⚠️ 本轮不允许写盘"),
        ),
        (
            "allowed-tools 集合外",
            ToolResult::denied(
                "⚠️ 技能「limited-skill」的 allowed-tools 未包含工具 write_file（声明集合：read_text_file, grep_files），已拒绝执行",
            ),
        ),
        (
            "工具规则 deny",
            ToolResult::denied("⚠️ 工具「run_python」已被工具规则禁用（设置页授权卡可调整）。如任务确实需要，请向用户说明。"),
        ),
        (
            "确认被拒",
            ToolResult::denied("用户拒绝：工具「delete_task」未获确认（后台无人应答）。"),
        ),
        (
            "skill 熔断",
            ToolResult::denied("skill「日报」超过最大步数上限（8 步），已强制终止"),
        ),
        (
            "子 agent 白名单外",
            ToolResult::denied(
                "⚠️ 子 agent 白名单外工具「edit_file」已拒绝（profile=Research，本次可用 8 个）。只能使用白名单内工具；遇到阻塞在收尾 JSON 的 blockers 里如实记录。",
            ),
        ),
        (
            "arguments 非法 JSON",
            ToolResult::denied(
                "⚠️ 工具「create_task」的 arguments 不是合法 JSON，已拒绝执行（未做任何改动）：expected value。",
            ),
        ),
    ];
    for (label, r) in &cases {
        assert_eq!(
            r.status,
            ToolStatus::Denied,
            "{label}：门禁早退必须是 Denied，用 warn 会被调度器/trace 记成成功"
        );
        assert!(
            !r.status.is_success(),
            "{label}：Denied 的 is_success 必须为 false"
        );
        assert!(
            crate::audit::tool_call_failed("t", &r.text),
            "{label}：文案必须能被 classify_tool_failure 认出，否则 error_class 与 Anthropic is_error 全丢：{}",
            r.text
        );
        assert!(
            crate::audit::classify_error_class(&r.text).is_some(),
            "{label}：error_class 不应为空"
        );
    }
    // 审计级别：被拒是预期内控制流，不该刷 ERROR
    for (label, r) in &cases {
        assert_eq!(
            crate::audit::AuditLevel::from_tool_status(r.status),
            crate::audit::AuditLevel::Warn,
            "{label}：Denied 映射 WARN（正常控制流），ERROR 只留给真执行失败"
        );
    }
}

/// 防止后来者把门禁早退又写成 ToolResult::warn —— 那正是本条修复的回归形态。
///
/// 全文件口径：dispatch 的早退只有两类语义——闸/拒绝类一律 Denied，工具
/// 本体自己跑完返回的结果一律由 tools.rs 构造。`ToolResult::warn` 在本文件
/// 不该有生产用途（真要表达「做成但有告警」应改用 `ok` + 告警文本，或在
/// 工具本体里显式 warn）。
#[test]
fn no_gate_early_return_uses_warn_status() {
    let production = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    assert!(
        !production.contains("ToolResult::warn("),
        "dispatch 的生产代码不得构造 ToolResult::warn（会把「没执行」记成成功）"
    );
    // 8 处：pre_tool 钩子拒绝 / allowed-tools 集合外 / 规则 deny / 规则 ask
    // 对子 agent 直接拒 / 确认被拒 / skill 熔断 / 子 agent 白名单外 / arguments
    // 非法 JSON。
    // （工具调用预算的阶梯在模型循环层，dispatch 不再有预算类早退；墙钟整套已取消。）
    // 数量变了请同步本测试与 every_gate_denial_* 的用例表。
    assert_eq!(
        production.matches("ToolResult::denied(").count(),
        8,
        "门禁早退点应恰好 8 处"
    );
}

/// ToolStatus 四态的成败语义：Warn 算成功（做成了但有告警），Error/Denied 不算。
#[test]
fn tool_status_success_semantics() {
    use crate::bot::registry::ToolStatus;
    assert!(ToolStatus::Ok.is_success());
    assert!(ToolStatus::Warn.is_success());
    assert!(!ToolStatus::Error.is_success());
    assert!(!ToolStatus::Denied.is_success());
}

#[test]
fn hook_failure_kv_names_hook_kind_and_error() {
    // fail-open 的审计必须能定位到「哪个钩子、哪种失败、什么错」，
    // 否则钩子里的 bug 在 bot.log 里无从排查。
    let trace = ToolCallTrace {
        turn: Some(1),
        tool_call_id: Some("call_x".into()),
    };
    let kv = hook_failure_kv(
        "builtin_observer",
        "panic",
        "boom",
        "read_text_file",
        Some("sess-2"),
        &trace,
    );
    assert_eq!(kv_get(&kv, "hook"), Some("builtin_observer"));
    assert_eq!(kv_get(&kv, "kind"), Some("panic"));
    assert_eq!(kv_get(&kv, "err"), Some("boom"));
    assert_eq!(kv_get(&kv, "tool"), Some("read_text_file"));
    assert_eq!(kv_get(&kv, "session_id"), Some("sess-2"));
    assert_eq!(kv_get(&kv, "turn"), Some("1"));
    assert_eq!(kv_get(&kv, "tool_call_id"), Some("call_x"));
}

#[test]
fn pre_tool_layer_is_wired_at_the_entry_of_the_three_gates() {
    // 纯函数单测钉不住「钩子层是否真接在三道闸入口」：把 run_pre_tool_layer
    // 挪到闸后（或写成调用还在但不可达的形状），其余单测照样全绿——
    // 而这一期的全部价值就是「每个工具调用都过一次钩子」。
    // execute_tool_impl 需要 Wry AppHandle，单测够不到，故以源码文本钉住
    // 调用位置（与 invalid_args_gate_is_wired_into_execute_tool_impl 同款理由）。
    let src = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    let body = src
        .split_once("async fn execute_tool_impl(")
        .expect("找不到 execute_tool_impl")
        .1
        .split("\n}\n")
        .next()
        .expect("execute_tool_impl 应以顶格 } 收尾");
    let call = body.find("tool.call").expect("找不到 tool.call 审计");
    let hook = body
        // 钉 `if let Some(...) = ` 而不只是调用本身：变异实测把它改成
        // `let _ = run_pre_tool_layer(...)`（seam 彻底死亡、钩子永远不
        // 影响任何工具调用）时，四条顺序断言照样全绿——只搜裸调用串会
        // 给 B3 一份假保证。这里钉的是**返回值被消费**。
        .find("if let Some(interrupted) = run_pre_tool_layer(")
        .expect("execute_tool_impl 必须消费 pre_tool 钩子层的判定并早退");
    let hook_return = body
        .find("return interrupted;")
        .expect("钩子 Interrupt 必须早退（否则拒绝判定被丢弃）");
    let rule_gate = body.find("tool_rule_gate(app, name").expect("找不到规则闸");
    let subagent_gate = body
        .find("crate::tool_guard::subagent_ctx(session_id)")
        .expect("找不到子 agent 白名单闸");
    let args_gate = body
        .find("if let Some(msg) = invalid_args_message(name, args)")
        .expect("找不到入参 JSON 闸");
    assert!(call < hook, "钩子层在 tool.call 之后（否则审计不成对）");
    assert!(
        hook < hook_return,
        "早退必须紧跟钩子层（中间插别的东西说明判定被丢弃或改写）"
    );
    assert!(
        hook < rule_gate && hook < subagent_gate && hook < args_gate,
        "钩子层必须插在三道闸入口之前：规则闸 {rule_gate} / 子 agent 闸 {subagent_gate} / 入参闸 {args_gate}，钩子 {hook}"
    );
}

#[test]
fn pre_tool_layer_passes_through_when_registry_has_no_denial() {
    // 无 AppHandle 的纯函数面：注册表默认全放行时判定为 Continue
    // （fail-open 与「无钩子拒绝」两条路径在 dispatch 侧都落到 None）。
    let trace = ToolCallTrace::default();
    let call = ToolCallView {
        tool: "read_text_file",
        args: "{}",
        session_id: None,
        trace: &trace,
    };
    let run = crate::bot::hooks::registry().run_pre_tool(&call);
    assert_eq!(run.decision, PreToolVerdict::Continue);
    assert!(
        run.failures.is_empty(),
        "内置钩子不得自报错：{:?}",
        run.failures
    );
}

/// 规则闸 Ask 分支的子 agent 短路：子 agent 是后台会话，确认窗无人认领
/// （停滞 600s 才被接替），且批准后子 agent 白名单闸仍会拒——先弹窗等于
/// 「批准了也无效」的空转。锚定「子 agent 检查在弹窗调用之前」（接线锚点，
/// 同 post_tool 层守卫的理由：行为测试够不到 dispatch 的接线顺序）。
#[test]
fn rule_gate_ask_branch_short_circuits_for_subagent_before_popup() {
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    let ask_branch = body
        .split_once("Some(crate::bot_fs::ToolRuleAction::Ask) => {")
        .expect("找不到规则闸 Ask 分支")
        .1;
    let subagent = ask_branch
        .find("is_subagent_session(session_id)")
        .expect("Ask 分支必须先判子 agent 会话：后台会话弹窗无人认领，直接拒");
    let popup = ask_branch
        .find("ask_user_confirm(")
        .expect("Ask 分支应保留主会话的确认弹窗");
    assert!(
        subagent < popup,
        "子 agent 短路必须在 ask_user_confirm 之前，否则弹窗照弹"
    );
}
