//! 指纹必须对「影响连接/行为的字段」敏感。env/headers 是机密字段，
//! `Serialize` 里被 `skip_serializing` 剥掉——用序列化值算指纹就看不见它们，
//! 改了 API token 不换指纹 = 不重连 = 旧凭据继续用（配置已保存、行为没变）。
use super::*;

fn cfg() -> McpServerConfig {
    McpServerConfig {
        id: "s1".into(),
        name: "fs".into(),
        transport: "stdio".into(),
        command: Some("npx".into()),
        ..Default::default()
    }
}

#[test]
fn fingerprint_changes_when_env_or_headers_change() {
    let fp = fingerprint(&cfg());
    let mut with_env = cfg();
    with_env.env.insert("TOKEN".into(), "v1".into());
    assert_ne!(
        fingerprint(&with_env),
        fp,
        "env 变了必须换指纹（否则改了机密不重连）"
    );
    let mut with_env2 = cfg();
    with_env2.env.insert("TOKEN".into(), "v2".into());
    assert_ne!(
        fingerprint(&with_env2),
        fingerprint(&with_env),
        "env 的值变了必须换指纹"
    );
    let mut with_headers = cfg();
    with_headers
        .headers
        .insert("Authorization".into(), "Bearer x".into());
    assert_ne!(fingerprint(&with_headers), fp, "headers 变了必须换指纹");
}

#[test]
fn fingerprint_is_stable_for_identical_config() {
    assert_eq!(fingerprint(&cfg()), fingerprint(&cfg()));
}
