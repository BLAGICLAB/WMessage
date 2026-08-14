import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import WidgetApp from "./components/WidgetApp";
import "./ui/main.css";

// 挂件窗口通过 URL hash 区分（Rust 侧加载 index.html#/widget）
const isWidget = window.location.hash.startsWith("#/widget");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isWidget ? <WidgetApp /> : <App />}</React.StrictMode>,
);
