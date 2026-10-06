# Batch Spec: N3-TOOLPOLISH 非任务卡工具六项升级

```json
{
  "batch_id": "N3-TOOLPOLISH",
  "family": "bot-tools",
  "expected_files": [
    "docs/batches/N3-TOOLPOLISH.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/ocr.rs",
    "src-tauri/src/py/document.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/tests/fixtures/tools_baseline.json"
  ],
  "max_lines_added": 520,
  "max_lines_removed": 150,
  "findings": [
    {"id": "N3-1", "file": "src-tauri/src/bot/tools.rs", "line": 1554, "fix": "fetch_url 加 offset 续读：纯函数 slice_fetch_output（无 offset 且不超限 = 旧输出零变化；截断带 offset 提示；offset 越界给总量提示）"},
    {"id": "N3-2", "file": "src-tauri/src/py/document.rs", "line": 64, "fix": "扫描版 PDF 兜底：EXTRACT_SCRIPT 文本层近空 → PDF_RENDER_SCRIPT（PyMuPDF 官方推荐，2x zoom 渲染，前 20 页）逐页 PNG → ocr::recognize_bytes 拼接；pymupdf 缺失优雅降级给 pip 指引"},
    {"id": "N3-3", "file": "src-tauri/src/py/document.rs", "line": 536, "fix": "MAKE_PDF_SCRIPT 重写为 platypus（官方推荐表格路径）：Paragraph(CJK wordWrap) + Table(Table Grid + 表头加粗 + 斑马纹) + 自动分页；create_pdf 加 tables 参数"},
    {"id": "N3-4", "file": "src-tauri/src/py/document.rs", "line": 74, "fix": "MAKE_DOCX_SCRIPT：段落 #/##/### 前缀 → Heading 1/2/3（黑体）；images 参数（Rust 侧校验仅 AI_Gen_Files 内已存在文件，被拒条目审计）→ add_picture"},
    {"id": "N3-5", "file": "src-tauri/src/bot_web.rs", "line": 165, "fix": "web_search 加 count(1..=10)/time_range(day|week|month|year)/site：Tavily→max_results+time_range+include_domains；Brave→count+freshness(pd/pw/pm/py)+site: 追加；Bing+百度→site: 追加、time_range 显式提示不支持；纯函数映射可测"},
    {"id": "N3-6", "file": "src-tauri/src/bot_fs.rs", "line": 639, "fix": "grep_files 加 context(0..=5)：命中行上下文渲染（path:line: 命中 / path-line- 上下文，块间 -- 分隔，重叠合并），纯函数可测；默认 0 = 旧输出零变化"},
    {"id": "N3-7", "file": "src-tauri/src/bot/registry.rs", "line": 170, "fix": "五处 schema 扩参（fetch_url/web_search/grep_files/create_pdf/create_word）+ extract_document 描述补扫描件兜底；baseline 重生成"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/tools.rs": 3,
    "src-tauri/src/bot_web.rs": 3,
    "src-tauri/src/bot_fs.rs": 2,
    "src-tauri/src/ocr.rs": 1
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

任务卡组（T1）之外的 23 个工具中，内容获取与产出两组存在六处真实短板：
fetch_url 长文截断无续读、扫描版 PDF 提取为空、create_pdf 无表格、create_word 无标题
层级与图片、web_search 无过滤参数、grep_files 无上下文行。本批一次补齐。

## 关键决策留档（官方推荐 > 最小依赖 > 复用仓内既有）

1. **PDF 页转图 = PyMuPDF（fitz）**：官方推荐的事实标准渲染库，全平台 pip 轮子零系统
   依赖；与仓内 pypdf / python-docx「缺失优雅降级」先例同级。否决 pdfium-render
   （需随包分发 pdfium 动态库，违背最小依赖）与 macOS sips（仅首页）。
2. **create_pdf 表格 = reportlab platypus**：reportlab 官方推荐的表格路径（Table
   flowable），顺带解决长文自动分页；字体沿用 STSong-Light CID（零字体文件依赖）。
3. **时间过滤映射**：Tavily `time_range`（day/week/month/year 原生）+ Brave
   `freshness`（pd/pw/pm/py）——官方 API 直查（2026-10 核对）；Bing/百度抓取链路
   显式提示不支持（诚实降级，不静默丢弃）。
4. **OCR 复用**：识别沿用既有 `ocr::recognize` 双引擎（macOS Vision / PP-OCRv6），
   仅加 pub(crate) 入口；隐私红线不变（字节全本地）。
