//! 启动白名单与状态机：白名单逐臂构造 + 只有活着的状态算占名额。

use super::*;

// ── 白名单与状态 ──

#[test]
fn build_command_rejects_non_allowlisted_launcher() {
    let r = build_command("bash", &[], Path::new("/tmp"));
    assert!(r.is_err(), "白名单外的启动器必须拒绝");
    let msg = r.err().unwrap_or_default();
    assert!(msg.contains("白名单"), "错误文案要点明白名单：{msg}");
}

#[test]
fn build_command_accepts_allowlisted_launcher() {
    let r = build_command("node", &["--version".to_string()], Path::new("/tmp"));
    assert!(r.is_ok(), "白名单内的启动器必须放行");
}

#[test]
fn build_command_allowlist_matches_mcp_source_of_truth() {
    // 与 MCP 侧同一份白名单（单一事实源）：多出来的启动器都该被拒
    for cmd in STDIO_LAUNCH_ALLOWLIST {
        assert!(
            build_command(cmd, &[], Path::new("/tmp")).is_ok(),
            "白名单内的 {cmd} 应放行"
        );
    }
    for cmd in ["bash", "sh", "/bin/sh", "curl", "sh -c"] {
        assert!(
            build_command(cmd, &[], Path::new("/tmp")).is_err(),
            "{cmd} 不在白名单内，必须拒绝"
        );
    }
}

#[test]
fn state_active_only_for_live_states() {
    assert!(ServiceState::Starting.is_active());
    assert!(ServiceState::Running.is_active());
    assert!(ServiceState::Stopping.is_active());
    assert!(!ServiceState::Exited.is_active());
    assert!(!ServiceState::Failed.is_active());
}
