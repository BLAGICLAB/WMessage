# Batch Spec: N4-DESKTOP-TIER1 电脑辅助 Tier1 原生四件 + 操控 MCP 接入指南

```json
{
  "batch_id": "N4-DESKTOP-TIER1",
  "family": "bot-tools",
  "expected_files": [
    "docs/batches/N4-DESKTOP-TIER1.spec.md",
    "DEVLOG.md",
    "docs/MCP-COMPUTER-USE-SETUP.md",
    "src-tauri/Cargo.toml",
    "src-tauri/src/lib.rs",
    "src-tauri/src/bot_desktop.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/prompts/system.rs",
    "tests-audit/audit_bot_tools_alignment.py"
  ],
  "max_lines_added": 420,
  "max_lines_removed": 40,
  "findings": [
    {"id": "N4-1", "file": "src-tauri/src/bot_desktop.rs", "line": 1, "fix": "reveal_path：访达/资源管理器定位文件，resolve_with_perm 白名单闸与读文件一致（opener 插件 reveal_item_in_dir，bot_skills/files.rs 先例同款）"},
    {"id": "N4-2", "file": "src-tauri/src/bot_desktop.rs", "line": 60, "fix": "open_url：默认浏览器打开，复用 bot_web check_public_url 公网闸（新增 ensure_public_http_url 包装：仅 http/https + DNS 后拒绝本机/内网/保留段）"},
    {"id": "N4-3", "file": "src-tauri/src/bot_desktop.rs", "line": 100, "fix": "clipboard_write：官方 tauri-plugin-clipboard-manager v2（新增依赖 + lib.rs 注册），validate_clip_text 空串拒/10 万字符上限（截断会静默丢内容不如让模型分段）"},
    {"id": "N4-4", "file": "src-tauri/src/bot_desktop.rs", "line": 140, "fix": "screenshot：macOS screencapture -x / Windows PowerShell System.Drawing（均系统内置零依赖），PNG 落 AI_Gen_Files 时间戳命名，零字节产物判定为权限问题提示授权；屏幕内容分析走既有 ocr_image 链路"},
    {"id": "N4-5", "file": "src-tauri/src/bot/registry.rs", "line": 273, "fix": "四 schema + 四 ToolDef（全 mutating=false，插在 cancel_subagent 后、write_artifact_file 前，子 agent 专属保持表尾）+ 适配器；计数 33→37/31→35；MCP 拼装契约 35+1"},
    {"id": "N4-6", "file": "src-tauri/src/prompts/system.rs", "line": 22, "fix": "追加规则 23：电脑辅助四工具使用时机 +「不做系统设置修改」边界声明（编号不动）"},
    {"id": "N4-7", "file": "docs/MCP-COMPUTER-USE-SETUP.md", "line": 1, "fix": "第 0 层接入指南：Peekaboo(macOS)/windows-mcp 系(Windows)/@playwright/mcp(浏览器,跨平台)——包名与版本经 npm 实查核实，Playwright MCP 本机 npx 冒烟通过（v0.0.83），最小权限 --caps=core 与先读后写原则留档"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_desktop.rs": 2,
    "src-tauri/src/bot/registry.rs": 2
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

电脑操控方向的第一批原生工具（调研结论：原生 Computer Use 需 VM 隔离 + 产品拍板，
不做；桌面/浏览器自动化经 MCP 通道接现成方案，接入指南随批交付）。

## 出界（显式不做）

- 鼠标/键盘/UI 自动化原生工具（第 3 层，官方不建议主机裸跑）
- AppleScript/pywinauto 白名单脚本注册制（第 2 层，等第 0 层真实需求验证）
- McpPanel 前端预设按钮（前端零改动；接入走设置页手动配置 + 指南文档）
