// WidgetApp 目录模块 re-export 入口。
// WidgetApp 是默认导出（main.tsx 用 `import WidgetApp from "./components/WidgetApp"`），
// index.tsx 必须保留默认导出语义。
// Vite 解析 `./components/WidgetApp` 时优先匹配目录 + index.tsx，
// index.tsx re-export  `./WidgetApp.tsx` 的 default 即可。

export { default } from "./WidgetApp";
