import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import WidgetApp from "./components/WidgetApp";
import "./ui/main.css";

// 挂件窗口通过 URL hash 区分（Rust 侧加载 index.html#/widget）
const isWidget = window.location.hash.startsWith("#/widget");

// root 缺失（模板错误/宿主页异常）时 fail-fast 带诊断信息，不让 ReactDOM 裸抛
const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("Root element #root not found in document");
}

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>{isWidget ? <WidgetApp /> : <App />}</React.StrictMode>,
);
