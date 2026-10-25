//! 真实子进程端到端：tests/fixtures/mcp_echo_server.py（零依赖手写协议桩）
//! 走完 spawn → initialize → tools/list → tools/call → 懒重连 → shutdown。
use super::*;
use crate::bot::mcp::config::McpServerConfig;
use crate::bot::mcp::tool_cache::ToolCache;

fn echo_cfg(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.into(),
        name: "echo".into(),
        transport: "stdio".into(),
        command: Some("python3".into()),
        args: vec![concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mcp_echo_server.py"
        )
        .into()],
        env: Default::default(),
        url: None,
        headers: Default::default(),
        timeout_secs: Some(30),
        enabled: true,
    }
}

async fn ensure_python3() -> bool {
    // 无 python3 的环境软跳过（不失败）：spawn 一次 `python3 --version` 探测
    tokio::process::Command::new("python3")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 挑一个当前空闲的本地端口（先占再放，让桩服务器去绑）。
fn free_local_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

#[tokio::test]
async fn connect_discover_call_roundtrip() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP stdio e2e");
        return;
    }
    // 与 registry 的「无连接」断言互斥（连接存活窗口会让它 flaky），
    // 并先清一次残留（前序用例 panic 时槽位可能未清）
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    // 走进程级单例（与生产同路径）：阶段 3 挂载层读的就是 shared()
    let mgr = shared();
    let cfg = echo_cfg("e2e-conn");
    mgr.ensure_connected(&cfg)
        .await
        .expect("连接 echo 服务器应成功");
    // 工具发现
    let tools = mgr.tools_of("e2e-conn");
    assert_eq!(tools.len(), 1, "应发现 1 个工具");
    assert_eq!(tools[0].name, "echo");
    assert_eq!(mgr.status_of("e2e-conn").state, "connected");
    // 阶段 3：挂载进主 agent 工具清单 + dispatch 反查
    let with = crate::bot::registry::tools_json_with_mcp(None);
    assert!(
        with.contains("\"mcp_echo_echo\""),
        "主 agent 清单应含 mcp_echo_echo"
    );
    let mounted = crate::bot::mcp::mount::find_mounted("mcp_echo_echo").expect("反查应命中");
    assert_eq!(mounted.tool_name, "echo");
    assert_eq!(mounted.server_id, "e2e-conn");
    // 阶段 6：stderr 管道捕获——桩启动即写一行 stderr，轮询等待环形缓冲收进
    //
    // **在调用之前**断言：按需启停下调用结束要走停止阶梯（SIGINT），
    // Python 桩会打一段 KeyboardInterrupt 回溯——那几行会把启动锚点挤出
    // 「尾部 N 行」。启动诊断的价值本来就在「连不上时看尾巴」，而连接失败
    // 那条路上没有回溯（那才是它要救的场景）。
    let mut stderr_seen = false;
    for _ in 0..20 {
        if mgr
            .stderr_tail_snapshot("e2e-conn", 5)
            .iter()
            .any(|l| l.contains("echo-test stderr ready"))
        {
            stderr_seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(stderr_seen, "stderr 环形缓冲应捕获桩的启动输出");
    // 阶段 4：调用内核（dispatch 路由的下游）——真实远端调用 + 结果整形
    let (direct, direct_bytes) =
        crate::bot::mcp::mount::execute_mcp_direct(&cfg, &mounted, r#"{"text": "路由测试"}"#).await;
    assert_eq!(direct.status, crate::bot::registry::ToolStatus::Ok);
    assert_eq!(direct.text, "echo: 路由测试");
    assert_eq!(
        direct_bytes,
        "echo: 路由测试".len(),
        "未超限结果的原始规模即文本长度"
    );
    // 非法参数 JSON：不再透传 Null 到远端（ 评审）——本地明确回 warn
    // 文本「参数非法」，模型拿着错误自己重发，省一轮下游类型困惑
    let (direct_bad, _) =
        crate::bot::mcp::mount::execute_mcp_direct(&cfg, &mounted, "not-json").await;
    assert_eq!(direct_bad.status, crate::bot::registry::ToolStatus::Warn);
    assert!(
        direct_bad.text.contains("不是合法 JSON"),
        "应本地拒非法参数而非透传远端：{}",
        direct_bad.text
    );
    // 模型编造的工具名：挂载表反查不命中（execute_mcp_tool 会回「未知工具」Error）
    assert!(
        crate::bot::mcp::mount::find_mounted("mcp_echo_nope").is_none(),
        "挂载表外名字不应命中"
    );
    // 按需启停：调用完即回收——活连接退回静默待命，工具清单留着
    assert_eq!(
        mgr.status_of("e2e-conn").state,
        "idle",
        "调用完应转静默待命（没有进程在跑）"
    );
    assert!(
        mgr.connected_children().is_empty(),
        "调用完不该再有子进程挂在连接槽上"
    );
    assert_eq!(
        mgr.tools_of("e2e-conn").len(),
        1,
        "静默待命时工具清单仍可见（模型还得看得见它才调得到）"
    );
    assert!(
        crate::bot::registry::tools_json_with_mcp(None).contains("\"mcp_echo_echo\""),
        "静默待命的服务器照样挂进主 agent 清单"
    );
    // 工具调用
    let result = mgr
        .call_tool(&cfg, "echo", serde_json::json!({ "text": "你好" }))
        .await
        .expect("echo 调用应成功");
    assert_ne!(result.is_error, Some(true), "不应是错误结果");
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert_eq!(text, "echo: 你好");
    mgr.shutdown().await;
    assert_eq!(mgr.status_of("e2e-conn").state, "absent", "shutdown 清槽");
    // 摘除后立即不可见（验收标准：删除/禁用后工具立即不可见）
    assert!(
        !crate::bot::registry::tools_json_with_mcp(None).contains("mcp_echo_echo"),
        "断开后工具应立即从清单消失"
    );
}

/// 按需启停 · 核心断言 1：**打开启用开关不启任何子进程**。
///
/// 这是本次改造的要点：以前 `reload_from_config` 会给每台启用的服务器
/// 连一次（node/npx 一上来就吃掉百来 MB 内存）。现在开关只表示
/// 「允许机器人用这台服务器」，进程留到模型真调用时才拉起。
#[tokio::test]
async fn enabling_a_server_does_not_spawn_a_process() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 按需启停 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let cfg = echo_cfg("e2e-lazy");
    // 配置对齐（= 用户打开启用开关后发生的事）
    mgr.apply_config(std::slice::from_ref(&cfg), &ToolCache::default())
        .await;
    assert!(
        mgr.connected_children().is_empty(),
        "打开启用开关不该拉起任何子进程"
    );
    assert_eq!(
        mgr.status_of("e2e-lazy").state,
        "pending",
        "已启用但工具清单还没发现 → 待发现"
    );
    assert!(
        mgr.tools_of("e2e-lazy").is_empty(),
        "还没发现过工具 → 清单为空"
    );
    mgr.shutdown().await;
}

/// 按需启停 · 核心断言 2：**调用才拉起，调用完即回收**。
///
/// 进程不跨调用存活——这是「按需启停」与「懒重连」的分水岭：
/// 懒重连是「断了再连」，按需启停是「没调用就不连」。
#[tokio::test]
async fn a_tool_call_spawns_the_child_then_reaps_it() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 按需启停 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let cfg = echo_cfg("e2e-call");
    mgr.apply_config(std::slice::from_ref(&cfg), &ToolCache::default())
        .await;
    // 调用前：没有进程
    assert!(mgr.connected_children().is_empty());
    let result = mgr
        .call_tool(&cfg, "echo", serde_json::json!({ "text": "按需" }))
        .await
        .expect("echo 调用应成功");
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert_eq!(text, "echo: 按需");
    // 调用完：进程回收了，但工具清单留着（模型还得看得见它才调得到）
    assert!(
        mgr.connected_children().is_empty(),
        "调用完子进程必须被回收（不跨调用存活）"
    );
    assert_eq!(mgr.status_of("e2e-call").state, "idle");
    assert_eq!(mgr.tools_of("e2e-call").len(), 1, "工具清单要留着");
    mgr.shutdown().await;
}

/// 按需启停 · 核心断言 3：**工具可见性与活连接解耦**。
///
/// 盘上快照里的工具清单，在没有任何进程的情况下照样挂进主 agent 清单——
/// 否则模型看不见这台服务器的工具，就永远不会去调它（死锁）。
#[tokio::test]
async fn cached_tools_are_mounted_without_any_live_connection() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 按需启停 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let mut cfg = echo_cfg("e2e-cached");
    // **换个服务器名**：函数名登记簿是进程级的，同前缀同工具名按序消歧——
    // 用别的名字才不会跟别的用例抢「mcp_echo_echo」这个名字（抢到就变成
    // mcp_echo_echo_2，别用例的精确断言会跟着飘）。
    cfg.name = "cachedpool".into();
    // 造一份「上次发现过」的盘上快照
    let tool = serde_json::from_value(serde_json::json!({
        "name": "echo",
        "description": "回显",
        "inputSchema": {"type": "object", "properties": {}}
    }))
    .expect("工具应能反序列化");
    let mut cache = ToolCache::default();
    cache.put(&cfg.id, fingerprint(&cfg), &cfg.name, vec![tool]);
    mgr.apply_config(std::slice::from_ref(&cfg), &cache).await;
    // 没有任何进程，但工具挂上了
    assert!(mgr.connected_children().is_empty(), "不该有进程");
    assert_eq!(mgr.status_of("e2e-cached").state, "idle", "静默待命");
    let mounted = mcp_function_names();
    // 函数名带消歧后缀是正常的（登记簿是进程级的，同前缀同工具名按序消歧）
    assert!(
        mounted.iter().any(|n| n.starts_with("mcp_cachedpool_echo")),
        "静默待命的服务器的工具要照样挂进主 agent 清单，实际：{mounted:?}"
    );
    mgr.shutdown().await;
}

/// 按需启停 · 核心断言 4：**发现到的工具清单落盘，下次会话不用再连**。
///
/// 落盘是「工具可见性」与「活连接」解耦的载体——没有它，每次对话都得
/// 先连一遍服务器才能拼工具清单（按需就退化成「每轮都启」）。
#[tokio::test]
async fn discovered_tools_are_written_to_the_disk_snapshot() {
    let dir = std::env::temp_dir().join(format!("wmessage-mcp-snap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("临时目录应可建");
    let mgr = McpManager::default();
    *mgr.cache_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
    let cfg = echo_cfg("snap-1");
    let tool = serde_json::from_value(serde_json::json!({
        "name": "echo",
        "description": "回显",
        "inputSchema": {"type": "object", "properties": {}}
    }))
    .expect("工具应能反序列化");
    mgr.write_tool_cache(&cfg, fingerprint(&cfg), &[tool])
        .expect("工具快照应能落盘");
    // 盘上那份能被读回来，且指纹对得上（= 下次启动不用连也能挂工具）
    let cache = ToolCache::load(&dir);
    let entry = cache
        .entry("snap-1", fingerprint(&cfg))
        .expect("快照应命中");
    assert_eq!(entry.tools.len(), 1);
    assert_eq!(entry.tools[0].name, "echo");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 外部 MCP 服务器一条超长行（5MB 的畸形 JSON）不能撑爆宿主内存，
/// 也不能打断会话：读侧行长上限把它截短 → 那条行不是消息 → 客户端不回
/// 响应；后续工具调用照常。
#[tokio::test]
async fn huge_stdio_line_is_truncated_and_session_survives() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 超长行 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let mut cfg = echo_cfg("e2e-flood");
    cfg.env = std::collections::BTreeMap::from([(
        "WM_TEST_FLOOD_BYTES".to_string(),
        (5 * 1024 * 1024).to_string(),
    )]);
    mgr.ensure_connected(&cfg)
        .await
        .expect("超长行不该打断连接");
    assert_eq!(mgr.tools_of("e2e-flood").len(), 1, "工具发现应照常完成");

    let result = mgr
        .call_tool(&cfg, "echo", serde_json::json!({ "text": "你好" }))
        .await
        .expect("超长行之后的调用应成功");
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert_eq!(text, "echo: 你好");

    // 上限真的在生效：超长行被截短 → 不是消息 → 客户端不会回响应。
    // （没有上限时这行会被当成「形状不对的 JSON-RPC」→ rmcp 回 Invalid Request）
    for _ in 0..20 {
        if mgr
            .stderr_tail_snapshot("e2e-flood", 20)
            .iter()
            .any(|l| l.contains("client-reply"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let tail = mgr.stderr_tail_snapshot("e2e-flood", 20);
    assert!(
        !tail.iter().any(|l| l.contains("client-reply")),
        "超长行不该被当成消息（客户端不该回响应）：{tail:?}"
    );
    mgr.shutdown().await;
}

/// 资源采样：已连接的子进程能被采到内存/累计 CPU，第二次采样起才有占用率。
///
/// 口径与受管服务层同一条（bot/procs 的 take_snapshot + tree_sample + cpu_usage_pct）。
#[tokio::test]
async fn child_resources_are_sampled_from_the_process_table() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 资源采样 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let cfg = echo_cfg("e2e-res");
    mgr.ensure_connected(&cfg).await.expect("连接应成功");

    let children = mgr.connected_children();
    let (pid, started_at_ms) = children
        .get("e2e-res")
        .copied()
        .expect("应记下子进程 pid 与起来时刻");
    assert!(crate::bot::procs::pid_exists(pid), "子进程此刻应活着");
    assert!(started_at_ms > 0, "应记下起来时刻");

    // 第一次采样：只有瞬时值，没有占用率（没有前值可比）
    let first = mgr.sample_children_within(&[pid], 0);
    let r1 = first.get(&pid).cloned().expect("应采到资源");
    assert!(r1.rss_bytes > 0, "内存必须采到非零值：{r1:?}");
    assert!(r1.cpu_seconds >= 0.0);
    assert!(
        r1.cpu_pct.is_none(),
        "第一次采样没有前值 → 不该有占用率：{r1:?}"
    );

    // 第二次（TTL=0 → 强制重采）：有前值了 → 有占用率
    let second = mgr.sample_children_within(&[pid], 0);
    let r2 = second.get(&pid).cloned().expect("应采到资源");
    assert!(r2.cpu_pct.is_some(), "第二次采样起应有占用率：{r2:?}");

    mgr.shutdown().await;
}

/// 子进程真在监听端口时，采样要能采到它。
///
/// 桩服务器按 `WM_TEST_LISTEN_PORT` 开一个真实 TCP 监听（127.0.0.1），
/// 断言采样结果里有这条 socket——「面板上显示的监听端口」不是猜的。
#[tokio::test]
async fn child_listening_sockets_are_sampled() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 监听采样 e2e");
        return;
    }
    let Some(port) = free_local_port() else {
        eprintln!("[skip] 拿不到空闲端口，跳过 MCP 监听采样 e2e");
        return;
    };
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    let mut cfg = echo_cfg("e2e-listen");
    cfg.env
        .insert("WM_TEST_LISTEN_PORT".to_string(), port.to_string());
    mgr.ensure_connected(&cfg).await.expect("连接应成功");

    let pid = mgr
        .connected_children()
        .get("e2e-listen")
        .copied()
        .expect("应记下子进程 pid")
        .0;

    // 采样可能要等一拍（子进程刚起来，平台 socket 表可能还没刷新）
    let mut found = false;
    let mut sampled = Vec::new();
    for _ in 0..40 {
        let out = mgr.sample_children_within(&[pid], 0);
        sampled = out.get(&pid).cloned().expect("应采到资源").listening;
        if sampled
            .iter()
            .any(|s| s.port == port && s.address == "127.0.0.1")
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    mgr.shutdown().await;
    assert!(
        found,
        "应采到子进程的监听 socket 127.0.0.1:{port}，实际采到 {sampled:?}"
    );
}

/// 退出/断开后 MCP 子进程必须真的没了——不是「令牌取消了、进程还在」那种孤儿。
///
/// 桩服务器启动时拉一个 setsid 逃逸的子孙（组杀管不到，只能靠 ppid 树回收），
/// 父子 pid 写进文件；断言 shutdown 后两个 pid 都不在了。
#[tokio::test]
async fn shutdown_stops_stdio_child_and_its_escaped_descendant() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP stdio 停止 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();

    let pidfile = std::env::temp_dir().join(format!(
        "wm-mcp-spawner-{}.pid",
        uuid::Uuid::new_v4().simple()
    ));
    let mut cfg = echo_cfg("e2e-stop");
    cfg.args = vec![concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mcp_spawner_server.py"
    )
    .into()];
    cfg.env = std::collections::BTreeMap::from([(
        "MCP_SPAWNER_PIDFILE".to_string(),
        pidfile.to_string_lossy().to_string(),
    )]);
    mgr.ensure_connected(&cfg).await.expect("连接应成功");

    // 桩把「自己 pid + 逃逸子孙 pid」写进文件
    let (server_pid, escaped_pid) = {
        let mut content = String::new();
        for _ in 0..50 {
            if let Ok(c) = std::fs::read_to_string(&pidfile) {
                if !c.trim().is_empty() {
                    content = c;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let mut it = content.split_whitespace();
        (
            it.next().and_then(|v| v.parse::<u32>().ok()),
            it.next().and_then(|v| v.parse::<u32>().ok()),
        )
    };
    let server_pid = server_pid.expect("桩应写出自己 pid");
    let escaped_pid = escaped_pid.expect("桩应写出逃逸子孙 pid");
    assert!(
        crate::bot::procs::pid_exists(server_pid),
        "MCP 子进程此刻应活着"
    );
    assert!(
        crate::bot::procs::pid_exists(escaped_pid),
        "逃逸子孙此刻应活着"
    );

    let stopped = mgr.shutdown().await;
    assert_eq!(stopped, 1, "应停掉 1 个本地子进程");

    // 回收是异步的（rmcp 的 Drop 才会 reap），给它一点时间
    for pid in [server_pid, escaped_pid] {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while crate::bot::procs::pid_exists(pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            !crate::bot::procs::pid_exists(pid),
            "shutdown 后 pid={pid} 必须真的没了（不能留成宿主管不到的孤儿）"
        );
    }
    let _ = std::fs::remove_file(&pidfile);
}

#[tokio::test]
async fn missing_binary_marks_down_and_lazy_reconnects() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP stdio e2e");
        return;
    }
    let mgr = McpManager::default();
    // 白名单内但参数指向不存在的脚本 → spawn/握手失败 → Down
    let mut bad = echo_cfg("e2e-bad");
    bad.args = vec!["/nonexistent/nope.py".into()];
    let err = mgr.ensure_connected(&bad).await.expect_err("应失败");
    assert!(err.contains("失败") || err.contains("超时"), "实际：{err}");
    assert_eq!(mgr.status_of("e2e-bad").state, "down");
    assert!(mgr.status_of("e2e-bad").error.is_some());
    // 懒重连：换成正确配置后再次 ensure_connected 即恢复（无需手动 disconnect）
    let good = echo_cfg("e2e-bad");
    mgr.ensure_connected(&good).await.expect("重建应成功");
    assert_eq!(mgr.status_of("e2e-bad").state, "connected");
    mgr.shutdown().await;
}

#[tokio::test]
async fn http_branch_url_gate_blocks_before_any_request() {
    // URL 安全闸在连接口：localhost 手改配置文件也进不来（阶段 7 验收）
    let mgr = McpManager::default();
    let mut cfg = echo_cfg("e2e-http-gate");
    cfg.transport = "http".into();
    cfg.command = None;
    cfg.args = Vec::new();
    cfg.url = Some("http://127.0.0.1:9999/mcp".into());
    let err = mgr
        .ensure_connected(&cfg)
        .await
        .expect_err("应被安全闸拦下");
    assert!(err.contains("URL 安全闸"), "实际：{err}");
    assert_eq!(mgr.status_of("e2e-http-gate").state, "down");
}

#[tokio::test]
async fn http_branch_connect_error_maps_to_clean_message() {
    // HTTP 传输错误路径冒烟：.invalid TLD DNS 快速失败 → 错误文案映射。
    //（公网连通性不作单测依赖；Streamable HTTP 的 serve/discover/call 机制
    // 与 stdio 共用，已由 echo 桩 e2e 覆盖全链路。）
    let mgr = McpManager::default();
    let mut cfg = echo_cfg("e2e-http-dns");
    cfg.transport = "http".into();
    cfg.command = None;
    cfg.args = Vec::new();
    cfg.url = Some("https://mcp-nonexistent.invalid/mcp".into());
    cfg.timeout_secs = Some(crate::bot::mcp::config::MCP_TIMEOUT_MIN_SECS);
    let err = mgr.ensure_connected(&cfg).await.expect_err("应连接失败");
    assert!(
        err.contains("失败") || err.contains("超时"),
        "错误应可读：{err}"
    );
    assert_eq!(mgr.status_of("e2e-http-dns").state, "down");
}

/// 实机卡片 S10 的地面真相（真子进程跑）：两台**同前缀不同名**的 stdio 服务器，
/// 消歧序号跨断连不漂移。
///
/// 卡片问模型「列出你现在能用的工具」——那份清单的真身就是
/// `registry::tools_json_with_mcp` 的产物（每轮拼进请求），所以这里直接看那份
/// JSON：比问模型硬，也不受模型复述影响。
#[tokio::test]
async fn disambiguated_names_survive_a_server_dropping_out() {
    if !ensure_python3().await {
        eprintln!("[skip] 无 python3，跳过 MCP 消歧稳定性 e2e");
        return;
    }
    let _serial = SHARED_MCP_TEST_LOCK.lock().await;
    shared().shutdown().await;
    let mgr = shared();
    // 两台同前缀不同名：`p x` / `p.x` → tool_prefix 都归成 `p_x`
    let mut cfg_a = echo_cfg("s10-a");
    cfg_a.name = "p x".into();
    let mut cfg_b = echo_cfg("s10-b");
    cfg_b.name = "p.x".into();
    mgr.ensure_connected(&cfg_a).await.expect("A 连接应成功");
    mgr.ensure_connected(&cfg_b).await.expect("B 连接应成功");

    // 两台都在场 → 两个函数名（base + 消歧后缀）
    let both = mcp_function_names();
    assert_eq!(
        both.len(),
        2,
        "两台各带一个工具 → 清单里两个函数名：{both:?}"
    );
    assert!(both.contains(&"mcp_p_x_echo".to_string()), "{both:?}");
    assert!(both.contains(&"mcp_p_x_echo_2".to_string()), "{both:?}");
    let name_a = function_name_of("s10-a");
    let name_b = function_name_of("s10-b");
    assert_ne!(name_a, name_b, "两台各自的函数名应不同");

    // 关掉 A（面板禁用 = reload_from_config → disconnect，槽位被摘掉）
    mgr.disconnect("s10-a").await;
    let after_a = mcp_function_names();
    assert_eq!(after_a.len(), 1, "A 断连后清单只剩一个：{after_a:?}");
    assert_eq!(
        after_a[0], name_b,
        "A 断连不能让 B 的消歧序号漂移（按在场顺序重编号会退化成无后缀的 base 名）"
    );
    // A 的旧名仍归 A：dispatch 查得到 → 不会被报成「未知工具」
    assert!(
        crate::bot::mcp::mount::find_mounted(&name_a).is_some(),
        "断连那台的旧名仍应能反查到归属"
    );

    // 反过来关 B：两次剩下的名字必须不同——相同即序号漂移
    mgr.ensure_connected(&cfg_a).await.expect("A 重连应成功");
    mgr.disconnect("s10-b").await;
    let after_b = mcp_function_names();
    assert_eq!(after_b.len(), 1, "B 断连后清单只剩一个：{after_b:?}");
    assert_eq!(after_b[0], name_a, "B 断连后 A 保住自己的名字");
    assert_ne!(
        after_b[0], after_a[0],
        "两次断连剩下的名字必须不同——相同就是序号在漂移"
    );

    mgr.shutdown().await;
}

/// 主 agent 这一轮看到的 MCP 函数名（真身 = tools_json_with_mcp 的产物）
fn mcp_function_names() -> Vec<String> {
    let json = crate::bot::registry::tools_json_with_mcp(None);
    let v: serde_json::Value = serde_json::from_str(json.as_ref()).expect("工具清单应是合法 JSON");
    v.as_array()
        .expect("工具清单应是数组")
        .iter()
        .filter_map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
        })
        .filter(|n| n.starts_with("mcp_"))
        .map(|s| s.to_string())
        .collect()
}

/// 某台服务器当前挂载的函数名（登记簿口径：名字 ↔ 服务器的绑定）
fn function_name_of(server_id: &str) -> String {
    crate::bot::mcp::mount::mcp_mounted_tools()
        .into_iter()
        .find(|t| t.server_id == server_id)
        .map(|t| t.func_name)
        .expect("该服务器应有挂载条目")
}
