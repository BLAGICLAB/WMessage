# Chunk 8: 跨平台打包 + 构建（v2 窄范围）

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：
  - 4 份原始审计 + chunk-1~5 结果（48 条新发现）
  - F-1~F-7 修复 plan
  - 老板 2026-08-14 实测的 Windows 打包踩坑：WebView2Loader.dll 必带 / mingw-w64 / NSIS 跨平台崩 / cargo build vs tauri build 区别 / 便携模式
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因
- **严格只读**，不改代码

## 重要（避免再爆 token）
- **不要使用 sub-agent / Explore / Task 工具**——直接 Read 下面的文件
- **不要并行**——一个一个顺序读
- 文件太大就按 2000 行切片读（offset + limit）
- 不要做任何 grep / 全文搜索（除非本 prompt 内明确允许的定向 grep）

## 任务
审查 wmessage **跨平台打包 + 构建配置**——找**target 条件编译 / 资源嵌入 / 跨平台差异 / bundle 配置 / 图标 / 权限 / Windows 资源 / macOS 公证 / Linux AppImage**的潜在 bug 根因。

## 范围（必读，按顺序）
- `src-tauri/Cargo.toml` (2.6 KB, 依赖 + features + target deps)
- `src-tauri/tauri.conf.json` (1.2 KB, bundle 配置 + 窗口 + 资源 + 图标)
- `src-tauri/build.rs` (39 字节, 资源嵌入脚本)
- `src-tauri/capabilities/default.json` (693 字节, 权限声明)
- `src-tauri/src/lib.rs` 中**所有 `#[cfg(target_os = ...)]` / `#[cfg(windows)]` / `#[cfg(unix)]` 块**（grep 定向抓，所有平台特定代码）
- `package.json` + `package.json` 里 `tauri` / `build` 脚本（影响前端打包 + tauri build 命令）

允许定向 grep：
- `grep -rn "cfg(target_os\|cfg(windows)\|cfg(unix)\|cfg(target_arch"` 看所有平台条件编译
- `grep -rn "include_str!\|include_bytes!" 看资源嵌入

## 重点关注
1. **target 条件编译完备性**：lib.rs 已有 `copy_file_with_title` 三平台分支（macOS / Windows / 其他）；其他 platform-specific 操作（剪贴板 / 废纸篓 / 快捷键 / 托盘 / 注册表）是否每个平台都有实现？还是某些平台 fallback 是 `let _ = ...; return Err` 让用户碰到运行时崩溃？
2. **资源嵌入完整性**：`tauri.conf.json` 的 `bundle.resources` 列出哪些文件？这些文件在 `tauri build` 时是否真被打进 bundle？开发模式 `tauri dev` 加载路径是否一致？
3. **Windows 特殊处理**：`WebView2Loader.dll` 是否在 tauri.conf.json 的 bundle.resources 里？mingw-w64 链接器配置是否在 `.cargo/config.toml` 而非污染 Cargo.toml？
4. **macOS 公证 / 签名**：tauri.conf.json 的 macOS bundle 配置（entitlements / hardenedRuntime / signingIdentity）？本地开发 OK 但 release 出包失败的可能坑？
5. **Linux AppImage / deb**：bundle targets 配置？glibc 兼容性？webview2 是否在 Linux 也用 webkit2gtk？
6. **capabilities 权限完整性**：`default.json` 声明的权限与代码实际 invoke 的能力是否一致？缺权限会编译期错还是运行期静默失败？
7. **build.rs 与 frontend 资源同步**：前端 `dist/` / `assets/` 在 build 前是否被生成？本地 dev 与 release 路径是否一致？
8. **跨平台图标 / 托盘**：icons 目录（macOS .icns / Windows .ico / Linux .png）是否齐全？托盘图标在不同 DPI 下是否清晰？

## 跳过（已审过，**不要列**）
- chunk-1~5 全部内容
- 已有 4 份原始审计覆盖的 ChatPanel UI / 拖拽 / 折叠规则 / CommandError / bot_chat 主循环
- middleware / intent_router / tool_guard / audit（chunk 4 审过）
- lib.rs 中**非跨平台部分**（setup / run / 命令注册）——留给 chunk 7
- error.rs / api_*.rs / db.rs / bot_*.rs 业务逻辑

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条
4. **统计**：cfg 平台分支 N 处 / 缺 fallback N 处 / 资源嵌入 N 处 / capabilities 声明 N 个

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不要用 Explore / Task / Agent 工具
- 不重复已有 P1/P2 项
- 没发现就明说"无新发现"