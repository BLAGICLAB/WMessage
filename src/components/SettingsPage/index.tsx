// SettingsPage 目录模块 re-export 入口。
// Vite 解析 `import { SettingsPage } from "./components/SettingsPage"` 时优先匹配目录 + index.tsx，
// 内部文件统一从 `SettingsPage.tsx` 真实组件文件导出。

export { SettingsPage } from "./SettingsPage";
