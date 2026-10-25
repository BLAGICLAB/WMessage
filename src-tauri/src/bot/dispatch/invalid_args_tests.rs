use super::*;

#[test]
fn empty_args_pass_as_no_arg_tools() {
    // 无参工具常态：空串 / 纯空白 / 只有换行都放行
    assert!(invalid_args_message("query_tasks", "").is_none());
    assert!(invalid_args_message("query_tasks", "   ").is_none());
    assert!(invalid_args_message("get_current_time", "\n\t ").is_none());
}

#[test]
fn parseable_args_pass_including_non_object_json() {
    // 只拦「解析不了」，不越权改判定：合法 JSON 一律放行
    // （部分网关/老模型会发 null / 数组形态，那是既有行为，不在本次收紧范围）
    assert!(invalid_args_message("create_task", r#"{"title":"x"}"#).is_none());
    assert!(invalid_args_message("create_task", "null").is_none());
    assert!(invalid_args_message("create_task", "{}").is_none());
}

#[test]
fn malformed_args_return_actionable_message_for_model() {
    let msg = invalid_args_message("create_task", r#"{"title":"x""#)
        .expect("非空但坏 JSON 必须拒绝并回可解释错误");
    assert!(msg.contains("create_task"), "文案须指名工具：{msg}");
    assert!(msg.contains("不是合法 JSON"), "{msg}");
    assert!(
        msg.contains("已拒绝执行") && msg.contains("未做任何改动"),
        "须说明未执行，模型才会换参数重试而不是以为已生效：{msg}"
    );
    // serde 的真实错因带出来，模型才知道是截断还是引号坏
    assert!(
        msg.contains("EOF while parsing"),
        "须带 serde 原始错因：{msg}"
    );
}

#[test]
fn malformed_args_message_is_classified_as_failure() {
    // 不带 ⚠️ 前缀就落不进 classify_tool_failure 的门禁臂：
    // Skill DSL 调度器会把拒跑记成步骤成功（scheduler 只看 tool_call_failed）
    let msg = invalid_args_message("create_task", "{坏}").expect("坏 JSON 必须拒");
    assert!(
        crate::audit::tool_call_failed("create_task", &msg),
        "拒跑文案必须被判失败：{msg}"
    );
}

#[test]
fn invalid_args_gate_is_wired_into_execute_tool_impl() {
    // 纯函数单测钉不住「闸是否真接在执行路径上」：变异实测把闸改成不可达
    // （if true { None } else { 闸 }），1511 个 lib 测试照样全绿——而这个提交
    // 的全部价值就是「工具不再执行」。execute_tool_impl 需要 Wry AppHandle，
    // 集成测试的 MockRuntime 够不到，故以源码文本钉住「语句形状 + 位置」。
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
    // 钉语句本身而非函数名：写成 if true { None } else { 闸(...) } 这类
    // 「调用还在但不可达」的形状也必须红
    let gate = body
        .find("if let Some(msg) = invalid_args_message(name, args)")
        .expect("execute_tool_impl 必须以闸的 if let 直接调用：坏 JSON 不得进工具");
    let hook = body
        .find("skill_on_step(app, name, args, session_id)")
        .expect("找不到 skill 步骤钩子");
    let lookup = body
        .find("tools_index().get(name)")
        .expect("找不到工具查表");
    assert!(
        hook < gate,
        "入参闸应在 skill 钩子之后（熔断原因比 JSON 错因更该先给模型）"
    );
    assert!(
        gate < lookup,
        "入参闸必须在工具查表与执行之前，否则等于没拦"
    );
}

#[test]
fn skill_variable_breakage_is_rejected_not_silently_nulled() {
    // bot_skills::vars 的已知限制（见该文件同名单测）：占位符紧邻转义引号时
    // 原样插入，结果是非合法 JSON。这里必须拒绝（原先静默 Null 后按缺参跑工具）
    let broken = r#"{"a": "say \"hi\" he said "no""}"#;
    assert!(
        invalid_args_message("create_task", broken).is_some(),
        "DSL 变量替换破坏的 JSON 必须被闸挡住"
    );
}
