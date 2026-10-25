//! 端口 / 监听地址闸：flag 解析、loopback 判定、L1-L3 判据、保留集。

use super::*;

// ── P2-③：端口 / 监听地址闸 ──

#[test]
fn parse_listen_flags_reads_common_forms() {
    let args = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };

    // --port 5173
    let f = parse_listen_flags(&args(&["--port", "5173"]));
    assert_eq!(f.port, Some(5173));
    assert_eq!(f.host, None);

    // --port=5173
    let f = parse_listen_flags(&args(&["--port=5173"]));
    assert_eq!(f.port, Some(5173));

    // -p 5173
    let f = parse_listen_flags(&args(&["-p", "5173"]));
    assert_eq!(f.port, Some(5173));

    // --host 0.0.0.0
    let f = parse_listen_flags(&args(&["--host", "0.0.0.0"]));
    assert_eq!(f.host.as_deref(), Some("0.0.0.0"));

    // --host=127.0.0.1
    let f = parse_listen_flags(&args(&["--host=127.0.0.1"]));
    assert_eq!(f.host.as_deref(), Some("127.0.0.1"));

    // --listen 0.0.0.0:8080（地址与端口一起给）
    let f = parse_listen_flags(&args(&["--listen", "0.0.0.0:8080"]));
    assert_eq!(f.host.as_deref(), Some("0.0.0.0"));
    assert_eq!(f.port, Some(8080));

    // --bind 127.0.0.1（只有地址）
    let f = parse_listen_flags(&args(&["--bind", "127.0.0.1"]));
    assert_eq!(f.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(f.port, None);

    // 无关参数不误伤
    let f = parse_listen_flags(&args(&["--verbose", "--outdir", "/tmp"]));
    assert_eq!(f.host, None);
    assert_eq!(f.port, None);
}

#[test]
fn parse_listen_flags_ignores_a_trailing_flag_without_value() {
    let args = vec!["--port".to_string()];
    let f = parse_listen_flags(&args);
    assert_eq!(f.port, None);
}

#[test]
fn is_loopback_tells_wildcard_apart() {
    assert!(is_loopback("127.0.0.1"));
    assert!(is_loopback("127.1.2.3"));
    assert!(is_loopback("::1"));
    // 通配不是 loopback——这正是 L1 要拦的
    assert!(!is_loopback("0.0.0.0"));
    assert!(!is_loopback("::"));
    assert!(!is_loopback("192.168.1.10"));
}

#[test]
fn judge_sockets_flags_non_loopback_bind() {
    let socks = vec![ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: "0.0.0.0".to_string(),
        port: 7000,
    }];
    let reserved = HashSet::new();
    match judge_sockets(&socks, true, &reserved, false) {
        GateVerdict::Violated {
            rule,
            address,
            port,
        } => {
            assert_eq!(rule, ViolationRule::NonLoopback);
            assert_eq!(address, "0.0.0.0");
            assert_eq!(port, 7000);
        }
        other => panic!("应判 L1 违规，实际 {other:?}"),
    }
    // 开关放开 → 不再报 L1
    assert_eq!(
        judge_sockets(&socks, true, &reserved, true),
        GateVerdict::Ok
    );
}

#[test]
fn judge_sockets_flags_reserved_and_privileged_ports() {
    let mut reserved = HashSet::new();
    reserved.insert(4763u16);
    let mk = |port: u16, addr: &str| ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: addr.to_string(),
        port,
    };

    // L2：占了宿主保留端口（地址是 loopback 也照样占）
    match judge_sockets(&[mk(4763, "127.0.0.1")], true, &reserved, false) {
        GateVerdict::Violated { rule, port, .. } => {
            assert_eq!(rule, ViolationRule::ReservedPort);
            assert_eq!(port, 4763);
        }
        other => panic!("应判 L2 违规，实际 {other:?}"),
    }

    // L3：特权端口
    match judge_sockets(&[mk(80, "127.0.0.1")], true, &reserved, false) {
        GateVerdict::Violated { rule, port, .. } => {
            assert_eq!(rule, ViolationRule::PrivilegedPort);
            assert_eq!(port, 80);
        }
        other => panic!("应判 L3 违规，实际 {other:?}"),
    }

    // 普通 loopback 端口 → 没违规
    assert_eq!(
        judge_sockets(&[mk(5173, "127.0.0.1")], true, &reserved, false),
        GateVerdict::Ok
    );
}

#[test]
fn judge_sockets_unsupported_when_sample_failed() {
    // 采集失败（lsof 起不来 / /proc/net/* 不可读）与「确实没人在监听」
    // 在类型层必须分开：前者是**没判**，不是「判了没事」。
    let reserved = HashSet::new();
    // 采空 + 采成功 → Ok（真没人在监听）
    assert_eq!(
        judge_sockets(&[], true, &reserved, false),
        GateVerdict::Ok,
        "采成功但没采到 = 判了没事"
    );
    // 采空 + 采失败 → Unsupported（没判）
    assert_eq!(
        judge_sockets(&[], false, &reserved, false),
        GateVerdict::Unsupported,
        "采集失败不得判成 Ok"
    );
    // 采样失败时即便采到东西也不判——那批数据不可信
    let socks = vec![ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: "0.0.0.0".to_string(),
        port: 7000,
    }];
    assert_eq!(
        judge_sockets(&socks, false, &reserved, false),
        GateVerdict::Unsupported,
        "采样失败时不得据其判违规"
    );
}

#[test]
fn reserved_dynamic_survives_a_failed_sample() {
    // 采样失败时保留集动态部分**不清空**：清空了 L2（防抢占宿主端口）
    // 就跟着一起静默失效了。
    refresh_reserved_ports(&[52001], true);
    assert!(reserved_ports().contains(&52001));
    refresh_reserved_ports(&[], false);
    assert!(
        reserved_ports().contains(&52001),
        "采样失败不得把动态保留集清空"
    );
    // 采样恢复后正常刷新
    refresh_reserved_ports(&[52002], true);
    assert!(reserved_ports().contains(&52002));
    assert!(!reserved_ports().contains(&52001));
}
