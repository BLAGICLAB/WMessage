//! Python 执行基础设施（facade）
//!
//! 原 3694 行 bot_py.rs 按 SRP 切片到 `py/` 子模块（env / runtime /
//! io / harvest / audit / document / commands）。本文件保留为 facade：
//! `pub use py::*` 重导出所有 `pub` 项，`crate::bot_py::X` 路径全部兼容（lib.rs /
//! tools.rs / 测试模块调用无修改）。
//!
//! **测试模块保持在本文件**（line 3694 之前的 `mod tests` / `mod exiting_tests` /
//! `mod platform_tests`）：`super::X` 经 pub use 解析为 `py::X`，无需改动测试内容。
//! **4 个回归锁除外**（读源字符串的），已改读 /src/py/ 对应子模块（runtime.rs / env.rs）。

// 显式 re-export 每个子模块的 pub 项（glob `pub use crate::py::*` 只展开
// py 的顶层项即子模块声明本身，不递归 re-export 子模块内部 pub 项）
pub use crate::py::audit::*;
pub use crate::py::commands::*;
pub use crate::py::document::*;
pub use crate::py::env::*;
pub use crate::py::harvest::*;
pub use crate::py::io::*;
pub use crate::py::runtime::*;
#[cfg(test)]
fn guard_test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    // 原 bot_py.rs 顶层 use（被 facade 收走后丢失，补回以让测试编译）
    use crate::audit::escape_for_log;
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
        // 64KB+ 输出：内容截断到 cap、标记 truncated，且剩余部分被排空读完
        //（排空的验证：Cursor 位置必须走到末尾，等价于管道读到 EOF，子进程不会吃 SIGPIPE）
        let cap = OUTPUT_CAP + 1024;
        let data = vec![b'x'; cap + 100];
        let cursor = std::io::Cursor::new(data);
        let (buf, truncated) = read_capped_drain(cursor, cap);
        assert!(truncated);
        assert_eq!(buf.len(), cap);
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

    // ── resolve_timeout / 资源限额（默认 60s、硬钳上限 300s、限额按 timeout 比例）──

    #[test]
    fn resolve_timeout_default_60s() {
        assert_eq!(resolve_timeout(None), (DEFAULT_TIMEOUT_SECS, false));
        assert_eq!(resolve_timeout(Some(30)), (30, false));
        assert_eq!(resolve_timeout(Some(300)), (300, false));
    }

    #[test]
    fn resolve_timeout_clamps_over_300s() {
        // 传入 10000 应被钳到 300 并报告 clamped
        assert_eq!(resolve_timeout(Some(10000)), (MAX_TIMEOUT_SECS, true));
        assert_eq!(resolve_timeout(Some(u64::MAX)), (MAX_TIMEOUT_SECS, true));
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
            &dir.to_path_buf(),
            None,
            &[],
            Some(120),
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
        };
        assert!(rlimit_warn_line(&all_ok).is_none());
        // 硬约束钉死：未生效项必须明说「限额未生效」；探测失败项必须明说
        // 「未探测到 setrlimit 可用性」+「限额未设」，不许静默、不许含糊。
        let off = rlimit_warn_line(&SetrlimitSupport {
            rlimit_as: SetrlimitProbe::Unavailable("ValueError: bad".to_string()),
            rlimit_cpu: SetrlimitProbe::Available,
        })
        .expect("未生效项必须留痕");
        assert!(off.contains("限额未生效"), "got: {off}");
        assert!(off.contains("RLIMIT_AS"), "got: {off}");
        let err = rlimit_warn_line(&SetrlimitSupport {
            rlimit_as: SetrlimitProbe::Available,
            rlimit_cpu: SetrlimitProbe::ProbeError("spawn 失败".to_string()),
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
        assert!(!dir.exists(), "超时路径应清理临时目录");
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
        assert_eq!(
            std::fs::read_to_string(dir.join("run.py")).unwrap(),
            "print(1)\n"
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

    // ── cleanup_after_fail（失败路径杀进程组 + 清临时目录）──

    #[test]
    fn cleanup_after_fail_kills_child_and_removes_dir() {
        // mock Child 难，直接 spawn `sleep` 验证：调用后子进程已退出、临时目录已删
        //（Unix 下 RunLimits::terminate 是 no-op，kill_tree 兜底 child.kill()）
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-cleanup");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "x").unwrap();
        let sleeper = if cfg!(windows) { "cmd" } else { "sleep" };
        let args: &[&str] = if cfg!(windows) {
            &["/C", "ping", "-n", "30", "127.0.0.1"]
        } else {
            &["30"]
        };
        let mut child = silent_cmd(sleeper).args(args).spawn().unwrap();
        let limits = RunLimits::new(0, 0);
        cleanup_after_fail(&mut child, &dir, &limits);
        assert!(!dir.exists(), "失败路径必须清理临时目录");
        assert!(
            child.try_wait().unwrap().is_some(),
            "子进程应已被杀死，不得留孤儿"
        );
    }

    // ── 退出清理注册表（kill_py_children 整树杀 + 守卫注销）──

    #[cfg(unix)]
    #[test]
    fn kill_py_children_kills_registered_process_group() {
        // 模拟生产 spawn（process_group(0) 自成组首）：注册后按 pid 整组强杀，
        // 子进程必须已退出（不得留孤儿）；守卫 Drop 后注册表必须清空
        use std::os::unix::process::CommandExt;
        let mut cmd = silent_cmd("sleep");
        cmd.arg("30");
        cmd.process_group(0);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        {
            let _guard = ChildRegGuard::register(&child);
            assert!(
                py_children()
                    .lock()
                    .unwrap_or_else(|e| {
                        eprintln!("[mutex_poisoned] bot_py::tests py_children: {e:?}");
                        e.into_inner()
                    })
                    .contains(&pid),
                "注册后必须能查到 pid"
            );
            assert_eq!(kill_py_children(&[pid]), 1, "应杀掉 1 个进程组");
            let status = child.wait().unwrap();
            assert!(!status.success(), "子进程应已被强杀");
        }
        assert!(
            !py_children()
                .lock()
                .unwrap_or_else(|e| {
                    eprintln!("[mutex_poisoned] bot_py::tests py_children: {e:?}");
                    e.into_inner()
                })
                .contains(&pid),
            "守卫 Drop 后必须注销（防 pid 复用误杀）"
        );
        // 已退出的 pid 再杀：组不存在按未杀计，不 panic
        assert_eq!(kill_py_children(&[pid]), 0);
    }

    // ── 残留目录清扫（启动时清 py-runs 超龄目录）──

    #[test]
    fn sweep_stale_py_runs_removes_only_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("py-runs");
        let stale = root.join("stale1");
        let fresh = root.join("fresh1");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(root.join("stray.txt"), b"x").unwrap();
        let now = std::time::SystemTime::now();
        // max_age=0 → 所有目录视为过期，全删（非目录文件不动）
        let removed = sweep_stale_py_runs_in(&root, Duration::ZERO, now);
        assert_eq!(removed, 2);
        assert!(!stale.exists() && !fresh.exists());
        assert!(root.join("stray.txt").exists(), "非目录文件不应被清扫");
        // 重建 fresh：max_age=1h → 刚建的目录必须保留
        std::fs::create_dir_all(&fresh).unwrap();
        let removed = sweep_stale_py_runs_in(&root, Duration::from_secs(3600), now);
        assert_eq!(removed, 0);
        assert!(fresh.exists());
        // root 不存在时静默返回 0
        assert_eq!(
            sweep_stale_py_runs_in(&tmp.path().join("no-such-dir"), Duration::ZERO, now),
            0
        );
    }

    // ── 运行目录产物回收（harvest_run_outputs）──

    #[test]
    fn harvest_moves_new_files_and_skips_harness_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run");
        let gen = tmp.path().join("gen");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::write(dir.join("run.py"), b"x").unwrap();
        std::fs::write(dir.join("params.json"), b"{}").unwrap();
        std::fs::write(dir.join("report.docx"), b"a").unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out").join("data.csv"), b"b").unwrap();
        // gen 里已有同名文件 → 搬入必须加 (1) 序号，不覆盖
        std::fs::write(gen.join("report.docx"), b"old").unwrap();
        let mut lines: Vec<String> = Vec::new();
        harvest_run_outputs(&dir, &gen, &mut |l: &str| lines.push(l.to_string()));
        assert!(dir.join("run.py").exists() && dir.join("params.json").exists());
        assert!(!dir.join("report.docx").exists() && !dir.join("out").exists());
        assert_eq!(std::fs::read(gen.join("report.docx")).unwrap(), b"old");
        assert_eq!(std::fs::read(gen.join("report (1).docx")).unwrap(), b"a");
        assert_eq!(
            std::fs::read(gen.join("out").join("data.csv")).unwrap(),
            b"b"
        );
        assert!(
            lines.iter().any(|l| l.contains("harvested=2")),
            "缺回收审计行: {lines:?}"
        );
    }

    #[test]
    fn dedup_dest_handles_dirs_and_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("d")).unwrap();
        assert_eq!(
            dedup_dest(dir, std::ffi::OsStr::new("a.txt")),
            dir.join("a (2).txt")
        );
        assert_eq!(
            dedup_dest(dir, std::ffi::OsStr::new("d")),
            dir.join("d (1)")
        );
        assert_eq!(
            dedup_dest(dir, std::ffi::OsStr::new("b.md")),
            dir.join("b.md")
        );
    }

    // ── 探测缓存（连续调用只探测一次；探测本身带超时）──

    #[test]
    fn cached_python_probes_only_once() {
        invalidate_python_cache();
        let _ = cached_python(); // 首次：真正探测
        let n1 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
        let a = cached_python();
        let b = cached_python();
        let n2 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(a, b, "缓存结果应稳定");
        assert_eq!(n2, n1, "缓存命中后不得重复 spawn 探测");
    }

    #[cfg(unix)]
    #[test]
    fn probe_version_times_out_on_hanging_shim() {
        // 卡死的 shim（sleep 远探测超时）必须在超时内按失败返回，不得挂住
        let start = Instant::now();
        let ok = probe_version_ok_with("sleep", &["10"], Duration::from_millis(300));
        assert!(!ok);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "探测挂死了：{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn spawn_not_found_flagged_for_cache_retry() {
        // 无效路径 spawn → RunFail.spawn_not_found=true（run_python 据此作废缓存重试）
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-notfound");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
        let r = run_python_at(
            "/nonexistent/python-zzz",
            Some("run.py"),
            &dir,
            None,
            &[],
            Some(1),
            &mut |_| {},
            None,
        );
        match r {
            Err(f) => assert!(f.spawn_not_found),
            Ok(_) => panic!("无效 python 路径不应成功"),
        }
    }

    // ── /stop 中断在途执行（停止令牌注入轮询循环）──

    #[test]
    fn run_python_at_stop_token_interrupts_promptly() {
        let Some(py) = detect_python() else {
            return; // 无 Python 环境跳过
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-stop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "import time\ntime.sleep(30)\n").unwrap();
        let guard = crate::bot_slash::StopGuard::new(&guard_test_handle(), false, None);
        let token = guard.token();
        let mut lines: Vec<String> = Vec::new();
        let start = Instant::now();
        // 50ms 后置位停止标志（模拟 /stop）；timeout 给足 60s，
        // 若停止检查失效就要等满 60s 才返回 —— 用耗时断言区分
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                guard.force_stop();
            });
            let r = run_python_at(
                &py,
                Some("run.py"),
                &dir,
                None,
                &[],
                Some(60),
                &mut |l: &str| lines.push(l.to_string()),
                Some(&token),
            );
            let e = match r {
                Err(f) => f.msg,
                Ok(_) => panic!("被停止的脚本不应成功返回"),
            };
            assert!(e.contains("已停止"), "got: {e}");
        });
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "/stop 后应立即中断，实际耗时 {:?}",
            start.elapsed()
        );
        assert!(
            lines.iter().any(|l| l.contains("kind=stopped")),
            "缺 stopped 审计行: {lines:?}"
        );
        assert!(!dir.exists(), "停止路径应清理临时目录");
    }

    // ── strip_known_ext（扩展名剥离大小写不敏感）──

    #[test]
    fn strip_known_ext_case_insensitive() {
        // 「周报.DOCX」+ docx → 剥掉后 gen_out_path 产出「周报.docx」而非「周报.DOCX.docx」
        assert_eq!(strip_known_ext("周报.DOCX", "docx"), "周报");
        assert_eq!(strip_known_ext("a.docx", "docx"), "a");
        assert_eq!(strip_known_ext("b.Docx", "docx"), "b");
        // 不匹配：无扩展名 / 别的扩展名 / 仅后缀相似（非 .ext），一律原样
        assert_eq!(strip_known_ext("报告", "docx"), "报告");
        assert_eq!(strip_known_ext("a.txt", "docx"), "a.txt");
        assert_eq!(strip_known_ext("adocx", "docx"), "adocx");
        assert_eq!(strip_known_ext("a.docx.docx", "docx"), "a.docx");
    }

    // ── escape_for_log（剥换行/管道符，防伪造日志行）──

    #[test]
    fn escape_for_log_strips_newlines_and_pipes() {
        // 验收用例："a\nb| c" → "a\\nb||  c"
        assert_eq!(escape_for_log("a\nb| c", 300), "a\\nb||  c");
        assert_eq!(escape_for_log("x\ry", 300), "x\\ry");
        assert_eq!(escape_for_log("plain", 300), "plain");
        // 伪造前缀注入：剥完后无法再伪装成行首分隔符
        assert_eq!(
            escape_for_log("evil\n[2026-01-01 00:00:00] INFO | fake", 300),
            "evil\\n[2026-01-01 00:00:00] INFO ||  fake"
        );
    }

    #[test]
    fn escape_for_log_truncates_after_escape() {
        let long = "x".repeat(400);
        let out = escape_for_log(&long, 300);
        assert_eq!(out.chars().count(), 301);
        assert!(out.ends_with('…'));
    }

    // ── 并发闸门（同一时刻只允许一个 Python 任务在执行）──

    #[test]
    fn py_run_gate_serializes_concurrent_runs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CUR: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);
        let mut joins = Vec::new();
        // 10 个并发请求同时抢闸门，临界区内 sleep 100ms 放大竞争窗口
        for _ in 0..10 {
            joins.push(std::thread::spawn(|| {
                let _g = py_run_gate().lock().unwrap_or_else(|e| {
                    eprintln!("[mutex_poisoned] py::runtime::py_run_gate: {e:?}");
                    e.into_inner()
                });
                let cur = CUR.fetch_add(1, Ordering::SeqCst) + 1;
                MAX.fetch_max(cur, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                CUR.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for j in joins {
            j.join().unwrap();
        }
        assert_eq!(
            MAX.load(Ordering::SeqCst),
            1,
            "闸门内并发数必须恒为 1（多余请求排队，不并行）"
        );
    }

    // ── spawn_blocking_map（doc_* async 命令不得把阻塞压在 runtime worker 上）──

    #[test]
    fn spawn_blocking_map_ok_passthrough() {
        let r = tauri::async_runtime::block_on(spawn_blocking_map(|| Ok::<_, String>(42)));
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn spawn_blocking_map_err_passthrough() {
        let r: Result<i32, String> =
            tauri::async_runtime::block_on(spawn_blocking_map(|| Err("业务错误".to_string())));
        assert_eq!(r.unwrap_err(), "业务错误");
    }

    #[test]
    fn spawn_blocking_map_panic_mapped_to_err() {
        // 闭包 panic → JoinError → 映射为错误字符串，而不是扩散到调用方
        let r: Result<(), String> =
            tauri::async_runtime::block_on(spawn_blocking_map(|| panic!("boom")));
        let e = r.unwrap_err();
        assert!(e.starts_with("执行线程异常"), "got: {e}");
    }

    // ── py_audit_to（py_audit 必须与 write_event/audit_log 共用 BOT_LOG_LOCK）──

    #[test]
    fn py_audit_to_writes_wellformed_line() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        py_audit_to(&p, "py_exec | script: hello");
        let content = std::fs::read_to_string(&p).unwrap();
        let line = content.trim_end();
        assert!(
            line.starts_with('[') && line.contains("] py_exec | script: hello"),
            "got: {line}"
        );
    }

    #[test]
    fn py_audit_to_concurrent_no_interleaved_lines() {
        // 8 线程 × 100 行并发写：上锁后每行必须完整（无撕裂/合并），行数精确
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        let mut joins = Vec::new();
        for t in 0..8 {
            let p = p.clone();
            joins.push(std::thread::spawn(move || {
                for i in 0..100 {
                    py_audit_to(&p, &format!("marker-{t}-{i}-{}", "x".repeat(64)));
                }
            }));
        }
        for j in joins {
            j.join().unwrap();
        }
        let content = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 800, "行数不符（可能有错行合并/撕裂）");
        for line in lines {
            assert!(
                line.starts_with("[20") && line.ends_with(&"x".repeat(64)),
                "错行: {line}"
            );
        }
    }

    // ── doc_* command 结构化错误迁移 ──

    /// doc_extract 错误路径：对话框未选中文件 → CommandError::Internal（code 稳定）
    #[test]
    fn f2_resolve_doc_path_cancel_returns_internal() {
        let err = resolve_doc_path(None).unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::Internal);
        assert!(err.message().contains("用户取消了选择"));
    }

    /// doc_extract happy path（纯函数层）：选中文件 → 路径透传
    #[test]
    fn f2_resolve_doc_path_picked_passes_through() {
        let p = resolve_doc_path(Some("/tmp/a.docx".into())).unwrap();
        assert_eq!(p, "/tmp/a.docx");
    }

    /// 6 个 doc_* command 的脚本失败分支：统一 Internal 变体，stderr trim 后入 message
    #[test]
    fn f2_script_fail_err_covers_all_doc_commands() {
        for (cmd, what) in [
            ("doc_extract", "提取失败"),
            ("doc_make_word", "生成 Word 失败"),
            ("doc_make_word_revisions", "生成修订版 Word 失败"),
            ("doc_make_excel", "生成 Excel 失败"),
            ("doc_make_pdf", "生成 PDF 失败"),
            ("doc_make_ppt", "生成 PPT 失败"),
        ] {
            let err = script_fail_err(what, "  boom\n");
            assert_eq!(
                err.code(),
                crate::error::CommandErrorCode::Internal,
                "{cmd} 错误 code 应为 INTERNAL"
            );
            assert!(
                err.message().contains(what) && err.message().contains("boom"),
                "{cmd} message 应含操作名 + stderr：{}",
                err.message()
            );
            assert!(!err.is_recoverable(), "{cmd} Internal 不可重试");
        }
    }

    /// doc_make_* happy path（共享逻辑层）：输出路径落在目标目录 + 扩展名正确
    #[test]
    fn f2_gen_out_path_in_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
        assert!(out.starts_with(dir.to_str().unwrap()));
        assert!(out.ends_with("周报.docx"), "out={out}");
        assert!(dir.is_dir(), "create_dir_all 应已建目录");
    }

    /// 同名不覆盖：已存在 周报.docx → 产出 周报 (1).docx
    #[test]
    fn f2_gen_out_path_in_conflict_appends_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("周报.docx"), b"x").unwrap();
        let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
        assert!(out.ends_with("周报 (1).docx"), "out={out}");
    }

    /// 防路径穿越：../../etc/evil → 只取 basename
    #[test]
    fn f2_gen_out_path_in_strips_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        let out = gen_out_path_in(&dir, Some("../../etc/evil"), "xlsx").unwrap();
        assert!(out.ends_with("evil.xlsx"), "out={out}");
        assert!(!out.contains("etc"), "out={out}");
    }

    /// doc_make_* 错误路径（共享逻辑层）：AI_Gen_Files 被文件占用 →
    /// create_dir_all 失败 → CommandError::IoError（不再走 String 逃生舱）
    #[test]
    fn f2_gen_out_path_in_io_error_maps_to_io_error_variant() {
        let tmp = tempfile::tempdir().unwrap();
        let file_as_dir = tmp.path().join("AI_Gen_Files");
        std::fs::write(&file_as_dir, b"not a dir").unwrap();
        let err = gen_out_path_in(&file_as_dir, Some("x"), "pdf").unwrap_err();
        assert_eq!(err.code(), crate::error::CommandErrorCode::IoError);
        assert!(!err.is_recoverable());
    }

    /// Windows 子进程直拉 Python 时强制 UTF-8 IO encoding，
    /// 否则 stdout 默认 cp936，Rust 端按 UTF-8 解码看到乱码。
    #[cfg(windows)]
    #[test]
    fn silent_cmd_on_windows_sets_python_utf8_env() {
        let cmd = silent_cmd("python");
        let mut found_io = false;
        let mut found_utf8 = false;
        for (k, v) in cmd.get_envs().flatten() {
            if k.to_str() == Some("PYTHONIOENCODING") && v.to_str() == Some("utf-8") {
                found_io = true;
            }
            if k.to_str() == Some("PYTHONUTF8") && v.to_str() == Some("1") {
                found_utf8 = true;
            }
        }
        assert!(found_io, "PYTHONIOENCODING=utf-8 未设置");
        assert!(found_utf8, "PYTHONUTF8=1 未设置");
    }
}

#[cfg(test)]
mod exiting_tests {
    /// 回归锁：EXITING 复查必须在 PY_RUN_GATE 拿锁之后
    ///（锁前检查挡不住「kill 完成后才拿到锁的排队者」）。
    /// 全局标志不在测试里翻转（会污染并行测试的 run_python），源码锁防回退。
    #[test]
    fn exiting_check_is_after_gate_lock() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/runtime.rs"))
                .unwrap();
        let fn_pos = text
            .find("pub fn run_python(")
            .expect("run_python 必须存在");
        let body = &text[fn_pos..];
        let gate = body.find("py_run_gate().lock()").expect("必须过并发闸门");
        let check = body.find("EXITING.load").expect("必须有退出标志复查");
        assert!(check > gate, "EXITING 复查必须在闸门拿锁之后");
    }
}

#[cfg(test)]
mod platform_tests {
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
    #[cfg(unix)]
    #[test]
    fn unix_scripts_get_parent_watchdog() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/runtime.rs"))
                .unwrap();
        assert!(text.contains("PARENT_WATCHDOG"), "必须有看门狗前导常量");
        let fn_pos = text
            .find("fn run_python_ungated(")
            .expect("run_python_ungated 必须存在");
        let body: String = text[fn_pos..].chars().take(1200).collect();
        assert!(
            body.contains("PARENT_WATCHDOG"),
            "看门狗必须在 run_python_ungated 注入（覆盖全部 Python 执行入口）"
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
}
