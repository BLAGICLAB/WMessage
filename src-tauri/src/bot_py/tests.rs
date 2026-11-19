//! Python 执行层的单测。

use super::*;
// 原 bot_py.rs 顶层 use（被 facade 收走后丢失，补回以让测试编译）
use crate::bot::constants::{APPS_MAX_TIMEOUT_SECS, PY_MAX_TIMEOUT_SECS};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ── drain_output（孙进程继承管道时 rx 收集不得永久阻塞）──

#[test]
fn drain_output_collects_both_channels() {
    let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
    tx.send(("out", b"hello".to_vec(), false)).unwrap();
    tx.send(("err", b"warn".to_vec(), false)).unwrap();
    drop(tx);
    let (out, err, complete, truncated) = drain_output(&rx, Duration::from_secs(1));
    assert!(complete);
    assert!(!truncated);
    assert_eq!(out, "hello");
    assert_eq!(err, "warn");
}

#[test]
fn drain_output_times_out_when_grandchild_holds_pipe() {
    // 模拟孙进程 fork 后 sleep 远超宽限、一直占着管道写端：
    // sender 线程 10s 后才发送（且只有一个方向），drain 必须在宽限到期后返回，
    // 而不是像原 rx.iter() 那样永久挂死
    let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(10));
        let _ = tx.send(("out", b"late".to_vec(), false));
    });
    let start = Instant::now();
    let (_out, _err, complete, _tr) = drain_output(&rx, Duration::from_millis(300));
    assert!(!complete, "发送方卡死时不得报告收齐");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "drain 挂死了：{:?}",
        start.elapsed()
    );
}

// ── read_capped_drain（触顶截断 + 排空防 SIGPIPE）──

#[test]
fn read_capped_drain_under_cap_not_truncated() {
    let data = b"short output".to_vec();
    let (buf, truncated) = read_capped_drain(std::io::Cursor::new(data.clone()), 1024);
    assert!(!truncated);
    assert_eq!(buf, data);
}

#[test]
fn read_capped_drain_over_cap_truncates_and_drains() {
    // 64KB+ 输出：内容截断到 cap、标记 truncated，且剩余部分被排空读完。
    // 计数 Read 适配器锁死「排空到 EOF」承诺：底层必须被读满 cap+100 字节——
    // 若排空逻辑（io::copy 到 sink）被删，子进程写端会吃 SIGPIPE
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    struct CountingReader<R> {
        inner: R,
        total: Arc<AtomicUsize>,
    }
    impl<R: std::io::Read> std::io::Read for CountingReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.total.fetch_add(n, Ordering::SeqCst);
            Ok(n)
        }
    }
    let cap = OUTPUT_CAP + 1024;
    let data = vec![b'x'; cap + 100];
    let total = Arc::new(AtomicUsize::new(0));
    let reader = CountingReader {
        inner: std::io::Cursor::new(data),
        total: total.clone(),
    };
    let (buf, truncated) = read_capped_drain(reader, cap);
    assert!(truncated);
    assert_eq!(buf.len(), cap);
    assert_eq!(
        total.load(Ordering::SeqCst),
        cap + 100,
        "截断后底层必须被排空到 EOF（防 SIGPIPE）"
    );
}

#[test]
fn drain_output_marks_truncated_from_reader() {
    // 模拟 reader 线程发 64KB+ 触顶数据（truncated=true），drain 必须透出标记
    let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
    tx.send(("out", vec![b'x'; OUTPUT_CAP + 1024], true))
        .unwrap();
    tx.send(("err", Vec::new(), false)).unwrap();
    drop(tx);
    let (out, _err, complete, truncated) = drain_output(&rx, Duration::from_secs(1));
    assert!(complete);
    assert!(truncated, "触顶标记必须透出");
    assert_eq!(out.len(), OUTPUT_CAP + 1024);
}

// ── reader 线程生命周期（StopReader 取消 + 带超时 join 兜底）──

#[test]
fn stop_reader_returns_partial_when_stopped() {
    // 本用例自带注入实例的 StopGuard，别的用例的全局广播不再置位它
    let data = vec![b'x'; 4096];
    let guard = crate::bot_slash::StopGuard::new(&guard_test_handle(), false, None);
    let token = guard.token();
    // 未停止：完整读取，行为与裸 read_capped_drain 一致
    let (buf, tr) = read_capped_drain(
        StopReader::new(std::io::Cursor::new(data.clone()), Some(token.clone())),
        1024 * 1024,
    );
    assert!(!tr);
    assert_eq!(buf, data);
    // 已停止：第一次 read 即 EOF，提前返回部分（此处为空）数据，不盲等
    guard.force_stop();
    let (buf, tr) = read_capped_drain(
        StopReader::new(std::io::Cursor::new(data), Some(token)),
        1024 * 1024,
    );
    assert!(!tr);
    assert!(buf.is_empty(), "stop 置位后应立即收尾返回部分数据");
}

#[test]
fn join_reader_threads_finished_no_audit() {
    // 正常场景：reader 已完成 → 秒 join，无审计
    let h1 = std::thread::spawn(|| {});
    let h2 = std::thread::spawn(|| {});
    let mut lines: Vec<String> = Vec::new();
    join_reader_threads(h1, h2, &mut |l: &str| lines.push(l.to_string()));
    assert!(lines.is_empty(), "正常退出不应记审计: {lines:?}");
}

#[test]
fn join_reader_threads_stuck_detaches_with_audit() {
    // 卡死场景：stdout reader 永不退出 → 超时 detach + ERROR 审计，不得永久挂住
    let h1 = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(30)));
    let h2 = std::thread::spawn(|| {});
    let mut lines: Vec<String> = Vec::new();
    let start = Instant::now();
    join_reader_threads(h1, h2, &mut |l: &str| lines.push(l.to_string()));
    assert!(
        start.elapsed() < READER_JOIN_TIMEOUT + Duration::from_secs(2),
        "卡死 reader 不得拖住收尾：{:?}",
        start.elapsed()
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("reader_timeout") && l.contains("reader=stdout")),
        "缺 reader_timeout 审计（含 reader 标识）: {lines:?}"
    );
}

#[cfg(unix)]
#[test]
fn run_python_at_grandchild_pipe_readers_joined() {
    // 孙进程继承 stdout/stderr 管道写端且不退（sleep 30）：主进程退出后
    // drain 2s 宽限到期 → 强杀进程组 → reader 拿到 EOF 必须能 join，
    // 不得记 reader_timeout（整体 deadline = timeout + 2s drain + 2s reader）
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-grandchild");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("run.py"),
        "import subprocess, sys\nsubprocess.Popen(['sleep', '30'], stdout=sys.stdout, stderr=sys.stderr)\nprint('main-done', flush=True)\n",
    )
    .unwrap();
    let mut lines: Vec<String> = Vec::new();
    let start = Instant::now();
    let r = run_python_at(
        &py,
        Some("run.py"),
        &dir,
        None,
        &[],
        Some(30),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    );
    let ok = match r {
        Ok(r) => r,
        Err(f) => panic!("主进程正常退出应返回结果，got err: {}", f.msg),
    };
    assert_eq!(ok.exit_code, Some(0));
    // drain 超时后主进程输出随 reader 在 kill 后才 EOF，按 C1 语义不收回，
    // 但 stderr 必须带「输出收集超时」提示
    assert!(
        ok.stderr.contains("输出收集超时"),
        "got stderr: {}",
        ok.stderr
    );
    assert!(
        lines.iter().any(|l| l.contains("drain_timeout")),
        "缺 drain_timeout 审计: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("reader_timeout")),
        "进程组已杀，reader 应能 join: {lines:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "整体兜底超时失效：{:?}",
        start.elapsed()
    );
}

// ── resolve_timeout_with_cap / 资源限额（默认 120s、硬钳上限 = PY_MAX_TIMEOUT_SECS、限额按 timeout 比例）──

#[test]
fn resolve_timeout_with_cap_default_120s() {
    assert_eq!(
        resolve_timeout_with_cap(None, PY_MAX_TIMEOUT_SECS),
        (DEFAULT_TIMEOUT_SECS, false)
    );
    assert_eq!(
        resolve_timeout_with_cap(Some(30), PY_MAX_TIMEOUT_SECS),
        (30, false)
    );
    assert_eq!(
        resolve_timeout_with_cap(Some(PY_MAX_TIMEOUT_SECS), PY_MAX_TIMEOUT_SECS),
        (PY_MAX_TIMEOUT_SECS, false)
    );
}

#[test]
fn resolve_timeout_with_cap_clamps_over_ceiling() {
    // 传入超上限的数应被钳到上限并报告 clamped
    assert_eq!(
        resolve_timeout_with_cap(Some(10000), PY_MAX_TIMEOUT_SECS),
        (PY_MAX_TIMEOUT_SECS, true)
    );
    assert_eq!(
        resolve_timeout_with_cap(Some(u64::MAX), PY_MAX_TIMEOUT_SECS),
        (PY_MAX_TIMEOUT_SECS, true)
    );
}

#[test]
fn resolve_timeout_with_cap_lets_apps_ceiling_through() {
    // apps 直跑的 1800s 天花板要在执行层真实生效：cap=1800 时声明值放行、
    // 超声明值仍钳；同一声明值走工具 cap（600）必须被钳——两条路径只有 cap 不同
    assert_eq!(
        resolve_timeout_with_cap(Some(1800), APPS_MAX_TIMEOUT_SECS),
        (APPS_MAX_TIMEOUT_SECS, false)
    );
    assert_eq!(
        resolve_timeout_with_cap(Some(9999), APPS_MAX_TIMEOUT_SECS),
        (APPS_MAX_TIMEOUT_SECS, true)
    );
    assert_eq!(
        resolve_timeout_with_cap(Some(1800), PY_MAX_TIMEOUT_SECS),
        (PY_MAX_TIMEOUT_SECS, true)
    );
    assert_eq!(
        resolve_timeout_with_cap(None, APPS_MAX_TIMEOUT_SECS),
        (DEFAULT_TIMEOUT_SECS, false)
    );
}

/// 修订测试共用：最小 docx 夹具（标题样式段 + 加粗段 + 待改段 + tab/超链接混合段）。
/// dotnet 与 Python 兜底两引擎用同一夹具，形成输出等价软锁。
fn write_revisions_fixture(orig: &std::path::Path) {
    const CT: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    const DOC: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>报告标题</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>加粗内容保留</w:t></w:r></w:p><w:p><w:r><w:t>这句要润色。</w:t></w:r></w:p><w:p><w:r><w:t>含</w:t></w:r><w:r><w:tab/></w:r><w:hyperlink><w:r><w:t>链接文字</w:t></w:r></w:hyperlink><w:r><w:t>保留，改三字。</w:t></w:r></w:p></w:body></w:document>"#;
    let f = std::fs::File::create(orig).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default();
    for (name, body) in [
        ("[Content_Types].xml", CT),
        ("_rels/.rels", RELS),
        ("word/document.xml", DOC),
    ] {
        zw.start_file(name, opt).unwrap();
        std::io::Write::write_all(&mut zw, body.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

/// 修订测试共用：读 docx（zip）里的 word/document.xml 文本。
fn read_docx_document_xml(path: &std::path::Path) -> String {
    let f = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut xml = String::new();
    use std::io::Read as _;
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

/// 修订标记结构断言（双引擎共用）：w:ins/w:del/w:delText 齐全 + 作者 + 无日期。
fn assert_valid_track_changes(xml: &str) {
    assert!(xml.contains("<w:ins "), "应有插入修订：{xml}");
    assert!(xml.contains("<w:del "), "应有删除修订：{xml}");
    assert!(
        xml.contains("<w:delText"),
        "w:del 内必须是 w:delText：{xml}"
    );
    assert!(xml.contains("WMessage AI"), "修订应有作者：{xml}");
    // 修订不写日期
    assert!(!xml.contains("w:date="), "修订不应带 w:date：{xml}");
}

/// 就地修订保格式断言（双引擎共用，与 dotnet 版断言逐条对齐）。
fn assert_in_place_preserves_formatting(xml: &str) {
    // 格式保留：标题样式 + 加粗 rPr 原样还在（equal 段落不动）
    assert!(xml.contains("w:val=\"Heading1\""), "标题样式应保留：{xml}");
    assert!(
        xml.contains("<w:b/>") || xml.contains("<w:b />"),
        "加粗 rPr 应保留：{xml}"
    );
    // 修订标记：改动段落行内 w:del + w:ins（文本拆 run，断言片段而非整串），无日期
    assert!(xml.contains("<w:del "), "应有删除修订：{xml}");
    assert!(xml.contains("<w:ins "), "应有插入修订：{xml}");
    assert!(
        xml.contains("<w:delText xml:space=\"preserve\">要</w:delText>"),
        "删除片段应在：{xml}"
    );
    assert!(
        xml.contains("<w:t xml:space=\"preserve\">过了</w:t>"),
        "插入片段应在：{xml}"
    );
    // 回归：tab/超链接段落改一个字走字符级 diff——只删「三」增「四」，
    // tab 保留、超链接文本作为 equal 片段保留（hyperlink 解包后文字不丢），
    // 不得整段标删（整段删会含完整旧句）
    assert!(
        xml.contains("<w:delText xml:space=\"preserve\">三</w:delText>"),
        "应只删「三」：{xml}"
    );
    assert!(
        xml.contains("<w:t xml:space=\"preserve\">四</w:t>"),
        "应只增「四」：{xml}"
    );
    assert!(
        xml.contains("<w:tab/>") || xml.contains("<w:tab />"),
        "tab 应保留：{xml}"
    );
    assert!(
        xml.contains(">链接文字</w:t>"),
        "超链接文本应作为 equal 片段保留：{xml}"
    );
    assert!(!xml.contains("改三字。</w:delText>"), "不得整段标删：{xml}");
    assert!(!xml.contains("w:date="), "修订不应带 w:date：{xml}");
}

/// python3 + python-docx 都在才返回 Some（缺则 None → 调用方 skip，CI 不红）。
fn python_with_docx() -> Option<String> {
    let py = detect_python()?;
    let ok = std::process::Command::new(&py)
        .args(["-c", "import docx"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ok.then_some(py)
}

/// Python 兜底引擎执行 MAKE_DOCX_REVISIONS_SCRIPT（与生产 run_python_ungated
/// 同形态：run.py + params.json 入运行目录，run_python_at 直跑）。
fn run_python_revisions(dir: &std::path::Path, params: &serde_json::Value) -> PyRunResult {
    let Some(py) = python_with_docx() else {
        panic!("python_with_docx 已判定可用");
    };
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("run.py"), MAKE_DOCX_REVISIONS_SCRIPT).unwrap();
    std::fs::write(dir.join("params.json"), params.to_string()).unwrap();
    let mut lines: Vec<String> = Vec::new();
    run_python_at(
        &py,
        Some("run.py"),
        dir,
        None,
        &[],
        Some(120),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    )
    .map_err(|f| f.msg)
    .expect("python 兜底应正常运行")
}

/// Python 兜底路径端到端（dotnet 不可用时的回退引擎）——与 dotnet 版同输入同断言。
/// 本机无 python3 或缺 python-docx 时跳过。
#[test]
fn python_revisions_tool_generates_valid_track_changes() {
    if python_with_docx().is_none() {
        eprintln!("skip: 无 python3 或未安装 python-docx");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.docx");
    let r = run_python_revisions(
        &tmp.path().join("run"),
        &serde_json::json!({
            "title": "", "original_path": "",
            "original": ["保持不动。", "这句要删掉。"],
            "revised": ["保持不动。", "这句改写法。"],
            "out": out,
        }),
    );
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    assert_valid_track_changes(&read_docx_document_xml(&out));
}

/// Python 兜底就地修订保格式——与 dotnet 版同夹具、断言逐条对齐（等价软锁）。
#[test]
fn python_revisions_in_place_preserves_formatting() {
    if python_with_docx().is_none() {
        eprintln!("skip: 无 python3 或未安装 python-docx");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let orig = tmp.path().join("orig.docx");
    write_revisions_fixture(&orig);
    let out = tmp.path().join("out.docx");
    let r = run_python_revisions(
        &tmp.path().join("run"),
        &serde_json::json!({
            "title": "", "original_path": orig,
            "original": [],
            "revised": ["报告标题", "加粗内容保留", "这句润色过了。", "含\t链接文字保留，改四字。"],
            "out": out,
        }),
    );
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    assert!(
        r.stdout.contains("保留原文格式"),
        "应走在地修订路径；stdout: {}",
        r.stdout
    );
    assert_in_place_preserves_formatting(&read_docx_document_xml(&out));
}

/// .NET 修订工具端到端——dotnet + dll 都在才跑（缺则跳过，CI 无 dotnet 不红）。
/// 用真实工具生成 docx，验证 OpenXML 修订标记（w:ins 用 w:t / w:del 用 w:delText）。
#[test]
fn dotnet_revisions_tool_generates_valid_track_changes() {
    // 入口定位收敛到 dotnet_revisions_entry（exe 优先/dll 兜底）
    let Some((prog, entry)) = dotnet_revisions_entry() else {
        eprintln!("skip: 未找到 wm-docx-revisions（先 dotnet build -c Release）");
        return;
    };
    let Some(dll) = entry else {
        eprintln!("skip: 命中随包 exe 形态（本用例只验 dotnet <dll> 直跑）");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run");
    std::fs::create_dir_all(&dir).unwrap();
    let out = tmp.path().join("out.docx");
    let params = serde_json::json!({
        "title": "", "original_path": "",
        "original": ["保持不动。", "这句要删掉。"],
        "revised": ["保持不动。", "这句改写法。"],
        "out": out,
    });
    std::fs::write(dir.join("params.json"), params.to_string()).unwrap();
    let dll_s = dll;
    let mut lines: Vec<String> = Vec::new();
    let r = run_python_at(
        &prog,
        Some(&dll_s),
        &dir,
        None,
        &["params.json".to_string()],
        Some(120),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    )
    .map_err(|f| f.msg)
    .expect("dotnet 工具应正常运行");
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    // 验证修订标记（zip 里 word/document.xml）
    assert_valid_track_changes(&read_docx_document_xml(&out));
}

/// 就地修订保留原文格式——夹具 docx（标题样式 + 加粗 run + 普通段落），
/// 修订后：equal 段落原样不动（pStyle / <w:b/> 保留），改动段落行内 w:ins/w:del，
/// 无 w:date。dotnet + dll 都在才跑（CI 无 dotnet 跳过）。
/// 回归：含 tab + 超链接的段落改几个字必须走字符级 diff
/// （口径不一致会必中整段删+整段增保底）。
#[test]
fn dotnet_revisions_in_place_preserves_formatting() {
    let Some((prog, entry)) = dotnet_revisions_entry() else {
        eprintln!("skip: 未找到 wm-docx-revisions（先 dotnet build -c Release）");
        return;
    };
    let Some(dll) = entry else {
        eprintln!("skip: 命中随包 exe 形态（本用例只验 dotnet <dll> 直跑）");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    // 最小 docx 夹具（与 Python 兜底测试共用 write_revisions_fixture）：
    // 标题样式段 + 加粗段 + 待改段 + tab/超链接混合段（回归：字符级 diff 而非整段标删）
    let orig = tmp.path().join("orig.docx");
    write_revisions_fixture(&orig);
    let dir = tmp.path().join("run");
    std::fs::create_dir_all(&dir).unwrap();
    let out = tmp.path().join("out.docx");
    let params = serde_json::json!({
        "title": "", "original_path": orig,
        "original": [],
        "revised": ["报告标题", "加粗内容保留", "这句润色过了。", "含\t链接文字保留，改四字。"],
        "out": out,
    });
    std::fs::write(dir.join("params.json"), params.to_string()).unwrap();
    let dll_s = dll;
    let mut lines: Vec<String> = Vec::new();
    let r = run_python_at(
        &prog,
        Some(&dll_s),
        &dir,
        None,
        &["params.json".to_string()],
        Some(120),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    )
    .map_err(|f| f.msg)
    .expect("dotnet 工具应正常运行");
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    assert!(
        r.stdout.contains("保留原文格式"),
        "应走在地修订路径；stdout: {}",
        r.stdout
    );
    assert_in_place_preserves_formatting(&read_docx_document_xml(&out));
}

#[test]
fn resource_limits_scale_with_timeout() {
    // 内存：8MB/s 比例，下限 256MB，上限 2GB
    assert_eq!(mem_limit_bytes(1), 256 * 1024 * 1024);
    assert_eq!(mem_limit_bytes(60), 480 * 1024 * 1024);
    assert_eq!(mem_limit_bytes(300), 2 * 1024 * 1024 * 1024);
    // CPU：timeout + 10s 宽限，且不溢出
    assert_eq!(cpu_limit_secs(60), 70);
    assert_eq!(cpu_limit_secs(u64::MAX), u64::MAX);
}

// ── S20：setrlimit 逐资源探测（探测失败 ≠ 可用；未生效必须 audit 留痕）──

#[cfg(unix)]
#[test]
fn setrlimit_probe_output_parse_covers_states() {
    // 双资源全可用
    let ok = parse_setrlimit_probe_output("WM_RLIMIT_AS=OK\nWM_RLIMIT_CPU=OK\n");
    assert_eq!(ok.rlimit_as, SetrlimitProbe::Available);
    assert_eq!(ok.rlimit_cpu, SetrlimitProbe::Available);
    // AS 被拒（裸 macOS 实测：CPython 把内核 EINVAL 映射成 ValueError）+ CPU 可用
    let partial = parse_setrlimit_probe_output(
        "WM_RLIMIT_AS=ValueError: current limit exceeds maximum limit\nWM_RLIMIT_CPU=OK\n",
    );
    assert_eq!(
        partial.rlimit_as,
        SetrlimitProbe::Unavailable("ValueError: current limit exceeds maximum limit".into())
    );
    assert_eq!(partial.rlimit_cpu, SetrlimitProbe::Available);
    // 缺 CPU 项 → 该资源按探测失败算，绝不猜成可用
    let missing = parse_setrlimit_probe_output("WM_RLIMIT_AS=OK\n");
    assert_eq!(missing.rlimit_as, SetrlimitProbe::Available);
    assert!(matches!(missing.rlimit_cpu, SetrlimitProbe::ProbeError(_)));
    // 全不可解析 → 双探测失败
    let garbage = parse_setrlimit_probe_output("Traceback (most recent call last):");
    assert!(matches!(garbage.rlimit_as, SetrlimitProbe::ProbeError(_)));
    assert!(matches!(garbage.rlimit_cpu, SetrlimitProbe::ProbeError(_)));
}

#[cfg(unix)]
#[test]
fn rlimit_warn_line_pins_hard_constraint() {
    let all_ok = SetrlimitSupport {
        rlimit_as: SetrlimitProbe::Available,
        rlimit_cpu: SetrlimitProbe::Available,
        rlimit_nproc: SetrlimitProbe::Available,
    };
    assert!(rlimit_warn_line(&all_ok).is_none());
    // 硬约束钉死：未生效项必须明说「限额未生效」；探测失败项必须明说
    // 「未探测到 setrlimit 可用性」+「限额未设」，不许静默、不许含糊。
    let off = rlimit_warn_line(&SetrlimitSupport {
        rlimit_as: SetrlimitProbe::Unavailable("ValueError: bad".to_string()),
        rlimit_cpu: SetrlimitProbe::Available,
        rlimit_nproc: SetrlimitProbe::Available,
    })
    .expect("未生效项必须留痕");
    assert!(off.contains("限额未生效"), "got: {off}");
    assert!(off.contains("RLIMIT_AS"), "got: {off}");
    let err = rlimit_warn_line(&SetrlimitSupport {
        rlimit_as: SetrlimitProbe::Available,
        rlimit_cpu: SetrlimitProbe::ProbeError("spawn 失败".to_string()),
        rlimit_nproc: SetrlimitProbe::Available,
    })
    .expect("探测失败必须留痕");
    assert!(err.contains("未探测到 setrlimit 可用性"), "got: {err}");
    assert!(err.contains("限额未设"), "got: {err}");
}

#[cfg(unix)]
#[test]
fn setrlimit_probe_real_python_gives_definite_verdict() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let s = run_setrlimit_probe(&py);
    // 有真实解释器在手，逐资源结论必须是确定的（Available/Unavailable）；
    // ProbeError 意味着探测层故障，本机不该出现。裸 macOS 的真相是
    // AS 被内核拒（Unavailable）+ CPU 可设（Available）——正是本批要暴露的。
    assert!(
        !matches!(s.rlimit_as, SetrlimitProbe::ProbeError(_)),
        "RLIMIT_AS 探测层故障: {s:?}"
    );
    assert!(
        !matches!(s.rlimit_cpu, SetrlimitProbe::ProbeError(_)),
        "RLIMIT_CPU 探测层故障: {s:?}"
    );
}

// ── 失败路径审计（超时 / spawn_fail 必留痕）──

#[test]
fn run_python_at_timeout_writes_audit_line() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过（测试不强制依赖真实子进程）
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-timeout");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.py"), "import time\ntime.sleep(30)\n").unwrap();
    let mut lines: Vec<String> = Vec::new();
    let r = run_python_at(
        &py,
        Some("run.py"),
        &dir,
        None,
        &[],
        Some(1),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    );
    let e = match r {
        Err(f) => f.msg,
        Ok(_) => panic!("1s 超时的 sleep 30 脚本不应成功"),
    };
    assert!(e.contains("超时"), "got: {e}");
    assert!(
        lines.iter().any(|l| l.contains("kind=timeout")),
        "缺 timeout 审计行: {lines:?}"
    );
    // 超时 = 脚本跑过了（可能跑了一半）→ 目录留着给模型读回
    assert!(dir.exists(), "超时路径应保留运行目录（脚本留在里面）");
}

#[test]
fn run_python_at_spawn_fail_writes_audit_line() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-spawn-fail");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
    let mut lines: Vec<String> = Vec::new();
    let r = run_python_at(
        "/nonexistent/python-zzz",
        Some("run.py"),
        &dir,
        None,
        &[],
        Some(1),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    );
    let e = match r {
        Err(f) => f.msg,
        Ok(_) => panic!("无效 python 路径不应成功"),
    };
    assert!(e.contains("启动 Python 失败"), "got: {e}");
    assert!(
        lines.iter().any(|l| l.contains("kind=spawn_fail")),
        "缺 spawn_fail 审计行: {lines:?}"
    );
    // spawn 失败必须清理已创建的临时目录，否则磁盘泄漏
    assert!(!dir.exists(), "spawn 失败后临时目录应被清理");
}

// ── spawn 失败 / 目录初始化失败的即时泄漏收尾 ──

#[test]
fn spawn_fail_cleans_populated_dir_and_audits() {
    // 非法 py 路径 → spawn 失败：含 run.py + params.json 的临时目录必须整体清理，
    // 且有 spawn_fail 审计（cleanup_after_spawn_fail 同族路径）
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-spawn-fail-p25");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
    std::fs::write(dir.join("params.json"), "{}").unwrap();
    let mut lines: Vec<String> = Vec::new();
    let r = run_python_at(
        "/nonexistent/python-zzz",
        Some("run.py"),
        &dir,
        None,
        &[],
        Some(1),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    );
    match r {
        Err(f) => assert!(f.msg.contains("启动 Python 失败"), "got: {}", f.msg),
        Ok(_) => panic!("无效 python 路径不应成功"),
    }
    assert!(
        lines.iter().any(|l| l.contains("kind=spawn_fail")),
        "缺 spawn_fail 审计行: {lines:?}"
    );
    assert!(!dir.exists(), "spawn 失败后临时目录应被清理");
    assert!(
        std::fs::read_dir(tmp.path()).unwrap().next().is_none(),
        "不得残留任何文件"
    );
}

#[test]
fn setup_run_dir_success_writes_script_and_params() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = setup_run_dir(tmp.path(), "print(1)\n", Some("{\"a\":1}")).unwrap();
    // 脚本正文原样落盘（模型写的那份）：报错行号与它一一对应
    assert_eq!(
        std::fs::read_to_string(dir.join("script.py")).unwrap(),
        "print(1)\n"
    );
    // 执行入口是宿主生成的：起看门狗后转交正文，不含用户脚本
    let entry = std::fs::read_to_string(dir.join("run.py")).unwrap();
    assert!(entry.contains("script.py"), "入口应转交 script.py：{entry}");
    assert!(
        !entry.contains("print(1)"),
        "入口不得夹带用户脚本（正文单独成文件）：{entry}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("params.json")).unwrap(),
        "{\"a\":1}"
    );
}

#[cfg(unix)]
#[test]
fn setup_run_dir_write_failure_cleans_up() {
    // base 只读 → create_dir_all 失败：返回 Err 且不得残留半成品目录
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ro = tmp.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = setup_run_dir(&ro, "print(1)\n", None);
    assert!(r.is_err(), "只读 base 下初始化应失败");
    assert!(
        std::fs::read_dir(&ro).unwrap().next().is_none(),
        "失败后不得残留临时目录"
    );
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ── cleanup_after_fail（失败路径杀进程组 + 保留运行目录）──

#[test]
fn cleanup_after_fail_kills_child_and_keeps_dir() {
    // mock Child 难，直接 spawn `sleep` 验证：调用后子进程已退出、运行目录留着
    //（Unix 下 RunLimits::terminate 是 no-op，kill_tree 兜底 child.kill()）
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run-cleanup");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("script.py"), "x").unwrap();
    let sleeper = if cfg!(windows) { "cmd" } else { "sleep" };
    let args: &[&str] = if cfg!(windows) {
        &["/C", "ping", "-n", "30", "127.0.0.1"]
    } else {
        &["30"]
    };
    let mut child = silent_cmd(sleeper).args(args).spawn().unwrap();
    let limits = RunLimits::new(0, 0);
    cleanup_after_fail(&mut child, &limits);
    assert!(
        dir.exists(),
        "失败路径必须保留运行目录——脚本留在里面，模型失败后才能读回"
    );
    assert!(
        child.try_wait().unwrap().is_some(),
        "子进程应已被杀死，不得留孤儿"
    );
}

// ── 失败留存脚本（模型要能读回「刚才跑了什么」）──

/// 脚本跑失败时，运行目录必须留着，且脚本正文原样可读。
///
/// 回归背景：脚本没跑成时目录被删掉，模型手里就只剩「失败」两个字——
/// 想改就得把整段脚本重写一遍（每次重写都重付一遍全文的输出 token）。
#[test]
fn failed_run_keeps_script_readable() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let script = "raise SystemExit('boom')\n";
    let dir = setup_run_dir(tmp.path(), script, None).unwrap();
    let mut lines: Vec<String> = Vec::new();
    let res = match run_python_at(
        &py,
        Some(ENTRY_FILE),
        &dir,
        None,
        &[],
        Some(10),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    ) {
        Ok(r) => r,
        Err(f) => panic!("非零退出码应如实返回结果而不是报错: {}", f.msg),
    };
    assert_eq!(res.exit_code, Some(1), "非零退出码要如实上报");
    assert!(dir.exists(), "失败后运行目录必须留着");
    assert_eq!(
        std::fs::read_to_string(dir.join(SCRIPT_FILE)).unwrap(),
        script,
        "脚本正文必须原样可读回"
    );
    assert!(
        res.retained_dir.is_some(),
        "失败时必须把留存目录告诉调用方（模型才知道去哪读）"
    );
}

/// 跑成功时运行目录连同产物一起清掉（不留垃圾）。
#[test]
fn successful_run_clears_run_dir() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = setup_run_dir(tmp.path(), "print(1)\n", None).unwrap();
    let mut lines: Vec<String> = Vec::new();
    let res = match run_python_at(
        &py,
        Some(ENTRY_FILE),
        &dir,
        None,
        &[],
        Some(10),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    ) {
        Ok(r) => r,
        Err(f) => panic!("正常脚本应跑成功: {}", f.msg),
    };
    assert_eq!(res.exit_code, Some(0));
    assert!(!dir.exists(), "成功后运行目录必须清掉");
    assert!(res.retained_dir.is_none(), "成功不留存目录（没失败可读回）");
}

/// traceback 的行号必须与脚本正文一致（模型照着报错行号改那一行才对得上）。
///
/// 回归背景：看门狗曾经拼在脚本正文前面，traceback 行号整体下移 7 行——
/// 模型按报错行号改自己的脚本会改错行，「只改出错那一行」就变成了「改错一行」。
#[test]
fn traceback_line_numbers_match_script_body() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let script = "x = 1\ny = 2\nraise ValueError('boom')\n";
    let dir = setup_run_dir(tmp.path(), script, None).unwrap();
    let mut lines: Vec<String> = Vec::new();
    let res = match run_python_at(
        &py,
        Some(ENTRY_FILE),
        &dir,
        None,
        &[],
        Some(10),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    ) {
        Ok(r) => r,
        Err(f) => panic!("脚本应跑完（只是非零退出）: {}", f.msg),
    };
    let frame = res
        .stderr
        .lines()
        .find(|l| l.contains(SCRIPT_FILE) && l.contains("line"))
        .unwrap_or_else(|| panic!("traceback 里没有脚本正文帧：{}", res.stderr));
    assert!(
        frame.contains("line 3"),
        "报错帧应指向 {SCRIPT_FILE} 第 3 行（与模型写的文件一致）：{frame}"
    );
}

/// 脚本必须仍以「主模块」身份执行，`__file__` 仍指向运行目录。
///
/// 回归背景：正文拆到 script.py 后由入口转交。转交方式选错（把主模块换成
/// 临时模块）会让 `__name__` 不再是 `__main__`——脚本里
/// `if __name__ == "__main__":` 那段就整段不执行，看起来「脚本跑了但什么也没做」。
#[test]
fn script_runs_as_main_module() {
    let Some(py) = detect_python() else {
        return; // 无 Python 环境跳过
    };
    let tmp = tempfile::tempdir().unwrap();
    let script = "import sys\nprint('name=' + __name__)\nprint('file=' + __file__)\n";
    let dir = setup_run_dir(tmp.path(), script, None).unwrap();
    let mut lines: Vec<String> = Vec::new();
    let res = match run_python_at(
        &py,
        Some(ENTRY_FILE),
        &dir,
        None,
        &[],
        Some(10),
        PY_MAX_TIMEOUT_SECS,
        &mut |l: &str| lines.push(l.to_string()),
        None,
    ) {
        Ok(r) => r,
        Err(f) => panic!("脚本应跑成功: {}", f.msg),
    };
    assert!(
        res.stdout.contains("name=__main__"),
        "脚本应以主模块身份执行：{}",
        res.stdout
    );
    assert!(
        res.stdout.contains("run.py"),
        "__file__ 应指向运行目录里的入口：{}",
        res.stdout
    );
}
