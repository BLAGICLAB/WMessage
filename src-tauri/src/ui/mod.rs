//! UI 自动化（跨平台）：窗口枚举 / 控件树读取 / 点击 / 输入。
//!
//! 分层：
//! - 本模块：跨平台数据模型 + 总开关（flag 文件）+ 平台分发；
//! - `ui/macos.rs`：AX（Accessibility）读 UI 树 + CGEvent 注入；
//! - `ui/windows.rs`：UIAutomation 读控件树 + SendInput 注入；
//! - `bot_ui.rs`：机器人工具层（六件套）+ 安全闸。
//!
//! **默认关闭**：设置页开关打开前，所有工具直接拒绝执行（flag 文件
//! `ui-enabled.flag` 存在 = 开启）。UI 自动化会代替用户操作别的应用——
//! 这是本仓里权限面最大的一档能力，必须由用户显式打开。
//!
//! **定位**（与 Playwright 的分工）：Playwright 在 DOM 层更强，本模块管
//! **页面之外**——原生应用（微信 / 飞书 / Excel）、系统弹窗（下载框 / 支付确认 /
//! 验证码）、以及接管已登录的窗口。为网站做 UI 自动化是降级（坐标点击不如
//! 元素定位），不做。

use std::path::PathBuf;

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// 可接管的窗口
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiWindow {
    /// 稳定 id：`<pid>:<窗口序号>`（窗口序号是该应用 AXWindows / 顶层控件数组下标）
    pub id: String,
    /// 应用名（微信 / 飞书 / Excel…）
    pub app: String,
    pub pid: u32,
    pub title: String,
    /// 该窗口当前是否在前台
    pub focused: bool,
}

/// 控件节点（窗口里的一层 UI 元素）
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNode {
    /// 稳定 id：`<pid>:<窗口序号>.<子序号>.<子序号>…`（树上路径）
    pub id: String,
    /// 角色（AXRole / UIA ControlType）：AXButton / AXTextField / AXStaticText…
    pub role: String,
    /// 标题（AXTitle / UIA Name）
    pub title: String,
    /// 描述（AXDescription）
    pub description: String,
    /// 当前值（输入框里的文字 / 选中项）
    pub value: Option<String>,
    /// 控件标识（AXIdentifier）
    pub identifier: Option<String>,
    /// 是否可用（AXEnabled / UIA IsEnabled）
    pub enabled: Option<bool>,
    /// 全局坐标（屏幕左上角为原点）
    pub position: Option<(f64, f64)>,
    /// 尺寸（宽 × 高）
    pub size: Option<(f64, f64)>,
    /// 深度（窗口直属控件 = 0）
    pub depth: usize,
    /// 是否支持「按控件本身触发」（AXPress / UIA Invoke）——比坐标点击稳，优先用
    pub actionable: bool,
    /// 是否是密码框（Windows UIA 的 IsPassword；macOS 用 AXSecureTextField 角色判定）
    pub is_password: bool,
}

// ── 总开关（flag 文件）──────────────────────────────────────────────────────

fn ui_flag_path(app: &AppHandle) -> PathBuf {
    crate::paths::flags_dir(app).join("ui-enabled.flag")
}

/// UI 自动化是否已开启（工具层统一走这里）
pub fn ui_enabled(app: &AppHandle) -> bool {
    ui_flag_path(app).exists()
}

#[tauri::command]
pub fn ui_get_enabled(app: AppHandle) -> bool {
    ui_enabled(&app)
}

#[tauri::command]
pub fn ui_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool> {
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    if enabled {
        std::fs::write(ui_flag_path(&app), b"1")
            .map_err(|e| CommandError::IoError(e.to_string()))?;
    } else {
        // 只吞 NotFound；权限/占用等失败必须上抛——否则盘上残留 flag，
        // ui_get_enabled 仍返回 true，与刚返回给调用方的「已禁用」自相矛盾
        match std::fs::remove_file(ui_flag_path(&app)) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CommandError::IoError(format!(
                    "删除 UI 自动化启用标志文件失败：{e}"
                )))
            }
        }
    }
    Ok(enabled)
}

// ── 平台分发 ───────────────────────────────────────────────────────────────

/// 枚举可接管窗口
pub fn list_windows() -> Result<Vec<UiWindow>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::list_windows()
    }
    #[cfg(windows)]
    {
        windows::list_windows()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 激活窗口（置前）
pub fn activate_window(window_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::activate_window(window_id)
    }
    #[cfg(windows)]
    {
        windows::activate_window(window_id)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = window_id;
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 读窗口的控件树
///
/// max_depth：最大深度（0 = 只看窗口直属控件）；max_nodes：最多返回多少节点。
pub fn inspect_tree(
    window_id: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<UiNode>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::inspect_tree(window_id, max_depth, max_nodes)
    }
    #[cfg(windows)]
    {
        windows::inspect_tree(window_id, max_depth, max_nodes)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (window_id, max_depth, max_nodes);
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 按控件本身触发（AXPress / UIA Invoke）
pub fn press_node(node_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::press_node(node_id)
    }
    #[cfg(windows)]
    {
        windows::press_node(node_id)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = node_id;
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 让控件获得焦点（输入前的准备）
pub fn focus_node(node_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::focus_node(node_id)
    }
    #[cfg(windows)]
    {
        windows::focus_node(node_id)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = node_id;
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 往当前焦点控件输入文本（CGEvent / SendInput）
pub fn type_text(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::type_text(text)
    }
    #[cfg(windows)]
    {
        windows::type_text(text)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = text;
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 屏幕像素坐标 → MOUSEEVENTF_ABSOLUTE 用的 0-65535 归一化值
///
/// SendInput 的绝对鼠标坐标不是像素，是「虚拟桌面宽高的 65536 分之一」。把像素
/// 直接塞进去，落点会缩到屏幕左上角附近（1920 宽屏上 x=500 落在约 15px 处），
/// 一次点击就打到完全无关的控件上。
///
/// 基准是整个虚拟桌面（配 MOUSEEVENTF_VIRTUALDESK 用），原点是虚拟桌面的左上角
/// ——多显示器时那个原点不是 (0, 0)。点落在虚拟桌面外返回 None（宁可不出手）。
#[cfg(any(windows, test))]
pub(crate) fn pixels_to_normalized_absolute(
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
) -> Option<(i32, i32)> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let nx = ((x - origin_x) / width * 65535.0).round();
    let ny = ((y - origin_y) / height * 65535.0).round();
    if !(0.0..=65535.0).contains(&nx) || !(0.0..=65535.0).contains(&ny) {
        return None;
    }
    Some((nx as i32, ny as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 接线守卫：六件套必须真的接进工具表（改了工具名而忘了登记，这里会红）
    #[test]
    fn ui_tools_are_wired_into_registry() {
        let src = include_str!("../bot/registry.rs");
        for name in [
            "list_windows",
            "activate_window",
            "ui_inspect",
            "ui_click",
            "ui_type",
            "wait_for_element",
        ] {
            assert!(
                src.contains(&format!("name: \"{name}\"")),
                "{name} 必须登记进 TOOLS_TABLE"
            );
        }
    }

    /// 接线守卫：开关命令必须注册进 invoke_handler
    #[test]
    fn ui_flag_commands_are_registered() {
        let src = include_str!("../lib.rs");
        assert!(
            src.contains("crate::ui::ui_get_enabled"),
            "ui_get_enabled 必须注册进 invoke_handler"
        );
        assert!(
            src.contains("crate::ui::ui_set_enabled"),
            "ui_set_enabled 必须注册进 invoke_handler"
        );
    }

    /// 像素 → 归一化：落点必须跟像素对得上，不能缩到屏幕左上角
    #[test]
    fn pixel_coordinates_must_not_be_used_as_normalized_values() {
        // 单屏 1920×1080：屏幕正中就该落在归一化坐标的中点附近
        let (nx, ny) = pixels_to_normalized_absolute(960.0, 540.0, 0.0, 0.0, 1920.0, 1080.0)
            .expect("屏幕内的点必须能换算");
        assert_eq!((nx, ny), (32768, 32768));
        // 左上角附近：几乎不动
        let (nx, ny) = pixels_to_normalized_absolute(10.0, 10.0, 0.0, 0.0, 1920.0, 1080.0)
            .expect("屏幕内的点必须能换算");
        assert!(nx < 400 && ny < 700, "左上角的点不该被放大到屏幕中央");
    }

    /// 虚拟桌面有偏移时（多显示器），原点在左下屏时也要算得对
    #[test]
    fn normalized_coordinates_follow_the_virtual_desktop_origin() {
        // 虚拟桌面从 x=-1920 起（左侧一块屏），宽 3840
        let (nx, ny) = pixels_to_normalized_absolute(0.0, 0.0, -1920.0, 0.0, 3840.0, 1080.0)
            .expect("虚拟桌面内的点必须能换算");
        assert_eq!((nx, ny), (32768, 0), "虚拟桌面左缘应落在归一化中点");
        // 桌面外的点不给出坐标（宁可不出手，也别点到别处）
        assert!(
            pixels_to_normalized_absolute(-3000.0, 0.0, -1920.0, 0.0, 3840.0, 1080.0).is_none(),
            "桌面外的点必须拒绝换算"
        );
        assert!(
            pixels_to_normalized_absolute(10.0, 10.0, 0.0, 0.0, 0.0, 1080.0).is_none(),
            "屏幕尺寸读不到时必须拒绝换算"
        );
    }
}
