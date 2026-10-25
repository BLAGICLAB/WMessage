use super::*;

fn test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

fn len_of(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> usize {
    crate::db::lock::lock_or_recover(
        &crate::app_state::sched_running(app),
        "bot_scheduler::tests::sched_running (test)",
    )
    .len()
}

#[test]
fn sched_guard_blocks_reentry_until_drop_on_injected_state() {
    let app = test_handle();
    let guard = SchedGuard::acquire(&app, "t-sched-1").expect("首次获取应成功");
    assert!(
        SchedGuard::acquire(&app, "t-sched-1").is_none(),
        "持有期间不得重复获取"
    );
    assert_eq!(len_of(&app), 1, "注入实例里应有 1 条在跑");
    drop(guard);
    // 注意：守卫必须绑定到变量——`assert!(SchedGuard::acquire(..).is_some())` 里它是临时值，
    // 语句结束即 Drop，会把刚插入的 id 又清掉（本测试第一次就是这么写错的）。
    let again = SchedGuard::acquire(&app, "t-sched-1").expect("Drop 后应可再获取");
    assert_eq!(
        len_of(&app),
        1,
        "Drop 清的必须是同一个实例（清错了这里就是 0）"
    );
    drop(again);
}

#[test]
fn sched_guard_isolated_between_injected_instances() {
    let a = test_handle();
    let b = test_handle();
    let _ga = SchedGuard::acquire(&a, "same-id").expect("a 首次");
    let gb = SchedGuard::acquire(&b, "same-id")
        .expect("两个注入实例之间不得互相干扰（同一 id 应各自可持有）");
    assert_eq!(len_of(&a), 1);
    assert_eq!(len_of(&b), 1);
    drop(gb);
    assert_eq!(len_of(&b), 0, "Drop b 的守卫只应该清 b 的表");
    assert_eq!(len_of(&a), 1, "a 的登记不得被 b 的守卫清掉");
}
