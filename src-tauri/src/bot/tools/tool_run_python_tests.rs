use crate::bot::tools::doc::{script_source, ScriptSource};

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
