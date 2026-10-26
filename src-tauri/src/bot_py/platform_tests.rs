//! 平台相关的回归锁（开发模式 dll 候选必须 debug 门控）。

/// 回归锁：开发模式 dll 候选（env!("CARGO_MANIFEST_DIR") 绝对路径）
/// 必须 cfg(debug_assertions) 门控——否则构建机路径烧进发布二进制
#[test]
fn dev_dll_candidate_is_debug_gated() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/env.rs")).unwrap();
    let pos = text
        .find("dotnet/WmDocxRevisions/bin/Release/net8.0")
        .expect("开发模式 dll 候选必须存在");
    // 字符安全截取（中文注释多字节，字节下标切片会 panic）
    let tail: String = text[..pos]
        .chars()
        .rev()
        .take(500)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    assert!(
        tail.contains("#[cfg(debug_assertions)]"),
        "CARGO_MANIFEST_DIR dll 候选必须 debug 门控: {tail:?}"
    );
}

/// 回归锁：绿色包随包 apphost exe 直跑候选必须存在且优先于 dll
///（self-contained 发布免装 .NET；framework-dependent apphost 自动找共享运行时）
#[test]
fn bundled_exe_entry_preferred_over_dll() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/env.rs")).unwrap();
    let fn_pos = text
        .find("fn dotnet_revisions_entry()")
        .expect("入口定位函数必须存在");
    let body: String = text[fn_pos..].chars().take(1500).collect();
    let exe_hit = body
        .find("tool_exe.is_file()")
        .expect("必须有随包 exe 候选");
    let dll_hit = body.find("dll.is_file()").expect("必须有 dll 候选");
    assert!(exe_hit < dll_hit, "随包 exe 候选必须先于 dll 判定");
}

/// 回归锁：Unix 脚本必须注入父进程看门狗（macOS 无 Job Object 等价物，
/// 主进程崩溃时睡眠型失控脚本靠 RLIMIT_CPU 管不住）
///
/// 注入点从「拼进脚本正文」改成「写进执行入口」：拼进正文会让 traceback
/// 行号整体下移，模型照报错行号改脚本就会改错行。守卫随之改为盯住注入链：
/// run_python_ungated → setup_run_dir → entry_source → PARENT_WATCHDOG。
#[cfg(unix)]
#[test]
fn unix_scripts_get_parent_watchdog() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/runtime.rs")).unwrap();
    assert!(text.contains("PARENT_WATCHDOG"), "必须有看门狗前导常量");
    let entry_pos = text
        .find("fn entry_source()")
        .expect("entry_source 必须存在");
    let entry: String = text[entry_pos..].chars().take(800).collect();
    assert!(
        entry.contains("PARENT_WATCHDOG"),
        "看门狗必须在执行入口里注入（覆盖全部 Python 执行入口）"
    );
    let setup_pos = text
        .find("pub fn setup_run_dir(")
        .expect("setup_run_dir 必须存在");
    let setup: String = text[setup_pos..].chars().take(1200).collect();
    assert!(
        setup.contains("entry_source()"),
        "运行目录必须写入含看门狗的执行入口"
    );
    let ungated_pos = text
        .find("fn run_python_ungated(")
        .expect("run_python_ungated 必须存在");
    let ungated: String = text[ungated_pos..].chars().take(2000).collect();
    assert!(
        ungated.contains("setup_run_dir("),
        "Python 执行入口必须经 setup_run_dir 落盘（看门狗才覆盖得到）"
    );
}

/// pid 复用防护——死 pid / 非组首不得杀（getpgid 失败返回 -1）
#[cfg(unix)]
#[test]
fn group_leader_check_rejects_dead_pid() {
    assert!(
        !super::is_live_group_leader(u32::MAX - 1),
        "不存在的 pid 不得判为组首"
    );
}
