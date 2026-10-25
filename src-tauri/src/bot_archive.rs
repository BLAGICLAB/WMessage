//! 归档工具：`unpack_archive`（解包）/ `pack_archive`（打包）。
//!
//! 压缩包是**外部输入**：条目名可以带 `..`、可以是绝对路径、可以是软链；声明的
//! 大小可以撒谎（zip bomb）。所以这里的每一道闸都在**落盘前**问一句——不是解完
//! 再补救。
//!
//! 格式：zip（stored + deflate）、tar、tar.gz。解包多认一种无压缩的 tar
//! （`tar` 命令造出来的常见形态）；打包只产 zip / tar.gz。
//!
//! 权限口径沿用 `bot_fs`：源走读白名单闸，目标走写白名单闸。解压出来的文件
//! 落在可写目录里，不存在「绕过白名单写盘」这条路。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::bot::registry::ToolResult;

// ── 上限阀 ────────────────────────────────────────────────────────────────
//
// 解包读的是不可信数据：上限按「最坏的合法用途」放宽，但必须有顶。
// 打包读的是用户自己的文件：上限放宽一档（几百 MB 的单个文件是正常需求）。

/// 解包：单个文件解压后上限 64MB。
const UNPACK_MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// 解包：整包解压总量上限 256MB。
const UNPACK_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// 解包：条目数上限 5000。
const UNPACK_MAX_ENTRIES: usize = 5000;
/// 解包：放大比上限 200:1。低于 64MB 不看这条（小文件本来就压得狠，
/// 一份 1MB 的重复文本能压到 5KB，比值 200:1 但毫无攻击性）。
const MAX_EXPANSION_RATIO: u64 = 200;
/// 放大比判定的地板：解压总量超过它才查比值。
const RATIO_FLOOR_BYTES: u64 = 64 * 1024 * 1024;

/// 打包：单个文件上限 512MB。
const PACK_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// 打包：整包总量上限 2GB。
const PACK_MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 打包：条目数上限 20000。
const PACK_MAX_ENTRIES: usize = 20_000;

/// 变更凭证回执上限：解出几千个文件时不能把回执列表撑爆（回执会进上下文）。
const MAX_RECEIPTS: usize = 50;

/// 打包时跳过的目录名（体积大、与交付无关）。与 `bot_fs` 的 SKIP_DIRS 同口径。
const SKIP_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", ".git"];

/// 归档格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
}

impl ArchiveFormat {
    fn as_str(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
        }
    }
}

/// 上限集合（解包 / 打包两套）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    max_entry_bytes: u64,
    max_total_bytes: u64,
    max_entries: usize,
}

impl Limits {
    pub(crate) const fn unpack() -> Self {
        Limits {
            max_entry_bytes: UNPACK_MAX_ENTRY_BYTES,
            max_total_bytes: UNPACK_MAX_TOTAL_BYTES,
            max_entries: UNPACK_MAX_ENTRIES,
        }
    }
    const fn pack() -> Self {
        Limits {
            max_entry_bytes: PACK_MAX_ENTRY_BYTES,
            max_total_bytes: PACK_MAX_TOTAL_BYTES,
            max_entries: PACK_MAX_ENTRIES,
        }
    }
}

/// 解包结果：写出的文件 + 被跳过的条目。
#[derive(Debug, Default)]
pub(crate) struct UnpackOutcome {
    /// 实际识别的格式（auto 时由文件头判定）
    pub(crate) format: Option<ArchiveFormat>,
    /// (落盘绝对路径, 字节数)
    pub(crate) written: Vec<(PathBuf, u64)>,
    /// (条目名, 跳过原因)
    pub(crate) skipped: Vec<(String, String)>,
    pub(crate) total_bytes: u64,
}

/// 打包结果。
#[derive(Debug, Default)]
struct PackOutcome {
    entries: usize,
    total_bytes: u64,
    /// (条目名, 跳过原因)
    skipped: Vec<(String, String)>,
}

// ── 路径安全 ──────────────────────────────────────────────────────────────

/// 把归档条目名解析成「目标目录下的相对路径」；不安全就返回 `None`。
///
/// 拒掉的东西（每一类都是真实存在的攻击手法）：
/// - `../escape.txt`、`a/../../b` —— zip slip：写到目标目录之外
/// - `/etc/passwd`、`C:\Windows\x`、`//server/share/x` —— 绝对路径 / 盘符 / UNC
/// - 含 `\` 的名字 —— Windows 造的 zip 里是路径分隔符，不当分隔符就会被拼成
///   一个落在目录外的文件名
/// - 含 NUL / 控制字符的名字 —— 落盘时的截断与显示层歧义
/// - 空名 / 只有 `.` 段 —— 没有对应文件
///
/// 反斜杠一律当分隔符处理（`a\b.txt` → `a/b.txt`），但**不含** `..` 段的
/// 判定仍然照做——反斜杠路径同样能逃出目标目录。
fn safe_rel_path(entry_name: &str) -> Option<PathBuf> {
    if entry_name.is_empty() {
        return None;
    }
    if entry_name.chars().any(|c| c.is_control()) {
        return None;
    }
    let norm = entry_name.replace('\\', "/");
    // 绝对路径与 UNC（`//server/share` 归一后也以 / 开头）
    if norm.starts_with('/') {
        return None;
    }
    // 盘符：`C:` / `c:` 开头（含 `C:foo` 这种「相对盘符」，Windows 会当绝对路径解）
    let b = norm.as_bytes();
    if b.len() >= 2 && b[1] == b':' && (b[0] as char).is_ascii_alphabetic() {
        return None;
    }
    let mut out = PathBuf::new();
    for seg in norm.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

/// unix 权限位里是不是符号链接（S_IFLNK）。
///
/// zip 的外部属性在 Unix 系统下是「权限位 << 16」，其中高 4 位是文件类型位。
/// Windows 造的 zip 没有类型位（`unix_mode()` 返 None 或普通文件位）——那类包里
/// 也不会有软链，按「不是软链」处理是对的。
fn is_symlink_mode(mode: Option<u32>) -> bool {
    match mode {
        Some(m) => m & 0o170000 == 0o120000,
        None => false,
    }
}

/// 归档条目名 → zip/tar 通用条目名（统一用 `/` 分隔）。
fn entry_name_of(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// 目标目录内的最终落盘路径；不在目录之下就拒（safe_rel_path 之后的第二道）。
fn joined_under(dest: &Path, rel: &Path) -> Option<PathBuf> {
    let p = dest.join(rel);
    if p.starts_with(dest) {
        Some(p)
    } else {
        None
    }
}

/// 打包时的目录名清洗：去掉路径分隔符与控制字符，长度封顶。
fn sanitize_dir_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        return "archive".to_string();
    }
    if trimmed.chars().count() > 60 {
        trimmed.chars().take(60).collect()
    } else {
        trimmed
    }
}

// ── 格式识别 ──────────────────────────────────────────────────────────────

/// 按文件头识别格式（不看后缀——后缀是撒谎的 commonplace）。
fn detect_format(path: &Path) -> std::io::Result<ArchiveFormat> {
    use std::io::Seek;
    let mut f = File::open(path)?;
    let mut head = [0u8; 4];
    let n = f.read(&mut head)?;
    // zip：本地文件头 PK\x03\x04；空包 PK\x05\x06；分卷 PK\x07\x08
    if n >= 2 && &head[..2] == b"PK" {
        return Ok(ArchiveFormat::Zip);
    }
    // gzip：\x1f\x8b
    if n >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        return Ok(ArchiveFormat::TarGz);
    }
    // tar：ustar 标记在第 257 字节；旧式 tar 没有，但旧式 tar 极少，
    // 交给 tar 解析器报错（错误文案会带上「无法识别的压缩格式」）。
    let mut magic = [0u8; 5];
    if f.seek(std::io::SeekFrom::Start(257)).is_ok()
        && f.read_exact(&mut magic).is_ok()
        && &magic == b"ustar"
    {
        return Ok(ArchiveFormat::Tar);
    }
    Ok(ArchiveFormat::Tar)
}

// ── 解包 ──────────────────────────────────────────────────────────────────

/// 解包（按格式分派；`format` 为 None 时按文件头识别）。
pub(crate) fn unpack(
    src: &Path,
    dest: &Path,
    limits: &Limits,
    format: Option<ArchiveFormat>,
) -> std::io::Result<UnpackOutcome> {
    let fmt = match format {
        Some(f) => f,
        None => detect_format(src)?,
    };
    let mut outcome = match fmt {
        ArchiveFormat::Zip => unpack_zip(src, dest, limits)?,
        ArchiveFormat::Tar => unpack_tar(File::open(src)?, dest, limits, file_size(src))?,
        ArchiveFormat::TarGz => unpack_tar_gz(src, dest, limits)?,
    };
    outcome.format = Some(fmt);
    Ok(outcome)
}

/// 文件字节数（拿不到就 0——放大比判定会跳过，比值检查不是硬闸）。
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn unpack_tar_gz(src: &Path, dest: &Path, limits: &Limits) -> std::io::Result<UnpackOutcome> {
    use flate2::read::GzDecoder;
    let f = File::open(src)?;
    // gzip 头里的原始大小不可信，放大比按「压缩包文件大小」算：够用且不会误判
    // （一个 2MB 的 tar.gz 解出 100MB，比值 50:1，正常）。
    let compressed = f.metadata().map(|m| m.len()).unwrap_or(0);
    let dec = GzDecoder::new(f);
    unpack_tar(dec, dest, limits, compressed)
}

fn unpack_zip(src: &Path, dest: &Path, limits: &Limits) -> std::io::Result<UnpackOutcome> {
    let file = File::open(src)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("zip 解析失败：{e}"),
        )
    })?;
    let mut out = UnpackOutcome::default();
    let mut compressed_total: u64 = 0;
    for i in 0..archive.len() {
        if out.written.len() + out.skipped.len() >= limits.max_entries {
            out.skipped.push((
                "…".into(),
                format!("条目数超过 {} 上限，其余条目未处理", limits.max_entries),
            ));
            break;
        }
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                out.skipped
                    .push((format!("#{i}"), format!("条目读取失败：{e}")));
                continue;
            }
        };
        let raw_name = entry.name().to_string();
        compressed_total = compressed_total.saturating_add(entry.compressed_size());
        // 软链条目不落盘：软链能把后续条目引到目标目录之外
        if is_symlink_mode(entry.unix_mode()) {
            out.skipped.push((
                raw_name,
                "符号链接条目未解压（防止链接把文件引到目标目录外）".into(),
            ));
            continue;
        }
        if entry.is_dir() {
            if let Some(rel) = safe_rel_path(raw_name.trim_end_matches('/')) {
                if let Some(dir) = joined_under(dest, &rel) {
                    let _ = std::fs::create_dir_all(dir);
                }
            }
            continue;
        }
        let Some(rel) = safe_rel_path(&raw_name) else {
            out.skipped.push((
                raw_name,
                "条目路径不安全（含 .. / 绝对路径 / 盘符 / 控制字符），已跳过".into(),
            ));
            continue;
        };
        if entry.size() > limits.max_entry_bytes {
            out.skipped.push((
                raw_name,
                format!(
                    "声明大小 {} 超过单文件 {}MB 上限，已跳过",
                    entry.size(),
                    limits.max_entry_bytes / 1024 / 1024
                ),
            ));
            continue;
        }
        let Some(final_path) = joined_under(dest, &rel) else {
            out.skipped
                .push((raw_name, "条目路径越出目标目录，已跳过".into()));
            continue;
        };
        if final_path.exists() {
            out.skipped.push((
                raw_name,
                "目标已存在，未覆盖（要覆盖请先删除同名文件）".into(),
            ));
            continue;
        }
        match write_entry(&mut entry, &final_path, limits) {
            Ok(written) => {
                out.total_bytes = out.total_bytes.saturating_add(written);
                out.written.push((final_path, written));
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                out.skipped.push((raw_name, format!("解压失败：{e}")));
            }
            Err(e) => return Err(e),
        }
        if out.total_bytes > limits.max_total_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "解压总量超过 {}MB 上限，已中止（疑似压缩炸弹）",
                    limits.max_total_bytes / 1024 / 1024
                ),
            ));
        }
        if out.total_bytes > RATIO_FLOOR_BYTES
            && compressed_total > 0
            && out.total_bytes / compressed_total > MAX_EXPANSION_RATIO
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "解压放大比超过 {}:1，已中止（疑似压缩炸弹）",
                    MAX_EXPANSION_RATIO
                ),
            ));
        }
    }
    Ok(out)
}

fn unpack_tar<R: Read>(
    reader: R,
    dest: &Path,
    limits: &Limits,
    compressed_total: u64,
) -> std::io::Result<UnpackOutcome> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("tar 解析失败：{e}"),
        )
    })?;
    let mut out = UnpackOutcome::default();
    for entry in entries {
        if out.written.len() + out.skipped.len() >= limits.max_entries {
            out.skipped.push((
                "…".into(),
                format!("条目数超过 {} 上限，其余条目未处理", limits.max_entries),
            ));
            break;
        }
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.skipped.push(("?".into(), format!("条目读取失败：{e}")));
                continue;
            }
        };
        let header = entry.header().clone();
        let etype = header.entry_type();
        // 软链 / 硬链条目不落盘（理由同上）
        if etype.is_symlink() || etype.is_hard_link() {
            let raw = entry
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "?".into());
            out.skipped
                .push((raw, "链接条目未解压（防止链接把文件引到目标目录外）".into()));
            continue;
        }
        let raw_name = match entry.path() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => {
                out.skipped
                    .push(("?".into(), "条目名不是合法 UTF-8，已跳过".into()));
                continue;
            }
        };
        if etype.is_dir() {
            if let Some(rel) = safe_rel_path(raw_name.trim_end_matches('/')) {
                if let Some(dir) = joined_under(dest, &rel) {
                    let _ = std::fs::create_dir_all(dir);
                }
            }
            continue;
        }
        let Some(rel) = safe_rel_path(&raw_name) else {
            out.skipped.push((
                raw_name,
                "条目路径不安全（含 .. / 绝对路径 / 盘符 / 控制字符），已跳过".into(),
            ));
            continue;
        };
        if entry.size() > limits.max_entry_bytes {
            out.skipped.push((
                raw_name,
                format!(
                    "声明大小 {} 超过单文件 {}MB 上限，已跳过",
                    entry.size(),
                    limits.max_entry_bytes / 1024 / 1024
                ),
            ));
            continue;
        }
        let Some(final_path) = joined_under(dest, &rel) else {
            out.skipped
                .push((raw_name, "条目路径越出目标目录，已跳过".into()));
            continue;
        };
        if final_path.exists() {
            out.skipped.push((
                raw_name,
                "目标已存在，未覆盖（要覆盖请先删除同名文件）".into(),
            ));
            continue;
        }
        match write_entry(&mut entry, &final_path, limits) {
            Ok(written) => {
                out.total_bytes = out.total_bytes.saturating_add(written);
                out.written.push((final_path, written));
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                out.skipped.push((raw_name, format!("解压失败：{e}")));
            }
            Err(e) => return Err(e),
        }
        if out.total_bytes > limits.max_total_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "解压总量超过 {}MB 上限，已中止（疑似压缩炸弹）",
                    limits.max_total_bytes / 1024 / 1024
                ),
            ));
        }
        if out.total_bytes > RATIO_FLOOR_BYTES
            && compressed_total > 0
            && out.total_bytes / compressed_total > MAX_EXPANSION_RATIO
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "解压放大比超过 {}:1，已中止（疑似压缩炸弹）",
                    MAX_EXPANSION_RATIO
                ),
            ));
        }
    }
    Ok(out)
}

/// 写一个条目：临时文件 + rename（失败不留半个文件）。
///
/// 上限在**读**的时候卡（不是读完再判）：声明的大小可以撒谎，实际解压出来
/// 多少字节才是真的——`take(cap+1)` 读到超限就停。
fn write_entry<R: Read>(
    reader: &mut R,
    final_path: &Path,
    limits: &Limits,
) -> std::io::Result<u64> {
    let parent = final_path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "目标路径没有父目录")
    })?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".wm-archive-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<u64> {
        let mut tf = File::create(&tmp)?;
        let mut limited = (&mut *reader).take(limits.max_entry_bytes + 1);
        let written = std::io::copy(&mut limited, &mut tf)?;
        tf.flush()?;
        Ok(written)
    })();
    let written = match result {
        Ok(w) => w,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if written > limits.max_entry_bytes {
        let _ = std::fs::remove_file(&tmp);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "解压后大小超过 {}MB 上限（声明的大小不可信），已跳过",
                limits.max_entry_bytes / 1024 / 1024
            ),
        ));
    }
    std::fs::rename(&tmp, final_path)?;
    Ok(written)
}

// ── 打包 ──────────────────────────────────────────────────────────────────

/// 打包（按格式分派）。
fn pack(
    src: &Path,
    out: &Path,
    format: ArchiveFormat,
    limits: &Limits,
) -> std::io::Result<PackOutcome> {
    // 输出不能落在源目录内部：否则会把自己打进自己，越打越大
    if src.is_dir() && out.starts_with(src) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "输出文件不能放在被打包的目录内部",
        ));
    }
    match format {
        ArchiveFormat::Zip => pack_zip(src, out, limits),
        ArchiveFormat::Tar | ArchiveFormat::TarGz => pack_tar(src, out, format, limits),
    }
}

/// 收集待打包的文件（相对源根的路径）。
///
/// 软链一律跳过：不跟随（跟随可能读出白名单外的文件），也不把链接本身写进包里。
fn collect_files(
    root: &Path,
    limits: &Limits,
) -> std::io::Result<(Vec<(PathBuf, PathBuf)>, Vec<(String, String)>)> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    if root.is_file() {
        files.push((root.to_path_buf(), PathBuf::from(&root_name)));
        return Ok((files, skipped));
    }
    let mut stack = vec![(root.to_path_buf(), PathBuf::from(&root_name))];
    while let Some((dir, rel_dir)) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                skipped.push((
                    rel_dir.to_string_lossy().to_string(),
                    format!("目录读取失败：{e}"),
                ));
                continue;
            }
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let path = ent.path();
            let ft = match ent.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                skipped.push((name, "软链未打包（不跟随、也不写入链接本身）".into()));
                continue;
            }
            let rel = rel_dir.join(&name);
            if ft.is_dir() {
                stack.push((path, rel));
                continue;
            }
            if !ft.is_file() {
                skipped.push((name, "不是普通文件，未打包".into()));
                continue;
            }
            let size = ent.metadata().map(|m| m.len()).unwrap_or(0);
            if size > limits.max_entry_bytes {
                skipped.push((
                    name,
                    format!(
                        "文件 {} 超过单文件 {}MB 上限，未打包",
                        size,
                        limits.max_entry_bytes / 1024 / 1024
                    ),
                ));
                continue;
            }
            if files.len() >= limits.max_entries {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("待打包文件数超过 {} 上限", limits.max_entries),
                ));
            }
            files.push((path, rel));
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok((files, skipped))
}

fn pack_zip(src: &Path, out: &Path, limits: &Limits) -> std::io::Result<PackOutcome> {
    let (files, skipped) = collect_files(src, limits)?;
    let file = File::create(out)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default_for_write());
    let mut total = 0u64;
    let mut count = 0usize;
    for (path, rel) in &files {
        let mut f = File::open(path)?;
        zw.start_file(entry_name_of(rel), opts)
            .map_err(|e| std::io::Error::other(format!("zip 写入失败：{e}")))?;
        let written = std::io::copy(&mut f, &mut zw)?;
        total = total.saturating_add(written);
        if total > limits.max_total_bytes {
            // 中止：留下半截的包也比超限的好——但仍要报错让模型知道
            let _ = zw.finish();
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "打包总量超过 {}GB 上限，已中止",
                    limits.max_total_bytes / 1024 / 1024 / 1024
                ),
            ));
        }
        count += 1;
    }
    zw.finish()
        .map_err(|e| std::io::Error::other(format!("zip 收尾失败：{e}")))?;
    Ok(PackOutcome {
        entries: count,
        total_bytes: total,
        skipped,
    })
}

fn pack_tar(
    src: &Path,
    out: &Path,
    format: ArchiveFormat,
    limits: &Limits,
) -> std::io::Result<PackOutcome> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let (files, skipped) = collect_files(src, limits)?;
    let file = File::create(out)?;
    let mut total = 0u64;
    let mut count = 0usize;
    match format {
        ArchiveFormat::TarGz => {
            let enc = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(enc);
            for (path, rel) in &files {
                let n = file_size(path);
                builder
                    .append_path_with_name(path, entry_name_of(rel))
                    .map_err(|e| std::io::Error::other(format!("tar 写入失败：{e}")))?;
                total = total.saturating_add(n);
                count += 1;
            }
            let enc = builder
                .into_inner()
                .map_err(|e| std::io::Error::other(format!("tar 收尾失败：{e}")))?;
            enc.finish()
                .map_err(|e| std::io::Error::other(format!("gzip 收尾失败：{e}")))?;
        }
        _ => {
            let mut builder = tar::Builder::new(file);
            for (path, rel) in &files {
                let n = file_size(path);
                builder
                    .append_path_with_name(path, entry_name_of(rel))
                    .map_err(|e| std::io::Error::other(format!("tar 写入失败：{e}")))?;
                total = total.saturating_add(n);
                count += 1;
            }
            builder
                .finish()
                .map_err(|e| std::io::Error::other(format!("tar 收尾失败：{e}")))?;
        }
    }
    if total > limits.max_total_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "打包总量超过 {}GB 上限，已中止",
                limits.max_total_bytes / 1024 / 1024 / 1024
            ),
        ));
    }
    Ok(PackOutcome {
        entries: count,
        total_bytes: total,
        skipped,
    })
}

// ── 工具层 ────────────────────────────────────────────────────────────────

/// 解压工具：压缩包 → 目录。
pub(crate) async fn tool_unpack_archive(
    app: &tauri::AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return ToolResult::denied("unpack_archive 缺少 path（压缩包路径）");
    };
    // 源：读白名单闸（压缩包本身必须在读白名单内）
    let canonical = match crate::bot_fs::resolve_with_perm(
        app,
        "unpack_archive",
        path,
        interactive,
        session_id,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return ToolResult::denied(e),
    };
    // 目标：给了就走写白名单闸；不给则默认 AI_Gen_Files/解压/<包名>/
    // （gen_dir 恒在可写根内，默认目标不过闸也不会越界）
    let dest = match v["destDir"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(d) => {
            match crate::bot_fs::resolve_writable(app, "unpack_archive", d, interactive, session_id)
                .await
            {
                Ok(p) => p,
                Err(e) => return ToolResult::denied(e),
            }
        }
        None => default_unpack_dir(app, &canonical),
    };
    // 格式：显式给了就用；不给（auto）按文件头识别
    let explicit: Option<ArchiveFormat> = match v["format"].as_str().map(str::trim) {
        Some("zip") => Some(ArchiveFormat::Zip),
        Some("tar") => Some(ArchiveFormat::Tar),
        Some("tar.gz") | Some("tgz") => Some(ArchiveFormat::TarGz),
        _ => None,
    };
    let dest_for_task = dest.clone();
    let archive = canonical.clone();
    let res = crate::py::document::spawn_blocking_map(move || -> Result<UnpackOutcome, String> {
        std::fs::create_dir_all(&dest_for_task).map_err(|e| format!("创建目标目录失败：{e}"))?;
        let dest_canon =
            std::fs::canonicalize(&dest_for_task).map_err(|e| format!("解析目标目录失败：{e}"))?;
        let limits = Limits::unpack();
        unpack(&archive, &dest_canon, &limits, explicit).map_err(|e| format!("解压失败：{e}"))
    })
    .await;
    let outcome = match res {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("解压失败：{e}"), Vec::new()),
    };
    // 文案：写出的清单 + 跳过的条目（模型要能看到「哪些没解、为什么」）
    let fmt_note = outcome.format.map(|f| f.as_str()).unwrap_or("压缩包");
    let mut lines = vec![format!(
        "已从 {fmt_note} 解压 {} 个文件到 {}（合计 {}）：",
        outcome.written.len(),
        dest.display(),
        human_size(outcome.total_bytes)
    )];
    for (p, n) in outcome.written.iter().take(100) {
        lines.push(format!("  {}（{}）", p.display(), human_size(*n)));
    }
    if outcome.written.len() > 100 {
        lines.push(format!(
            "  ……（其余 {} 个文件未逐条列出）",
            outcome.written.len() - 100
        ));
    }
    if !outcome.skipped.is_empty() {
        lines.push(format!("已跳过 {} 个条目：", outcome.skipped.len()));
        for (name, why) in outcome.skipped.iter().take(50) {
            lines.push(format!("  {name}：{why}"));
        }
        if outcome.skipped.len() > 50 {
            lines.push(format!(
                "  ……（其余 {} 条未列出）",
                outcome.skipped.len() - 50
            ));
        }
    }
    let mut result = ToolResult::ok(lines.join("\n"), Vec::new());
    // 变更凭证：解出的文件算工具产物（模型可以如实说「已解压出 N 个文件」）
    for (p, _) in outcome.written.iter().take(MAX_RECEIPTS) {
        result = result.with_file_change(crate::bot_fs::build_binary_file_receipt(
            &p.to_string_lossy(),
        ));
    }
    result
}

/// 打包工具：目录 / 文件 → 压缩包。
pub(crate) async fn tool_pack_archive(
    app: &tauri::AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(src) = v["sourcePath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("pack_archive 缺少 sourcePath（要打包的目录或文件）");
    };
    // 源：读白名单闸
    let canonical =
        match crate::bot_fs::resolve_with_perm(app, "pack_archive", src, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    // 格式：显式给了就用它；否则按输出路径后缀判定
    let explicit = match v["format"].as_str().map(str::trim) {
        Some("zip") => Some(ArchiveFormat::Zip),
        Some("tar.gz") | Some("tgz") => Some(ArchiveFormat::TarGz),
        Some("tar") => Some(ArchiveFormat::Tar),
        _ => None,
    };
    // 输出：给了就走写白名单闸；不给则默认 AI_Gen_Files/<源名>.zip
    let out = match v["outputPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(o) => {
            match crate::bot_fs::resolve_writable(app, "pack_archive", o, interactive, session_id)
                .await
            {
                Ok(p) => p,
                Err(e) => return ToolResult::denied(e),
            }
        }
        None => default_pack_path(app, &canonical, explicit),
    };
    let format = explicit.unwrap_or_else(|| format_from_path(&out));
    let src_for_task = canonical.clone();
    let out_for_task = out.clone();
    let res = crate::py::document::spawn_blocking_map(move || -> Result<PackOutcome, String> {
        let limits = Limits::pack();
        pack(&src_for_task, &out_for_task, format, &limits).map_err(|e| format!("打包失败：{e}"))
    })
    .await;
    let outcome = match res {
        Ok(o) => o,
        Err(e) => return ToolResult::error(e, Vec::new()),
    };
    let size = std::fs::metadata(&out)
        .map(|m| m.len())
        .unwrap_or(outcome.total_bytes);
    let mut lines = vec![format!(
        "已打包 {} 个文件（{}）到：{}",
        outcome.entries,
        human_size(size),
        out.display()
    )];
    if !outcome.skipped.is_empty() {
        lines.push(format!("已跳过 {} 项：", outcome.skipped.len()));
        for (name, why) in outcome.skipped.iter().take(50) {
            lines.push(format!("  {name}：{why}"));
        }
        if outcome.skipped.len() > 50 {
            lines.push(format!(
                "  ……（其余 {} 项未列出）",
                outcome.skipped.len() - 50
            ));
        }
    }
    lines.push("向用户报告时必须附上完整路径。".to_string());
    ToolResult::ok(lines.join("\n"), Vec::new()).with_file_change(
        crate::bot_fs::build_binary_file_receipt(&out.to_string_lossy()),
    )
}

/// 字节数 → 人看的尺寸。
fn human_size(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

/// 默认解压目录：AI_Gen_Files/解压/<包名>/。
fn default_unpack_dir(app: &tauri::AppHandle, archive: &Path) -> PathBuf {
    let gen = crate::db::gen_dir(app).unwrap_or_else(|_| std::env::temp_dir());
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    gen.join("解压").join(sanitize_dir_name(&stem))
}

/// 默认打包输出：AI_Gen_Files/<源名>.zip（tar.gz 时换后缀）。
fn default_pack_path(app: &tauri::AppHandle, src: &Path, format: Option<ArchiveFormat>) -> PathBuf {
    let gen = crate::db::gen_dir(app).unwrap_or_else(|_| std::env::temp_dir());
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    let stem = sanitize_dir_name(&name);
    let ext = match format.unwrap_or(ArchiveFormat::Zip) {
        ArchiveFormat::TarGz => "tar.gz",
        _ => "zip",
    };
    gen.join(format!("{stem}.{ext}"))
}

/// 按输出路径后缀判格式。
fn format_from_path(out: &Path) -> ArchiveFormat {
    let name = out.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        ArchiveFormat::TarGz
    } else {
        ArchiveFormat::Zip
    }
}

#[cfg(test)]
mod tests;
