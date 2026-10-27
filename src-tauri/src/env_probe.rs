//! 本机运行环境探测 —— 设置页「本机运行环境」卡的数据源。
//!
//! 覆盖三组，各答一个问题：
//! - **Python 包**：办公文档族（Word/Excel/PPT/PDF）+ 数据/图表/抓取/模板
//! - **Node 包**：全局 npm 里的办公工具（Word/PPT/Excel/PDF/飞书）
//! - **运行时与启动器**：MCP stdio 启动器白名单里的命令 + dotnet/git/libreoffice
//!
//! **为什么不用 `import` 判存在**：`import pandas` / `import matplotlib` /
//! `import cv2` 光导入就 0.5–1s，18 个包串起来会撞穿探针 3s 超时——被超时
//! 杀掉的探针输出为空，面板会把「装了」全报成「缺」。改用
//! `importlib.util.find_spec`（只找不执行，18 个包实测 23ms），顺带避开
//! matplotlib 导入时初始化绘图后端的副作用。
//!
//! **为什么 Node 全局包按目录判定**：`npm ls -g --depth=0` 慢且输出格式脆；
//! `npm root -g` 给出全局 node_modules 根（实测 69ms），再按目录是否存在
//! 判定即可——scoped 包（`@larksuite/cli`）在根下就是同名子目录。
//!
//! **命令定位**：PATH 解析（不经 shell），macOS 补 brew 前缀（从 Finder
//! 启动的 GUI 应用 PATH 极简，只有 /usr/bin:/bin）、Windows 补
//! `.exe/.cmd/.bat`（CreateProcess 只补 .exe，而 npm 的 npx/npm 是 .cmd）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::py::env::probe_output_with_timeout;

// ── 输出结构 ──

/// 一次环境检查的完整结果（前端按三组渲染）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EnvCheck {
    /// 探测线程整体失败（panic）：三组数据都不可信，别显示成「什么都没装」
    pub failed: bool,
    pub python: PythonEnv,
    pub node: NodeEnv,
    pub launchers: Vec<LauncherEnv>,
}

/// Python 解释器 + 包可用性
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PythonEnv {
    /// 解释器是否存在
    pub available: bool,
    /// 解释器命令或路径
    pub python: String,
    /// 解释器版本串
    pub version: String,
    /// 包可用性；**空 = 探针没跑出结果**（不是「全都没装」）
    pub packages: Vec<PyPackage>,
}

/// 单个 Python 包
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PyPackage {
    /// pip 包名（安装命令用它）
    pub name: String,
    pub available: bool,
    /// 产品硬依赖：缺了对应工具直接不可用（缺了只是少个增强）
    pub required: bool,
    /// 用途说明
    pub purpose: String,
}

/// Node 运行时 + 全局 npm 包可用性
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnv {
    /// Node 是否存在
    pub available: bool,
    /// node 可执行文件路径
    pub node: String,
    /// node 版本串
    pub version: String,
    /// npm 可执行文件路径
    pub npm: String,
    /// npm 版本串
    pub npm_version: String,
    /// 全局包可用性
    pub packages: Vec<NodePackage>,
}

/// 单个全局 npm 包
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodePackage {
    /// npm 包名
    pub name: String,
    pub available: bool,
    /// 用途说明
    pub purpose: String,
}

/// 单个运行时 / 启动器命令
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LauncherEnv {
    /// 命令名
    pub name: String,
    pub available: bool,
    /// 用途说明
    pub purpose: String,
}

// ── 清单 ──

/// Python 包清单：(pip 名, import 候选名, 是否产品硬依赖, 用途)。
///
/// import 名与 pip 名不同的必须分列——安装命令用 pip 名，探测用 import 名：
/// python-docx→docx、python-pptx→pptx、Pillow→PIL、beautifulsoup4→bs4、
/// python-dateutil→dateutil、opencv-python→cv2。
///
/// `required=true` 的 5 个是产品文档工具族的硬依赖（make_xlsx/make_docx/
/// make_pptx/extract/make_pdf 脚本直接 import 它们），缺了对应工具直接不可用；
/// 其余是办公增强项（数据/图表/抓取/模板/图像），缺了只是少个能力。
///
/// **PyMuPDF 的 import 名有两个**：`pymupdf` 是新模块名，`fitz` 是历史别名
/// （新版本里 import fitz 会打弃用警告）。两个都试，老版本才认 fitz。
const PY_PACKAGES: &[(&str, &[&str], bool, &str)] = &[
    ("openpyxl", &["openpyxl"], true, "Excel 读写"),
    ("python-docx", &["docx"], true, "Word 生成"),
    ("python-pptx", &["pptx"], true, "PPT 生成"),
    ("pypdf", &["pypdf"], true, "PDF 文本层提取"),
    ("reportlab", &["reportlab"], true, "PDF 生成"),
    (
        "PyMuPDF",
        &["pymupdf", "fitz"],
        false,
        "扫描版 PDF 转图（OCR 兜底）",
    ),
    (
        "xlsxwriter",
        &["xlsxwriter"],
        false,
        "Excel 写入（多表/图表）",
    ),
    ("pdfplumber", &["pdfplumber"], false, "PDF 表格提取"),
    ("Pillow", &["PIL"], false, "图片裁剪/压缩/格式转换"),
    ("pandas", &["pandas"], false, "数据表处理"),
    ("numpy", &["numpy"], false, "数值计算"),
    ("matplotlib", &["matplotlib"], false, "图表出图"),
    ("lxml", &["lxml"], false, "XML/HTML 解析"),
    ("requests", &["requests"], false, "HTTP 抓取"),
    ("beautifulsoup4", &["bs4"], false, "HTML 解析"),
    ("jinja2", &["jinja2"], false, "模板渲染"),
    ("python-dateutil", &["dateutil"], false, "日期/时区处理"),
    ("opencv-python", &["cv2"], false, "图像预处理"),
];

/// 全局 npm 包清单：(包名, 用途)
const NODE_PACKAGES: &[(&str, &str)] = &[
    ("docx", "Word 文档生成"),
    ("pptxgenjs", "PPT 生成"),
    ("exceljs", "Excel 读写"),
    ("xlsx", "xls/xlsx 读写"),
    ("pdf-lib", "PDF 生成/合并"),
    ("mammoth", "docx 文本提取"),
    ("sharp", "图片裁剪/压缩"),
    ("@larksuite/cli", "飞书 CLI"),
    ("@larksuiteoapi/node-sdk", "飞书开放平台 SDK"),
];

/// 运行时 / 启动器清单：(命令名, 用途)。
///
/// 前 10 个里 npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman
/// 是 MCP stdio 启动器白名单（`bot::mcp::config::STDIO_LAUNCH_ALLOWLIST`，
/// 单一事实源）——用户在 MCP 面板挑了启动器，本机有没有装必须看得见，
/// 否则只能等连接失败才知道。dotnet 是 Word 修订工具的前置条件
/// （`py::env::detect_dotnet` 早就能探，但从不进面板）。
const LAUNCHERS: &[(&str, &str)] = &[
    ("dotnet", "Word 修订工具（.NET 8+）"),
    ("node", "Node 运行时"),
    ("npm", "npm 包管理器"),
    ("npx", "MCP 服务器启动器（npm）"),
    ("bun", "Bun 运行时"),
    ("bunx", "MCP 服务器启动器（Bun）"),
    ("uv", "Python 工具管理器"),
    ("uvx", "MCP 服务器启动器（uv）"),
    ("pipx", "MCP 服务器启动器（pipx）"),
    ("python", "MCP 服务器启动器（Python）"),
    ("python3", "MCP 服务器启动器（Python）"),
    ("deno", "Deno 运行时"),
    ("docker", "容器运行时"),
    ("podman", "容器运行时"),
    ("git", "版本库"),
    ("soffice", "LibreOffice 命令行（Office 转 PDF）"),
];

// ── 探测入口 ──

/// 同步探测核心（后台线程运行）
pub fn env_check_blocking() -> EnvCheck {
    EnvCheck {
        failed: false,
        python: python_env(),
        node: node_env(),
        launchers: launcher_env(),
    }
}

/// tauri command：一次性探测本机运行环境（Python 包 / Node 包 / 运行时启动器）。
///
/// 放后台线程跑：探测要 spawn 十几个子进程（python 探针 + npm + 版本查询），
/// 同步跑会卡住主线程几秒。
#[tauri::command]
pub async fn env_check() -> EnvCheck {
    tauri::async_runtime::spawn_blocking(env_check_blocking)
        .await
        .unwrap_or_else(|e| {
            // Join 失败（探测任务 panic）：三组数据都没探出来，标 failed 让前端
            // 显示「探测异常」——静默返回空组会被读成「本机什么都没装」
            eprintln!("[env] env_check 探测线程失败（按未探测降级）：{e}");
            EnvCheck {
                failed: true,
                python: PythonEnv {
                    available: false,
                    python: String::new(),
                    version: String::new(),
                    packages: Vec::new(),
                },
                node: NodeEnv {
                    available: false,
                    node: String::new(),
                    version: String::new(),
                    npm: String::new(),
                    npm_version: String::new(),
                    packages: Vec::new(),
                },
                launchers: Vec::new(),
            }
        })
}

fn python_env() -> PythonEnv {
    let Some(py) = crate::py::env::detect_python() else {
        return PythonEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            packages: Vec::new(),
        };
    };
    let version = probe_output_with_timeout(&py, &["--version"])
        .map(|o| o.trim().to_string())
        .unwrap_or_default();
    let script = python_probe_script();
    let packages = match probe_output_with_timeout(&py, &["-c", &script]) {
        Some(out) => parse_probe_output(&out),
        None => Vec::new(),
    };
    PythonEnv {
        available: true,
        python: py,
        version,
        packages,
    }
}

fn node_env() -> NodeEnv {
    let node = resolve_executable("node");
    let npm = resolve_executable("npm");
    let root = npm_global_root(npm.as_deref());
    let packages = NODE_PACKAGES
        .iter()
        .map(|(name, purpose)| NodePackage {
            name: name.to_string(),
            // scoped 包（@larksuite/cli）在全局根下就是同名子目录
            available: root.as_deref().is_some_and(|r| r.join(name).is_dir()),
            purpose: purpose.to_string(),
        })
        .collect();
    NodeEnv {
        available: node.is_some(),
        node: path_str(node.as_deref()),
        version: version_of(node.as_deref(), &["--version"]),
        npm: path_str(npm.as_deref()),
        npm_version: version_of(npm.as_deref(), &["--version"]),
        packages,
    }
}

fn launcher_env() -> Vec<LauncherEnv> {
    LAUNCHERS
        .iter()
        .map(|(name, purpose)| LauncherEnv {
            name: name.to_string(),
            available: resolve_executable(name).is_some(),
            purpose: purpose.to_string(),
        })
        .collect()
}

// ── Python 探针 ──

/// 生成 Python 包探测脚本（find_spec 方案，见模块头注释）。
///
/// 包名与 import 名硬编码在 `PY_PACKAGES` 里且**全 ASCII**——Rust 的
/// `{:?}` 把非 ASCII 转义成 `\u{...}`，Python 字面量不认这个写法。
/// 「全 ASCII」这条前提由单测钉住。
fn python_probe_script() -> String {
    let mut s = String::from(
        "import importlib.util\n\
         def _probe(name, mods):\n\
         \x20   for m in mods:\n\
         \x20       try:\n\
         \x20           if importlib.util.find_spec(m) is not None:\n\
         \x20               print(name + ':OK')\n\
         \x20               return\n\
         \x20       except Exception:\n\
         \x20           pass\n\
         \x20   print(name + ':缺')\n",
    );
    for (name, imports, _required, _purpose) in PY_PACKAGES {
        s.push_str(&format!("_probe({name:?}, {imports:?})\n"));
    }
    s
}

/// 解析探针输出 → 包名 → 是否可用。
///
/// 只认清单里的包名：sitecustomize 之类额外 stdout 混进来时不得伪装成
/// 包条目。**返回条数不足 = 探针中途死了**——此时不返回任何包（宁可说
/// 「探测失败」也不把没判说成「全缺」）。
fn parse_probe_output(out: &str) -> Vec<PyPackage> {
    let mut map: HashMap<&str, bool> = HashMap::new();
    for line in out.lines() {
        let line = line.trim();
        let Some((name, tag)) = line.rsplit_once(':') else {
            continue;
        };
        let Some((pip_name, _, _, _)) = PY_PACKAGES.iter().find(|(n, ..)| *n == name) else {
            continue;
        };
        let ok = match tag {
            "OK" => true,
            "缺" => false,
            _ => continue,
        };
        map.insert(pip_name, ok);
    }
    if map.len() != PY_PACKAGES.len() {
        return Vec::new();
    }
    PY_PACKAGES
        .iter()
        .map(|(name, _, required, purpose)| PyPackage {
            name: name.to_string(),
            available: map.get(name).copied().unwrap_or(false),
            required: *required,
            purpose: purpose.to_string(),
        })
        .collect()
}

// ── 命令与路径定位 ──

/// 全局 node_modules 根：优先 `npm root -g`（跟着用户的 npm 配置走，
/// nvm/volta/自定义 prefix 都对）；npm 不可用时退回平台常见路径。
fn npm_global_root(npm: Option<&Path>) -> Option<PathBuf> {
    if let Some(npm) = npm {
        if let Some(out) = probe_output_with_timeout(&npm.to_string_lossy(), &["root", "-g"]) {
            // npm 偶尔在前面打警告行，取最后一行非空内容当路径
            if let Some(line) = out.lines().rev().find(|l| !l.trim().is_empty()) {
                let p = PathBuf::from(line.trim());
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }
    fallback_global_root()
}

/// npm 不可用时的常见全局根（brew / 官方安装 / Windows %APPDATA%）
fn fallback_global_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let candidates = [
        PathBuf::from("/opt/homebrew/lib/node_modules"),
        PathBuf::from("/usr/local/lib/node_modules"),
    ];
    #[cfg(windows)]
    let candidates: [PathBuf; 1] = [std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join("npm").join("node_modules"))
        .unwrap_or_default()];
    #[cfg(not(any(target_os = "macos", windows)))]
    let candidates: [PathBuf; 0] = [];

    candidates.into_iter().find(|p| p.is_dir())
}

/// 在 PATH（含平台常见前缀）里定位可执行文件。None = 没装。
fn resolve_executable(name: &str) -> Option<PathBuf> {
    for dir in search_dirs() {
        for cand in exe_candidates(name) {
            let p = dir.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 搜索目录：PATH + 平台常见前缀。
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // macOS：从 Finder 启动的 GUI 应用 PATH 极简（只有 /usr/bin:/bin），
    // brew 装的 node/npm/uv 都在 /opt/homebrew/bin 下 → 补固定前缀
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        // LibreOffice 不在 PATH 里，只在 .app 包内
        dirs.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS",
        ));
    }
    // Windows：npm 全局 shim（npm.cmd / npx.cmd）在 %APPDATA%\npm
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    }
    dirs
}

/// 可执行文件名候选：Windows 上 CreateProcess 只补 .exe，不吃 PATHEXT——
/// 而 npm 装的 npx/npm 是 .cmd / .bat。
fn exe_candidates(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

/// 跑一次 `--version` 拿版本串（拿不到就空串，不算失败）
fn version_of(program: Option<&Path>, args: &[&str]) -> String {
    program
        .and_then(|p| probe_output_with_timeout(&p.to_string_lossy(), args))
        .map(|o| o.trim().to_string())
        .unwrap_or_default()
}

fn path_str(p: Option<&Path>) -> String {
    p.map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 探针脚本覆盖全部包且名字全_ascii() {
        let script = python_probe_script();
        for (name, imports, _, _) in PY_PACKAGES {
            assert!(
                name.is_ascii(),
                "包名 {name} 非 ASCII：Rust 的 Debug 转义与 Python 字面量不兼容"
            );
            assert!(
                script.contains(&format!("_probe({name:?},")),
                "探针脚本漏了包 {name}"
            );
            for imp in *imports {
                assert!(imp.is_ascii(), "import 名 {imp} 非 ASCII");
            }
        }
        // 找而不执行：import 一遍 pandas/matplotlib 就是秒级，会撞穿 3s 探针超时
        assert!(script.contains("importlib.util.find_spec"));
        assert!(!script.contains("importlib.import_module"));
    }

    #[test]
    fn 探针输出解析出全部包() {
        let mut out = String::new();
        for (name, _, _, _) in PY_PACKAGES {
            out.push_str(&format!("{name}:OK\n"));
        }
        let pkgs = parse_probe_output(&out);
        assert_eq!(pkgs.len(), PY_PACKAGES.len());
        assert!(pkgs.iter().all(|p| p.available));
        let docx = pkgs.iter().find(|p| p.name == "python-docx").unwrap();
        assert!(docx.required, "python-docx 是文档工具族硬依赖");
        assert_eq!(docx.purpose, "Word 生成");
    }

    #[test]
    fn 探针输出缺行时不当成全缺() {
        // 探针中途死 → 只出了一半行
        let out = "openpyxl:OK\npython-docx:OK\n";
        assert!(
            parse_probe_output(out).is_empty(),
            "部分输出必须报「探测失败」而不是把没探到的包说成缺"
        );
    }

    #[test]
    fn 探针输出忽略清单外的行() {
        let mut out = String::new();
        for (name, _, _, _) in PY_PACKAGES {
            out.push_str(&format!("{name}:OK\n"));
        }
        out.push_str("sitecustomize:OK\nrandom-garbage\n");
        let pkgs = parse_probe_output(&out);
        assert_eq!(pkgs.len(), PY_PACKAGES.len());
        assert!(!pkgs.iter().any(|p| p.name == "sitecustomize"));
    }

    #[test]
    fn 缺标记与_ok_标记分别解析() {
        let mut out = String::new();
        for (i, (name, _, _, _)) in PY_PACKAGES.iter().enumerate() {
            out.push_str(&format!(
                "{name}:{}\n",
                if i % 2 == 0 { "OK" } else { "缺" }
            ));
        }
        let pkgs = parse_probe_output(&out);
        assert_eq!(pkgs.len(), PY_PACKAGES.len());
        assert!(pkgs[0].available);
        assert!(!pkgs[1].available);
    }

    #[test]
    fn 包名不重复() {
        let mut names: Vec<&str> = PY_PACKAGES.iter().map(|(n, ..)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "包名重复：解析按名建表，重复会少一条");
    }

    #[test]
    fn 必填依赖与脚本实际导入一致() {
        // 硬依赖的包必须真的被某个生成脚本 import（否则「硬依赖」是自说自话）
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/py/scripts");
        let bodies: Vec<String> = [
            "make_xlsx.py",
            "make_docx.py",
            "make_pptx.py",
            "extract.py",
            "make_pdf.py",
        ]
        .iter()
        .map(|f| std::fs::read_to_string(dir.join(f)).unwrap_or_else(|e| panic!("{f} 应存在：{e}")))
        .collect();
        for (pip_name, imports, required, _) in PY_PACKAGES {
            if !*required {
                continue;
            }
            let imported = bodies.iter().any(|body| {
                imports.iter().any(|imp| {
                    body.contains(&format!("import {imp}")) || body.contains(&format!("from {imp}"))
                })
            });
            assert!(imported, "硬依赖 {pip_name} 在任何生成脚本里都没被 import");
        }
    }

    #[test]
    fn 启动器清单覆盖_mcp_白名单() {
        // MCP stdio 启动器白名单里的命令必须都在启动器清单里
        // （用户在 MCP 面板挑启动器，本机有没有装必须看得见）
        for launcher in crate::bot::mcp::config::STDIO_LAUNCH_ALLOWLIST {
            assert!(
                LAUNCHERS.iter().any(|(n, _)| n == launcher),
                "启动器清单漏了 MCP 白名单里的 {launcher}"
            );
        }
    }

    #[test]
    fn 可执行文件候选按平台给扩展名() {
        let cands = exe_candidates("npx");
        assert!(cands.contains(&"npx".to_string()));
        #[cfg(windows)]
        {
            assert!(cands.contains(&"npx.exe".to_string()));
            assert!(cands.contains(&"npx.cmd".to_string()));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(cands.len(), 1, "非 Windows 不该造 .cmd/.exe 候选");
        }
    }

    #[test]
    fn 能定位到本机存在的可执行文件() {
        // shell 这类必然存在的命令（跨平台都有）
        let found = resolve_executable("sh");
        assert!(found.is_some(), "本机应有 sh，PATH 解析却没找到");
        assert!(found.unwrap().is_file());
    }

    #[test]
    fn 真实探测覆盖清单里每个包() {
        // 端到端：跑真探针（会 spawn 子进程）
        let Some(_py) = crate::py::env::detect_python() else {
            eprintln!("skip: 本机无 python3");
            return;
        };
        let env = env_check_blocking();
        assert!(!env.failed);
        assert!(env.python.available);
        assert_eq!(
            env.python.packages.len(),
            PY_PACKAGES.len(),
            "探针必须覆盖清单里每个包（探针中途死 = 少行 = packages 空）"
        );
        eprintln!("python={} {}", env.python.python, env.python.version);
        for p in &env.python.packages {
            eprintln!("  {:<16} {}", p.name, if p.available { "OK" } else { "缺" });
        }
        eprintln!("node={} npm={}", env.node.version, env.node.npm_version);
        for p in &env.node.packages {
            eprintln!("  {:<24} {}", p.name, if p.available { "OK" } else { "缺" });
        }
        for l in &env.launchers {
            eprintln!("  {:<10} {}", l.name, if l.available { "OK" } else { "缺" });
        }
    }
}
