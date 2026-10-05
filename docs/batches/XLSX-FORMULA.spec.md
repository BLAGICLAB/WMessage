# Batch Spec: XLSX-FORMULA Excel 生成公式注入过滤落地——修空操作

```json
{
  "batch_id": "XLSX-FORMULA",
  "family": "doc-gen",
  "expected_files": [
    "docs/batches/XLSX-FORMULA.spec.md",
    "DEVLOG.md",
    "src-tauri/src/py/document.rs"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 5,
  "max_new_files_lines": 80,
  "findings": [
    {"id": "XLSX-F-1", "file": "src-tauri/src/py/document.rs", "line": 525, "fix": "MAKE_XLSX_SCRIPT 公式过滤原为空操作（条件两边都是 v）：= 开头的模型输出被 openpyxl 存成活公式（WEBSERVICE/DDE 注入面）。改为 ws.append 后按行把 data_type=='f' 的单元格翻回 's'——原样显示为文本不执行，数字/空值（data_type 'n'/None）不受影响；openpyxl 3.1.5 实测重载 type=s 且原始 XML 无 <f> 节点"},
    {"id": "XLSX-F-2", "file": "DEVLOG.md", "line": 5, "fix": "补 XLSX-FORMULA 条目：背景（Mimosa 12 条误报复核时发现）+ 实现 + 验证"}
  ],
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

Mimosa 深度扫描的 12 条 path-traversal advisory 人工复核全部为误报
（document.rs 六个内嵌脚本的真实数据流均有闸：模型可控读路径过
extract_path_check 白名单 + strict/ask/yolo 分流，输出路径 gen_out_path
基名消毒落 AI_Gen_Files；sink「eval/config.rs:64 load」唯一调用方是
eval_run CLI，无 web 来源）。复核中发现 MAKE_XLSX_SCRIPT 第 525 行
`v if not (...) else v` 两侧相同——公式过滤从未生效。本批落地：
`= ` 开头单元格翻回字符串类型，保留原文显示、杜绝公式执行。
