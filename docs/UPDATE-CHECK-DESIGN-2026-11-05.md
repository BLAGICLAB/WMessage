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

## 三、发版纪律（每次出包三步）

1. `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/wmessage-updater.key)"`
   与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""`，再走安装版出包两步（暂存 + 构建）。
   `createUpdaterArtifacts` 只在 `tauri.windows.conf.json`——mac 构建不需要签名
   环境变量，两平台解耦。
2. `bash scripts/make-update-manifest.sh <版本> "<说明>"` 生成仓库根 `latest.json`
   （缺安装器/缺签名即断）。
3. GitHub Releases 建 `v<版本>`：上传 `wmessage_x64-setup.exe`（setup.exe 改名，
   固定资产名）与 `latest.json`。

**丢私钥 = 更新链路全断**：`.key` 与口令务必另存一份（密码管理器/离线介质）；
若重生成密钥，`tauri.conf.json` 的 pubkey 必须同批换。

## 四、不做清单

- 启动自动检查、静默更新、更新倒计时强推——更新永远手动。
- macOS 自动更新（dmg 不公开发布；mac 检查入口本就不渲染）。
- CI 自动发版（gh CLI 未装；发版三步全手动）。

## 五、验收

- 组件五用例：`npx vitest run src/components/SettingsPage/UpdateCard.test.tsx`
- 实机卡：`docs/acceptance/cards-08-platform.md` ACC-P-15（需首个 release）
- SPEC 6-120 / 矩阵 6-120 已登记，`audit_acceptance_matrix.py` 对拍通过。
