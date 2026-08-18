//! 个人资料：用户 / 机器人 头像 + 姓名。
//! 存储：数据目录 profile.json；头像文件拷贝到 profile/ 子目录（便携模式随 exe 走）。
//! 读取返回 base64 data URL，前端无需 asset protocol / CSP 配置。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::error::{CommandError, CommandResult};

const MAX_AVATAR_BYTES: u64 = 5 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 24;
const AVATAR_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

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

fn profile_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("profile.json")
}

fn profile_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("profile")
}

fn load_data(app: &AppHandle) -> ProfileData {
    std::fs::read_to_string(profile_path(app))
        .ok()
        .and_then(|s| serde_json::from_str::<ProfileData>(&s).ok())
        .unwrap_or_default()
}

fn save_data(app: &AppHandle, data: &ProfileData) -> Result<(), String> {
    let s = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    std::fs::write(profile_path(app), s).map_err(|e| e.to_string())
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

fn entry_view(app: &AppHandle, kind: &str, e: &ProfileEntry) -> ProfileEntryView {
    let name = if e.name.trim().is_empty() {
        default_name(kind)
    } else {
        e.name.clone()
    };
    let data_url = e.avatar.as_ref().and_then(|f| {
        let path = profile_dir(app).join(f);
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

fn build_view(app: &AppHandle, data: &ProfileData) -> ProfileView {
    ProfileView {
        user: entry_view(app, "user", &data.user),
        bot: entry_view(app, "bot", &data.bot),
    }
}

fn valid_kind(kind: &str) -> bool {
    kind == "user" || kind == "bot"
}

/// 资料变更后广播给所有窗口（主窗口 + 挂件）
fn broadcast(app: &AppHandle) {
    let data = load_data(app);
    let _ = app.emit("profile-changed", build_view(app, &data));
}

#[tauri::command]
pub fn profile_get(app: AppHandle) -> ProfileView {
    let data = load_data(&app);
    build_view(&app, &data)
}

#[tauri::command]
pub fn profile_set_name(app: AppHandle, kind: String, name: String) -> CommandResult<ProfileView> {
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
    let mut data = load_data(&app);
    let entry = if kind == "bot" {
        &mut data.bot
    } else {
        &mut data.user
    };
    entry.name = name.to_string();
    save_data(&app, &data)?;
    broadcast(&app);
    Ok(build_view(&app, &data))
}

#[tauri::command]
pub fn profile_set_avatar(
    app: AppHandle,
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
    entry.avatar = Some(fname.to_string_lossy().into_owned());
    // 原子性：保存失败时删掉刚拷贝的头像文件，不留孤儿（二次审计 P3）
    if let Err(e) = save_data(&app, &data) {
        let _ = std::fs::remove_file(&dest);
        return Err(CommandError::IoError(e));
    }
    broadcast(&app);
    Ok(build_view(&app, &data))
}

#[tauri::command]
pub fn profile_remove_avatar(app: AppHandle, kind: String) -> CommandResult<ProfileView> {
    if !valid_kind(&kind) {
        return Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: kind,
            reason: "必须为 user 或 bot".into(),
        });
    }
    let mut data = load_data(&app);
    let entry = if kind == "bot" {
        &mut data.bot
    } else {
        &mut data.user
    };
    if let Some(f) = entry.avatar.take() {
        let _ = std::fs::remove_file(profile_dir(&app).join(&f));
    }
    save_data(&app, &data)?;
    broadcast(&app);
    Ok(build_view(&app, &data))
}
