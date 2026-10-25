/// `ChatGuard` / `ExecGuard` 迁入 `AppState` 后的守卫语义与实例隔离。
/// 这两个守卫此前没有单测（只由 `tests/task_chat_exec.rs` 间接覆盖），而它们正是
/// 「同会话/同卡并发重复执行」的唯一闸门；Drop 清错实例 = 闸门泄漏。
use super::*;

fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

#[test]
fn chat_guard_blocks_same_session_and_releases_on_drop() {
    let app = test_handle();
    let guard = ChatGuard::acquire(&app, Some("sess-guard-1")).expect("首次应可进入");
    assert!(
        ChatGuard::acquire(&app, Some("sess-guard-1")).is_err(),
        "同会话并发应被拒"
    );
    assert!(
        ChatGuard::acquire(&app, Some("sess-other")).is_ok(),
        "别的会话不受影响"
    );
    assert!(chat_guard_is_held(&app, "sess-guard-1"));
    drop(guard);
    assert!(
        !chat_guard_is_held(&app, "sess-guard-1"),
        "Drop 后应释放（清错实例这里就会是 true）"
    );
    // 无会话 id 不加锁：连续两次都应成功
    assert!(ChatGuard::acquire(&app, None).is_ok());
    assert!(ChatGuard::acquire(&app, None).is_ok());
}

#[test]
fn guards_isolated_between_injected_instances() {
    let a = test_handle();
    let b = test_handle();
    let _ga = ChatGuard::acquire(&a, Some("same-sess")).expect("a 首次");
    assert!(
        ChatGuard::acquire(&b, Some("same-sess")).is_ok(),
        "两个注入实例之间不得互相干扰（ChatGuard）"
    );
    let _ea = ExecGuard::acquire(&a, "same-task").expect("a 首次");
    assert!(
        ExecGuard::acquire(&b, "same-task").is_some(),
        "两个注入实例之间不得互相干扰（ExecGuard）"
    );
    // 反向：注入实例与兜底实例也必须互不可见
    let bare = tauri::test::mock_app().handle().clone();
    assert!(
        ChatGuard::acquire(&bare, Some("same-sess")).is_ok(),
        "未注入的 handle 走兜底实例，不得看到注入实例里的会话"
    );
}
