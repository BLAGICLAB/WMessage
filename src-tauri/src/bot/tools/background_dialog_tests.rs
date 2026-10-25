/// 回归锁：后台执行（interactive=false）不得弹系统文件选择框——
/// tool_bind_file 已下线（bind_files 复数形是前端走的），现仅 extract_document
/// 无 path 后台必须拒绝。源码锁防回退。
#[test]
fn background_execution_never_pops_file_dialog() {
    // 工具实现拆成 bot/tools/ 子模块后，源码锁要覆盖整棵生产子树
    // （门面 + 四个生产模块）；测试文件不入扫描——死函数回退锁盯的是生产代码。
    let text = format!(
        "{}\n{}\n{}\n{}\n{}",
        include_str!("../tools.rs"),
        include_str!("common.rs"),
        include_str!("doc.rs"),
        include_str!("service.rs"),
        include_str!("task.rs"),
    );
    assert!(
        !text.contains(concat!("tool_bind", "_file(")),
        "tool_bind_file 死函数不得回退（前端走 bind_files 复数形）"
    );
    assert!(
        text.contains("后台执行不能弹窗选文件，请提供 path 参数"),
        "extract_document 无 path 后台必须拒绝"
    );
}
