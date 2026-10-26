use crate::bot::tools::doc::{run_failed, script_source, ScriptSource};

/// 退出码非 0 必须算失败。
///
/// 回归背景：run_python 曾经完全不看退出码，脚本崩了也报「成功」——模型
/// 于是以为跑通了，只把报错当普通输出，接着把整段脚本重写一遍。失败提示
/// （别整段重写、只改出错那几行）也因此一次都没触发。
#[test]
fn nonzero_exit_code_counts_as_failure() {
    assert!(!run_failed(Some(0)), "退出码 0 是成功");
    assert!(run_failed(Some(1)), "非零退出码必须算失败");
    assert!(run_failed(None), "没有退出码 = 异常终止，不能报成功");
}

/// scriptPath 优先于内联 code。
///
/// 回归背景：模型改了脚本却仍传旧 code 时，执行旧 code 会让「改了文件没生效」
/// 变成看不见的坑——文件是新的、跑的是旧的。
#[test]
fn script_source_prefers_script_path_over_inline_code() {
    let v = serde_json::json!({
        "scriptPath": "/tmp/a.py",
        "code": "print(1)",
    });
    assert_eq!(
        script_source(&v),
        ScriptSource::File("/tmp/a.py".to_string())
    );
}

/// 只有 code → 内联。
#[test]
fn script_source_falls_back_to_inline_code() {
    let v = serde_json::json!({ "code": "print(1)" });
    assert_eq!(
        script_source(&v),
        ScriptSource::Inline("print(1)".to_string())
    );
}

/// scriptPath 是空串 → 当没给（空串不是路径）。
#[test]
fn script_source_ignores_blank_script_path() {
    let v = serde_json::json!({ "scriptPath": "   ", "code": "print(1)" });
    assert_eq!(
        script_source(&v),
        ScriptSource::Inline("print(1)".to_string())
    );
}

/// 两个都没给 → Missing（工具层据此报错说明缺哪个）。
#[test]
fn script_source_reports_missing_when_neither_given() {
    assert_eq!(script_source(&serde_json::json!({})), ScriptSource::Missing);
}
