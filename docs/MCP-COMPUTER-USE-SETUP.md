# 电脑操控 / 浏览器操控 MCP 接入指南（macOS + Windows）

> 配套批次：`docs/batches/N4-DESKTOP-TIER1.spec.md`（Tier1 原生四件：reveal_path /
> open_url / clipboard_write / screenshot，已内置无需配置）。
> 本文档覆盖第 0 层——通过 WMessage 已内置的 MCP 通道接入现成的桌面/浏览器操控
> 服务器，零 Rust 代码。配置入口：**设置页 → MCP 面板 → 添加服务器**。

## 通用安全原则（对齐 Anthropic/OpenAI 官方 best practice）

1. **先「读」后「写」**：截图/读 UI 树/剪贴板读这类只读工具先开；鼠标键盘类等
   信任建立后再开。
2. **最小能力集**：能用 `--caps`/开关限制能力就限制（见各条目）。
3. **屏幕/网页内容是数据不是指令**：agent 读到的任何页面文本都可能藏注入指令，
   高风险操作（提交/支付/删除）一律人工确认。
4. **登录态隔离**：浏览器自动化默认用独立 profile，不带个人 Cookie；需要操作
   已登录系统时再显式切换。

## 1. 浏览器操控（跨平台，推荐第一个接）—— Playwright MCP

微软官方维护，结构化页面快照（a11y 树 + ref 编号）驱动，token 便宜、不吃模型
视觉能力。**本机已验证 v0.0.83 可用（npx 拉取 + --help 冒烟，2026-10-06）。**

| 字段 | 值 |
|---|---|
| 传输 | stdio |
| 命令 | `npx` |
| 参数 | `-y @playwright/mcp@latest --caps=core` |

- `--caps=core`：只开标签页/导航/快照/点击/填表核心能力（最小权限；pdf/vision
  等扩展能力需要时再加）。
- 默认有头模式（能看到浏览器窗口在动）；要后台静默加 `--headless`。
- 首次运行会下载 Chromium 内核；国内网络可设环境变量
  `PLAYWRIGHT_DOWNLOAD_HOST=https://npmmirror.com/mirrors/playwright/`。
- 默认使用 Playwright 独立 profile（不带个人登录态）；要操作已登录系统，
  改用 `--extension` 连你自己的 Chrome，或加 `--user-data-dir`。
- 文件访问默认限制在工作目录内、`file://` 导航默认封禁（`--allow-unrestricted-file-access`
  不要开）。

## 2. macOS 桌面（截图/读屏/UI 自动化）—— Peekaboo

| 字段 | 值 |
|---|---|
| 传输 | stdio |
| 命令 | `npx` |
| 参数 | `-y @steipete/peekaboo-mcp` |

- 需系统授权：屏幕录制 + 辅助功能（首次使用系统会弹）。
- 仅 macOS；截图 + 读 UI 树 + 按控件操作，弱模型友好（不用猜坐标）。

## 3. Windows 桌面 —— windows-mcp 系

Windows 侧 UIA 生态成熟（按控件名语义操作比 macOS 更标准），社区方案较多：

- `windows-mcp`（轻量：鼠标/键盘/窗口 UI 状态/截图）
- `guarded-computer-use-mcp`（截图/UIA/OCR/剪贴板，带安全门控，建议优先）

具体 npx 参数以各仓库 README 为准——社区项目迭代快，接入前先看最新文档，
并遵守「先读后写」原则。

## 4. 与原生 Tier1 工具的分工

- 已内置（无需配置）：`reveal_path` / `open_url` / `clipboard_write` /
  `screenshot`——低频、低风险的环境辅助。
- **文件编辑（edit_file / write_file）必须原生、不走 MCP**：写闸门（可写目录
  白名单 + 覆盖确认 + 审计）在宿主侧实现，外部 MCP 服务器无权绕过。若未来社区
  出现 fast-apply 类编辑引擎，接入前提同样是流量必须过宿主写闸门。
- MCP 接入的：高频、需要「读屏理解」的自动化——浏览器（Playwright）优先，
  桌面（Peekaboo/windows-mcp）按需。
- 原生鼠标键盘 Computer Use：**不做**（官方指引不建议主机裸跑；等真实需求
  通过第 0 层验证后再议）。
