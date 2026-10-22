//! Windows UI 自动化后端：UIAutomation 读控件树 + SendInput 注入。
//!
//! 读用 UIAutomation（与 macOS 的 AX 同位）：能拿到控件类型、名称、是否可用，
//! 优先用 Invoke 模式触发控件（比坐标点击稳）；Invoke 不支持时才退回坐标点击。
//!
//! COM 纪律：UIAutomation 是 COM 接口，调用线程必须先 `CoInitializeEx`；
//! 每个阻塞线程各初始化各释放（CoInitializeEx/CoUninitialize 必须成对同线程）。
//!
//! 前提：UIAutomation 读别的进程不需要额外权限，但**注入输入**受 UIPI 保护
//! ——高权限进程（如以管理员运行的程序）不会被注入，这是 Windows 的硬限制。

use windows::core::GUID;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationCondition, IUIAutomationElement, IUIAutomationInvokePattern,
    TreeScope_Children, UIA_InvokePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};

use super::{UiNode, UiWindow};

/// UIAutomation 的 CLSID / IID（系统固定值）
const CLSID_UIAUTOMATION: GUID = GUID::from_u128(0xFF48DBA4_60EF_4201_AA87_54103EEF594E);

/// COM 生命周期守卫：同线程成对初始化 / 释放
struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self, String> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_ok() {
            Ok(ComGuard)
        } else {
            Err(format!("COM 初始化失败：{hr:?}"))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn automation() -> Result<IUIAutomation, String> {
    // CoCreateInstance（windows 0.61）：接口 IID 由目标类型推导，不用手传
    let obj: IUIAutomation =
        unsafe { CoCreateInstance(&CLSID_UIAUTOMATION, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("创建 UIAutomation 失败：{e}"))?;
    Ok(obj)
}

fn true_condition(auto: &IUIAutomation) -> Result<IUIAutomationCondition, String> {
    unsafe { auto.CreateTrueCondition() }.map_err(|e| format!("创建查询条件失败：{e}"))
}

/// 读 BSTR 文本（失败/空 → 空串）
fn bstr_text(b: windows::core::Result<windows::core::BSTR>) -> String {
    b.map(|s| s.to_string()).unwrap_or_default()
}

/// 列出可接管窗口（UIAutomation 根元素的直接子元素 = 桌面上的顶层窗口）
pub fn list_windows() -> Result<Vec<UiWindow>, String> {
    let _com = ComGuard::new()?;
    let auto = automation()?;
    let root = unsafe { auto.GetRootElement() }.map_err(|e| format!("取桌面根元素失败：{e}"))?;
    let cond = true_condition(&auto)?;
    let arr = unsafe { root.FindAll(TreeScope_Children, &cond) }
        .map_err(|e| format!("枚举顶层窗口失败：{e}"))?;
    let len = unsafe { arr.Length() }.unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..len {
        let Ok(el) = (unsafe { arr.GetElement(i) }) else {
            continue;
        };
        let title = bstr_text(unsafe { el.CurrentName() });
        if title.trim().is_empty() {
            continue; // 无标题的（面板/托盘）不进清单，避免模型误选
        }
        let pid = unsafe { el.CurrentProcessId() }.unwrap_or(0);
        let hwnd = unsafe { el.CurrentNativeWindowHandle() }.unwrap_or(HWND::default());
        out.push(UiWindow {
            id: format!("{pid}:{}", hwnd.0 as isize),
            app: pid_to_app_name(pid),
            pid: pid as u32,
            title,
            focused: is_foreground(hwnd),
        });
    }
    Ok(out)
}

/// 进程名（取可执行文件名；拿不到就退回 pid）
fn pid_to_app_name(pid: i32) -> String {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let Ok(h) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) })
    else {
        return format!("pid {pid}");
    };
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let got = unsafe {
        QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(h) };
    match got {
        Ok(()) => {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit(['\\', '/'])
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&full)
                .to_string()
        }
        Err(_) => format!("pid {pid}"),
    }
}

/// 窗口是否在前台
fn is_foreground(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let fg = unsafe { GetForegroundWindow() };
    fg == hwnd
}

/// 激活窗口（还原 + 置前）
pub fn activate_window(window_id: &str) -> Result<(), String> {
    let _com = ComGuard::new()?;
    let hwnd = parse_hwnd(window_id)?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    if unsafe { SetForegroundWindow(hwnd) }.as_bool() {
        Ok(())
    } else {
        // Windows 对前台切换有限制（SetForegroundWindow 可能被拒），
        // 如实报错让模型知道「窗口可能没被激活」，不假装成功
        Err("系统拒绝了置前请求（Windows 前台切换限制）".to_string())
    }
}

/// 从窗口 id 里取 HWND
fn parse_hwnd(window_id: &str) -> Result<HWND, String> {
    let (_pid, hwnd_str) = window_id
        .split_once(':')
        .ok_or("窗口 id 格式应为 <pid>:<hwnd>")?;
    let raw: isize = hwnd_str
        .parse()
        .map_err(|_| format!("窗口 id 的 hwnd 段无法解析：{window_id}"))?;
    Ok(HWND(raw as *mut std::ffi::c_void))
}

/// 节点 id 解析：`<pid>:<hwnd>` 或 `<pid>:<hwnd>.<子序号>…`
struct NodeRef {
    hwnd: HWND,
    path: Vec<usize>,
}

fn parse_node_id(id: &str) -> Result<NodeRef, String> {
    let (_pid, rest) = id
        .split_once(':')
        .ok_or("节点 id 格式应为 <pid>:<hwnd>[.<子路径>]")?;
    let mut parts = rest.split('.');
    let hwnd_raw: isize = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("节点 id 的 hwnd 段无法解析：{id}"))?;
    let mut path = Vec::new();
    for p in parts {
        let idx: usize = p
            .parse()
            .map_err(|_| format!("节点 id 的路径段无法解析：{id}"))?;
        path.push(idx);
    }
    Ok(NodeRef {
        hwnd: HWND(hwnd_raw as *mut std::ffi::c_void),
        path,
    })
}

/// 按 id 解析出 UIA 元素（重新走树定位，不缓存 COM 句柄）
fn resolve(node_id: &str) -> Result<IUIAutomationElement, String> {
    let _com = ComGuard::new()?;
    let r = parse_node_id(node_id)?;
    let auto = automation()?;
    let mut cur = unsafe { auto.ElementFromHandle(r.hwnd) }
        .map_err(|e| format!("找不到该窗口（窗口已关闭？）：{e}"))?;
    let cond = true_condition(&auto)?;
    for idx in r.path {
        let arr = unsafe { cur.FindAll(TreeScope_Children, &cond) }
            .map_err(|e| format!("读取子控件失败：{e}"))?;
        let len = unsafe { arr.Length() }.unwrap_or(0);
        if idx as i32 >= len {
            return Err(format!("控件路径在第 {idx} 段不存在（界面已变？）"));
        }
        cur = unsafe { arr.GetElement(idx as i32) }
            .map_err(|e| format!("读取第 {idx} 个子控件失败：{e}"))?;
    }
    Ok(cur)
}

/// 读窗口的控件树
pub fn inspect_tree(
    window_id: &str,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Vec<UiNode>, String> {
    let _com = ComGuard::new()?;
    let r = parse_node_id(window_id)?;
    let auto = automation()?;
    let root = unsafe { auto.ElementFromHandle(r.hwnd) }
        .map_err(|e| format!("找不到该窗口（窗口已关闭？）：{e}"))?;
    let cond = true_condition(&auto)?;
    let mut out = Vec::new();
    collect(
        &auto,
        &root,
        window_id.to_string(),
        0,
        max_depth,
        max_nodes,
        &cond,
        &mut out,
    );
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn collect(
    auto: &IUIAutomation,
    el: &IUIAutomationElement,
    prefix: String,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    cond: &IUIAutomationCondition,
    out: &mut Vec<UiNode>,
) {
    if out.len() >= max_nodes {
        return;
    }
    let Ok(arr) = (unsafe { el.FindAll(TreeScope_Children, cond) }) else {
        return;
    };
    let len = unsafe { arr.Length() }.unwrap_or(0);
    for i in 0..len {
        if out.len() >= max_nodes {
            return;
        }
        let Ok(child) = (unsafe { arr.GetElement(i) }) else {
            continue;
        };
        let id = format!("{prefix}.{i}");
        let rect = unsafe { child.CurrentBoundingRectangle() }.ok();
        out.push(UiNode {
            id: id.clone(),
            role: bstr_text(unsafe { child.CurrentLocalizedControlType() }),
            title: bstr_text(unsafe { child.CurrentName() }),
            description: bstr_text(unsafe { child.CurrentHelpText() }),
            value: None,
            identifier: Some(bstr_text(unsafe { child.CurrentAutomationId() }))
                .filter(|s| !s.is_empty()),
            enabled: unsafe { child.CurrentIsEnabled() }
                .ok()
                .map(|b| b.as_bool()),
            position: rect.map(|r| (r.left as f64, r.top as f64)),
            size: rect.map(|r| {
                (
                    r.right as f64 - r.left as f64,
                    r.bottom as f64 - r.top as f64,
                )
            }),
            depth,
            actionable: supports_invoke(&child),
        });
        if depth < max_depth {
            collect(auto, &child, id, depth + 1, max_depth, max_nodes, cond, out);
        }
    }
}

/// 元素是否支持 Invoke 模式（能直接触发）
fn supports_invoke(el: &IUIAutomationElement) -> bool {
    unsafe { el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) }.is_ok()
}

/// 按控件本身触发（Invoke 模式；不支持则退回坐标点击）
pub fn press_node(node_id: &str) -> Result<(), String> {
    let _com = ComGuard::new()?;
    let el = resolve(node_id)?;
    let pattern =
        unsafe { el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) };
    if let Ok(p) = pattern {
        unsafe { p.Invoke() }.map_err(|e| format!("触发控件失败：{e}"))?;
        return Ok(());
    }
    // 没有 Invoke 模式（输入框之类）→ 坐标点击兜底
    let rect: RECT = unsafe { el.CurrentBoundingRectangle() }
        .map_err(|e| format!("控件不支持触发且读不到位置：{e}"))?;
    click_at(
        (rect.left + rect.right) as f64 / 2.0,
        (rect.top + rect.bottom) as f64 / 2.0,
    )
}

/// 让控件获得焦点
pub fn focus_node(node_id: &str) -> Result<(), String> {
    let _com = ComGuard::new()?;
    let el = resolve(node_id)?;
    unsafe { el.SetFocus() }.map_err(|e| format!("聚焦控件失败：{e}"))
}

/// 坐标点击（0-65535 归一化的绝对坐标）
fn click_at(x: f64, y: f64) -> Result<(), String> {
    let (sx, sy) = (x as i32, y as i32);
    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: sx,
                    dy: sy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: sx,
                    dy: sy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_LEFTUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(format!(
            "注入点击失败（SendInput 只发出 {sent}/{} 条——多半是被 UIPI 拦了：目标程序权限更高）",
            inputs.len()
        ));
    }
    Ok(())
}

/// 往当前焦点控件输入文本（SendInput + KEYEVENTF_UNICODE，不改键盘布局）
pub fn type_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut inputs: Vec<INPUT> = Vec::with_capacity(units.len() * 2);
    for ch in &units {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: *ch,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(format!(
            "注入文本失败（SendInput 只发出 {sent}/{} 条——多半是被 UIPI 拦了：目标程序权限更高）",
            inputs.len()
        ));
    }
    Ok(())
}
