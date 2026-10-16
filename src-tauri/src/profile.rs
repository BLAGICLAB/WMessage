//! 个人资料：用户 / 机器人 头像 + 姓名。
//! 存储：数据目录 profile.json。头像为**预设 ID 制**：应用内置 16 张头像
//! （打包在前端 assets），profile / 导出信封 / people 表全链路只传 ID，
//! 对端安装按 ID 解析回同一张内置图——导出数据因此能携带头像身份。
//! 读取返回 avatarId，前端本地解析资源，不再 base64 data URL 走 IPC。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

use crate::error::{CommandError, CommandResult};

const MAX_NAME_CHARS: usize = 24;

/// 内置预设头像 ID 全集（与前端 src/avatars.ts 注册表一一对应，双端测试各锁 16）。
/// 顺序即设置页选择器网格顺序（行优先）。
pub const AVATAR_IDS: [&str; 16] = [
    "avatar-01",
    "avatar-02",
    "avatar-03",
    "avatar-04", //
    "avatar-05",
    "avatar-06",
    "avatar-07",
    "avatar-08", //
    "avatar-09",
    "avatar-10",
    "avatar-11",
    "avatar-12", //
    "avatar-13",
    "avatar-14",
    "avatar-15",
    "avatar-16",
];

/// 非法 ID（旧版拷贝文件名、手改垃圾值、穿越串）一律视为无头像：
/// 不上 view、不落 people 表
pub fn is_valid_avatar_id(id: &str) -> bool {
    AVATAR_IDS.contains(&id)
}

/// profile read-modify-write 进程内串行锁（同 db::DB_WRITE_LOCK 模式）。
/// 并发改 profile 时，整条 ProfileData 覆写会让后写者覆盖前写者 → 丢更新；
/// 所有写路径（set_name / set_avatar / remove_avatar）的 R-M-W 全程持锁。
/// load_data / profile_get 读路径频繁，不持锁避免阻塞读。
static PROFILE_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntry {
    #[serde(default)]
    pub name: String,
    /// 预设头像 ID（AVATAR_IDS 之一）；None 用默认占位。
    /// 旧版 profile.json 的 avatar 字段（拷贝文件名）随 ID 制弃用：未知键被
    /// serde 忽略 → None → 回默认占位（旧拷贝文件留在数据目录，无引用无害）
    #[serde(default)]
    pub avatar_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProfileData {
    #[serde(default)]
    pub user: ProfileEntry,
    #[serde(default)]
    pub bot: ProfileEntry,
    /// 本人 personId（任务图谱设计 §1.2）：首次生成后固化。
    /// 旧 profile.json 无此字段 → default None → ensure_person_id 补生成
    #[serde(default)]
    pub person_id: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntryView {
    pub name: String,
    pub avatar_id: Option<String>,
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

/// 数据目录： 起委托 paths::probe_log_dir（原与 db::db_dir 两处拷贝，已合一）。
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

fn load_data<R: Runtime>(app: &AppHandle<R>) -> ProfileData {
    let path = profile_path(app);
    // 读失败（含首启文件不存在）→ 默认配置，不算损坏，不触发备份
    let Ok(s) = std::fs::read_to_string(&path) else {
        return ProfileData::default();
    };
    match serde_json::from_str::<ProfileData>(&s) {
        Ok(d) => d,
        Err(e) => {
            // 损坏不再静默吞——备份现场 profile.json.bak-<ts> + ERROR 审计，
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
    // 原子写（tmp + rename，复用 Phase 3  的 crate::db::atomic_write）——
    // 崩溃在写中途只留 tmp 残件，profile.json 要么旧完整版要么新完整版，不留半截
    crate::db::atomic_write(&profile_path(app), &s)
}

/// 头像 ID 出 json 前再过一遍白名单：手改/损坏的 profile.json 塞垃圾值
/// 当作无头像，不外泄到 view / 导出信封
fn sanitized_avatar_id(e: &ProfileEntry) -> Option<String> {
    e.avatar_id
        .as_deref()
        .filter(|id| is_valid_avatar_id(id))
        .map(str::to_string)
}

fn entry_view(kind: &str, e: &ProfileEntry) -> ProfileEntryView {
    let name = if e.name.trim().is_empty() {
        default_name(kind)
    } else {
        e.name.clone()
    };
    ProfileEntryView {
        name,
        avatar_id: sanitized_avatar_id(e),
    }
}

fn build_view(data: &ProfileData) -> ProfileView {
    ProfileView {
        user: entry_view("user", &data.user),
        bot: entry_view("bot", &data.bot),
    }
}

fn valid_kind(kind: &str) -> bool {
    kind == "user" || kind == "bot"
}

/// 资料变更后广播给前端（bot-config-changed 同款页面间同步）
fn broadcast<R: Runtime>(app: &AppHandle<R>) {
    let data = load_data(app);
    let _ = app.emit("profile-changed", build_view(&data));
}

/// 本人显示名（空名时用默认「我」；people_list 合并本人行用）
pub fn self_display_name<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let data = load_data(app);
    let name = data.user.name.trim();
    Some(if name.is_empty() {
        default_name("user")
    } else {
        name.to_string()
    })
}

/// 本人 personId：缺失则生成 UUID v4 固化到 profile.json，并在 people 表
/// 落 is_self=1 行（任务图谱设计 §1.2）。导出/导入/people_list 共用入口，
/// 副作用只发生一次（生成后幂等返回）。返回 (personId, 显示名)。
pub fn ensure_person_id<R: Runtime>(app: &AppHandle<R>) -> (String, String) {
    let name = self_display_name(app).unwrap_or_else(|| default_name("user"));
    let data = load_data(app);
    if let Some(pid) = data.person_id.filter(|s| !s.trim().is_empty()) {
        return (pid, name);
    }
    let pid = uuid::Uuid::new_v4().to_string();
    // 锁内重取 canonical pid：并发/多窗口下后来者发现已有 person_id 时必须沿用
    // 已有值——若仍用本地 pid 落 people 行并返回，会产生双 is_self 行 + 孤儿行
    let pid = {
        let _g =
            crate::db::lock::lock_or_recover(&PROFILE_WRITE_LOCK, "profile::PROFILE_WRITE_LOCK");
        let mut data = load_data(app);
        match data
            .person_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            // 双检：已有非空值即 canonical，不再覆盖
            Some(canonical) => canonical.to_string(),
            None => {
                data.person_id = Some(pid.clone());
                if let Err(e) = save_data(app, &data) {
                    eprintln!("[profile] personId 持久化失败（本次内存生效）：{e}");
                }
                pid
            }
        }
    };
    // people 表本人行（失败不致命：people_list 对 is_self 行会用 profile 名兜底；
    // 但缺失会让本人行整个消失，尽力补建）。
    // 注意不加 DB_WRITE_LOCK——调用方（导入/导出）可能已持锁，普通 Mutex 不可重入；
    // 单条 UPSERT 自身原子，无需多语句事务保护。
    // avatar 传 None：本人头像是 profile 的单一事实源，不复制进 people 表
    if let Ok(conn) = crate::db::open_db(app) {
        if let Err(e) = crate::db::people::people_upsert_entry(&conn, &pid, &name, true, None) {
            eprintln!("[profile] people 本人行写入失败：{e}");
        }
    }
    (pid, name)
}

/// 导出信封的本人资料卡：avatarId 随卡携带——预设 ID 是稳定小字符串，
/// 对端安装解析回同一张内置图（v1「dataUrl 太肥」的顾虑随 ID 制消失）
pub fn export_profile_card<R: Runtime>(app: &AppHandle<R>) -> (String, String, Option<String>) {
    let (pid, name) = ensure_person_id(app);
    let data = load_data(app);
    (pid, name, sanitized_avatar_id(&data.user))
}

#[tauri::command]
pub fn profile_get<R: Runtime>(app: AppHandle<R>) -> ProfileView {
    let data = load_data(&app);
    build_view(&data)
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
    // R-M-W（load → 改 → save）全程持锁，广播与 view 构造放锁外
    let data = {
        let _g =
            crate::db::lock::lock_or_recover(&PROFILE_WRITE_LOCK, "profile::PROFILE_WRITE_LOCK");
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
    Ok(build_view(&data))
}

#[tauri::command]
pub fn profile_set_avatar<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
    avatar_id: String,
) -> CommandResult<ProfileView> {
    if !valid_kind(&kind) {
        return Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: kind,
            reason: "必须为 user 或 bot".into(),
        });
    }
    if !is_valid_avatar_id(&avatar_id) {
        return Err(CommandError::InvalidArgument {
            field: "avatarId".into(),
            value: avatar_id,
            reason: "必须是内置头像 ID（avatar-01 ~ avatar-16）".into(),
        });
    }
    let data = {
        let _g =
            crate::db::lock::lock_or_recover(&PROFILE_WRITE_LOCK, "profile::PROFILE_WRITE_LOCK");
        let mut data = load_data(&app);
        let entry = if kind == "bot" {
            &mut data.bot
        } else {
            &mut data.user
        };
        entry.avatar_id = Some(avatar_id);
        save_data(&app, &data)?;
        data
    };
    broadcast(&app);
    Ok(build_view(&data))
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
    let data = {
        let _g =
            crate::db::lock::lock_or_recover(&PROFILE_WRITE_LOCK, "profile::PROFILE_WRITE_LOCK");
        let mut data = load_data(&app);
        let entry = if kind == "bot" {
            &mut data.bot
        } else {
            &mut data.user
        };
        entry.avatar_id = None;
        save_data(&app, &data)?;
        data
    };
    broadcast(&app);
    Ok(build_view(&data))
}

// 单元测试
//
// 测试策略：profile_get / profile_set_name / profile_set_avatar / profile_remove_avatar
// 都依赖 profile.rs::data_dir(app)。
// - 进程内：所有访问该目录的测试必须串行（ENV_LOCK 互斥），防并行写同一份 profile.json；
// - 进程间：data_dir 在 cfg(test) 下按 pid 分子目录（见 test_isolated_dir），
//   这样 `cargo nextest run`（每测试一进程）不会让并行进程共用同一份 profile.json。
// 默认 default_name / valid_kind / is_valid_avatar_id 等纯函数不需要锁，可自由并发。

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 串行化访问共享 data_dir 的测试（profile.json）。
    /// 任一时刻只允许一个 profile 测试运行，防止并行写同一份 profile.json。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 构造一个全新 mock app + 清掉残留 profile.json。
    /// 调用前必须先拿 ENV_LOCK。
    fn fresh_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let data_dir = data_dir(app.handle());
        let _ = std::fs::remove_file(data_dir.join("profile.json"));
        // 失败注入会把 profile.json.tmp 占成目录，残留要清掉
        let _ = std::fs::remove_dir_all(data_dir.join("profile.json.tmp"));
        // 损坏 load 会生成 profile.json.bak-<ts> 备份，残留要清掉
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
        app
    }

    ///  失败注入：把 profile.json.tmp 占成目录 → atomic_write 写临时文件必败
    /// → save_data 必败；profile.json 本体仍可读——
    /// 模拟「磁盘 json 可读、保存失败」的真实故障窗口。
    /// （ 前注入用「profile.json 只读」；atomic_write 走 tmp+rename，
    /// rename 覆盖不看目标文件权限，旧注入已无法让 save 失败。）
    fn make_save_fail_via_tmp_dir(data_dir: &std::path::Path) {
        std::fs::create_dir_all(data_dir.join("profile.json.tmp")).unwrap();
    }

    fn restore_save(data_dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(data_dir.join("profile.json.tmp"));
    }

    // 纯函数测试（无需锁）

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
    fn avatar_ids_exactly_16_unique_and_wellformed() {
        assert_eq!(
            AVATAR_IDS.len(),
            16,
            "内置头像必须是 16 张（与前端注册表对齐）"
        );
        let mut uniq = AVATAR_IDS.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 16, "头像 ID 不得重复");
        for id in AVATAR_IDS {
            let num = id.strip_prefix("avatar-").expect("ID 形如 avatar-NN");
            assert_eq!(num.len(), 2, "两位编号：{id}");
            assert!(num.bytes().all(|b| b.is_ascii_digit()), "编号纯数字：{id}");
        }
        assert!(is_valid_avatar_id("avatar-01"));
        assert!(!is_valid_avatar_id("avatar-17"));
        assert!(!is_valid_avatar_id(""));
        // 旧版拷贝文件名不再是合法 ID
        assert!(!is_valid_avatar_id("avatar-user.png"));
        // 穿越串不可能是合法 ID
        assert!(!is_valid_avatar_id("../../wmessage.db"));
    }

    #[test]
    fn sanitized_avatar_id_drops_garbage() {
        let mut e = ProfileEntry::default();
        assert_eq!(sanitized_avatar_id(&e), None);
        e.avatar_id = Some("avatar-07".into());
        assert_eq!(sanitized_avatar_id(&e), Some("avatar-07".into()));
        e.avatar_id = Some("garbage".into());
        assert_eq!(sanitized_avatar_id(&e), None, "手改垃圾值当无头像");
    }

    // 共享状态测试（需 ENV_LOCK）

    #[test]
    fn profile_missing_returns_default_view() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let view = profile_get(app.handle().clone());
        assert_eq!(view.user.name, "我");
        assert_eq!(view.bot.name, "机器人");
        assert!(view.user.avatar_id.is_none());
        assert!(view.bot.avatar_id.is_none());
    }

    #[test]
    fn profile_save_and_load_roundtrip() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        profile_set_name(handle.clone(), "user".into(), "Alice".into()).unwrap();
        profile_set_name(handle.clone(), "bot".into(), "助手小北".into()).unwrap();

        let view = profile_get(handle);
        assert_eq!(view.user.name, "Alice");
        assert_eq!(view.bot.name, "助手小北");
        assert!(view.user.avatar_id.is_none());
        assert!(view.bot.avatar_id.is_none());
    }

    #[test]
    fn profile_set_name_trims_whitespace() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        // set_name 自己 trim；前后空格应被剥除
        profile_set_name(handle.clone(), "user".into(), "  Alice  ".into()).unwrap();
        let view = profile_get(handle);
        assert_eq!(view.user.name, "Alice");
    }

    #[test]
    fn profile_set_name_rejects_empty_after_trim() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_name(handle, "user".into(), "     ".into()) {
            Err(CommandError::InvalidArgument { .. }) => {} // expected
            _ => panic!("expected InvalidArgument for empty/whitespace name"),
        }
    }

    #[test]
    fn profile_set_name_rejects_too_long() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        // MAX_NAME_CHARS = 24（含汉字：按 chars 计）
        let long = "啊".repeat(MAX_NAME_CHARS + 1);
        let err = profile_set_name(handle, "user".into(), long);
        assert!(err.is_err());
    }

    #[test]
    fn profile_set_name_rejects_invalid_kind() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_name(handle, "admin".into(), "Bob".into()) {
            Err(CommandError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "kind");
            }
            _ => panic!("expected InvalidArgument for invalid kind"),
        }
    }

    // 头像 ID 写路径

    #[test]
    fn profile_set_avatar_and_remove_roundtrip() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        profile_set_avatar(handle.clone(), "user".into(), "avatar-03".into()).unwrap();
        let view = profile_get(handle.clone());
        assert_eq!(view.user.avatar_id.as_deref(), Some("avatar-03"));

        // bot 独立，互不串档
        profile_set_avatar(handle.clone(), "bot".into(), "avatar-16".into()).unwrap();
        let view = profile_get(handle.clone());
        assert_eq!(view.user.avatar_id.as_deref(), Some("avatar-03"));
        assert_eq!(view.bot.avatar_id.as_deref(), Some("avatar-16"));

        // 覆盖换选
        profile_set_avatar(handle.clone(), "user".into(), "avatar-09".into()).unwrap();
        let view = profile_get(handle.clone());
        assert_eq!(view.user.avatar_id.as_deref(), Some("avatar-09"));

        profile_remove_avatar(handle.clone(), "user".into()).unwrap();
        let view = profile_get(handle);
        assert!(view.user.avatar_id.is_none(), "删除后回默认占位");
        assert_eq!(view.bot.avatar_id.as_deref(), Some("avatar-16"));
    }

    #[test]
    fn profile_set_avatar_rejects_unknown_id() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        for bad in ["avatar-17", "avatar-00", "avatar-user.png", "", "../../x"] {
            match profile_set_avatar(handle.clone(), "user".into(), bad.into()) {
                Err(CommandError::InvalidArgument { field, .. }) => assert_eq!(field, "avatarId"),
                _ => panic!("expected InvalidArgument for {bad:?}"),
            }
        }
        // 拒绝后不落盘
        let view = profile_get(handle);
        assert!(view.user.avatar_id.is_none());
    }

    #[test]
    fn profile_set_avatar_rejects_invalid_kind() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        match profile_set_avatar(handle, "guest".into(), "avatar-01".into()) {
            Err(CommandError::InvalidArgument { field, .. }) => assert_eq!(field, "kind"),
            _ => panic!("expected InvalidArgument for invalid kind"),
        }
    }

    #[test]
    fn profile_remove_avatar_noop_when_never_set() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        // 没设过头像，remove 不应崩
        let view =
            profile_remove_avatar(handle, "bot".into()).expect("remove on empty should succeed");
        assert!(view.bot.avatar_id.is_none());
    }

    #[test]
    fn profile_remove_avatar_rejects_invalid_kind() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();

        let err = profile_remove_avatar(handle, "guest".into());
        assert!(err.is_err());
    }

    // 手改 / 旧版 profile.json 的容错

    #[test]
    fn legacy_and_forged_avatar_values_fall_back_to_none() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();
        // 旧版拷贝文件名 / 手改穿越串：ID 制下都是未知键值 → view 视为无头像
        let forged = ProfileData {
            user: ProfileEntry {
                name: "x".into(),
                avatar_id: Some("../../wmessage.db".into()),
            },
            ..Default::default()
        };
        std::fs::write(
            data_dir(&handle).join("profile.json"),
            serde_json::to_string_pretty(&forged).unwrap(),
        )
        .unwrap();
        let view = profile_get(handle);
        assert!(
            view.user.avatar_id.is_none(),
            "非法 avatarId 必须被拦截为 None，不外泄到 view"
        );
        assert_eq!(view.user.name, "x", "姓名不受头像校验影响");
    }

    // set_avatar save 失败不落盘

    #[test]
    fn set_avatar_save_failure_keeps_previous_value() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();
        let data_dir = data_dir(&handle);

        profile_set_avatar(handle.clone(), "user".into(), "avatar-02".into()).unwrap();

        // 失败注入：profile.json.tmp 占成目录 → save_data 必败、load_data 仍可读
        make_save_fail_via_tmp_dir(&data_dir);
        let r = profile_set_avatar(handle.clone(), "user".into(), "avatar-05".into());
        restore_save(&data_dir);

        assert!(r.is_err(), "save_data 失败应返回 Err");
        let view = profile_get(handle);
        assert_eq!(
            view.user.avatar_id.as_deref(),
            Some("avatar-02"),
            "save 失败时在役头像保持原状"
        );
    }

    // 原子写 + 损坏备份

    #[test]
    fn save_data_atomic_leaves_complete_json_and_no_tmp() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
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
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
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

    // 并发 read-modify-write 不丢更新

    #[test]
    fn profile_concurrent_set_name_no_lost_update() {
        let _g = crate::db::lock::lock_or_recover(&ENV_LOCK, "profile::ENV_LOCK");
        let app = fresh_app();
        let handle = app.handle().clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // 模拟两路并发各改 user / bot：两条 R-M-W 都覆写整条 ProfileData，
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
