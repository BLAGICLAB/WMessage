//! 归档工具的纯函数与集成测试。

use super::*;

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 造一个 zip：条目名 → 内容（内容为 None 表示目录项）。
fn make_zip(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
    let f = File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in entries {
        match content {
            Some(bytes) => {
                zw.start_file(*name, opts).unwrap();
                zw.write_all(bytes).unwrap();
            }
            None => {
                zw.add_directory(*name, opts).unwrap();
            }
        }
    }
    zw.finish().unwrap();
}

/// 造一个 zip，里面放一个符号链接条目。
///
/// zip crate 的 writer 造不出软链条目——`unix_permissions()` 只保留权限位、
/// 丢掉文件类型位（它自己的测试就钉着这条：「unix_permissions() throws away
/// upper bits」）。所以这里先造普通条目，再手工补中央目录里的外部属性
/// （偏移 38 处的 4 字节 = 类型位|权限位 << 16），还原真实世界里
/// `zip -y` 造出来的软链条目。
fn make_zip_with_symlink(path: &Path, link_name: &str, target: &str) {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file(link_name, opts).unwrap();
        zw.write_all(target.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    let cd = buf
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("中央目录头应存在");
    let attrs: u32 = 0o120777u32 << 16; // S_IFLNK | 0777
    buf[cd + 38..cd + 42].copy_from_slice(&attrs.to_le_bytes());
    std::fs::write(path, &buf).unwrap();
}

/// 造一个 tar.gz：条目名 → 内容。
fn make_targz(path: &Path, entries: &[(&str, &[u8])]) {
    let f = File::create(path).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, *name, *content).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

// ── 路径安全 ──────────────────────────────────────────────────────────

#[test]
fn safe_rel_path_rejects_escapes() {
    assert!(safe_rel_path("../evil.txt").is_none(), ".. 必须拒");
    assert!(
        safe_rel_path("a/../../evil.txt").is_none(),
        "中间 .. 必须拒"
    );
    assert!(safe_rel_path("/etc/passwd").is_none(), "绝对路径必须拒");
    assert!(safe_rel_path("C:\\Windows\\evil").is_none(), "盘符必须拒");
    assert!(safe_rel_path("c:evil").is_none(), "相对盘符也必须拒");
    assert!(safe_rel_path("//server/share/x").is_none(), "UNC 必须拒");
    assert!(
        safe_rel_path("\\\\server\\share\\x").is_none(),
        "反斜杠 UNC 必须拒"
    );
    assert!(
        safe_rel_path("a\\..\\..\\evil").is_none(),
        "反斜杠 .. 必须拒"
    );
    assert!(safe_rel_path("bad\0name").is_none(), "NUL 必须拒");
    assert!(safe_rel_path("bad\nname").is_none(), "控制字符必须拒");
    assert!(safe_rel_path("").is_none());
    assert!(safe_rel_path(".").is_none());
}

#[test]
fn symlink_mode_detection() {
    assert!(is_symlink_mode(Some(0o120777)));
    assert!(is_symlink_mode(Some(0o120000)));
    assert!(!is_symlink_mode(Some(0o100644)), "普通文件不是软链");
    assert!(!is_symlink_mode(Some(0o40755)), "目录不是软链");
    assert!(!is_symlink_mode(None), "无 unix 权限位即无软链");
}

#[test]
fn safe_rel_path_accepts_normal_names() {
    assert_eq!(safe_rel_path("a/b/c.txt"), Some(PathBuf::from("a/b/c.txt")));
    assert_eq!(safe_rel_path("./a.txt"), Some(PathBuf::from("a.txt")));
    assert_eq!(safe_rel_path("a//b.txt"), Some(PathBuf::from("a/b.txt")));
    // 反斜杠当分隔符处理
    assert_eq!(safe_rel_path("a\\b.txt"), Some(PathBuf::from("a/b.txt")));
    // 目录项（尾部斜杠）
    assert_eq!(safe_rel_path("dir/"), Some(PathBuf::from("dir")));
}

// ── 解包 ──────────────────────────────────────────────────────────────

#[test]
fn unpack_writes_files_and_dirs() {
    let d = tmpdir();
    let zip = d.path().join("a.zip");
    make_zip(
        &zip,
        &[
            ("top.txt", Some(b"hello")),
            ("sub/", None),
            ("sub/inner.txt", Some(b"inner")),
        ],
    );
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = unpack(&zip, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(outcome.written.len(), 2);
    assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    assert_eq!(
        std::fs::read(dest.join("top.txt")).unwrap(),
        b"hello".to_vec()
    );
    assert_eq!(
        std::fs::read(dest.join("sub/inner.txt")).unwrap(),
        b"inner".to_vec()
    );
}

#[test]
fn unpack_zip_slip_never_writes_outside_dest() {
    let d = tmpdir();
    let zip = d.path().join("evil.zip");
    make_zip(
        &zip,
        &[
            ("../escaped.txt", Some(b"pwned")),
            ("ok.txt", Some(b"fine")),
        ],
    );
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = unpack(&zip, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(outcome.written.len(), 1, "只应写出 ok.txt");
    assert!(!d.path().join("escaped.txt").exists(), "逃逸文件必须不存在");
    assert_eq!(outcome.skipped.len(), 1);
    assert!(
        outcome.skipped[0].1.contains("路径不安全"),
        "{:?}",
        outcome.skipped[0]
    );
}

#[test]
fn unpack_skips_symlink_entries() {
    let d = tmpdir();
    let zip = d.path().join("link.zip");
    make_zip_with_symlink(&zip, "link", "/etc/passwd");
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = unpack(&zip, &dest, &Limits::unpack(), None).unwrap();
    assert!(outcome.written.is_empty(), "软链不得落盘");
    assert_eq!(outcome.skipped.len(), 1);
    assert!(
        outcome.skipped[0].1.contains("符号链接"),
        "{:?}",
        outcome.skipped[0]
    );
}

#[test]
fn unpack_skips_existing_targets_without_overwriting() {
    let d = tmpdir();
    let zip = d.path().join("a.zip");
    make_zip(&zip, &[("keep.txt", Some(b"from-zip"))]);
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("keep.txt"), b"original").unwrap();
    let outcome = unpack(&zip, &dest, &Limits::unpack(), None).unwrap();
    assert!(outcome.written.is_empty());
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(
        std::fs::read(dest.join("keep.txt")).unwrap(),
        b"original".to_vec(),
        "已有文件不得被覆盖"
    );
}

#[test]
fn unpack_enforces_total_size_cap() {
    let d = tmpdir();
    let zip = d.path().join("bomb.zip");
    // 声明大小撒谎：头里写小、实际内容大——读的时候卡上限才拦得住
    make_zip(&zip, &[("big.bin", Some(&vec![7u8; 1024 * 1024]))]);
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let mut limits = Limits::unpack();
    limits.max_total_bytes = 1024; // 1KB 顶
    let err = unpack(&zip, &dest, &limits, None).unwrap_err();
    assert!(err.to_string().contains("上限"), "超总量必须报错：{err}");
}

#[test]
fn unpack_enforces_entry_count_cap() {
    let d = tmpdir();
    let zip = d.path().join("many.zip");
    let names: Vec<(String, Vec<u8>)> = (0..10)
        .map(|i| (format!("f{i}.txt"), vec![1u8, 2]))
        .collect();
    let refs: Vec<(&str, Option<&[u8]>)> = names
        .iter()
        .map(|(n, c)| (n.as_str(), Some(c.as_slice())))
        .collect();
    make_zip(&zip, &refs);
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let mut limits = Limits::unpack();
    limits.max_entries = 3;
    let outcome = unpack(&zip, &dest, &limits, None).unwrap();
    assert_eq!(outcome.written.len(), 3, "条目数必须被卡住");
    assert!(outcome.skipped.iter().any(|(n, _)| n == "…"));
}

#[test]
fn unpack_targz_reads_entries() {
    let d = tmpdir();
    let tgz = d.path().join("a.tar.gz");
    make_targz(&tgz, &[("x.txt", b"content"), ("y/z.txt", b"deep")]);
    let dest = d.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = unpack(&tgz, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(outcome.written.len(), 2, "{:?}", outcome.skipped);
    assert_eq!(
        std::fs::read(dest.join("y/z.txt")).unwrap(),
        b"deep".to_vec()
    );
}

#[test]
fn detect_format_by_magic_bytes() {
    let d = tmpdir();
    let zip = d.path().join("x.zip");
    make_zip(&zip, &[("a.txt", Some(b"a"))]);
    assert_eq!(detect_format(&zip).unwrap(), ArchiveFormat::Zip);
    let tgz = d.path().join("x.tar.gz");
    make_targz(&tgz, &[("a.txt", b"a")]);
    assert_eq!(detect_format(&tgz).unwrap(), ArchiveFormat::TarGz);
    // 后缀骗人：以 .zip 结尾的 gzip 仍按文件头认
    let lying = d.path().join("lying.zip");
    std::fs::copy(&tgz, &lying).unwrap();
    assert_eq!(detect_format(&lying).unwrap(), ArchiveFormat::TarGz);
}

// ── 打包 ──────────────────────────────────────────────────────────────

#[test]
fn pack_zip_then_unpack_roundtrip() {
    let d = tmpdir();
    let src = d.path().join("proj");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), b"alpha").unwrap();
    std::fs::write(src.join("sub/b.txt"), b"beta").unwrap();
    let out = d.path().join("out.zip");
    let outcome = pack(&src, &out, ArchiveFormat::Zip, &Limits::pack()).unwrap();
    assert_eq!(outcome.entries, 2);
    let dest = d.path().join("unpacked");
    std::fs::create_dir_all(&dest).unwrap();
    let back = unpack(&out, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(back.written.len(), 2, "{:?}", back.skipped);
    // 目录名随包带出（zip -r 口径）：解出来是 proj/
    assert_eq!(
        std::fs::read(dest.join("proj/a.txt")).unwrap(),
        b"alpha".to_vec()
    );
    assert_eq!(
        std::fs::read(dest.join("proj/sub/b.txt")).unwrap(),
        b"beta".to_vec()
    );
}

#[test]
fn pack_targz_then_unpack_roundtrip() {
    let d = tmpdir();
    let src = d.path().join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("x.txt"), b"payload").unwrap();
    let out = d.path().join("out.tar.gz");
    let outcome = pack(&src, &out, ArchiveFormat::TarGz, &Limits::pack()).unwrap();
    assert_eq!(outcome.entries, 1);
    let dest = d.path().join("unpacked");
    std::fs::create_dir_all(&dest).unwrap();
    let back = unpack(&out, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(back.written.len(), 1, "{:?}", back.skipped);
    assert_eq!(
        std::fs::read(dest.join("data/x.txt")).unwrap(),
        b"payload".to_vec()
    );
}

#[test]
fn pack_refuses_output_inside_source() {
    let d = tmpdir();
    let src = d.path().join("proj");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), b"a").unwrap();
    let out = src.join("self.zip");
    let err = pack(&src, &out, ArchiveFormat::Zip, &Limits::pack()).unwrap_err();
    assert!(err.to_string().contains("内部"), "{err}");
}

#[test]
fn pack_skips_symlinks_in_source() {
    let d = tmpdir();
    let src = d.path().join("proj");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("real.txt"), b"real").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", src.join("link.txt")).unwrap();
    let out = d.path().join("out.zip");
    let outcome = pack(&src, &out, ArchiveFormat::Zip, &Limits::pack()).unwrap();
    assert_eq!(outcome.entries, 1, "软链不得进包");
    assert!(
        outcome.skipped.iter().any(|(_, why)| why.contains("软链")),
        "{:?}",
        outcome.skipped
    );
}

#[test]
fn pack_single_file_keeps_file_name() {
    let d = tmpdir();
    let file = d.path().join("note.txt");
    std::fs::write(&file, b"note").unwrap();
    let out = d.path().join("out.zip");
    pack(&file, &out, ArchiveFormat::Zip, &Limits::pack()).unwrap();
    let dest = d.path().join("unpacked");
    std::fs::create_dir_all(&dest).unwrap();
    let back = unpack(&out, &dest, &Limits::unpack(), None).unwrap();
    assert_eq!(back.written.len(), 1);
    assert_eq!(
        std::fs::read(dest.join("note.txt")).unwrap(),
        b"note".to_vec()
    );
}

#[test]
fn format_from_path_reads_suffix() {
    assert_eq!(
        format_from_path(Path::new("a.tar.gz")),
        ArchiveFormat::TarGz
    );
    assert_eq!(format_from_path(Path::new("a.tgz")), ArchiveFormat::TarGz);
    assert_eq!(format_from_path(Path::new("a.zip")), ArchiveFormat::Zip);
    assert_eq!(format_from_path(Path::new("a.bin")), ArchiveFormat::Zip);
}

#[test]
fn sanitize_dir_name_strips_separators() {
    assert_eq!(sanitize_dir_name("a/b"), "a_b");
    assert_eq!(sanitize_dir_name("  "), "archive");
    assert_eq!(sanitize_dir_name(".."), "archive");
    assert!(sanitize_dir_name(&"x".repeat(200)).chars().count() <= 60);
}

// ── 接线守卫 ────────────────────────────────────────────────────────

/// 两个归档工具必须真在工具表里、且标成「会改状态」。
///
/// `mutating: false` 的后果不是「少一个标记」——变更声称守卫（模型说
/// 「已解压」时的那道闸）靠 `mutating` 决定认不认这次工具调用。标错就
/// 变成：模型如实汇报被当成凭空声称。
#[test]
fn archive_tools_are_registered_as_mutating() {
    use crate::bot::registry::TOOLS_TABLE;
    for name in ["unpack_archive", "pack_archive"] {
        let t = TOOLS_TABLE
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("工具表里应有 {name}"));
        assert!(t.mutating, "{name} 会写盘，必须标 mutating");
        assert!(
            !t.claims_patterns.is_empty(),
            "{name} 必须有关键词——否则模型如实说「已解压」会被守卫当成编造"
        );
    }
}

/// 档位归属：research 是只读档（不装归档工具），coder / general 装。
#[test]
fn archive_tools_land_in_coder_and_general_profiles_only() {
    use crate::bot::registry::{CODER_TOOLS, GENERAL_TOOLS, RESEARCH_TOOLS};
    for name in ["unpack_archive", "pack_archive"] {
        assert!(CODER_TOOLS.contains(&name), "coder 档应有 {name}");
        assert!(GENERAL_TOOLS.contains(&name), "general 档应有 {name}");
        assert!(
            !RESEARCH_TOOLS.contains(&name),
            "research 是只读档，不该装 {name}"
        );
    }
}
