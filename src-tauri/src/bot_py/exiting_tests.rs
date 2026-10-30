//! EXITING 复查必须落在许可闸门内的回归锁。

/// 回归锁（改有界并发后平移）：EXITING 复查必须在许可闸门内完成——
/// 排队者过闸（拿到许可）即复查，挡住「kill 完成后才排到」的请求。
/// 全局标志不在测试里翻转（会污染并行测试的 run_python），源码锁防回退。
#[test]
fn exiting_check_is_inside_gate_acquire() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/runtime.rs")).unwrap();
    // 生产入口必须过闸门（函数体边界：到下一个顶层收括号，防「闸门调用只在
    // 后续辅助函数里」的假阳性——评审 M 采纳）
    let fn_pos = text
        .find("pub fn run_python(")
        .expect("run_python 必须存在");
    let run_body_end = text[fn_pos..]
        .find("\n}\n")
        .expect("run_python 顶层收括号必须存在");
    let run_body = &text[fn_pos..fn_pos + run_body_end];
    assert!(
        run_body.contains("py_gate_acquire_with_stop(stop)"),
        "run_python 必须过并发闸门"
    );
    // 闸门内核：EXITING 终检必须在等待循环体内、且位于成功取许可分支之后
    //（「拿到许可后复查」这条防线不许被挪到循环外变死代码——评审 M 采纳）
    let gate_pos = text
        .find("fn py_gate_acquire_inner(")
        .expect("闸门内核必须存在");
    let gate_body_end = text[gate_pos..]
        .find("\n}\n")
        .expect("闸门内核顶层收括号必须存在");
    let gate_body = &text[gate_pos..gate_pos + gate_body_end];
    let loop_pos = gate_body.find("loop {").expect("闸门内核必须有等待循环");
    let success_pos = gate_body
        .find("if *avail > 0")
        .expect("闸门内核必须有成功取许可分支");
    let check = gate_body
        .rfind("EXITING.load")
        .expect("闸门内必须有退出复查");
    assert!(
        check > loop_pos && check > success_pos,
        "EXITING 终检必须在等待循环内且位于成功取许可分支之后（拿到许可后复查）"
    );
}
