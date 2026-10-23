//! macOS UI 自动化后端：AX（Accessibility）读 UI 树 + CGEvent 注入。
//!
//! 读用 AX 而不用坐标猜：AX 能拿到控件的角色、标题、值、是否可用，AXPress
//! 直接触发控件（比坐标点击稳，控件移动也不怕）。坐标点击只在控件不支持
//! AXPress 时兜底。
//!
//! 两个系统前提（都靠工具报错如实告知用户，代码层面无法绕过）：
//! - **辅助功能权限**：本进程须被授予 Accessibility 权限（`AXIsProcessTrusted`），
//!   否则 AX 调用全部失败——这是 macOS 的硬闸；
//! - **屏幕录制权限**：只在需要截图留证时才有（见 bot_ui 的动作留痕）。
//!
//! CF 类型处理说明：AX 的属性值是 CFType（可能�� CFString / CFArray /
//! AXValue / CFNumber），绑定层只给 CFString 等实现了 ConcreteType 的类型提供
//! downcast；数组按类型 ID 判定后强转（CFArray 的元素类型是 PhantomData，
//! 布局不变）。

use std::ptr::NonNull;

use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType};

use super::{NodeIdentity, UiNode, UiWindow};

// AX 属性名：AX 头文件里是 CFSTR 常量（kAXWindowsAttribute = CFSTR("AXWindows")），
// 绑定没导出这批常量，按字面量建等价 CFString。
const AX_WINDOWS: &str = "AXWindows";
const AX_CHILDREN: &str = "AXChildren";
const AX_TITLE: &str = "AXTitle";
const AX_ROLE: &str = "AXRole";
const AX_SUBROLE: &str = "AXSubrole";
const AX_DESCRIPTION: &str = "AXDescription";
const AX_VALUE: &str = "AXValue";
const AX_IDENTIFIER: &str = "AXIdentifier";
const AX_ENABLED: &str = "AXEnabled";
const AX_POSITION: &str = "AXPosition";
const AX_SIZE: &str = "AXSize";
const AX_ACTIONS: &str = "AXActions";
const AX_PRESS: &str = "AXPress";
const AX_FOCUSED_WINDOW: &str = "AXFocusedWindow";

fn cfstr(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

/// 本进程是否已获辅助功能权限
pub fn accessibility_trusted() -> bool {
    unsafe { objc2_application_services::AXIsProcessTrusted() }
}

/// 读一个 AX 属性值（+1 retained 由 CFRetained 接管）
///
/// # Safety
/// 内部全是 FFI 调用；调用方只拿到 CFRetained<CFType>，不会拿到裸指针。
unsafe fn copy_attr(el: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut value: *const CFType = std::ptr::null();
    let attr = cfstr(name);
    let err = el.copy_attribute_value(&attr, NonNull::new(&mut value)?);
    if err != AXError::Success || value.is_null() {
        return None;
    }
    Some(CFRetained::from_raw(NonNull::new(value as *mut CFType)?))
}

/// 读字符串属性
fn string_attr(el: &AXUIElement, name: &str) -> Option<String> {
    let ty = unsafe { copy_attr(el, name) }?;
    ty.downcast_ref::<CFString>().map(|s| s.to_string())
}

/// 读布尔属性（AXEnabled 之类返回 CFBoolean）
fn bool_attr(el: &AXUIElement, name: &str) -> Option<bool> {
    let ty = unsafe { copy_attr(el, name) }?;
    // CFBoolean 与 CFBoolean 的 type id 相同（kCFBooleanTrue/False 是同一类型）
    let b = ty.downcast_ref::<objc2_core_foundation::CFBoolean>()?;
    Some(b.as_bool())
}

/// 读子元素数组
fn children(el: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let Some(ty) = (unsafe { copy_attr(el, AX_CHILDREN) }) else {
        return Vec::new();
    };
    // CFArray 的 ConcreteType 只对默认元素类型实现，按类型 ID 判定后强转
    if ty.downcast_ref::<CFArray>().is_none() {
        return Vec::new();
    }
    let arr: &CFArray<AXUIElement> =
        unsafe { &*((&*ty as *const CFType) as *const CFArray<AXUIElement>) };
    arr.iter().collect()
}

/// 读窗口数组（AXWindows）
fn windows_of(app: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let Some(ty) = (unsafe { copy_attr(app, AX_WINDOWS) }) else {
        return Vec::new();
    };
    if ty.downcast_ref::<CFArray>().is_none() {
        return Vec::new();
    }
    let arr: &CFArray<AXUIElement> =
        unsafe { &*((&*ty as *const CFType) as *const CFArray<AXUIElement>) };
    arr.iter().collect()
}

/// 读 AXValue 的 CGPoint / CGSize
fn point_attr(el: &AXUIElement, name: &str) -> Option<(f64, f64)> {
    let ty = unsafe { copy_attr(el, name) }?;
    let v = ty.downcast_ref::<AXValue>()?;
    let mut pt = objc2_core_foundation::CGPoint { x: 0.0, y: 0.0 };
    let ok = unsafe {
        v.value(
            AXValueType::CGPoint,
            NonNull::new(&mut pt as *mut _).unwrap().cast(),
        )
    };
    if ok {
        Some((pt.x, pt.y))
    } else {
        None
    }
}

fn size_attr(el: &AXUIElement, name: &str) -> Option<(f64, f64)> {
    let ty = unsafe { copy_attr(el, name) }?;
    let v = ty.downcast_ref::<AXValue>()?;
    let mut sz = objc2_core_foundation::CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe {
        v.value(
            AXValueType::CGSize,
            NonNull::new(&mut sz as *mut _).unwrap().cast(),
        )
    };
    if ok {
        Some((sz.width, sz.height))
    } else {
        None
    }
}

/// 该元素是否支持 AXPress
fn can_press(el: &AXUIElement) -> bool {
    let Some(ty) = (unsafe { copy_attr(el, AX_ACTIONS) }) else {
        return false;
    };
    if ty.downcast_ref::<CFArray>().is_none() {
        return false;
    }
    let arr: &CFArray<CFString> =
        unsafe { &*((&*ty as *const CFType) as *const CFArray<CFString>) };
    arr.iter().any(|a| a.to_string() == AX_PRESS)
}

// ── 应用 / 窗口枚举 ────────────────────────────────────────────────────────

/// 运行中的 GUI 应用：pid → (名称, NSRunningApplication)
fn running_apps() -> Vec<(
    i32,
    String,
    objc2::rc::Retained<objc2_app_kit::NSRunningApplication>,
)> {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
    let ws = NSWorkspace::sharedWorkspace();
    let apps = ws.runningApplications();
    let mut out = Vec::new();
    for app in apps.iter() {
        // 只要有界面的普通应用（后台进程/辅助进程没有可接管窗口）
        if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
            continue;
        }
        let name = app
            .localizedName()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("pid {}", app.processIdentifier()));
        out.push((app.processIdentifier(), name, app.clone()));
    }
    out
}

/// 列出所有可接管窗口
pub fn list_windows() -> Result<Vec<UiWindow>, String> {
    if !accessibility_trusted() {
        return Err(
            "本进程尚未获得 macOS「辅助功能」权限，无法读取窗口内容。请到 系统设置 → 隐私与安全性 → 辅助功能 勾选本应用后重试。"
                .to_string(),
        );
    }
    let mut out = Vec::new();
    for (pid, name, _app) in running_apps() {
        let app_el = unsafe { AXUIElement::new_application(pid as libc::pid_t) };
        for (idx, win) in windows_of(&app_el).into_iter().enumerate() {
            let title = string_attr(&win, AX_TITLE).unwrap_or_default();
            // 无标题的窗口（面板、托盘窗）也列出来，但标注出来让模型自己挑
            let focused = is_foreground_window(pid, &title);
            out.push(UiWindow {
                id: format!("{pid}:{idx}"),
                app: name.clone(),
                pid: pid as u32,
                title,
                focused,
            });
        }
    }
    Ok(out)
}

/// 节点 id 解析：`<pid>:<窗口序号>` 或 `<pid>:<窗口序号>.<子序号>…`
struct NodeRef {
    pid: i32,
    win_idx: usize,
    path: Vec<usize>,
}

fn parse_node_id(id: &str) -> Result<NodeRef, String> {
    let (pid_str, rest) = id
        .split_once(':')
        .ok_or("节点 id 格式应为 <pid>:<窗口序号>[.<子路径>]")?;
    let pid: i32 = pid_str
        .parse()
        .map_err(|_| format!("节点 id 的 pid 段无法解析：{id}"))?;
    let mut parts = rest.split('.');
    let win_idx: usize = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("节点 id 的窗口序号无法解析：{id}"))?;
    let mut path = Vec::new();
    for p in parts {
        let idx: usize = p
            .parse()
            .map_err(|_| format!("节点 id 的路径段无法解析：{id}"))?;
        path.push(idx);
    }
    Ok(NodeRef { pid, win_idx, path })
}

/// 按 id 解析出 AX 元素（重新走树定位——不缓存平台句柄，跨调用不会失效）
fn resolve(node_id: &str) -> Result<CFRetained<AXUIElement>, String> {
    let r = parse_node_id(node_id)?;
    let app_el = unsafe { AXUIElement::new_application(r.pid as libc::pid_t) };
    let wins = windows_of(&app_el);
    let win = wins
        .get(r.win_idx)
        .ok_or_else(|| format!("窗口序号 {} 不存在（窗口已关闭？）", r.win_idx))?;
    let mut cur = win.clone();
    for idx in r.path {
        let kids = children(&cur);
        cur = kids
            .get(idx)
            .cloned()
            .ok_or_else(|| format!("控件路径在第 {idx} 段不存在（界面已变？）"))?;
    }
    Ok(cur)
}

/// 激活窗口（把应用置前）
pub fn activate_window(window_id: &str) -> Result<(), String> {
    use objc2_app_kit::{NSApplicationActivationOptions, NSWorkspace};
    let (pid, _win) = window_id
        .split_once(':')
        .ok_or("窗口 id 格式应为 <pid>:<窗口序号>")?;
    let pid: i32 = pid.parse().map_err(|_| "窗口 id 的 pid 段无法解析")?;
    let ws = NSWorkspace::sharedWorkspace();
    for (p, _name, app) in running_apps() {
        if p == pid {
            let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
            if ok {
                return Ok(());
            }
            return Err("系统拒绝了激活请求".to_string());
        }
    }
    let _ = ws;
    Err(format!("找不到 pid 为 {pid} 的应用（应用已退出？）"))
}

/// 目标窗口是否在前台
///
/// macOS 没有公开的「哪个窗口在前台」标志，只能两级判断：应用级（前台应用的
/// pid）+ 窗口级（该应用 AXFocusedWindow 的标题对不对得上）。
pub fn window_is_foreground(window_id: &str) -> Result<bool, String> {
    let (pid_str, _win) = window_id
        .split_once(':')
        .ok_or("窗口 id 格式应为 <pid>:<窗口序号>")?;
    let pid: i32 = pid_str.parse().map_err(|_| "窗口 id 的 pid 段无法解析")?;
    let target_title = resolve(window_id)
        .ok()
        .and_then(|el| string_attr(&el, AX_TITLE))
        .unwrap_or_default();
    Ok(is_foreground_window(pid, &target_title))
}

/// 该窗口是不是前台窗口
///
/// 与注入闸（`window_is_foreground`）同一口径：list_windows 里标的「（前台）」
/// 必须和闸门的结论一致，否则模型照着标注操作却被闸门拦下。
fn is_foreground_window(pid: i32, title: &str) -> bool {
    let frontmost = frontmost_app_pid();
    if frontmost <= 0 || frontmost != pid {
        return false;
    }
    foreground_matches(frontmost, pid, title, focused_window_title(pid).as_deref())
}

/// 前台判定的纯逻辑：前台应用的 pid 对上，且该应用聚焦窗口的标题对得上
///
/// frontmost_pid 拿不到时是 0（不等于任何真实 pid）——此时一律不算前台。
fn foreground_matches(
    frontmost_pid: i32,
    target_pid: i32,
    target_title: &str,
    focused_title: Option<&str>,
) -> bool {
    frontmost_pid > 0
        && frontmost_pid == target_pid
        && same_window_by_title(target_title, focused_title)
}

/// 前台应用的 pid（拿不到就是 0——不等于任何真实 pid，等于「不在前台」）
fn frontmost_app_pid() -> i32 {
    use objc2_app_kit::NSWorkspace;
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| a.processIdentifier())
        .unwrap_or(0)
}

/// 该应用当前聚焦窗口的标题（读不到就是 None）
fn focused_window_title(pid: i32) -> Option<String> {
    let app_el = unsafe { AXUIElement::new_application(pid as libc::pid_t) };
    let ty = unsafe { copy_attr(&app_el, AX_FOCUSED_WINDOW) }?;
    let win = ty.downcast_ref::<AXUIElement>()?;
    string_attr(win, AX_TITLE)
}

/// 同应用内的逐窗口判定：聚焦窗口的标题与目标窗口标题一致才算同一个窗口
///
/// 标题读不到时退化为「同应用即可」——macOS 没有更细的公开标志可比。
fn same_window_by_title(target_title: &str, focused_title: Option<&str>) -> bool {
    match focused_title {
        Some(f) if !f.is_empty() && !target_title.is_empty() => f == target_title,
        _ => true,
    }
}

// ── 控件树 ─────────────────────────────────────────────────────────────────

/// 读窗口的控件树
pub fn inspect_tree(
    window_id: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<UiNode>, String> {
    if !accessibility_trusted() {
        return Err(
            "本进程尚未获得 macOS「辅助功能」权限，无法读取窗口内容。请到 系统设置 → 隐私与安全性 → 辅助功能 勾选本应用后重试。"
                .to_string(),
        );
    }
    let win = resolve(window_id)?;
    let mut out = Vec::new();
    collect(
        &win,
        window_id.to_string(),
        0,
        max_depth,
        max_nodes,
        &mut out,
    );
    Ok(out)
}

fn collect(
    el: &AXUIElement,
    prefix: String,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    out: &mut Vec<UiNode>,
) {
    if out.len() >= max_nodes {
        return;
    }
    for (idx, child) in children(el).into_iter().enumerate() {
        if out.len() >= max_nodes {
            return;
        }
        let id = format!("{prefix}.{idx}");
        out.push(UiNode {
            id: id.clone(),
            role: string_attr(&child, AX_ROLE).unwrap_or_default(),
            title: string_attr(&child, AX_TITLE).unwrap_or_default(),
            description: string_attr(&child, AX_DESCRIPTION).unwrap_or_default(),
            value: string_attr(&child, AX_VALUE),
            identifier: string_attr(&child, AX_IDENTIFIER),
            enabled: bool_attr(&child, AX_ENABLED),
            position: point_attr(&child, AX_POSITION),
            size: size_attr(&child, AX_SIZE),
            depth,
            actionable: can_press(&child),
            // 原生密码框（NSSecureTextField）的 AXRole 只是 AXTextField，密码
            // 身份在 AXSubrole——只看角色判不出，敏感闸就拦不住代填
            is_password: string_attr(&child, AX_SUBROLE).as_deref() == Some("AXSecureTextField"),
        });
        if depth < max_depth {
            collect(&child, id, depth + 1, max_depth, max_nodes, out);
        }
    }
}

/// 核对控件身份：解析出的元素必须还是闸门审过的那个控件
///
/// 闸门审的是控件树快照里的节点，这里解析出的是「现在」的元素——界面重排后
/// 同一个路径可能已经指向别的控件。对不上就放弃这次操作（闸门与动作必须
/// 落在同一个控件上）。
fn verify_identity(node_id: &str, el: &AXUIElement, expect: &NodeIdentity) -> Result<(), String> {
    let actual = NodeIdentity {
        title: string_attr(el, AX_TITLE).unwrap_or_default(),
        role: string_attr(el, AX_ROLE).unwrap_or_default(),
        position: point_attr(el, AX_POSITION),
    };
    if expect.same_as(&actual) {
        return Ok(());
    }
    Err(format!(
        "界面已变：控件 {node_id} 现在是「{}」（{}），与刚才审过的那个（{}，{}）不是同一个控件——放弃这次操作",
        actual.title, actual.role, expect.title, expect.role
    ))
}

/// 触发控件（AXPress）
fn press(el: &AXUIElement) -> Result<(), String> {
    let action = cfstr(AX_PRESS);
    let err = unsafe { el.perform_action(&action) };
    if err == AXError::Success {
        Ok(())
    } else {
        Err(format!("控件不支持按压操作（AXError {:?}）", err))
    }
}

/// 按控件本身触发（AXPress）
pub fn press_node(node_id: &str, expect: &NodeIdentity) -> Result<(), String> {
    let el = resolve(node_id)?;
    verify_identity(node_id, &el, expect)?;
    press(&el)
}

/// 让控件获得焦点（输入前的准备）
///
/// AX 没有「设焦点」这个通用属性：常规做法是对文本控件 AXPress（多数应用会
/// 因此聚焦输入框），失败则如实报错。
pub fn focus_node(node_id: &str, expect: &NodeIdentity) -> Result<(), String> {
    let el = resolve(node_id)?;
    verify_identity(node_id, &el, expect)?;
    press(&el).map_err(|e| format!("无法聚焦该控件（{e}）"))
}

/// 往当前焦点控件输入文本（CGEvent 造 Unicode 键盘事件）
pub fn type_text(text: &str) -> Result<(), String> {
    use objc2_core_graphics::{CGEvent, CGEventSource, CGEventSourceStateID};
    if text.is_empty() {
        return Ok(());
    }
    let source =
        CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok_or("无法创建键盘事件源")?;
    let units: Vec<u16> = text.encode_utf16().collect();
    // 一次只发一个字符：CGEvent 的 Unicode 字符串长度字段是 16 位，
    // 长文本一次性塞进去会被系统截断，逐字发最稳（也更像真人输入）
    for (i, ch) in units.iter().enumerate() {
        let key = CGEvent::new_keyboard_event(Some(&source), 0, true).ok_or("无法创建键盘事件")?;
        unsafe {
            CGEvent::keyboard_set_unicode_string(Some(&key), 1, ch as *const u16);
            CGEvent::post(
                objc2_core_graphics::CGEventTapLocation::HIDEventTap,
                Some(&key),
            );
        }
        let up = CGEvent::new_keyboard_event(Some(&source), 0, false).ok_or("无法创建键盘事件")?;
        CGEvent::post(
            objc2_core_graphics::CGEventTapLocation::HIDEventTap,
            Some(&up),
        );
        let _ = i;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同应用内的逐窗口判定：聚焦窗口标题对不上就不是同一个窗口
    #[test]
    fn same_window_check_compares_titles_within_one_app() {
        assert!(
            same_window_by_title("微信", Some("微信")),
            "标题一致算同一个窗口"
        );
        assert!(
            !same_window_by_title("微信", Some("另一个窗口")),
            "同应用里的另一个窗口不算前台窗口"
        );
        assert!(
            same_window_by_title("微信", None),
            "读不到聚焦窗口时退化为「同应用即可」"
        );
        assert!(
            same_window_by_title("", Some("另一个窗口")),
            "目标窗口没标题时读不到可比的东西，退化为「同应用即可」"
        );
    }

    /// 前台判定：前台应用 pid 对上、聚焦窗口标题对得上才算前台窗口
    ///
    /// list_windows 的「（前台）」标注必须与注入闸同一口径——标成前台却被闸门
    /// 拦下，模型会照着标注反复重试。
    #[test]
    fn foreground_window_check_needs_the_same_app_and_title() {
        assert!(
            foreground_matches(501, 501, "微信", Some("微信")),
            "同应用同标题算前台窗口"
        );
        assert!(
            !foreground_matches(501, 502, "微信", Some("微信")),
            "不是前台应用就不算前台窗口"
        );
        assert!(
            !foreground_matches(0, 0, "微信", Some("微信")),
            "读不到前台应用（pid=0）一律不算前台"
        );
        assert!(
            !foreground_matches(501, 501, "微信", Some("另一个窗口")),
            "同应用的另一个窗口不算前台窗口"
        );
    }

    /// collect 必须读 AXSubrole 判密码框：原生 NSSecureTextField 的 AXRole 只是
    /// AXTextField（密码身份在 AXSubrole），角色判不出；is_password 再写死
    /// false，敏感闸对原生密码框就是放行。
    #[test]
    fn collect_must_flag_password_fields_from_the_subrole() {
        let src = include_str!("macos.rs");
        assert!(
            src.contains("AX_SUBROLE"),
            "必须读 AXSubrole——原生密码框的角色只是普通 AXTextField"
        );
        assert!(
            !src.contains(concat!("is_password", ": false")),
            "is_password 写死 false 等于敏感闸对 macOS 原生密码框放行"
        );
    }

    /// 真实 AX 冒烟（macOS）：读当前运行的窗口列表，验 AX 读树这条路真能通。
    /// 依赖本进程获「辅助功能」权限（AXIsProcessTrusted），未授权直接失败——
    /// 不授权时整条 UI 自动化链路都不可用，这是必须让人看见的事实。
    /// 手动跑：cargo test --lib ui::macos -- --ignored --nocapture
    #[test]
    #[ignore = "真实 AX 冒烟（依赖辅助功能授权，手动跑）"]
    fn macos_ax_list_windows_smoke() {
        assert!(
            accessibility_trusted(),
            "本进程未获 macOS「辅助功能」权限：先到 系统设置 → 隐私与安全性 → 辅助功能 授权"
        );
        let wins = list_windows().expect("列窗口应成功");
        for w in wins.iter().take(5) {
            println!("{} | {} | {}", w.id, w.app, w.title);
        }
        assert!(!wins.is_empty(), "至少应读到一些窗口");
    }
}
