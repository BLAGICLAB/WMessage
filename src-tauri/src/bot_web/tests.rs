use super::*;

/// 公网闸的两类失败必须分开：**策略拒绝** vs **DNS 解析失败**。
///
/// 合成一个 Err 时，调用方只能一律按「换个地址重试」落地——真实网络故障
/// 于是被降级成 WARN/Denied。解析走桩：真 DNS 依赖网络环境，无 DNS 的机器
/// 上会一直等到系统超时（实测 30s），门禁不该等这个。
#[tokio::test]
async fn public_url_gate_separates_denial_from_dns_failure() {
    let url = url::Url::parse("https://example.com/page").expect("URL 合法");

    // 解析失败：网络故障，不是策略拒绝
    let err = check_public_url_with(&url, |_host| async { Err("os error 11001".to_string()) })
        .await
        .expect_err("解析失败必须报错");
    assert!(
        matches!(err, UrlGateError::Unresolved { .. }),
        "DNS 解析失败必须单独归类，不能混进策略拒绝：{err:?}"
    );
    assert!(
        err.reason().contains("域名解析失败"),
        "解析失败要给出可诊断的原因：{}",
        err.reason()
    );

    // 解析到内网：策略拒绝
    let err = check_public_url_with(&url, |_host| async {
        Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], 80))])
    })
    .await
    .expect_err("解析到内网必须被拒");
    assert!(
        matches!(err, UrlGateError::Denied { .. }),
        "解析到内网属于策略拒绝：{err:?}"
    );
    assert!(
        err.reason().contains("已拒绝：域名解析到本机/内网地址"),
        "拒绝理由要给模型看：{}",
        err.reason()
    );

    // 解析到公网：放行（端口按 URL 真实端口重写）
    let addrs = check_public_url_with(&url, |_host| async {
        Ok(vec![std::net::SocketAddr::from(([93, 184, 216, 34], 80))])
    })
    .await
    .expect("公网地址应放行");
    assert_eq!(
        addrs[0].port(),
        443,
        "端口按 URL 真实端口重写（https 默认 443，不是解析时的 80）"
    );

    // 公网闸判定的文案带域标签（与既有文案一致）
    assert!(
        err.gate_message().starts_with("[web] "),
        "公网闸文案带 [web] 域标签：{}",
        err.gate_message()
    );
}

/// open_url 准入门：内网地址 / 非 http/https / 非法网址一律是**策略拒绝**
/// （这三条不触网，判定确定）。
#[tokio::test]
async fn open_url_gate_rejects_policy_violations_as_denied() {
    for raw in [
        "http://127.0.0.1:8080/",
        "http://192.168.1.1/",
        "ftp://example.com/",
        "这不是一个网址",
    ] {
        let err = ensure_public_http_url(raw)
            .await
            .expect_err("策略违规必须被拒");
        assert!(
            matches!(err, UrlGateError::Denied { .. }),
            "策略违规必须是策略拒绝（{raw}）：{err:?}"
        );
    }
}

#[test]
fn route_none_switch_keeps_legacy_auto() {
    // 老配置（无开关字段、无 brave 字段）：配了 key 自动走 Tavily，没配走双引擎
    assert_eq!(
        resolve_search_route(None, Some("tvly-x"), None, None),
        SearchRoute::Tavily("tvly-x".into())
    );
    assert_eq!(
        resolve_search_route(None, None, None, None),
        SearchRoute::Dual
    );
    assert_eq!(
        resolve_search_route(None, Some("   "), None, None),
        SearchRoute::Dual
    );
}

#[test]
fn route_explicit_off_forces_dual_even_with_key() {
    assert_eq!(
        resolve_search_route(Some(false), Some("tvly-x"), None, None),
        SearchRoute::Dual
    );
    assert_eq!(
        resolve_search_route(Some(false), None, None, None),
        SearchRoute::Dual
    );
    // Brave 也显式关：两边都有 key 也应走双引擎
    assert_eq!(
        resolve_search_route(Some(false), Some("tvly-x"), Some(false), Some("bsa-x")),
        SearchRoute::Dual
    );
}

#[test]
fn route_explicit_on_requires_key() {
    assert_eq!(
        resolve_search_route(Some(true), None, None, None),
        SearchRoute::MissingKey
    );
    assert_eq!(
        resolve_search_route(Some(true), Some("  "), None, None),
        SearchRoute::MissingKey
    );
    // key 首尾空白应裁掉
    assert_eq!(
        resolve_search_route(Some(true), Some(" tvly-x "), None, None),
        SearchRoute::Tavily("tvly-x".into())
    );
}

#[test]
fn route_brave_on_with_key() {
    assert_eq!(
        resolve_search_route(None, None, Some(true), Some(" bsa-x ")),
        SearchRoute::Brave("bsa-x".into())
    );
}

#[test]
fn route_brave_on_without_key_errors() {
    assert_eq!(
        resolve_search_route(None, None, Some(true), None),
        SearchRoute::MissingBraveKey
    );
    assert_eq!(
        resolve_search_route(None, None, Some(true), Some("  ")),
        SearchRoute::MissingBraveKey
    );
}

#[test]
fn route_both_on_conflict() {
    // 双开（显式或自动态）都明确报错，不静默猜
    assert_eq!(
        resolve_search_route(Some(true), Some("tvly-x"), Some(true), Some("bsa-x")),
        SearchRoute::Conflict
    );
    assert_eq!(
        resolve_search_route(None, Some("tvly-x"), None, Some("bsa-x")),
        SearchRoute::Conflict
    );
}

#[test]
fn route_brave_none_switch_auto_by_key() {
    // 与 Tavily 的 None+key 旧行为对齐：无显式开关但有 brave key 自动启用 Brave
    assert_eq!(
        resolve_search_route(None, None, None, Some("bsa-x")),
        SearchRoute::Brave("bsa-x".into())
    );
    // Brave 自动态优先于「都没配」的双引擎，但不影响显式关 Tavily
    assert_eq!(
        resolve_search_route(Some(false), Some("tvly-x"), None, Some("bsa-x")),
        SearchRoute::Brave("bsa-x".into())
    );
    // Brave 显式关：即使配了 brave key 也不走；Tavily 逻辑不受影响
    assert_eq!(
        resolve_search_route(None, Some("tvly-x"), Some(false), Some("bsa-x")),
        SearchRoute::Tavily("tvly-x".into())
    );
}

#[test]
fn parse_brave_results_ok() {
    let body = r#"{"web":{"results":[
        {"title":"标题一","url":"https://a.com/x","description":"摘要一"},
        {"title":"标题二","url":"https://b.com/y","description":"摘要二","extra":1}
    ]},"query":{"original":"q"}}"#;
    let r = parse_brave_results(body).unwrap_or_default();
    assert_eq!(r.len(), 2);
    assert_eq!(
        r[0],
        ("标题一".into(), "https://a.com/x".into(), "摘要一".into())
    );
    assert_eq!(r[1].1, "https://b.com/y");
}

#[test]
fn parse_brave_results_web_missing_and_empty() {
    // web 字段缺失 → 空列表（防御，不报错）
    let r = parse_brave_results(r#"{"query":{"original":"q"}}"#).unwrap_or_default();
    assert!(r.is_empty());
    // results 为空数组 → 空列表
    let r = parse_brave_results(r#"{"web":{"results":[]}}"#).unwrap_or_default();
    assert!(r.is_empty());
    // 字段缺失的条目按空串填充
    let r =
        parse_brave_results(r#"{"web":{"results":[{"title":"只有标题"}]}}"#).unwrap_or_default();
    assert_eq!(
        r,
        vec![("只有标题".to_string(), String::new(), String::new())]
    );
}

#[test]
fn parse_brave_results_bad_json() {
    let err = parse_brave_results("not json").unwrap_err();
    assert!(err.contains("Brave 响应解析失败"), "应明确报错：{err}");
}

#[test]
fn search_body_decodes_lossily_not_fails() {
    // 结果体混入非法 UTF-8 字节（\xFF、GB 码半字）：lossy 替换为 U+FFFD 继续，
    // 不得让整个引擎假失败「无结果」
    let text = decode_search_body(b"\xEF\xBB\xBF{\xFF\"json\"\xB0}");
    assert!(text.contains("\"json\""));
    assert!(text.contains('\u{FFFD}'));
    // 合法 UTF-8 原样通过（含 BOM 不剥——调用方解析器自处理）
    assert_eq!(decode_search_body("中文ok".as_bytes()), "中文ok");
}

#[test]
fn tag_open_exact_match_skips_prefix_siblings() {
    // <abbr> 前置于 <a>：first_open_tag / first_between 须取真 <a> 节点
    let h = r#"<h3 class="t"><abbr>缩写</abbr><a href="http://x/1">真标题</a></h3>"#;
    let tag = first_open_tag(h, "<a").unwrap();
    assert!(
        tag.starts_with("<a "),
        "应取真 <a> 开标签而非 <abbr>：{tag}"
    );
    assert!(href_from_tag(tag).unwrap().contains("http://x/1"));
    let text = first_between(h, "<a", "</a>").unwrap();
    assert_eq!(text, "真标题");
    // 正常形态不回归：<a> 直接出现
    let h2 = r#"<h2><a href="http://x/2">标题2</a></h2>"#;
    assert!(first_open_tag(h2, "<a").unwrap().starts_with("<a href"));
    // <p 不误中 <pre>
    let b = r#"<pre>code</pre><p>摘要</p>"#;
    assert_eq!(first_between(b, "<p", "</p>").unwrap(), "摘要");
}

#[test]
fn parse_bing_fixture() {
    let html = std::fs::read_to_string("/tmp/bing.html").unwrap_or_default();
    if html.is_empty() {
        eprintln!("fixture 不存在，跳过");
        return;
    }
    let results = parse_bing(&html);
    assert!(!results.is_empty(), "fixture 应能解析出结果");
    assert!(results[0].1.starts_with("http"), "首条应带链接");
    eprintln!(
        "parsed {} results, first: {} | {}",
        results.len(),
        results[0].0,
        results[0].1
    );
}

#[test]
fn parse_baidu_fixture() {
    let html = std::fs::read_to_string("/tmp/baidu.html").unwrap_or_default();
    if html.is_empty() {
        eprintln!("fixture 不存在，跳过");
        return;
    }
    let results = parse_baidu(&html);
    assert!(!results.is_empty(), "百度 fixture 应能解析出结果");
    assert!(
        results[0].1.contains("baidu.com/link"),
        "链接应为百度跳转链接"
    );
    for (t, l, s) in results.iter().take(5) {
        eprintln!(
            "baidu: {} | {} | snip {} 字",
            t.chars().take(30).collect::<String>(),
            l.chars().take(40).collect::<String>(),
            s.chars().count()
        );
    }
}

#[test]
fn private_host_guard() {
    for h in [
        "127.0.0.1",
        "192.168.1.1",
        "10.0.0.1",
        "172.16.0.1",
        "169.254.1.1",
        "localhost",
        "nas.local",
        "x.internal",
        "::1",
        "fc00::1",
        "fe80::1",
        // unspecified 段已实现，
        // 此处把 0.0.0.0 / :: 钉进回归
        "0.0.0.0",
        "::",
    ] {
        assert!(is_private_host(h), "应拦截 {h}");
    }
    for h in ["www.bing.com", "example.com", "api.minimaxi.com"] {
        assert!(!is_private_host(h), "不应拦截 {h}");
    }
}

#[test]
fn strip_tags_entities() {
    assert_eq!(
        strip_tags("<p>你好 &amp; 世界&nbsp;！</p>"),
        "你好 & 世界 ！"
    );
    assert_eq!(strip_tags("<a href=\"x\">标题</a>"), "标题");
}

#[test]
fn extract_main_prefers_article_over_nav() {
    let html = r#"<html><body>
        <nav><a href="/">首页</a><a href="/about">关于我们</a></nav>
        <article><h1>正文标题</h1><p>这是正文内容，应该被提取出来，而导航链接不应该出现在结果里。正文需要足够长才能超过 200 字阈值，所以这里多写一些内容来凑够长度。继续补充正文内容，确保测试稳定通过，再多写一点点内容。</p></article>
        <footer>版权所有 2026</footer>
    </body></html>"#;
    let main = extract_main_content(html);
    assert!(main.contains("正文标题"), "应提取 article 正文");
    assert!(!main.contains("关于我们"), "nav 噪声应被剔除");
    assert!(!main.contains("版权所有"), "footer 噪声应被剔除");
}

#[test]
fn extract_main_semantic_div_fallback() {
    // 没有 article/main 标签时，用语义 class 的最大 div
    let html = format!(
        r#"<html><body><div class="sidebar">侧边栏短</div><div id="content"><p>{}</p></div></body></html>"#,
        "正文内容".repeat(60)
    );
    let main = extract_main_content(&html);
    assert!(main.contains("正文内容"), "应命中 id=content 的 div");
    assert!(!main.contains("侧边栏短"), "sidebar 不应被选中");
}

#[test]
fn remove_tag_blocks_nested_same_tag() {
    let html = "<div>a</div><script>var x = '<script>nested</script>';</script><p>keep</p>";
    let out = remove_tag_blocks(html, "script");
    assert!(out.contains("keep"));
    assert!(!out.contains("nested"));
}

#[test]
fn clean_snippet_trims_ellipsis_and_whitespace() {
    assert_eq!(clean_snippet("  多  空白\n合并 ……"), "多 空白 合并");
    assert_eq!(clean_snippet("结尾省略…"), "结尾省略");
    assert_eq!(clean_snippet("正常文本。"), "正常文本。");
}

#[test]
fn domain_of_extracts_host() {
    assert_eq!(domain_of("https://www.example.com/a/b"), "www.example.com");
    assert_eq!(domain_of("不是链接"), "");
}

#[test]
fn jina_reader_url_wraps_original() {
    assert_eq!(
        jina_reader_url("https://example.com/a"),
        "https://r.jina.ai/https://example.com/a"
    );
    assert_eq!(
        jina_reader_url("  http://foo.bar  "),
        "https://r.jina.ai/http://foo.bar",
        "首尾空白应裁掉"
    );
}

// N3-5：搜索过滤参数映射

#[test]
fn time_range_parse_and_mappings() {
    assert_eq!(TimeRange::parse(Some("week")), Some(TimeRange::Week));
    assert_eq!(TimeRange::parse(Some("bogus")), None);
    assert_eq!(TimeRange::parse(None), None);
    // Tavily 原生 day/week/month/year；Brave freshness pd/pw/pm/py（官方 API）
    assert_eq!(TimeRange::Day.tavily_value(), "day");
    assert_eq!(TimeRange::Year.tavily_value(), "year");
    assert_eq!(TimeRange::Day.brave_freshness(), "pd");
    assert_eq!(TimeRange::Week.brave_freshness(), "pw");
    assert_eq!(TimeRange::Month.brave_freshness(), "pm");
    assert_eq!(TimeRange::Year.brave_freshness(), "py");
}

#[test]
fn sanitize_site_strips_scheme_path_and_rejects_blank() {
    assert_eq!(sanitize_site("github.com"), Some("github.com".into()));
    assert_eq!(
        sanitize_site("https://docs.rs/serde/latest/"),
        Some("docs.rs".into()),
        "剥 scheme 与路径"
    );
    assert_eq!(sanitize_site("  a.com  "), Some("a.com".into()));
    assert_eq!(sanitize_site(""), None);
    assert_eq!(sanitize_site("  "), None);
    assert_eq!(sanitize_site("two words.com"), None, "含空白非法");
}

#[test]
fn tavily_payload_maps_opts() {
    let mut opts = SearchOpts {
        count: 5,
        time_range: Some(TimeRange::Month),
        site: Some("github.com".into()),
    };
    let body = tavily_payload("k", "rust 编译", &opts);
    assert_eq!(body["max_results"], 5);
    assert_eq!(body["time_range"], "month");
    assert_eq!(body["include_domains"][0], "github.com");
    // 缺省：无 time_range/include_domains 键（不送空值给 API）
    opts.time_range = None;
    opts.site = None;
    let body = tavily_payload("k", "q", &opts);
    assert!(body.get("time_range").is_none());
    assert!(body.get("include_domains").is_none());
}

#[test]
fn brave_query_and_params_maps_opts() {
    let opts = SearchOpts {
        count: 3,
        time_range: Some(TimeRange::Day),
        site: Some("example.com".into()),
    };
    let (q, params) = brave_query_and_params("缓存 策略", &opts);
    // 返回的是编码后的 q（与现有 form_urlencoded 风格一致），site 运算符同样被编码
    assert!(q.contains("site%3Aexample.com"), "{q}");
    assert!(params.contains("count=3"), "{params}");
    assert!(params.contains("freshness=pd"), "{params}");
    // 无 site：q 原样
    let plain = SearchOpts {
        count: 8,
        ..Default::default()
    };
    let (q, params) = brave_query_and_params("hello", &plain);
    assert_eq!(q, "hello");
    assert!(params.contains("count=8"));
    assert!(!params.contains("freshness"), "{params}");
}

#[test]
fn network_search_and_fetch() {
    // 真实网络测试：环境可达时验证（cn.bing.com 从 Mac 可达）
    match tauri::async_runtime::block_on(web_search("北京今天天气", &SearchOpts::default())) {
        Ok(out) => {
            assert!(out.contains("http"), "搜索结果应带链接");
            eprintln!("SEARCH OK:\n{}", out.chars().take(200).collect::<String>());
        }
        Err(e) => eprintln!("search failed (环境相关): {e}"),
    }
    match tauri::async_runtime::block_on(fetch_text("https://example.com")) {
        Ok(t) => eprintln!(
            "FETCH OK, {} chars, head: {}",
            t.chars().count(),
            t.chars().take(80).collect::<String>()
        ),
        Err(e) => eprintln!("fetch failed (环境相关): {e}"),
    }
    // 内网地址应被拒
    let blocked = tauri::async_runtime::block_on(fetch_text("http://127.0.0.1:4763/api/health"));
    assert!(blocked.is_err(), "本机地址必须被拒绝");
    let bad_scheme = tauri::async_runtime::block_on(fetch_text("file:///etc/hosts"));
    assert!(bad_scheme.is_err(), "非 http(s) 协议必须被拒绝");
}

/// 回归：http_client 禁止自动重定向——
/// 302 必须原样返回给 fetch_text 的手工逐跳校验，不得自动跟随到 Location 目标
///（自动跟随会让公网 URL 302 进内网）。
#[tokio::test]
async fn http_client_does_not_follow_redirects() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            use std::io::{Read, Write};
            // 先读完请求头再回响应（hyper 对未消费请求即收响应会报 UnexpectedMessage）
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let resp = http_client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .expect("本地 302 端点应可连");
    assert_eq!(
        resp.status().as_u16(),
        302,
        "必须原样返回 302（手工逐跳校验接管），不得自动跟随"
    );
}

/// 回归：IPv4-mapped IPv6（::ffff:127.0.0.1）与 CGNAT（100.64.0.0/10）判内网
#[test]
fn private_detection_covers_mapped_v6_and_cgnat() {
    assert!(ipv6_is_private("::ffff:127.0.0.1".parse().unwrap()));
    assert!(
        ipv6_is_private("::ffff:a9fe:a9fe".parse().unwrap()),
        "169.254.169.254 mapped"
    );
    assert!(
        !ipv6_is_private("2606:4700:4700::1111".parse().unwrap()),
        "公网 v6 放行"
    );
    assert!(ipv4_is_private("100.64.0.1".parse().unwrap()), "CGNAT 起始");
    assert!(
        ipv4_is_private("100.127.255.254".parse().unwrap()),
        "CGNAT 末尾"
    );
    assert!(
        !ipv4_is_private("100.128.0.1".parse().unwrap()),
        "CGNAT 段外"
    );
    assert!(
        !ipv4_is_private("99.255.0.1".parse().unwrap()),
        "CGNAT 段外"
    );
}

/// FIX-PLAN #7a（DNS TOCTOU）回归：钉住解析结果后请求必须走钉住的地址——
/// 用 .invalid 域名（RFC 2606，真实 DNS 必解析失败）钉到本地回环服务器，
/// 能连通即证明 reqwest 没有二次解析；Host 头必须仍是原域名。
/// （http 场景 URL 端口覆盖钉住的 80；https 场景相反、按 addr 端口连接——
/// 端口正确性由 check_public_url 的 with_url_port 重写保证，见下方单测）
#[tokio::test]
async fn pinned_client_uses_validated_addrs() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let got_host = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let got_host2 = got_host.clone();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if let Some(line) = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("host:"))
            {
                *got_host2.lock().unwrap() = line[5..].trim().to_string();
            }
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            );
        }
    });
    let url = url::Url::parse(&format!("http://pinned.invalid:{port}/")).unwrap();
    let pinned = [std::net::SocketAddr::from(([127, 0, 0, 1], 80))];
    let resp = http_client_pinned(&url, &pinned)
        .expect("钉住 client 构建失败")
        .get(url)
        .send()
        .await
        .expect("钉住 127.0.0.1 后请求应成功（.invalid 真实 DNS 必失败，无二次解析）");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        got_host.lock().unwrap().as_str(),
        format!("pinned.invalid:{port}"),
        "钉 IP 不改 Host 头（TLS 场景 SNI/证书同理仍按原域名）"
    );
}

/// with_url_port：https 默认端口 → 443；显式端口保留
#[test]
fn with_url_port_rewrites_to_real_port() {
    let mk = |a: [u8; 4], p: u16| std::net::SocketAddr::from((a, p));
    // https 无显式端口 → 443（曾因 80 占位导致 TLS 发去 80 端口全挂）
    let https = url::Url::parse("https://example.com/").unwrap();
    assert_eq!(
        with_url_port(vec![mk([1, 2, 3, 4], 80)], &https),
        vec![mk([1, 2, 3, 4], 443)]
    );
    // 显式端口保留
    let explicit = url::Url::parse("https://example.com:8443/x").unwrap();
    assert_eq!(
        with_url_port(vec![mk([1, 2, 3, 4], 80), mk([5, 6, 7, 8], 80)], &explicit),
        vec![mk([1, 2, 3, 4], 8443), mk([5, 6, 7, 8], 8443)]
    );
    // http 无显式端口 → 80（不变）
    let http = url::Url::parse("http://example.com/").unwrap();
    assert_eq!(
        with_url_port(vec![mk([1, 2, 3, 4], 12345)], &http),
        vec![mk([1, 2, 3, 4], 80)]
    );
}

/// 哪些状态码值得换渲染代理再试。
///
/// 这条判据决定了「直连打不通」时还有没有第二次机会，判反了两头都亏：
/// 把 404 判成值得重试 = 白等一次代理，还把 URL 送到第三方；把 5xx 判成不
/// 值得重试 = 目标站自己在维护时直接判死刑，而代理往往能拿到它渲染后的内容。
#[test]
fn proxy_retry_only_for_server_side_failures() {
    for s in [500, 502, 503, 504, 408] {
        assert!(
            status_worth_proxy_retry(s),
            "{s} 是服务端/超时类失败，换出口还有救"
        );
    }
    for s in [400, 401, 403, 404, 410, 429] {
        assert!(
            !status_worth_proxy_retry(s),
            "{s} 是目标明确表态，代理只会拿到同一个答案"
        );
    }
}

/// SSRF 闸拦下的 URL 绝不进代理兜底。
///
/// 兜底路要把目标 URL 发给 r.jina.ai，被本机闸拒掉的地址（回环/内网）一旦
/// 也走这条路，等于把「不该访问的目标」转手交给第三方——而闸拦发生在任何
/// 网络 I/O 之前，所以这条用回环地址就能真跑，无需起服务。
/// 归错类（Rejected 写成 Unreachable）会直接把这条断言打红。
#[tokio::test]
async fn ssrf_rejection_never_reaches_the_proxy() {
    let err = fetch_direct("http://127.0.0.1:4763/api/health")
        .await
        .expect_err("回环地址必须被 SSRF 闸拒绝");
    assert!(
        matches!(err, FetchErr::Rejected(_)),
        "闸拦属于「换出口也没用」，归 Unreachable 会把内网地址送去第三方代理重试"
    );
}
