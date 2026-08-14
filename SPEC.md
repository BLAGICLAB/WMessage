# WMessage — SPEC v1

> 唯一依据。来源：老板 2026-08-13 发来的最终 OpenClaw 完整 Prompt（含样式说明）。

## 项目定位

WMessage：Tauri + React + TailwindCSS 的 Todo 看板，带系统侧边磁吸挂件。

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3（`@layer components` 新拟态组件类）
- 拖拽：`@dnd-kit/core`
- 编译产出：Win10/Win11（exe 安装包）、macOS（dmg，Intel + Apple Silicon）

## UI 规范（新拟态 nm 体系）

- 基色 `#f4f7fa`；凸卡片圆角 `rounded-2xl`
- 凸卡片 `.nm-card`：`box-shadow: 6px 6px 12px #d8dee6, -6px -6px 12px #ffffff`
- hover 上浮 `.nm-card-hover`：`translateY(-3px) scale(1.01)` + 阴影增强
- 内凹 `.nm-inset`：inset 阴影，用于按钮/列头/触发条，与凸卡片对比
- 侧边面板 `.nm-sidebar-panel`：左侧圆角 + 单侧投影
- 先浅色主题；深色模式后续扩展

## 功能需求

1. 主看板：卡片列 + 拖拽（列间移动）
2. 系统侧边磁吸挂件：默认贴边隐藏，鼠标悬停滑出单列清单；视图切换（全部任务 / 今日专注）
3. 任务绑定本地文件：打开本地文件；一键复制文件 + 附带任务标题文本
4. 本地持久化存储
5. 全局快捷键
6. 挂件常驻锁定开关
7. 挂件 UI 与主看板视觉统一

## 里程碑

- M1 脚手架 + 样式体系 + 看板静态 ✓
- M2 拖拽看板（dnd-kit）+ localStorage 持久化
- M3 文件绑定：打开 / 复制（Tauri opener + clipboard + fs）
- M4 侧边挂件多窗口（磁吸、悬停滑出、锁定常驻）
- M5 全局快捷键
- M6 打包（NSIS exe + dmg universal）

## Logo 规范

- 视觉使用规范 V1.0（正式版）：`docs/logo/WMessage-LOGO-GUIDELINES.md`
- Logo 资产：`docs/logo/assets/`（已处理：去水印、透明底、多尺寸）

## 目录约定

- 样式入口：`src/ui/main.css`
- 组件：`src/components/`
- 类型：`src/types.ts`
