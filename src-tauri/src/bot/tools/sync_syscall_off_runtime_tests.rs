//! 早退路径审计事件序列单测（tool.call 配平 tool.return）。
// 此处覆盖 parse_task_files_arg 单测；
// execute_tool 的 mock 路径见 skill_e2e.rs。

/// async 工具函数里的同步 syscall 必须包 spawn_blocking_map。
///
/// async runtime 线程上跑同步 syscall 会阻塞全部 Tauri command / event
/// ——extract_path_check 里的 per-task canonicalize 循环尤其明显（任务多、
/// 绑定文件多时是几十上百次 syscall）。同文件 sanitize_task_files_arg 早
/// 就是这个范式。
///
/// 行为级用例测不出「跑在哪个线程上」，故扫源码钉住接线。
#[test]
fn async_tool_paths_do_not_run_sync_syscalls_inline() {
    // 工具实现拆成 bot/tools/ 子模块后，源码锁要覆盖整棵生产子树
    // （门面 + 四个生产模块）；测试文件不入扫描——本文件自己就写着这两个签名，
    // 扫进去会先命中字面量，断言就变成在比谁先出现。
    let src = format!(
        "{}\n{}\n{}\n{}\n{}",
        include_str!("../tools.rs"),
        include_str!("common.rs"),
        include_str!("doc.rs"),
        include_str!("service.rs"),
        include_str!("task.rs"),
    );
    for (sig, label) in [
        ("async fn extract_path_check(", "extract_path_check"),
        (
            "pub(crate) async fn tool_link_file_to_task(",
            "tool_link_file_to_task",
        ),
    ] {
        let start = src
            .find(sig)
            .unwrap_or_else(|| panic!("应能找到 {label} 的定义"));
        let rest = &src[start..];
        // 函数体到顶格 "}" 为止（嵌套块的 "}" 都带缩进，只有顶层收尾是顶格）
        let end = rest[1..].find("\n}\n").map(|i| i + 1).unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            body.contains("spawn_blocking_map"),
            "{label} 的同步 syscall 必须包 spawn_blocking_map：{body}"
        );
        assert!(
            !body.contains("std::fs::canonicalize"),
            "{label} 体内不得直接调 std::fs::canonicalize（必须包进 spawn_blocking_map）：{body}"
        );
    }
}
