# 检查更新方案（2026-11-05）

> **状态**：代码侧已落地（C1–C4），实机更新链路待首个 GitHub release 后验证。
> 一句话：设置页「检查更新」按钮（仅 Windows 渲染），官方 updater 插件 +
> GitHub Releases 匿名端点，minisign 验签后 passive 静默安装 + 重启。

## 一、拍板

| 项 | 决策 |
|---|---|
| 档位 | 方案二全量（官方 tauri-plugin-updater + tauri-plugin-process） |
| 更新源 | `https://github.com/BLAGICLAB/WMessage/releases/latest/download/latest.json`（匿名，仓已确认公开） |
| 安装模式 | `passive`（进度条自动装完重启；可配 quiet 全静默） |
| 触发 | 仅设置页按钮手动触发 |

四条红线：不做启动自动检查 / 不做静默更新（定时作业、无人值守不被打断）；
不嵌任何 token（匿名 HTTPS 端点）；失败显式报错、不许「检查失败当最新」；
签名私钥不进仓（`~/.tauri/wmessage-updater.key`，chmod 600，空口令）。

## 二、机制（已实证）

- 插件在 Windows 对 NSIS 安装器追加 `/UPDATE`（updater.rs:699）——**原地覆盖装，
  不跑旧卸载器**，不碰 `%APPDATA%` 数据（对照 installer.nsi 的 UpdateMode 分支）。
- `/R` 参数让安装器装完自动重启应用；前端再调 `relaunch()` 兜底（官方口径）。
- 端点返回 latest.json（version / notes / pub_date / platforms.windows-x86_64
  {signature, url}）；signature = .sig 文件原文（多行内容 JSON 转义）。
- **检查请求带 15 秒超时**：插件 `timeout` 默认 `None`（updater.rs:234），前端
  不传则 reqwest 不设超时——更新服务器不可达时请求挂到操作系统的连接超时，
  按钮全程停在「检查中」。`check({ timeout: 15000 })` 的单位是**毫秒**（Rust 侧
  `Duration::from_millis`）；它只约束清单请求，下载阶段的超时是
  `download` / `download_and_install` 各自的独立参数，不传则安装包下载不受限。
- **失败出路**：失败态按阶段（检查没拿到清单 / 安装包没装上）给中文提示，
  并提供「打开发布页」。按阶段而不是解析错误文本——插件经 IPC 传回的是
  字符串，按内容猜类型是脆口径。
- **逐端点回退确实存在**（updater.rs:545-658，插件 2.14.1）：请求失败 / 非 2xx /
  响应解析失败都会落到下一条端点，只有「成功返回」才不回退。机制在，但本仓
  只配了一条端点（见不做清单）。
- **请求跟随系统代理**：reqwest 0.13 的默认特性含 `system-proxy`，应用声明
  reqwest 时用默认特性——应用内更新请求跟随系统代理与环境变量代理。

## 三、发版纪律（每次出包四步）

0. **bump 版本号**：改 `src-tauri/Cargo.toml` 的 `version`（运行时版本唯一来源；
   latest.json 的 version 必须**大于**已发布版本，否则用户检查恒「已是最新」；
   清单脚本对相等版本会警告）。
1. `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/wmessage-updater.key)"`
   与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""`，再走安装版出包两步（暂存 + 构建）。
   `createUpdaterArtifacts` 只在 `tauri.windows.conf.json`——mac 构建不需要签名
   环境变量，两平台解耦。
2. `bash scripts/make-update-manifest.sh <版本> "<说明>"` 生成仓库根 `latest.json`
   （缺安装器/缺签名即断；版本与 Cargo 相等时警告）。
3. GitHub Releases 建 `v<版本>`：上传 `wmessage_x64-setup.exe`（setup.exe 改名，
   固定资产名）与 `latest.json`。

**丢私钥 = 更新链路全断**：`.key` 与口令务必另存一份（密码管理器/离线介质）；
若重生成密钥，`tauri.conf.json` 的 pubkey 必须同批换。

**更新即退出的语义**（插件源码实证）：点「下载并安装」→ 下载验签通过后插件
拉起安装器并 `exit(0)` 硬退——在途回合 / ask_user 等待 / 定时作业执行被中断
（重启后按既有启动恢复链处理），这是手动确认过的行为，不另做优雅停机。

## 四、不做清单

- 启动自动检查、静默更新、更新倒计时强推——更新永远手动。
- macOS 自动更新（dmg 不公开发布；mac 检查入口本就不渲染）。
- CI 自动发版（gh CLI 未装；发版三步全手动）。
- **更新源镜像兜底——不做**（2026-11-24 实测定案）。背景：`github.com` 的下载
  端点在本机网络间歇阻断（11-09、11-19 两次发布各 3/3 超时；11-24 复测 3/3
  可达，0.6–1.5 秒）。候选逐一否决，实测均为本机 curl 直连、同日：
  - `api.github.com/repos/…/releases/latest`：可达，但返回 GitHub API JSON，
    不是 updater 要的清单格式（缺 `version` / `platforms`），插件解析不了；
  - `raw.githubusercontent.com`：TLS 握手被断（curl 35，3/3）；且仓库根
    `latest.json` 按约定不上 git，即使网络通取到的也是 404；
  - jsDelivr：默认域 `cdn.jsdelivr.net` TLS 被断，`fastly` / `gcore` 节点可达，
    但它只分发 git 仓库里的文件——安装包不进 git，取不到；就算只镜像清单，
    清单里的安装包地址仍在 `github.com`，阻断时照样装不上；
  - `blagiclab.github.io`（Pages）：域可达但未开通；v0.4.0 安装器约 91MB 接近
    Pages 单文件上限，其它网络的可达性无保证，发版还要多一步同步。

  结论：**没有「只改配置」就能生效的镜像**。清单与安装包现都在 `github.com`
  域，阻断时一起失效；要做到阻断时仍可更新，得有两者都能落的托管（Pages 同步
  或自有对象存储），等真有需要再立项。当前兜底 = 超时快速失败 + 出路提示 +
  发布页手动下载（浏览器与 updater 的网络路径未必相同）。

## 五、验收

- 组件八用例：`npx vitest run src/components/SettingsPage/UpdateCard.test.tsx`
  （含「检查请求必须带超时」——去掉超时参数即红，已变异验证）
- 实机卡：`docs/acceptance/cards-08-platform.md` ACC-P-15（需首个 release）
- SPEC 6-120 / 矩阵 6-120 已登记，`audit_acceptance_matrix.py` 对拍通过。
