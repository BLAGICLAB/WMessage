import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import WidgetApp from "./components/WidgetApp";
import { loadAppConsts } from "./lib/consts";
import "./ui/main.css";

// 共享常量启动拉取（Phase B）：不阻塞首屏渲染，回退值与 Rust 侧同值
void loadAppConsts();

// 挂件窗口通过 URL hash 区分（Rust 侧加载 index.html#/widget）
const isWidget = window.location.hash.startsWith("#/widget");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isWidget ? <WidgetApp /> : <App />}</React.StrictMode>,
);
