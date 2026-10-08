import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import WidgetApp from "./components/WidgetApp";
import "./ui/main.css";

// 挂件窗口通过 URL hash 区分（Rust 侧加载 index.html#/widget）
const isWidget = window.location.hash.startsWith("#/widget");

// CSP 违规上报（安全加固配套，docs/CSP-TIGHTEN-VERIFY-2026-10-07.md §8）：
// securitypolicyviolation → Rust 审计日志（frontend.csp_violation），让运行时
// 违规可感知。风暴双闸：前端每会话上限 20 条，Rust 侧另有签名去重（32 签名）。
// 上报失败静默（审计通道自身不得产生用户可见副作用）。StrictMode 双挂载下
// 重复 addEventListener 会双发——用标志位保证只装一次。
let cspReports = 0;
let cspListenerInstalled = false;
if (!cspListenerInstalled) {
  cspListenerInstalled = true;
  document.addEventListener("securitypolicyviolation", (e) => {
    if (cspReports >= 20) return;
    cspReports += 1;
    invoke("frontend_event_report", {
      event: "csp_violation",
      detail: `${e.violatedDirective} @ ${e.sourceFile || ""}:${e.lineNumber ?? ""} sample=${(e.sample || "").slice(0, 100)}`,
    }).catch(() => {});
  });
}

// root 缺失（模板错误/宿主页异常）时 fail-fast 带诊断信息，不让 ReactDOM 裸抛
const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("Root element #root not found in document");
}

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>{isWidget ? <WidgetApp /> : <App />}</React.StrictMode>,
);
