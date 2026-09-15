//! 个人资料：用户 / 机器人 头像 + 姓名。
//! 存储：数据目录 profile.json；头像文件拷贝到 profile/ 子目录（便携模式随 exe 走）。
//! 读取返回 base64 data URL，前端无需 asset protocol / CSP 配置。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

use crate::error::{CommandError, CommandResult};

const MAX_AVATAR_BYTES: u64 = 5 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 24;
const AVATAR_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// D4：profile read-modify-write 进程内串行锁（同 db::DB_WRITE_LOCK 模式）。
/// 主窗 + 挂件并发改 profile 时，整条 ProfileData 覆写会让后写者覆盖前写者 → 丢更新；
/// 所有写路径（set_name / set_avatar / remove_avatar）的 R-M-W 全程持锁。
/// load_data / profile_get 读路径频繁，不持锁避免阻塞读。
static PROFILE_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntry {
    #[serde(default)]
    pub name: String,
    /// 头像文件名（存 profile/ 目录）；None 用默认占位
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProfileData {
    #[serde(default)]
    pub user: ProfileEntry,
    #[serde(default)]
    pub bot: ProfileEntry,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntryView {
    pub name: String,
    pub avatar_data_url: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub user: ProfileEntryView,
    pub bot: ProfileEntryView,
}

fn default_name(kind: &str) -> String {
    if kind == "bot" { "机器人" } else { "我" }.to_string()
}

/// 数据目录：P2-19 起委托 paths::probe_log_dir（原与 db::db_dir 两处拷贝，已合一）。
/// Runtime 泛型（避免在 profile.rs 里硬编码 Wry，
/// 这样单元测试可以用 tauri::test::MockRuntime 跑同一条生产代码路径）。
/// cargo test 下 current_exe().parent() = target/debug/deps/ 可写，直接命中该分支。
///
/// 单元测试再按**进程**分一层子目录，见 `test_isolated_dir`。
fn data_dir<R: Runtime>(app: &AppHandle<R>) -> std::path::PathBuf {
    let base = crate::paths::probe_log_dir(app);
    #[cfg(test)]
    let base = test_isolated_dir(base);
    base
}

/// 单元测试专用：每个测试**进程**一个独立 profile 目录。
///
/// 为什么：`probe_log_dir` 在 cargo test 下解析到 `target/debug/deps/`——那是发布路径里
/// 与 bot.log 共用的目录，不能改；而 nextest 是**每测试一进程**，并行进程会共用同一份
/// `profile.json` 互相踩（`cargo nextest run` 下
/// `load_data_corrupt_json_backs_up_and_returns_default` / `profile_overwrite_replaces_old_avatar`
/// 等用例随机挂，已复现 6 次且每次失败集合不同）。按 pid 分子目录后：
/// - `cargo test --lib`：单进程内所有 profile 测试共用一个目录，仍由 `ENV_LOCK` 串行；
/// - `cargo nextest run`：每进程独立目录，天然隔离，不再依赖跨进程假设。
/// 残留：`target/debug/deps/profile-tests-<pid>/` 会随运行累积（各自几 KB），
/// 不做自动清理——跨进程清理本身就是竞态，且 target/ 可随时删。
#[cfg(test)]
fn test_isolated_dir(base: std::path::PathBuf) -> std::path::PathBuf {
    let dir = base.join(format!("profile-tests-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn profile_path<R: Runtime>(app: &AppHandle<R>) -> std::path::PathBuf {
    data_dir(app).join("profile.json")
}

fn profile_dir<R: Runtime>(app: &AppHandle<R>) -> std::path::PathBuf {
    data_dir(app).join("profile")
}

fn load_data<R: Runtime>(app: &AppHandle<R>) -> ProfileData {
    let path = profile_path(app);
    // 读失败（含首启文件不存在）→ 默认配置，不算损坏，不触发备份
    let Ok(s) = std::fs::read_to_string(&path) else {
        return ProfileData::default();
    };
    match serde_json::from_str::<ProfileData>(&s) {
        Ok(d) => d,
        Err(e) => {
            // D3：损坏不再静默吞——备份现场 profile.json.bak-<ts> + ERROR 审计，
            // 再回默认值保证程序可启动（原 unwrap_or_default 让用户资料无声消失）
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
            let bak = path.with_file_name(format!("profile.json.bak-{ts}"));
            let backup = match std::fs::rename(&path, &bak) {
                Ok(()) => bak.to_string_lossy().into_owned(),
                Err(be) => format!("备份失败: {be}"),
            };
            let err = e.to_string();
            crate::audit::write_error_audit(
                app,
                "profile_load_fail",
                &[("backup", backup.as_str()), ("err", err.as_str())],
            );
            ProfileData::default()
        }
    }
}

fn save_data<R: Runtime>(app: &AppHandle<R>, data: &ProfileData) -> Result<(), String> {
    let s = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    // D3：原子写（tmp + rename，复用 Phase 3 NEW-B-6 的 crate::db::atomic_write）——
    // 崩溃在写中途只留 tmp 残件，profile.json 要么旧完整版要么新完整版，不留半截
    crate::db::atomic_write(&profile_path(app), &s)
}

/// NEW-D-2：set_avatar 保存失败回滚——仅当新文件不是「在役头像本身」时才删。
/// 同扩展名覆盖场景 dest 就是在役文件（copy 已覆盖其内容），再删会让磁盘 json 悬挂引用、
/// 在役头像静默丢失。返回是否执行了删除（测试可断言）。
fn cleanup_set_avatar_failure(old_avatar: Option<&str>, new_dest: &std::path::Path) -> bool {
    let new_name = new_dest.file_name().and_then(|n| n.to_str());
    if new_name.is_some() && new_name == old_avatar {
        return false; // dest 就是在役头像，不能删
    }
    std::fs::remove_file(new_dest).is_ok()
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// NEW-D-4：avatar 字段必须是纯文件名，防手改/损坏的 profile.json 路径穿越。
/// 拒绝：空串、含 `/` / `\` 路径分隔符、含 `..` 父目录跳转。
/// 合法例：`avatar-user.png`；非法例：`../../wmessage.db`、`/etc/passwd`、`a\b.png`。
fn avatar_filename_only(f: &str) -> Option<&str> {
    if f.is_empty() || f.contains('/') || f.contains('\\') || f.contains("..") {
        return None;
    }
    Some(f)
}

fn entry_view<R: Runtime>(app: &AppHandle<R>, kind: &str, e: &ProfileEntry) -> ProfileEntryView {
    let name = if e.name.trim().is_empty() {
        default_name(kind)
    } else {
        e.name.clone()
    };
    // NEW-D-4：先过纯文件名校验，非法值当作无头像（None），不拼路径
    let data_url = e
        .avatar
        .as_deref()
        .and_then(avatar_filename_only)
        .and_then(|f| {
            let path = profile_dir(app).join(f);
            // P2-17：读取端同样限 5MB——写入端 profile_set_avatar 有大小校验，
            // 但手改/损坏的 profile.json 可指向任意大文件，读出 base64 广播会撑爆 IPC
            let meta = std::fs::metadata(&path).ok()?;
            if meta.len() > MAX_AVATAR_BYTES {
                crate::audit::write_error_audit(
                    app,
                    "profile_avatar_too_large",
                    &[
                        ("kind", kind),
                        ("file", f),
                        ("len", &meta.len().to_string()),
                    ],
                );
                return None;
            }
            let bytes = std::fs::read(&path).ok()?;
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
            Some(format!(
                "data:{};base64,{}",
                mime_for(ext),
                B64.encode(bytes)
            ))
        });
    ProfileEntryView {
        name,
        avatar_data_url: data_url,
    }
}

fn build_view<R: Runtime>(app: &AppHandle<R>, data: &ProfileData) -> ProfileView {
    ProfileView {
        user: entry_view(app, "user", &data.user),
        bot: entry_view(app, "bot", &data.bot),
    }
}

fn valid_kind(kind: &str) -> bool {
    kind == "user" || kind == "bot"
}

/// 资料变更后广播给所有窗口（主窗口 + 挂件）
fn broadcast<R: Runtime>(app: &AppHandle<R>) {
    let data = load_data(app);
    let _ = app.emit("profile-changed", build_view(app, &data));
}

#[tauri::command]
pub fn profile_get<R: Runtime>(app: AppHandle<R>) -> ProfileView {
    let data = load_data(&app);
    build_view(&app, &data)
}

#[tauri::command]
pub fn profile_set_name<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
    name: String,
) -> CommandResult<ProfileView> {
    if !valid_kind(&kind) {
        return Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: kind,
            reason: "必须为 user 或 bot".into(),
        });
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: String::new(),
            reason: "姓名不能为空".into(),
        });
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name.to_string(),
            reason: format!("姓名最长 {MAX_NAME_CHARS} 字"),
        });
    }
    // D4：R-M-W（load → 改 → save）全程持锁，广播与 view 构造放锁外
    let data = {
        let _g = PROFILE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = load_data(&app);
        let entry = if kind == "bot" {
            &mut data.bot
        } else {
            &mut data.user
        };
        entry.name = name.to_string();
        save_data(&app, &data)?;
        data
    };
    broadcast(&app);
    Ok(build_view(&app, &data))
}

#[tauri::command]
pub fn profile_set_avatar<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
    path: String,
) -> CommandResult<ProfileView> {
    if !valid_kind(&kind) {
        return Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: kind,
            reason: "必须为 user 或 bot".into(),
        });
    }
    let src = std::path::PathBuf::from(&path);
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if !AVATAR_EXTS.contains(&ext.as_str()) {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path.clone(),
            reason: "仅支持 png/jpg/gif/webp 图片".into(),
        });
    }
    let meta = std::fs::metadata(&src)?;
    if meta.len() > MAX_AVATAR_BYTES {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "头像图片不能超过 5MB".into(),
        });
    }
    // D4：整个写路径（清旧文件 → 拷贝 → R-M-W json）持锁，防主窗 + 挂件并发丢更新
    let data = {
        let _g = PROFILE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = profile_dir(&app);
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(format!("avatar-{kind}.{ext}"));
        // 清掉该 kind 的旧头像文件（扩展名可能不同）
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name();
                let fname = fname.to_string_lossy();
                if fname.starts_with(&format!("avatar-{kind}.")) && entry.path() != dest {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        std::fs::copy(&src, &dest)?;
        let mut data = load_data(&app);
        let entry = if kind == "bot" {
            &mut data.bot
        } else {
            &mut data.user
        };
        let Some(fname) = dest.file_name() else {
            return Err(CommandError::InvalidArgument {
                field: "path".into(),
                value: dest.to_string_lossy().into_owned(),
                reason: "头像目标路径无效".into(),
            });
        };
        // NEW-D-2：覆盖前先记在役头像名，save 失败回滚时区分「在役文件」与「新文件」
        let old_avatar = entry.avatar.clone();
        entry.avatar = Some(fname.to_string_lossy().into_owned());
        // 原子性：保存失败时回滚刚拷贝的头像文件，不留孤儿（二次审计 P3）。
        // NEW-D-2：同扩展名覆盖场景 dest 就是在役头像本身（copy 已覆盖内容），
        // 误删会让磁盘 json 悬挂引用、在役头像静默丢失——只在 dest 是「新文件」时才删。
        if let Err(e) = save_data(&app, &data) {
            cleanup_set_avatar_failure(old_avatar.as_deref(), &dest);
            return Err(CommandError::IoError(e));
        }
        data
    };
    broadcast(&app);
    Ok(build_view(&app, &data))
}

#[tauri::command]
pub fn profile_remove_avatar<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
) -> CommandResult<ProfileView> {
    if !valid_kind(&kind) {
        return Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: kind,
            reason: "必须为 user 或 bot".into(),
        });
    }
    // D4：R-M-W + 删文件全程持锁——删文件也放锁内，
    // 否则并发 set_avatar 同扩展名时可能删掉对方刚拷好的新头像
    let (data, removed) = {
        let _g = PROFILE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = load_data(&app);
        let entry = if kind == "bot" {
            &mut data.bot
        } else {
            &mut data.user
        };
        // NEW-D-3：先 save 把 json 引用清掉，成功后再删文件。
        // 原顺序（先删后 save）在 save 失败时磁盘 json 悬挂引用已删文件。
        // save 失败走 `?` 早退：avatar 文件 + json 引用都保持原状。
        let removed = entry.avatar.take();
        save_data(&app, &data)?;
        if let Some(f) = &removed {
            let _ = std::fs::remove_file(profile_dir(&app).join(f));
        }
        (data, removed)
    };
    let _ = removed; // 语义已在锁内消费（删文件）
    broadcast(&app);
    Ok(build_view(&app, &data))
}

// ───────────────────────── 单元测试 ─────────────────────────
//
// 测试策略：profile_get / profile_set_name / profile_set_avatar / profile_remove_avatar
// 都依赖 profile.rs::data_dir(app)。
// - 进程内：所有访问该目录的测试必须串行（ENV_LOCK 互斥），防并行写同一份 profile.json；
// - 进程间：data_dir 在 cfg(test) 下按 pid 分子目录（见 test_isolated_dir），
//   这样 `cargo nextest run`（每测试一进程）不会让并行进程共用同一份 profile.json。
// 默认 default_name / valid_kind / mime_for 等纯函数不需要锁，可自由并发。

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 串行化访问共享 data_dir 的测试（profile.json + profile/）。
    /// 任一时刻只允许一个 profile 测试运行，防止并行写同一份 profile.json。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 构造一个全新 mock app + 清掉残留 profile.json / profile/ 目录。
    /// 调用前必须先拿 ENV_LOCK。
    fn fresh_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let data_dir = data_dir(app.handle());
        let _ = std::fs::remove_file(data_dir.join("profile.json"));
        // D3：失败注入会把 profile.json.tmp 占成目录，残留要清掉
        let _ = std::fs::remove_dir_all(data_dir.join("profile.json.tmp"));
        // D3：损坏 load 会生成 profile.json.bak-<ts> 备份，残留要清掉
        if let Ok(rd) = std::fs::read_dir(&data_dir) {
            for e in rd.flatten() {
                if e.file_name()
                    .to_string_lossy()
                    .starts_with("profile.json.bak-")
                {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let profile_d = data_dir.join("profile");
        if profile_d.exists() {
            let _ = std::fs::remove_dir_all(&profile_d);
        }
        app
    }

    /// D3 失败注入：把 profile.json.tmp 占成目录 → atomic_write 写临时文件必败
    /// → save_data 必败；profile.json 本体仍可读——
    /// 模拟「磁盘 json 可读、保存失败」的真实故障窗口。
    /// （D3 前注入用「profile.json 只读」；atomic_write 走 tmp+rename，
    /// rename 覆盖不看目标文件权限，旧注入已无法让 save 失败。）
    fn make_save_fail_via_tmp_dir(data_dir: &std::path::Path) {
        std::fs::create_dir_all(data_dir.join("profile.json.tmp")).unwrap();
    }

    fn restore_save(data_dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(data_dir.join("profile.json.tmp"));
    }

    /// 1×1 透明 PNG 字节（最小有效 PNG，足够通过扩展名校验与大小校验）
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length+tag
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // 8bit RGBA + CRC
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, // IDAT length+tag
        0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, // zlib stream
        0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, // CRC
        0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82, // IEND
    ];

    fn make_src_avatar(ext: &str) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join(format!("src.{ext}"));
        std::fs::write(&src, TINY_PNG).unwrap();
        tmp
    }

    // ────── 纯函数测试（无需锁） ──────

    #[test]
    fn profile_default_names_zh() {
        assert_eq!(default_name("user"), "我");
        assert_eq!(default_name("bot"), "机器人");
        // 未知 kind 兜底为 user 默认（防呆）
        assert_eq!(default_name("admin"), "我");
    }

    #[test]
    fn profile_valid_kind_strict() {
        assert!(valid_kind("user"));
        assert!(valid_kind("bot"));
        assert!(!valid_kind(""));
        assert!(!valid_kind("admin"));
        assert!(!valid_kind("USER")); // 大小写敏感——避免误传
        assert!(!valid_kind("Bot ")); // 尾部空格也算
    }

    #[test]
    fn profile_mime_for_known_extensions() {
        assert_eq!(mime_for("png"), "image/png");
        assert_eq!(mime_for("jpg"), "image/jpeg");
        assert_eq!(mime_for("jpeg"), "image/jpeg");
        assert_eq!(mime_for("gif"), "image/gif");
        assert_eq!(mime_for("webp"), "image/webp");
        // mime_for 是 case-sensitive 的 const 查表——上游调用者负责 to_lowercase
        // (profile_set_avatar 在检查前已 to_ascii_lowercase，所以传过来的都是小写)
        assert_eq!(mime_for("bmp"), "application/octet-stream"); // 未知扩展兜底
        assert_eq!(mime_for(""), "application/octet-stream");
    }

    // ────── 共享状态测试（需 ENV_LOCK） ──────

    #[test]
    fn profile_missing_returns_default_view() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let view = profile_get(app.handle().clone());
        assert_eq!(view.user.name, "我");
        assert_eq!(view.bot.name, "机器人");
        assert!(view.user.avatar_data_url.is_none());
        assert!(view.bot.avatar_data_url.is_none());
    }

    #[test]
    fn profile_save_and_load_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        profile_set_name(handle.clone(), "user".into(), "Alice".into()).unwrap();
        profile_set_name(handle.clone(), "bot".into(), "助手小北".into()).unwrap();

        let view = profile_get(handle);
        assert_eq!(view.user.name, "Alice");
        assert_eq!(view.bot.name, "助手小北");
        assert!(view.user.avatar_data_url.is_none());
        assert!(view.bot.avatar_data_url.is_none());
    }

    #[test]
    fn profile_set_name_trims_whitespace() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        // set_name 自己 trim；前后空格应被剥除
        profile_set_name(handle.clone(), "user".into(), "  Alice  ".into()).unwrap();
        let view = profile_get(handle);
        assert_eq!(view.user.name, "Alice");
    }

    #[test]
    fn profile_set_name_rejects_empty_after_trim() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_name(handle, "user".into(), "     ".into()) {
            Err(CommandError::InvalidArgument { .. }) => {} // expected
            _ => panic!("expected InvalidArgument for empty/whitespace name"),
        }
    }

    #[test]
    fn profile_set_name_rejects_too_long() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        // MAX_NAME_CHARS = 24（含汉字：按 chars 计）
        let long = "啊".repeat(MAX_NAME_CHARS + 1);
        let err = profile_set_name(handle, "user".into(), long);
        assert!(err.is_err());
    }

    #[test]
    fn profile_set_name_rejects_invalid_kind() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_name(handle, "admin".into(), "Bob".into()) {
            Err(CommandError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "kind");
            }
            _ => panic!("expected InvalidArgument for invalid kind"),
        }
    }

    #[test]
    fn profile_set_avatar_and_remove_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        // 上传 PNG 头像
        let tmp = make_src_avatar("png");
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            tmp.path().join("src.png").to_string_lossy().into_owned(),
        )
        .unwrap();
        let avatar_path = data_dir.join("profile").join("avatar-user.png");
        assert!(
            avatar_path.exists(),
            "上传后头像文件应存在：{avatar_path:?}"
        );

        // 此时 view 应带 base64 data URL
        let view = profile_get(handle.clone());
        let url = view.user.avatar_data_url.as_ref().expect("应返回 data URL");
        assert!(url.starts_with("data:image/png;base64,"));
        // 验证 base64 内容可解码回原字节
        let b64_part = url.trim_start_matches("data:image/png;base64,");
        let decoded = B64.decode(b64_part).unwrap();
        assert_eq!(decoded, TINY_PNG);

        profile_remove_avatar(handle.clone(), "user".into()).unwrap();
        assert!(!avatar_path.exists(), "删除后头像文件应消失");
        let view = profile_get(handle);
        assert!(
            view.user.avatar_data_url.is_none(),
            "删除后 data URL 应为空"
        );
    }

    #[test]
    fn profile_remove_avatar_noop_when_never_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        // 没设过头像，remove 不应崩
        let view =
            profile_remove_avatar(handle, "bot".into()).expect("remove on empty should succeed");
        assert!(view.bot.avatar_data_url.is_none());
    }

    #[test]
    fn profile_remove_avatar_rejects_invalid_kind() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        let err = profile_remove_avatar(handle, "guest".into());
        assert!(err.is_err());
    }

    #[test]
    fn profile_set_avatar_rejects_bad_extension() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        // .txt 不在白名单
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("note.txt");
        std::fs::write(&src, b"hello").unwrap();

        match profile_set_avatar(handle, "user".into(), src.to_string_lossy().into_owned()) {
            Err(CommandError::InvalidArgument { reason, .. }) => {
                assert!(reason.contains("png") || reason.contains("jpg"));
            }
            _ => panic!("expected InvalidArgument for .txt extension"),
        }
    }

    #[test]
    fn profile_set_avatar_rejects_missing_source() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_avatar(handle, "user".into(), "/nonexistent/avatar.png".into()) {
            Err(CommandError::IoError(_)) => {} // expected: fs::metadata fails
            _ => panic!("expected IoError for missing source file"),
        }
    }

    #[test]
    fn profile_set_avatar_rejects_too_large() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        // 写 5MB+1 字节的 PNG（扩展名合法但超 MAX_AVATAR_BYTES）
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("huge.png");
        let big = vec![0u8; (MAX_AVATAR_BYTES as usize) + 1];
        std::fs::write(&src, &big).unwrap();

        match profile_set_avatar(handle, "user".into(), src.to_string_lossy().into_owned()) {
            Err(CommandError::InvalidArgument { reason, .. }) => {
                assert!(reason.contains("5MB") || reason.contains("5 MB"));
            }
            _ => panic!("expected InvalidArgument for oversized avatar"),
        }
    }

    #[test]
    fn profile_overwrite_replaces_old_avatar() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);
        let profile_d = data_dir.join("profile");

        // 先传 PNG
        let png_tmp = make_src_avatar("png");
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            png_tmp
                .path()
                .join("src.png")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();
        assert!(profile_d.join("avatar-user.png").exists());

        // 再传 JPG——旧 PNG 应被清理，新 JPG 出现
        let jpg_tmp = tempfile::TempDir::new().unwrap();
        let jpg_src = jpg_tmp.path().join("new.jpg");
        std::fs::write(&jpg_src, b"\xFF\xD8\xFF\xE0fake-jpg").unwrap();
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            jpg_src.to_string_lossy().into_owned(),
        )
        .unwrap();
        assert!(
            !profile_d.join("avatar-user.png").exists(),
            "旧 PNG 应被清掉"
        );
        assert!(profile_d.join("avatar-user.jpg").exists(), "新 JPG 应就位");

        // view 应反映 JPG（mime 是 jpeg）
        let view = profile_get(handle);
        let url = view.user.avatar_data_url.unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn profile_user_and_bot_avatars_are_isolated() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        // 同时给 user 和 bot 各自传一张——互不覆盖
        let user_tmp = make_src_avatar("png");
        let bot_tmp = make_src_avatar("gif");
        // GIF 头部最少 6 字节即可
        std::fs::write(
            bot_tmp.path().join("src.gif"),
            b"GIF89a\x01\x00\x01\x00\x00\x00\x00;",
        )
        .unwrap();

        profile_set_avatar(
            handle.clone(),
            "user".into(),
            user_tmp
                .path()
                .join("src.png")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();
        profile_set_avatar(
            handle.clone(),
            "bot".into(),
            bot_tmp
                .path()
                .join("src.gif")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();

        assert!(data_dir.join("profile").join("avatar-user.png").exists());
        assert!(data_dir.join("profile").join("avatar-bot.gif").exists());

        let view = profile_get(handle);
        assert!(view
            .user
            .avatar_data_url
            .as_ref()
            .unwrap()
            .starts_with("data:image/png"));
        assert!(view
            .bot
            .avatar_data_url
            .as_ref()
            .unwrap()
            .starts_with("data:image/gif"));
    }

    // ────── NEW-D-2：set_avatar save 失败回滚不得误删在役头像 ──────

    #[test]
    fn cleanup_failure_keeps_dest_when_it_is_live_avatar() {
        // 同扩展名覆盖：dest 文件名 == 在役头像名 → 不删（纯 helper，tempfile 隔离无需锁）
        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("avatar-1.png");
        std::fs::write(&dest, b"live").unwrap();
        assert!(!cleanup_set_avatar_failure(Some("avatar-1.png"), &dest));
        assert!(dest.exists(), "在役头像不得被回滚删除");
    }

    #[test]
    fn cleanup_failure_removes_dest_when_it_is_new_file() {
        // 不同文件名：dest 是新文件（孤儿）→ 删掉
        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("avatar-2.png");
        std::fs::write(&dest, b"orphan").unwrap();
        assert!(cleanup_set_avatar_failure(Some("avatar-1.png"), &dest));
        assert!(!dest.exists(), "孤儿新文件应被回滚删除");
        // 在役为 None（从未设过头像）→ 同样是孤儿，删
        let dest2 = tmp.path().join("avatar-3.png");
        std::fs::write(&dest2, b"orphan").unwrap();
        assert!(cleanup_set_avatar_failure(None, &dest2));
        assert!(!dest2.exists());
    }

    #[test]
    fn set_avatar_save_failure_same_ext_keeps_live_avatar() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        // 先成功设一张 PNG（在役 avatar-user.png）
        let tmp = make_src_avatar("png");
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            tmp.path().join("src.png").to_string_lossy().into_owned(),
        )
        .unwrap();
        let live = data_dir.join("profile").join("avatar-user.png");
        assert!(live.exists());

        // 失败注入：profile.json.tmp 占成目录 → save_data 必败、load_data 仍可读
        make_save_fail_via_tmp_dir(&data_dir);

        // 再传同扩展名 PNG → dest == 在役文件；save 失败回滚不得删它
        let tmp2 = make_src_avatar("png");
        let r = profile_set_avatar(
            handle.clone(),
            "user".into(),
            tmp2.path().join("src.png").to_string_lossy().into_owned(),
        );
        restore_save(&data_dir);
        assert!(r.is_err(), "save_data 失败应返回 Err");
        assert!(live.exists(), "save 失败回滚误删了在役头像（NEW-D-2 回归）");
    }

    #[test]
    fn set_avatar_save_failure_new_ext_removes_orphan() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        let tmp = make_src_avatar("png");
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            tmp.path().join("src.png").to_string_lossy().into_owned(),
        )
        .unwrap();

        make_save_fail_via_tmp_dir(&data_dir);

        // 换 JPG → dest = avatar-user.jpg 是新文件；save 失败回滚应删孤儿
        let jpg_tmp = tempfile::TempDir::new().unwrap();
        let jpg_src = jpg_tmp.path().join("new.jpg");
        std::fs::write(&jpg_src, b"\xFF\xD8\xFF\xE0fake-jpg").unwrap();
        let r = profile_set_avatar(
            handle.clone(),
            "user".into(),
            jpg_src.to_string_lossy().into_owned(),
        );
        restore_save(&data_dir);
        assert!(r.is_err());
        assert!(
            !data_dir.join("profile").join("avatar-user.jpg").exists(),
            "孤儿新文件应被回滚删除"
        );
    }

    // ────── NEW-D-3：remove_avatar save 失败不得留下悬挂引用 ──────

    #[test]
    fn remove_avatar_save_failure_keeps_avatar_file() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        // 先成功设一张 PNG
        let tmp = make_src_avatar("png");
        profile_set_avatar(
            handle.clone(),
            "user".into(),
            tmp.path().join("src.png").to_string_lossy().into_owned(),
        )
        .unwrap();
        let avatar_path = data_dir.join("profile").join("avatar-user.png");
        assert!(avatar_path.exists());

        // 失败注入：profile.json.tmp 占成目录 → save_data 必败
        make_save_fail_via_tmp_dir(&data_dir);
        let r = profile_remove_avatar(handle.clone(), "user".into());
        restore_save(&data_dir);

        assert!(r.is_err(), "save_data 失败应返回 Err");
        assert!(
            avatar_path.exists(),
            "save 失败时 avatar 文件必须保留（先删后 save 会留悬挂引用，NEW-D-3 回归）"
        );
        // json 引用也保持原状（磁盘文件未被截断写）
        let view = profile_get(handle);
        assert!(
            view.user.avatar_data_url.is_some(),
            "save 失败时 json 引用应保持原状"
        );
    }

    // ────── NEW-D-4：avatar 字段纯文件名校验（防路径穿越） ──────

    #[test]
    fn avatar_filename_only_accepts_plain_filename() {
        assert_eq!(avatar_filename_only("avatar.png"), Some("avatar.png"));
        assert_eq!(
            avatar_filename_only("avatar-user.webp"),
            Some("avatar-user.webp")
        );
        assert_eq!(
            avatar_filename_only("带空格 头像.png"),
            Some("带空格 头像.png")
        );
    }

    #[test]
    fn avatar_filename_only_rejects_traversal_and_separators() {
        for bad in [
            "../../wmessage.db",
            "/etc/passwd",
            "..\\..\\bad",
            "",
            "a/b.png",
            "a\\b.png",
            "..",
            "foo..png",
            "./avatar.png",
        ] {
            assert_eq!(avatar_filename_only(bad), None, "应拒绝: {bad:?}");
        }
    }

    #[test]
    fn entry_view_malicious_avatar_returns_none() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        // 手改 profile.json：avatar 指向数据目录内的敏感文件（相对跳一级）
        let malicious = ProfileData {
            user: ProfileEntry {
                name: "x".into(),
                avatar: Some("../wmessage.db".into()),
            },
            ..Default::default()
        };
        std::fs::write(
            data_dir(&handle).join("profile.json"),
            serde_json::to_string(&malicious).unwrap(),
        )
        .unwrap();
        let view = profile_get(handle);
        assert!(
            view.user.avatar_data_url.is_none(),
            "路径穿越的 avatar 必须被拦截为 None（NEW-D-4 回归）"
        );
    }

    #[test]
    fn entry_view_oversized_avatar_returns_none_and_audits() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);
        // 审计日志写在 probe_log_dir（生产同目录）；profile 数据目录自 3.3 起按进程隔离，
        // 两者已不是同一目录 —— 这里显式取日志目录，并只校验「本次新增」的尾部，
        // 不删共享 bot.log（删它会误伤并行用例，也掩盖不了回归）。
        let log_path = crate::paths::probe_log_dir(&handle).join("bot.log");
        let log_len_before = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        // 绕过写入端 5MB 校验（模拟手改/旧版写入）：直接落 6MB 头像文件 + 手改 profile.json
        let profile_d = data_dir.join("profile");
        std::fs::create_dir_all(&profile_d).unwrap();
        std::fs::write(
            profile_d.join("avatar-user.png"),
            vec![0u8; 6 * 1024 * 1024],
        )
        .unwrap();
        let forged = ProfileData {
            user: ProfileEntry {
                name: "x".into(),
                avatar: Some("avatar-user.png".into()),
            },
            ..Default::default()
        };
        std::fs::write(
            data_dir.join("profile.json"),
            serde_json::to_string(&forged).unwrap(),
        )
        .unwrap();
        let view = profile_get(handle);
        assert!(
            view.user.avatar_data_url.is_none(),
            "超过 5MB 的头像读取端必须拦截为 None"
        );
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        let appended = if log.len() as u64 >= log_len_before {
            &log[log_len_before as usize..]
        } else {
            // 日志被别的用例轮转/截断过 → 退化为全量校验（不 panic）
            log.as_str()
        };
        assert!(
            appended.contains("profile_avatar_too_large"),
            "超限读取必须记 ERROR 审计（读取端拦截 oversized avatar），本次新增段: {appended}"
        );
    }

    // ────── D3：原子写 + 损坏备份 ──────

    #[test]
    fn save_data_atomic_leaves_complete_json_and_no_tmp() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        profile_set_name(handle.clone(), "user".into(), "Alice".into()).unwrap();

        // 写后 profile.json 内容完整可解析，且无 tmp 残件
        let p = data_dir(&handle).join("profile.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(parsed["user"]["name"], "Alice");
        assert!(
            !data_dir(&handle).join("profile.json.tmp").exists(),
            "原子写完成后不得残留 tmp 文件"
        );
    }

    #[test]
    fn load_data_corrupt_json_backs_up_and_returns_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();

        let p = data_dir(&handle).join("profile.json");
        std::fs::write(&p, "{ 这不是合法 json").unwrap();

        // 返回默认让程序能启动
        let view = profile_get(handle.clone());
        assert_eq!(view.user.name, "我");
        assert_eq!(view.bot.name, "机器人");

        // 损坏现场被备份为 profile.json.bak-<ts>，原路径已挪走
        assert!(!p.exists(), "损坏文件应被 rename 到 .bak");
        let baks: Vec<_> = std::fs::read_dir(data_dir(&handle))
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("profile.json.bak-")
            })
            .collect();
        assert_eq!(baks.len(), 1, "应有且仅有一个损坏备份");
        assert_eq!(
            std::fs::read_to_string(baks[0].path()).unwrap(),
            "{ 这不是合法 json",
            "备份内容必须是原始损坏现场"
        );
    }

    // ────── D4：并发 read-modify-write 不丢更新 ──────

    #[test]
    fn profile_concurrent_set_name_no_lost_update() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let app = fresh_app();
        let handle = app.handle().clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // 模拟主窗改 user、挂件改 bot：两条 R-M-W 都覆写整条 ProfileData，
        // 无锁时两者都基于空快照改写 → 后写者覆盖前写者，必丢一个字段
        let mut joins = Vec::new();
        for (kind, name) in [("user", "线程A"), ("bot", "线程B")] {
            let h = handle.clone();
            let d = done.clone();
            joins.push(std::thread::spawn(move || {
                profile_set_name(h, kind.into(), name.into()).unwrap();
                d.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }
        for j in joins {
            j.join().unwrap();
        }
        assert_eq!(done.load(std::sync::atomic::Ordering::SeqCst), 2);

        let view = profile_get(handle);
        assert_eq!(view.user.name, "线程A", "并发写丢了 user 更新（D4 回归）");
        assert_eq!(view.bot.name, "线程B", "并发写丢了 bot 更新（D4 回归）");
    }
}
