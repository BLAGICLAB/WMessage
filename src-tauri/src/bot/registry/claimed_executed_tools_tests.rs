use super::{claimed_executed_tools, default_execution_claim_patterns};

fn claimed(text: &str) -> Vec<&'static str> {
    claimed_executed_tools(text, &default_execution_claim_patterns())
}

/// 清单写法要点名得出来：成功标记与工具名同行。
///
/// 真机 trace 101 的原话形态：零工具调用，终稿报「✅ 调用成功」。成功标记
/// （✅）是内置的，不受设置页词表影响——词表被删空这道也得在。
#[test]
fn checklist_form_names_the_tool() {
    assert_eq!(
        claimed("工具调用如下：\n- create_word：✅ 调用成功，返回路径 `/tmp/x.docx`"),
        vec!["create_word"]
    );
    assert_eq!(
        claimed("- create_word（周报20261020）：✅ 调用成功，返回路径 `/tmp/x.docx`"),
        vec!["create_word"]
    );
}

/// 失败形态不点名：如实汇报工具不可用是诚实行为。
///
/// 真机 trace 103 的原话形态。词表收的是完成态措辞，「调用失败 / 调用被拒」
/// 不在其中。
#[test]
fn failure_form_is_not_a_claim() {
    assert!(claimed("- run_python：调用失败，返回错误 `[ErrNo -7]`").is_empty());
    assert!(claimed("- complete_task：❌ 任务不存在，调用被拒").is_empty());
}

/// 建议语气不点名：提到工具名不等于声称调过它。
///
/// 词在名字前面（「任务已经执行完了，你可以用 grep_files 搜」）也不算——
/// 那个「执行完」说的是任务，不是 grep_files。
#[test]
fn advice_form_is_not_a_claim() {
    assert!(claimed("这个问题你可以用 grep_files 搜，但我没权限。").is_empty());
    assert!(claimed("任务我已经执行完了，但这一步你可以用 grep_files 搜，我没权限。").is_empty());
}

/// 执行措辞与工具名同句才算是「点名了这次执行」——分句边界是这道闸的护栏。
///
/// 标题那行带执行措辞、工具名在下一行，说的是两件事：全局判一次会把如实
/// 汇报失败也拖下水。
#[test]
fn claim_word_must_share_the_sentence_with_the_tool_name() {
    assert!(claimed("工具调用结果：\n- run_python：调用失败").is_empty());
    assert_eq!(
        claimed("工具调用如下：\n- create_word：✅ 调用成功"),
        vec!["create_word"]
    );
}

/// 别名同样要点名得出来：模型写「子 agent」而表里是 spawn_subagent。
#[test]
fn alias_form_names_the_tool() {
    assert_eq!(
        claimed("子 agent 跑 sleep(5) 实测约 5013 ms。"),
        vec!["spawn_subagent"]
    );
}
