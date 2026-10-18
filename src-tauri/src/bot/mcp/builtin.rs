//! 内置 MCP 服务器（当前只有一台：Playwright 浏览器操控）。
//!
//! 职责两件：
//! ① 定位自包含的 `playwright-mcp/` 包目录（npm 包 + Chromium 内核都在里面）；
//! ② 启动时把它写进 `bot-config.json` 的 `mcpServers`——**缺失才写**，用户
//! 改过的条目（名字/参数/开关）原样保留。
//!
//! 包目录形态（仓库根，随程序分发）：
//!
//! ```text
//! playwright-mcp/
//!   node_modules/@playwright/mcp/cli.js   ← 命令行入口（node 直接跑）
//!   browsers/chromium-<rev>/…              ← `npx playwright install chromium` 落到这
//! ```
//!
//! 用 `node <cli.js>` 而不是 `npx`：npx 认的是 cwd 下的 node_modules，
//! 打包后 cwd 不是仓库根，必然回落去联网重装——内置服务器要的是「拷进来就能跑」。
//!
//! 内置条目默认 **停用**：开一次就会拉起浏览器子进程（首次还要建 profile），
//! 由用户在设置页看过命令行再开，与面板里手加服务器同一条确认路径。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::bot::BotConfig;

use super::config::{upsert_in_config, McpServerConfig};

/// 内置条目的稳定 id：写进配置后即用户数据的定位键，**不可改**——改了等于
/// 换了一台服务器（用户改过的参数会被丢弃重置）。
pub const PLAYWRIGHT_ID: &str = "builtin-playwright";
/// 显示名用 ASCII：工具名前缀 `mcp_{前缀}_{工具}` 由它派生，中文会被逐字
/// 替换成 `_`（Playwright 浏览器 → `mcp_playwright____browser_navigate`），
/// 25 个工具全带这串噪声。想显示中文，在设置页改这张卡的名字即可。
const PLAYWRIGHT_NAME: &str = "Playwright";

/// 自包含包目录名（仓库根 / exe 同目录 / Tauri 资源目录三处同名）
const DIR_NAME: &str = "playwright-mcp";
/// 显式指定包目录的环境变量（便携模式把目录换位置时用；显式指定不回退）
const DIR_ENV: &str = "WMESSAGE_PLAYWRIGHT_MCP_DIR";
/// npm 包入口（相对包目录）；`npm install --prefix playwright-mcp @playwright/mcp` 的产物
const CLI_REL: &str = "node_modules/@playwright/mcp/cli.js";
/// 内核目录（相对包目录）。两个根分开放：**`playwright install` 会清空目标
/// 修订目录再解包**，两个平台共用一个根时后装的会把先装的抹掉；分开后各装各的、
/// 并行下载也不抢锁。mac 侧可只下第二个根，就把 Windows 构建备齐。
const BROWSERS_REL: &str = "browsers";
const BROWSERS_WIN_REL: &str = "browsers-win";

/// id 是否为内置条目（删除守卫 + 前端隐藏删除按钮的唯一判据）
pub fn is_builtin_id(id: &str) -> bool {
    id.trim() == PLAYWRIGHT_ID
}

// 目录解析（纯函数，便于单测）

/// 包目录解析：显式环境变量 → exe 同目录 → Tauri 资源目录 → 仓库根。
/// 命中条件只是「候选目录存在」，**不要求 cli.js 在位**：内置卡片必须先出现，
/// 用户把包拷进来才连得上；反过来（拷了包但没卡片）用户无从得知这条服务器存在。
/// 显式指定环境变量时不回退——路径写错应当显式失败，而不是静默换目录。
fn pick_package_dir(
    env_dir: Option<&str>,
    exe_dir: Option<PathBuf>,
    resource_dir: Option<PathBuf>,
    dev_dir: PathBuf,
) -> PathBuf {
    if let Some(d) = env_dir.map(str::trim).filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(p) = exe_dir.map(|d| d.join(DIR_NAME)).filter(|p| p.is_dir()) {
        return p;
    }
    if let Some(p) = resource_dir
        .map(|d| d.join(DIR_NAME))
        .filter(|p| p.is_dir())
    {
        return p;
    }
    if dev_dir.is_dir() {
        return dev_dir;
    }
    dev_dir
}

fn resolve_package_dir(app: &tauri::AppHandle) -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let resource_dir = app.path().resource_dir().ok().map(|p| p.to_path_buf());
    let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join(DIR_NAME))
        .unwrap_or_default();
    pick_package_dir(
        std::env::var(DIR_ENV).ok().as_deref(),
        exe_dir,
        resource_dir,
        dev_dir,
    )
}

/// 包是否已就位（cli.js 在位）：卡片文案与审计用它区分「命令指了个空目录」
/// 与「服务器连不上」。
pub fn package_present(pkg_dir: &Path) -> bool {
    pkg_dir.join(CLI_REL).is_file()
}

/// Chromium 内核二进制（`<包目录>/browsers/chromium-<rev>/…`）：取修订号最大的
/// 一个。内核目录名带 Playwright 锁定的修订号，写死路径必然随包升级失效，
/// 扫目录取最新才是跟着包走的那条路。
fn find_chromium(pkg_dir: &Path) -> Option<PathBuf> {
    [BROWSERS_REL, BROWSERS_WIN_REL]
        .iter()
        .find_map(|root| newest_chromium_in(&pkg_dir.join(root)))
}

/// 单个内核根目录里取修订号最大的一个内核二进制。
fn newest_chromium_in(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // 非 chromium-* 目录（chromium_headless_shell-* / ffmpeg-*）一律不认
        let Some(rev) = name
            .strip_prefix("chromium-")
            .and_then(|r| r.parse::<u64>().ok())
        else {
            continue;
        };
        // 目录项读不出（权限/竞态）跳过，不让整次解析失败
        let Some(bin) = platform_binary(&entry.path()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(r, _)| rev > *r) {
            best = Some((rev, bin));
        }
    }
    best.map(|(_, bin)| bin)
}

/// 各平台内核二进制（相对修订目录）的候选路径。
///
/// 新版 Playwright 装的是 **Chrome for Testing**（mac `chrome-mac-arm64` /
/// `chrome-mac-x64`、win `chrome-win64`），旧版装 **Chromium**（`chrome-mac` /
/// `chrome-win`）：两套都得认，否则包一升级探测就落空，卡片连不上浏览器。
/// 同一修订目录下各平台构建是并列子目录，同一份包可以同时装 mac 与 win 构建。
#[cfg(target_os = "macos")]
const BIN_CANDIDATES: &[&[&str]] = &[
    &[
        "chrome-mac-arm64",
        "Google Chrome for Testing.app",
        "Contents",
        "MacOS",
        "Google Chrome for Testing",
    ],
    &[
        "chrome-mac-x64",
        "Google Chrome for Testing.app",
        "Contents",
        "MacOS",
        "Google Chrome for Testing",
    ],
    &[
        "chrome-mac",
        "Chromium.app",
        "Contents",
        "MacOS",
        "Chromium",
    ],
];

#[cfg(target_os = "windows")]
const BIN_CANDIDATES: &[&[&str]] = &[
    &["chrome-win64", "chrome.exe"],
    &["chrome-win", "chrome.exe"],
];

#[cfg(all(unix, not(target_os = "macos")))]
const BIN_CANDIDATES: &[&[&str]] = &[
    &["chrome-linux64", "chrome"],
    &["chrome-linux-arm64", "chrome"],
    &["chrome-linux", "chrome"],
];

#[cfg(not(any(unix, windows)))]
const BIN_CANDIDATES: &[&[&str]] = &[];

fn platform_binary(rev_dir: &Path) -> Option<PathBuf> {
    BIN_CANDIDATES
        .iter()
        .map(|parts| rev_dir.join(parts.iter().collect::<PathBuf>()))
        .find(|p| p.is_file())
}

// 内置条目构造

/// Playwright 条目：`node <包目录>/cli.js` + 两条固定参数（+ 内核在位时的一条）。
///
/// - `--isolated`：profile 只留在内存。持久 profile 同一时刻只能被一个实例用，
///   重连或第二个客户端接入就会撞锁，表现为工具调用随机超时。
/// - `--headless`：机器人后台驱动，不抢用户桌面（要看窗口就在设置页删掉该参数）。
/// - `--executable-path`：内核随包分发，指过去免得运行时再去下载。
///   内核缺失时不传——让 Playwright 自己报「请先 install」比传个空路径清楚。
///
/// 不传 `--caps`：该参数是**追加**额外能力（vision/pdf/devtools），
/// 导航/快照/点击这些核心工具默认就在，传 `--caps=core` 会被判为非法值。
pub fn build_playwright_server(pkg_dir: &Path) -> McpServerConfig {
    let mut args = vec![
        pkg_dir.join(CLI_REL).to_string_lossy().into_owned(),
        "--isolated".to_string(),
        "--headless".to_string(),
    ];
    if let Some(bin) = find_chromium(pkg_dir) {
        args.push("--executable-path".to_string());
        args.push(bin.to_string_lossy().into_owned());
    }
    McpServerConfig {
        id: PLAYWRIGHT_ID.to_string(),
        name: PLAYWRIGHT_NAME.to_string(),
        transport: "stdio".to_string(),
        command: Some("node".to_string()),
        args,
        env: BTreeMap::new(),
        url: None,
        headers: BTreeMap::new(),
        timeout_secs: None,
        enabled: false,
    }
}

/// 配置写入前的纯逻辑闸（可单测）：缺失才补写，返回是否动过配置。
fn ensure_builtin_servers(cfg: &mut BotConfig, pkg_dir: &Path) -> bool {
    let has = cfg
        .mcp_servers
        .as_ref()
        .is_some_and(|l| l.iter().any(|s| is_builtin_id(&s.id)));
    if has {
        return false;
    }
    upsert_in_config(cfg, build_playwright_server(pkg_dir)).is_ok()
}

/// 启动时把内置条目补进配置。
///
/// 只在缺失时写：用户改名/改参数/关开关都原样保留，只有整条不见了才补。
/// （删除路径已按不可删拦截，正常操作下这等价于「首启 + 老用户升级各写一次」。）
///
/// 失败只审计不阻断启动——内置服务器是增强项，写不进去只让它缺席。
///
/// 必须在启动的配置迁移**之后**调用：明文 env/headers 未迁进凭据存储就写回，
/// 会被 `skip_serializing` 剥掉等于静默丢配置。
pub fn materialize_at_startup(app: &tauri::AppHandle) {
    let pkg_dir = resolve_package_dir(app);
    let name = PLAYWRIGHT_NAME;
    let _g = crate::bot::config::io::lock_config_write();
    let mut cfg = crate::bot::config::io::load_config(app);
    let written = ensure_builtin_servers(&mut cfg, &pkg_dir);
    if !written {
        return;
    }
    let outcome =
        crate::bot::config::io::write_bot_config_file_locked(&crate::db::data_dir(app), cfg);
    crate::bot::audit_log(
        app,
        &match &outcome {
            Ok(()) => format!(
                "mcp.builtin_materialized | id: {PLAYWRIGHT_ID} | name: {name} | 包: {}（{}）",
                pkg_dir.display(),
                if package_present(&pkg_dir) {
                    "就位"
                } else {
                    "缺失，开关打开会连不上"
                }
            ),
            Err(e) => format!(
                "mcp.builtin_materialize_failed | id: {PLAYWRIGHT_ID} | name: {name} | err: {e}"
            ),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wmessage-builtin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录应可建");
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn empty_config() -> BotConfig {
        BotConfig::default()
    }

    #[test]
    fn is_builtin_id_only_matches_playwright() {
        assert!(is_builtin_id(PLAYWRIGHT_ID));
        assert!(is_builtin_id("  builtin-playwright  "));
        assert!(!is_builtin_id("srv-1"));
        assert!(!is_builtin_id("builtin-playwright-2"));
        assert!(!is_builtin_id(""));
    }

    #[test]
    fn pick_package_dir_prefers_env_then_exe_then_resource_then_dev() {
        let dev = tmpdir("pick");
        let exe = tmpdir("pick-exe");
        let res = tmpdir("pick-res");
        for d in [&exe, &res, &dev] {
            std::fs::create_dir_all(d.join(DIR_NAME)).unwrap();
        }
        // exe 同目录在位 → 选它（便携模式布局）
        assert_eq!(
            pick_package_dir(
                None,
                Some(exe.clone()),
                Some(res.clone()),
                dev.join(DIR_NAME)
            ),
            exe.join(DIR_NAME)
        );
        // exe 没有、资源目录有 → 选资源目录（打包 .app 布局）
        let no_exe_dir = tmpdir("pick-empty");
        assert_eq!(
            pick_package_dir(
                None,
                Some(no_exe_dir.clone()),
                Some(res.clone()),
                dev.join(DIR_NAME)
            ),
            res.join(DIR_NAME)
        );
        // 只剩仓库根有 → 选它（tauri dev 布局）
        let empty_res = tmpdir("pick-empty2");
        assert_eq!(
            pick_package_dir(
                None,
                Some(no_exe_dir.clone()),
                Some(empty_res),
                dev.join(DIR_NAME)
            ),
            dev.join(DIR_NAME)
        );
        // 显式环境变量优先，且不回退（目录不存在也照它走）
        let explicit = tmpdir("pick-env");
        let dev2 = dev.clone();
        assert_eq!(
            pick_package_dir(explicit.to_str(), Some(exe.clone()), Some(res), dev2),
            explicit
        );
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&exe);
        let _ = std::fs::remove_dir_all(&no_exe_dir);
    }

    #[test]
    fn pick_package_dir_returns_dev_path_even_when_absent() {
        let dev = tmpdir("pick-missing");
        let got = pick_package_dir(None, None, None, dev.clone());
        assert_eq!(got, dev, "包没拷进来也要给出路径，卡片才指得对地方");
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn find_chromium_takes_highest_revision() {
        let pkg = tmpdir("chromium");
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-1181")
                .join(platform_rel()),
        );
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-1200")
                .join(platform_rel()),
        );
        // 认不出来的目录名一律跳过，不让它们挡住内核解析
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium_headless_shell-1201")
                .join(platform_rel()),
        );
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-x")
                .join(platform_rel()),
        );
        let got = find_chromium(&pkg).expect("应找到内核");
        assert!(
            got.to_string_lossy().contains("chromium-1200"),
            "应取修订号最大的，实际 {}",
            got.display()
        );
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn find_chromium_falls_back_to_windows_root() {
        // 备用根：只装了 Windows 构建的机器（mac 上备 win 构建）也要能找到内核
        let pkg = tmpdir("chromium-winroot");
        let planted = pkg
            .join(BROWSERS_WIN_REL)
            .join("chromium-1247")
            .join(platform_rel());
        touch(&planted);
        assert_eq!(find_chromium(&pkg), Some(planted), "备用根里的内核也应被认");
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn find_chromium_prefers_primary_root() {
        // 两个根都有内核时取本机根（browsers/ 在前）
        let pkg = tmpdir("chromium-two-roots");
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-1200")
                .join(platform_rel()),
        );
        touch(
            &pkg.join(BROWSERS_WIN_REL)
                .join("chromium-1247")
                .join(platform_rel()),
        );
        let got = find_chromium(&pkg).expect("应定位到内核");
        assert!(
            got.to_string_lossy().contains("chromium-1200"),
            "本机根优先，实际 {}",
            got.display()
        );
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn find_chromium_absent_dir_is_none() {
        let pkg = tmpdir("chromium-none");
        assert!(find_chromium(&pkg).is_none());
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn find_chromium_accepts_legacy_layout() {
        // 旧版包（Chromium 而非 Chrome for Testing）也要认：候选表末位就是旧布局
        let legacy: PathBuf = BIN_CANDIDATES
            .last()
            .map(|parts| parts.iter().collect::<PathBuf>())
            .unwrap_or_default();
        assert!(!legacy.as_os_str().is_empty(), "候选表应含旧布局兜底");
        let pkg = tmpdir("chromium-legacy");
        touch(&pkg.join(BROWSERS_REL).join("chromium-900").join(&legacy));
        let got = find_chromium(&pkg).expect("旧布局也应能定位内核");
        assert!(got.to_string_lossy().contains("chromium-900"));
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn find_chromium_new_layout_wins_over_legacy() {
        // 同一修订目录里新旧布局并存时，取新布局（列表在前的先命中）
        let legacy: PathBuf = BIN_CANDIDATES
            .last()
            .map(|parts| parts.iter().collect::<PathBuf>())
            .unwrap_or_default();
        let pkg = tmpdir("chromium-both");
        touch(&pkg.join(BROWSERS_REL).join("chromium-1247").join(&legacy));
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-1247")
                .join(platform_rel()),
        );
        let got = find_chromium(&pkg).expect("应定位到内核");
        assert_eq!(
            got,
            pkg.join(BROWSERS_REL)
                .join("chromium-1247")
                .join(platform_rel())
        );
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn build_playwright_server_shape() {
        let pkg = tmpdir("build");
        touch(&pkg.join(CLI_REL));
        let s = build_playwright_server(&pkg);
        assert_eq!(s.id, PLAYWRIGHT_ID);
        assert_eq!(s.command.as_deref(), Some("node"));
        // 名字进工具名前缀：必须是 ASCII，否则每个工具名都带一串 `_` 噪声
        let prefix = super::super::config::tool_prefix(&s.name);
        assert!(
            prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "工具名前缀应全 ASCII，实际 {prefix}"
        );
        assert!(!s.enabled, "内置条目默认停用：开一次就会拉起浏览器子进程");
        assert_eq!(s.args[0], pkg.join(CLI_REL).to_string_lossy());
        assert!(s.args.contains(&"--isolated".to_string()));
        assert!(s.args.contains(&"--headless".to_string()));
        assert!(
            !s.args.contains(&"--caps=core".to_string()),
            "核心工具默认开启，--caps 只收 vision/pdf/devtools"
        );
        // 内核不在位时不传 --executable-path（传空路径比让 Playwright 自己报错更糊）
        assert!(!s.args.iter().any(|a| a == "--executable-path"));
        touch(
            &pkg.join(BROWSERS_REL)
                .join("chromium-1200")
                .join(platform_rel()),
        );
        let with_bin = build_playwright_server(&pkg);
        let i = with_bin
            .args
            .iter()
            .position(|a| a == "--executable-path")
            .expect("内核在位时应带上可执行文件路径");
        assert!(with_bin.args[i + 1].contains("chromium-1200"));
        // 校验能过（白名单 node + 长度限制），否则播种进去的条目是坏的
        assert!(super::super::config::validate_server(&with_bin).is_ok());
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn ensure_builtin_servers_seeds_once_and_keeps_user_edits() {
        let pkg = tmpdir("ensure");
        touch(&pkg.join(CLI_REL));
        let mut cfg = empty_config();
        assert!(ensure_builtin_servers(&mut cfg, &pkg), "首次应写入");
        let list = cfg.mcp_servers.clone().unwrap_or_default();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, PLAYWRIGHT_ID);
        assert!(!list[0].enabled);

        // 用户改名 + 打开开关 → 不动
        {
            let servers = cfg.mcp_servers.as_mut().unwrap();
            servers[0].name = "我的浏览器".into();
            servers[0].enabled = true;
        }
        assert!(
            !ensure_builtin_servers(&mut cfg, &pkg),
            "已存在时不应重复写入"
        );
        let list = cfg.mcp_servers.clone().unwrap_or_default();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "我的浏览器");
        assert!(list[0].enabled, "用户的编辑与开关状态必须保留");

        // 与用户自建服务器共存
        cfg.mcp_servers.as_mut().unwrap().push(McpServerConfig {
            id: "srv-1".into(),
            name: "filesystem".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            ..Default::default()
        });
        assert!(!ensure_builtin_servers(&mut cfg, &pkg));
        assert_eq!(cfg.mcp_servers.unwrap_or_default().len(), 2);
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn ensure_builtin_servers_respects_name_collision() {
        // 用户已有同名条目：补写会撞「名称唯一」闸，此时不该把配置搞坏
        let pkg = tmpdir("ensure-collide");
        touch(&pkg.join(CLI_REL));
        let mut cfg = empty_config();
        cfg.mcp_servers = Some(vec![McpServerConfig {
            id: "srv-1".into(),
            name: PLAYWRIGHT_NAME.into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            ..Default::default()
        }]);
        assert!(!ensure_builtin_servers(&mut cfg, &pkg));
        let list = cfg.mcp_servers.clone().unwrap_or_default();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "srv-1", "撞名时保留用户原有条目，不覆盖");
        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn ensure_builtin_servers_writes_into_none_field() {
        let pkg = tmpdir("ensure-none");
        touch(&pkg.join(CLI_REL));
        let mut cfg = empty_config();
        cfg.mcp_servers = None;
        assert!(ensure_builtin_servers(&mut cfg, &pkg));
        assert_eq!(cfg.mcp_servers.unwrap_or_default().len(), 1);
        let _ = std::fs::remove_dir_all(&pkg);
    }

    /// 本机平台的内核二进制相对路径（测试造目录用；与 BIN_CANDIDATES[0] 同源）
    fn platform_rel() -> PathBuf {
        BIN_CANDIDATES
            .first()
            .map(|parts| parts.iter().collect::<PathBuf>())
            .unwrap_or_default()
    }
}
