//! UI 自动化（跨平台）：窗口枚举 / 控件树读取 / 点击 / 输入。
//!
//! 分层：
//! - 本模块：跨平台数据模型 + 总开关（flag 文件）+ 平台分发；
//! - `ui/macos.rs`：AX（Accessibility）读 UI 树 + CGEvent 注入；
//! - `ui/windows.rs`：UIAutomation 读控件树 + SendInput 注入；
//! - `bot_ui.rs`：机器人工具层（六件套）+ 安全闸。
//!
//! **默认关闭**：设置页开关打开前，所有工具直接拒绝执行（flag 文件
//! `ui-enabled.flag` 存在 = 开启；unix 下该文件只有当前用户可写）。UI 自动化
//! 会代替用户操作别的应用——这是本仓里权限面最大的一档能力，必须由用户显式
//! 打开，开关变更留审计。
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
    ///
    /// macOS 按「前台应用的 pid + 该应用聚焦窗口的标题」判定，Windows 按
    /// GetForegroundWindow 实测——两边口径一致，标注与注入闸的结论相符。
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

/// 控件身份快照：闸门审过的那个控件的样子
///
/// 闸门审的是快照里的节点，动作却按 id 重新解析了一遍树——界面重排后同一个
/// id 已经指向别的控件，于是闸门审的是旧控件、动的是新控件。动作前拿重新
/// 解析出的控件跟这份快照对一次，对不上就放弃这次操作。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeIdentity {
    /// 标题（AXTitle / UIA Name）
    pub title: String,
    /// 角色（AXRole / UIA ControlType）
    pub role: String,
    /// 屏幕坐标（控件左上角）
    pub position: Option<(f64, f64)>,
}

/// 位置比对的容差（像素）：同一控件两次读取允许有亚像素抖动
const POSITION_EPSILON: f64 = 1.0;

impl NodeIdentity {
    /// 从控件快照取身份
    pub fn of(node: &UiNode) -> Self {
        Self {
            title: node.title.clone(),
            role: node.role.clone(),
            position: node.position,
        }
    }

    /// 同一性判定：标题 / 角色 / 位置都对得上才算同一个控件
    ///
    /// 有一边读不到位置时没有可比信号，不据此拒动作（标题与角色仍要比）。
    pub fn same_as(&self, other: &NodeIdentity) -> bool {
        self.title == other.title
            && self.role == other.role
            && match (self.position, other.position) {
                (Some((ax, ay)), Some((bx, by))) => {
                    (ax - bx).abs() <= POSITION_EPSILON && (ay - by).abs() <= POSITION_EPSILON
                }
                _ => true,
            }
    }
}

// ── 总开关（flag 文件）──────────────────────────────────────────────────────

fn ui_flag_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::paths::flags_dir(app).join("ui-enabled.flag")
}

/// UI 自动化是否已开启（工具层统一走这里）
pub fn ui_enabled<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    ui_flag_path(app).exists()
}

/// 当前 OS 用户名（拿不到就是空串）
fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

/// 写 flag 文件（unix 下创建即 0600）
///
/// flag 文件存在 = UI 自动化开启。0644 时同机的其它进程照样能创建这个文件，
/// 把闸门顶开——0600 之后只有当前用户能写。创建时就带 0600（不是先建后 chmod
/// 的竞态）：盘上若已有宽权限的旧 flag，创建时的 mode 不生效，写完再补一刀。
///
/// Windows 没有 unix 权限位：这层收紧在 Windows 上不生效，flag 只受用户目录
/// 的 ACL 保护（同用户的其它进程仍能创建它）——那里真正的边界是窗口级授权与
/// 危险动作确认两道闸，不是这个文件。
fn write_flag_file(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(b"1")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, b"1")
    }
}

#[tauri::command]
pub fn ui_get_enabled<R: tauri::Runtime>(app: AppHandle<R>) -> bool {
    ui_enabled(&app)
}

#[tauri::command]
pub fn ui_set_enabled<R: tauri::Runtime>(app: AppHandle<R>, enabled: bool) -> CommandResult<bool> {
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let before = ui_enabled(&app);
    if enabled {
        write_flag_file(&ui_flag_path(&app)).map_err(|e| CommandError::IoError(e.to_string()))?;
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
    let after = ui_enabled(&app);
    // 开关变更是本仓权限面最大的一次开口，必须留痕：事后要查得到
    // 「UI 自动化是什么时候被谁打开的」
    if before != after {
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "ui_automation_toggle",
            &[("enabled", after.to_string()), ("user", os_user())],
        );
    }
    Ok(after)
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

/// 目标窗口当前是否在前台（键盘 / 坐标注入的前提）
///
/// 键盘事件路由给前台应用：目标窗口不在前台时，注入的字符会打进用户正在用的
/// 另一个应用（另一扇聊天窗、终端、地址栏）。所以注入前必须问一句。
pub fn window_is_foreground(window_id: &str) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        macos::window_is_foreground(window_id)
    }
    #[cfg(windows)]
    {
        windows::window_is_foreground(window_id)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = window_id;
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 按控件本身触发（AXPress / UIA Invoke）
///
/// expect 是闸门审过的那个控件的身份：解析出的控件对不上就放弃（界面重排后
/// 同一个 id 可能已经指向别的控件）。
pub fn press_node(node_id: &str, expect: &NodeIdentity) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::press_node(node_id, expect)
    }
    #[cfg(windows)]
    {
        windows::press_node(node_id, expect)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (node_id, expect);
        Err("当前平台不支持 UI 自动化".into())
    }
}

/// 让控件获得焦点（输入前的准备）
///
/// expect 同 [`press_node`]：动作前核对控件身份。
pub fn focus_node(node_id: &str, expect: &NodeIdentity) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::focus_node(node_id, expect)
    }
    #[cfg(windows)]
    {
        windows::focus_node(node_id, expect)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (node_id, expect);
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

    /// flag 路径在 mock app 的数据目录里（进程内共享）——动 flag 的测试串着跑
    static UI_FLAG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// 控件身份比对：界面重排后同一个 id 指向别的控件时必须判定为「不是同一个」
    ///
    /// 闸门审的是控件树快照里的节点，动作按 id 重新解析——两者对不上时，
    /// 动的就不是闸门审过的那个控件（标题 / 角色 / 位置任一不同即视为换了）。
    #[test]
    fn node_identity_detects_a_replaced_control() {
        let node = UiNode {
            id: "1:0.2".into(),
            role: "AXButton".into(),
            title: "发送".into(),
            description: String::new(),
            value: None,
            identifier: None,
            enabled: Some(true),
            position: Some((100.0, 200.0)),
            size: Some((30.0, 20.0)),
            depth: 0,
            actionable: true,
            is_password: false,
        };
        let identity = NodeIdentity::of(&node);
        assert!(
            identity.same_as(&identity.clone()),
            "同一个控件必须认得出来"
        );
        let renamed = NodeIdentity {
            title: "删除".into(),
            ..identity.clone()
        };
        assert!(
            !identity.same_as(&renamed),
            "标题变了就不是同一个控件（闸门审的是旧控件）"
        );
        let other_role = NodeIdentity {
            role: "AXTextField".into(),
            ..identity.clone()
        };
        assert!(!identity.same_as(&other_role), "角色变了就不是同一个控件");
        let moved = NodeIdentity {
            position: Some((100.0, 260.0)),
            ..identity.clone()
        };
        assert!(
            !identity.same_as(&moved),
            "位置变了就不是同一个控件（无标题的图标按钮靠位置认人）"
        );
        let jitter = NodeIdentity {
            position: Some((100.4, 200.4)),
            ..identity.clone()
        };
        assert!(identity.same_as(&jitter), "亚像素抖动不算换了控件");
    }

    /// 接线守卫：两个平台后端都必须在动作前核对控件身份
    ///
    /// Windows 后端在本机（macOS）编不出来，这道守卫保证它不会漏掉核对。
    #[test]
    fn platform_backends_must_verify_node_identity_before_acting() {
        for (name, src) in [
            ("macos.rs", include_str!("macos.rs")),
            ("windows.rs", include_str!("windows.rs")),
        ] {
            assert!(
                src.contains("fn verify_identity("),
                "{name} 必须实现控件身份核对"
            );
            assert!(
                src.contains("verify_identity(node_id, &el, expect)?"),
                "{name} 必须在动作前调用身份核对"
            );
        }
    }

    /// 开关变更必须留审计事件：事后要查得到「UI 自动化是什么时候被谁打开的」
    #[test]
    fn ui_toggle_change_must_leave_an_audit_event() {
        fn bot_log(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> String {
            let p = crate::db::data_dir(app).join("bot.log");
            std::fs::read_to_string(p).unwrap_or_default()
        }
        let _g =
            crate::db::lock::lock_or_recover(&UI_FLAG_TEST_LOCK, "ui::tests::UI_FLAG_TEST_LOCK");
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        // 先回到「关」：盘上可能残留着别的测试留下的 flag
        let _ = ui_set_enabled(handle.clone(), false);
        let before = bot_log(&handle);
        ui_set_enabled(handle.clone(), true).expect("开启 UI 自动化");
        let after = bot_log(&handle);
        assert!(
            after.contains("ui_automation_toggle"),
            "开关变更必须写审计事件（原来只写 / 删文件，事后无从查起）"
        );
        assert!(
            after.len() > before.len(),
            "审计事件必须真的落盘（只读文件不算留痕）"
        );
        // 复原：关掉并清掉 flag，别影响别的测试
        ui_set_enabled(handle.clone(), false).expect("关闭 UI 自动化");
    }

    /// flag 文件权限必须收紧：同机任意进程都能创建它把闸门顶开
    ///
    /// flag 文件存在 = UI 自动化开启。0644 时同机的其它进程照样能创建这个
    /// 文件，把闸门顶开——收紧到只有当前用户可读写。
    #[cfg(unix)]
    #[test]
    fn ui_flag_file_must_not_be_world_accessible() {
        use std::os::unix::fs::PermissionsExt;
        let _g =
            crate::db::lock::lock_or_recover(&UI_FLAG_TEST_LOCK, "ui::tests::UI_FLAG_TEST_LOCK");
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        ui_set_enabled(handle.clone(), true).expect("开启 UI 自动化");
        let path = ui_flag_path(&handle);
        let mode = std::fs::metadata(&path)
            .expect("读 flag 元数据")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "flag 文件不能给同机其它用户读写（现在 {mode:o}）"
        );
        assert_eq!(
            mode & 0o600,
            0o600,
            "当前用户必须仍能读写 flag 文件（现在 {mode:o}）"
        );
        // 复原：关掉并清掉 flag，别影响别的测试
        ui_set_enabled(handle.clone(), false).expect("关闭 UI 自动化");
    }

    /// 盘上已有的宽权限 flag 也要被收紧（创建时的 mode 只在新建时生效）
    #[cfg(unix)]
    #[test]
    fn pre_existing_wide_permission_flag_gets_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("wm-ui-flag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建测试目录");
        let path = dir.join("ui-enabled.flag");
        // 模拟旧版本留下的 0644 flag
        std::fs::write(&path, b"1").expect("写旧 flag");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("设旧权限");
        write_flag_file(&path).expect("重写 flag");
        let mode = std::fs::metadata(&path)
            .expect("读 flag 元数据")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "旧的宽权限 flag 也要被收紧（现在 {mode:o}）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 接线守卫：Windows 后端必须真读 UIA ValuePattern
    ///
    /// Windows 后端在本机（macOS）编不出来，这道守卫保证它不会把 value 又写成
    /// 恒 None（控件值恒空，「按输入框内容找控件」整体失效）。
    #[test]
    fn windows_backend_must_read_the_value_pattern() {
        let src = include_str!("windows.rs");
        assert!(
            src.contains("IUIAutomationValuePattern"),
            "Windows 侧必须读 UIA ValuePattern"
        );
        assert!(
            src.contains("CurrentValue"),
            "Windows 侧必须读 ValuePattern 的 CurrentValue"
        );
        assert!(src.contains("value_of(&child)"), "读到的值必须填进控件节点");
        assert!(
            src.contains("if is_password"),
            "密码框不能读值（明文会漏进工具输出）"
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
