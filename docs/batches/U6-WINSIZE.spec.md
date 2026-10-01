# Batch Spec: U6-WINSIZE

## 目的

老板需求（单点配置批）：主窗口默认尺寸太小，宽高各增加一半——
`tauri.conf.json` 主窗口 800×600 → **1200×900**。

- 漂移核对：主窗口尺寸唯一来源即该配置（Rust 侧 WebviewWindowBuilder 仅
  挂件触发条 `inner_size(44.0, 220.0)` 与测试 mock 窗口，无第二处硬编码）。
- 挂件窗口（触发条/面板，WidgetApp 运行时自管 size/position）零改动。
- 不设 min 尺寸/居中等其他属性（用户未要求，保持最小改动）。

## 测试

- 配置批：test-fast 走 cargo fmt/check + 桥一致性（src-tauri/ 前缀触发）；
  无行为代码变更，窗口尺寸由 tauri 运行时读取，无既有断言涉及。

## spec 起草后自查三条

1. expected_files = staged 全集 2 项（含本 spec）。
2. 预算：+3/-2（纯数值改动）；无新文件（本 spec 为新文件计入预算）。
3. findings 留空（单点配置批，无复审意见）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U6-WINSIZE",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U6-WINSIZE.spec.md",
    "src-tauri/tauri.conf.json"
  ],
  "max_lines_added": 20,
  "max_lines_removed": 10,
  "max_new_files_lines": 80,
  "findings": [],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
cd src-tauri && cargo check   # 配置 JSON 由 tauri 构建期解析，check 过即合法
```
