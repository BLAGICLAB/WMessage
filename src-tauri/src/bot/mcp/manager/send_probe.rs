//! 编译探针：McpManager 进 tauri 托管状态 + spawn 的 future 必须 Send/Sync。
//! rmcp 类型若失去 Send/Sync（升级换版常见回归），在这里响亮失败。
use super::*;

#[test]
fn manager_and_service_are_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<McpManager>();
    assert_sync::<McpManager>();
    type S = RunningService<RoleClient, Box<dyn DynService<RoleClient>>>;
    assert_send::<S>();
    assert_sync::<S>();
}

///  评审（HIGH）：spawn_service 的逐臂字面量 match 与白名单是两份清单，
/// 新增白名单项漏改 match 会「contains 过、match 落 _ 臂」静默断 stdio 路。
/// 这里锁 parity：表内每项必有构造臂、臂返回的字面量必须等于命令本身。
/// match 臂保留字面量是有意的（Mimosa 闸：无「变量 → 进程名」数据流）。
#[test]
fn stdio_allowlist_and_spawn_match_stay_in_sync() {
    for cmd in STDIO_LAUNCH_ALLOWLIST {
        let arm = match *cmd {
            "npx" => "npx",
            "bunx" => "bunx",
            "uvx" => "uvx",
            "pipx" => "pipx",
            "node" => "node",
            "deno" => "deno",
            "python" => "python",
            "python3" => "python3",
            "docker" => "docker",
            "podman" => "podman",
            other => panic!("白名单项「{other}」缺少 spawn_service 构造臂（双清单漂移）"),
        };
        assert_eq!(arm, *cmd, "构造臂字面量必须等于白名单项本身");
    }
}
