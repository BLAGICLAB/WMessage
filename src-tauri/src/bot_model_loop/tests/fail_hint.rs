//! 工具失败后的「换策略」提示：按工具分流。

use crate::bot_model_loop::core::fail_hint_for;

/// run_python 失败必须指向「落盘再改那一行」，不能只说「换策略」。
///
/// 回归背景：内联代码出错时，模型最常见的反应是把整段代码重写一遍——每次重写
/// 都重付一遍全文的输出 token，且改错无从对照（对不上原来哪一行）。一次 18 页
/// PPT 的脚本改三遍就是三遍全文重写。
#[test]
fn run_python_failure_hint_points_to_save_then_edit() {
    let hint = fail_hint_for("run_python", "SyntaxError: invalid syntax", 1);
    assert!(
        hint.contains("write_file"),
        "提示应指示先把代码落盘：{hint}"
    );
    assert!(
        hint.contains("scriptPath"),
        "提示应指明用 scriptPath 执行落盘脚本：{hint}"
    );
    assert!(
        hint.contains("edit_file"),
        "提示应指示只改出错的那几行：{hint}"
    );
    assert!(
        hint.contains("不要把整段代码重写一遍"),
        "提示应明确禁止整段重写：{hint}"
    );
}

/// 连续失败两次时补上「禁止再以相同方式调用」——通用提示词里已有这条，
/// run_python 这条不能因为特化就丢掉。
#[test]
fn run_python_failure_hint_keeps_repeat_guard_on_second_failure() {
    let hint = fail_hint_for("run_python", "boom", 2);
    assert!(
        hint.contains("已连续失败 2 次"),
        "第二次失败应带连续失败计数：{hint}"
    );
    assert!(
        hint.contains("禁止再以相同方式调用该工具"),
        "第二次失败应带禁止重复调用：{hint}"
    );
}

/// 其他工具走通用提示词，不带 run_python 的落盘指引。
#[test]
fn other_tools_keep_the_generic_failure_hint() {
    let hint = fail_hint_for("create_ppt", "参数缺失", 1);
    assert!(
        hint.contains("换参数、换工具"),
        "通用提示应保留换策略指引：{hint}"
    );
    assert!(
        !hint.contains("scriptPath"),
        "非 run_python 工具不该带 scriptPath 指引：{hint}"
    );
}
