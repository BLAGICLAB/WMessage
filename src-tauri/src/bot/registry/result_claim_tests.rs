//! 工具单源真相的守卫相关纯函数：结果值抽取与回声判定。
use super::{
    claimed_result_numbers, default_execution_claim_patterns, is_echo_call, result_numbers_in,
    states_execution_not_done,
};

fn patterns() -> Vec<String> {
    default_execution_claim_patterns()
}

/// 判据 A 的正例：执行完成态措辞 + 结果措辞 + 数值，同一段里齐了才摘。
#[test]
fn claimed_result_numbers_picks_execution_result_values() {
    assert_eq!(
        claimed_result_numbers("已跑完，输出 5050", &patterns()),
        vec!["5050".to_string()]
    );
    assert_eq!(
        claimed_result_numbers("执行完毕，返回 8191", &patterns()),
        vec!["8191".to_string()]
    );
    // 小数与千分位照原样摘（出处比对那侧再归一）
    assert_eq!(
        claimed_result_numbers("我跑了，结果是 2.5", &patterns()),
        vec!["2.5".to_string()]
    );
}

/// 算术与讲解句不摘：没有执行措辞，它们说的不是执行产物。
/// 「1+1 的结果是 2」是 SPEC 里点名不收「结果是」的原因。
#[test]
fn claimed_result_numbers_skips_arithmetic_and_explanations() {
    assert!(claimed_result_numbers("1+1 的结果是 2", &patterns()).is_empty());
    assert!(claimed_result_numbers("这个函数返回了 None", &patterns()).is_empty());
    // 计数不是结果：3 是数出来的个数
    assert!(claimed_result_numbers("已跑完，共 3 步", &patterns()).is_empty());
    // 估算不是确切值
    assert!(claimed_result_numbers("实测约 5013 ms", &patterns()).is_empty());
    // 编号不是结果
    assert!(claimed_result_numbers("已执行，规则 21/19 这条我会卡死", &patterns()).is_empty());
}

/// 路径、日期、标识符里的数字不是结果值。
#[test]
fn result_numbers_in_skips_identifiers_paths_and_dates() {
    assert!(result_numbers_in("/Users/renshi/Desktop/2026Q3复盘.docx").is_empty());
    assert!(result_numbers_in("2026-10-20 的结果").is_empty());
    assert!(result_numbers_in("第 21 条").is_empty());
    assert!(result_numbers_in("1. 把新内容写进已有文件").is_empty());
    assert_eq!(result_numbers_in("输出 5050"), vec!["5050".to_string()]);
}

/// 判据 B 的正例：模型把结论写进参数、让工具原样吐回来。
/// 真机原形（trace 106）：browser_evaluate 的函数体是硬编码字符串。
#[test]
fn is_echo_call_detects_hardcoded_assertion_echoed_back() {
    let args = r#"{"function":"async () => { return { desktopDocx: 'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found' }; }"}"#;
    let output = "### Result\n{\n  \"desktopDocx\": \"only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\"\n}\n### Ran Playwright code\n```js\nawait page.evaluate('async () => { return { desktopDocx: \\'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\\' }; }');\n```\n### Page\n- Page URL: about:blank";
    assert!(
        is_echo_call(args, output),
        "返回内容全来自入参 = 这次调用没替模型核实任何东西"
    );
}

/// 判据 B 的负例：真的去读了现场，返回里有入参没有的东西。
#[test]
fn is_echo_call_accepts_calls_that_retrieved_new_content() {
    let args = "import os\np = \"/Users/renshi/Desktop/季度复盘.docx\"\nprint(\"exists:\", os.path.exists(p))";
    let output =
        "exists: False\nisfile: False\ndocx on desktop: ['/Users/renshi/Desktop/2026Q3复盘.docx']";
    assert!(
        !is_echo_call(args, output),
        "返回里有入参没有的现场信息，不算回声"
    );
    // 目录列举：文件名不在入参里
    assert!(!is_echo_call(
        r#"{"dir":"/Users/renshi/Desktop","pattern":"*.docx"}"#,
        "/Users/renshi/Desktop（1 条）：\n2026Q3复盘.docx"
    ));
}

/// 返回得太少判不出「有没有新信息」——不猜，不算回声。
#[test]
fn is_echo_call_needs_enough_content_to_judge() {
    assert!(!is_echo_call(r#"{"code":"print(\"OK\")"}"#, "OK"));
}

/// 让开条件：终稿如实说明执行没做成（或明说那是推算）→ 结果值判据整档让开。
#[test]
fn states_execution_not_done_covers_failure_and_derivation_forms() {
    assert!(states_execution_not_done("我跑了，但开关没开，跑不了。"));
    assert!(states_execution_not_done("答案是 5050（按公式推算的）"));
    assert!(states_execution_not_done(
        "run_python 调用失败：Python 编程未开启"
    ));
    // 正常的执行完成态终稿不该命中让开
    assert!(!states_execution_not_done("已跑完，输出 5050"));
}
