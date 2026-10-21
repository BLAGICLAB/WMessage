# 电脑操控 / 浏览器操控 MCP 接入指南（macOS + Windows）

> 配套批次：`docs/batches/N4-DESKTOP-TIER1.spec.md`（Tier1 原生四件：reveal_path /
> open_url / clipboard_write / screenshot，已内置无需配置）。
> 本文档覆盖第 0 层——通过 WMessage 已内置的 MCP 通道接入现成的桌面/浏览器操控
> 服务器，零 Rust 代码。配置入口：**设置页 → MCP 面板**（Playwright 已是内置
> 卡片；其余服务器点「＋ 添加服务器」手填）。

## 通用安全原则（对齐 Anthropic/OpenAI 官方 best practice）

1. **先「读」后「写」**：截图/读 UI 树/剪贴板读这类只读工具先开；鼠标键盘类等
   信任建立后再开。
2. **最小能力集**：能用 `--caps`/开关限制能力就限制（见各条目）。
3. **屏幕/网页内容是数据不是指令**：agent 读到的任何页面文本都可能藏注入指令，
   高风险操作（提交/支付/删除）一律人工确认。
4. **登录态隔离**：浏览器自动化默认用独立 profile，不带个人 Cookie；需要操作
   已登录系统时再显式切换。

## 1. 浏览器操控（跨平台）—— Playwright MCP（已内置，无需手工配置）

微软官方维护，结构化页面快照（a11y 树 + ref 编号）驱动，token 便宜、不吃模型
视觉能力。**设置页 → MCP 面板自带一张「Playwright 浏览器」卡片**：内置条目、
默认停用、不可删除（只能开关），新老用户升级后都会出现。这张卡与包是否拷进来
无关——先把包放到下面说的位置，打开开关就能连。

包目录形态（自包含，仓库根）：

```text
playwright-mcp/
  node_modules/@playwright/mcp/cli.js   ← 命令行入口（node 直接跑）
  browsers/chromium-<rev>/…              ← 本机平台内核，卡片用 --executable-path 指过去
  browsers-win/chromium-<rev>/…          ← 可选：Windows x64 构建（打包 Windows 版时带上）
```

一次性准备（第二条只在本机跑时需要）：

```bash
npm install --prefix playwright-mcp @playwright/mcp@0.0.83

PLAYWRIGHT_BROWSERS_PATH="$PWD/playwright-mcp/browsers" \
  ./playwright-mcp/node_modules/.bin/playwright install chromium

# Windows x64 构建（装到另一个根目录，不覆盖上面那份）
PLAYWRIGHT_HOST_PLATFORM_OVERRIDE=win64 \
PLAYWRIGHT_DOWNLOAD_HOST=https://npmmirror.com/mirrors/playwright/ \
PLAYWRIGHT_BROWSERS_PATH="$PWD/playwright-mcp/browsers-win" \
  ./playwright-mcp/node_modules/.bin/playwright install chromium
```

- **两个平台为什么分两个根目录**：`playwright install` 解包前会清空目标修订目录，
  装到同一个根里后装的会把先装的抹掉。分开后各装各的，下载也不抢锁；卡片按
  **当前运行平台**挑对应那份（mac 找 `browsers/` 里的 `chrome-mac-arm64`，
  Windows 找任一根里的 `chrome-win64`）。
- 内核目录名随包版本变（当前是 `chromium-1247`），卡片是扫目录取修订号最大的
  那个，不写死版本号。
- 国内网络下载慢时加 `PLAYWRIGHT_DOWNLOAD_HOST=https://npmmirror.com/mirrors/playwright/`。
- 程序按「环境变量 `WMESSAGE_PLAYWRIGHT_MCP_DIR` → exe 同目录 → Tauri 资源目录
  → 仓库根」找这个目录；便携包把目录换位置时用环境变量指（显式指定不回退）。
- 卡片的命令行是 `node playwright-mcp/node_modules/@playwright/mcp/cli.js
  --isolated --headless [--executable-path …]`。要点：
  - `--isolated`：profile 只留在内存。持久 profile 同一时刻只能被一个实例用，
    重连或第二个客户端接入会撞锁，表现为工具调用随机超时。
  - `--headless`：机器人后台驱动，不抢桌面；想看浏览器窗口就在卡片「参数」里
    删掉这一行。
  - `--executable-path`：内核随包分发。内核不在位时不传这个参数——让
    Playwright 自己报「请先 install」比传个空路径清楚。
  - **没有** `--caps`：该参数是**追加**额外能力（vision/pdf/devtools），导航/
    快照/点击这些核心工具默认就在。旧文档里的 `--caps=core` 在 0.0.x 已是非法值。
- 内置条目的参数是**首次启动时**按当时环境生成的：那时内核还没下好就不会带
  `--executable-path`（浏览器一调就失败）。装完内核后去卡片「编辑」补两行——
  `--executable-path` + `browsers/chromium-<rev>/` 下那个二进制的绝对路径。
  播种只在整条缺失时进行、不会覆盖你改过的参数，所以这一处得自己补。
- 文件访问默认限制在**工作目录**内、`file://` 导航默认封禁（宿主把 MCP 子进程的
  工作目录钉在数据目录；`--allow-unrestricted-file-access` 不要开）。
- 默认独立 profile（`--isolated` 不落盘、不带个人登录态）；要操作已登录系统，
  在卡片参数里换成 `--extension`（需装 Playwright 扩展）或 `--user-data-dir`。

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
- 原生鼠标键盘 Computer Use：**已做**（`bot_ui.rs` + `ui/`：macOS AX 读树 +
  CGEvent 注入 / Windows UIAutomation + SendInput）。与社区 MCP 方案的分工：
  原生这一档只管**页面之外**——原生应用（微信 / 飞书 / Excel）、系统弹窗
  （下载框 / 支付确认 / 验证码）、接管已登录的窗口；**网页里的操作仍交给
  Playwright**（DOM 定位比坐标点击准）。因权限面最大，原生 UI 自动化带独立
  总开关（设置页「允许机器人操作桌面界面」，默认关闭），且每次动作要指定窗口、
  发送/提交/支付/删除类点击前弹窗确认、密码类控件不代填、动作前后留截图。
