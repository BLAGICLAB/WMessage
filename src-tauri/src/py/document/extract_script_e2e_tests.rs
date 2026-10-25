//! 真 python 执行层复现：不支持的扩展名 → 失败原因必须出现在 stderr

use super::*;

#[test]
fn unsupported_extension_reports_reason_on_stderr() {
    let probe = std::process::Command::new("python3")
        .arg("--version")
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("[skip] 无 python3，跳过脚本 e2e");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("a.txt");
    std::fs::write(&target, b"plain").unwrap();
    std::fs::write(
        tmp.path().join("params.json"),
        serde_json::json!({ "path": target.to_string_lossy() }).to_string(),
    )
    .unwrap();
    std::fs::write(tmp.path().join("s.py"), EXTRACT_SCRIPT).unwrap();
    let out = std::process::Command::new("python3")
        .arg("s.py")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "不支持的格式必须失败退出");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("不支持的格式"),
        "失败原因必须进 stderr（否则 script_fail_err 拿到空 detail）：{stderr}"
    );
}
