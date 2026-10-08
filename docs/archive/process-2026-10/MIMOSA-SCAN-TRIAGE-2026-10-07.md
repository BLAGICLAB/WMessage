# Mimosa 扫描核查记录：py/document.rs 路径污点 advisory 判定误报

- Scan: `scan-2026-10-06T17-22-30.275Z-488c526b76a9`（seal `sha256:9749ba…2a84f`，depth=deep）
- Findings: 14 条（HIGH/MEDIUM 成对 × 7 处），全部指向 `src-tauri/src/py/document.rs`
  行 25/78/142/532/561/617/638，污点链「Web 请求输入 → eval/config.rs:64 load() 路径操作」
- 结论：**误报（analyzer artifact），无需代码改动**。核查日期 2026-10-07。

## 三层证据

1. **7 个「入口」全是 Python 字符串常量，不是 Rust 代码。** 逐行比对：7 个行号
   恰好是 `EXTRACT_SCRIPT` / `MAKE_DOCX_SCRIPT` / `MAKE_DOCX_REVISIONS_SCRIPT` /
   `MAKE_XLSX_SCRIPT` / `MAKE_PDF_SCRIPT` / `PDF_RENDER_SCRIPT` / `MAKE_PPTX_SCRIPT`
   七个 `r#"..."#` 内嵌脚本的第一条语句 `p = json.load(open('params.json', …))`。
   `params.json` 是 Rust 侧写入子进程工作目录的**固定相对文件名**，路径不受输入控制。
   分析器把内嵌 Python 当成了被分析的 Rust 源码。

2. **被链接的 sink 是生产死代码。** `eval/config.rs::load()`（`fs::read_to_string`）
   在 src 下无任何生产调用方（grep 仅命中其自身单测；`eval/runner.rs:239` 只引用了
   `RunFrequency` 类型）。即便调用，路径来自 `default_config_path()` = 固定相对
   `bot-config.json`。「名字叫 load 的跨文件污点链」是 `json.load`（Python 字符串里）
   与 Rust `load` 的名字错配。

3. **「Web 请求输入」源标签不成立——文档工具链的真实路径防线齐全。**
   - `tool_extract_document`（bot/tools.rs:1221）：path 白名单（任务卡绑定文件 /
     AI_Gen_Files 静默放行）→ `bot_fs::resolve_with_perm`（allowed_dirs + perm_mode）。
   - `tool_create_word` 的 images：`sanitize_image_paths_arg` 仅放行 AI_Gen_Files
     内已存在图片，被拒审计（N3-4，防模型借参数探测/读任意路径）。
   - 模型来源的任务卡 files 绑定：`sanitize_task_files_arg` 同口径收窄 fail-closed。

## 处置

- 不改代码、不入 triage 基线（非真实弱点）。后续扫描若复报同一 anchor
  （`sha256:d575d5b1…` / `sha256:8f396b2f…`），凭本记录直接判误报。
- 扫描 run status=inconclusive（静态、调用图部分不完整），verdict effect=none，
  与本结论一致。

---

# Mimosa 重跑记录：2026-10-08 — 16 条全部误报（14 凭记录 + 2 新增给据）

- Scan: `scan-2026-10-08T04-19-30.884Z-5d82a2c565c6`（seal `sha256:c807682b…`，
  depth=deep），**本次完整跑完**——此前多轮 hook 报 `scanner_enobufs` 的问题未复现。
- 依赖扫描：1120 包全部完成，离线 advisory 0 命中。

## 构成与处置

1. **py/document.rs 七锚点（25/78/142/532/561/617/638）HIGH/MEDIUM 成对 ×7 = 14 条**
   ——与 10-06 轮完全同锚点，凭上文记录直接判误报（内嵌 Python 字符串被当
   Rust 源码 + `eval/config.rs::load()` 生产死代码）。
2. **新增 gui-test-screenshots/graph-demo-entry.tsx:89 MEDIUM — 误报**。
   `stress` URL 参数仅经 `Number()` 强转后作数量用，本文件无 innerHTML；
   所称汇点 `AI_Gen_Files/lists.js:147` 是沙箱 AI 生成演示产物（本次扫描时
   已被清理、路径不存在），且 graph-demo 治具不在生产 bundle（src/ 零引用，
   仅截图治具用）。
3. **新增 src/main.tsx:36 MEDIUM — 误报**。该行是
   `ReactDOM.createRoot(rootEl).render(...)`，无 URL 输入、无 innerHTML；
   所称汇点同为不存在的 `lists.js:147`——分析器把生产入口与沙箱产物跨文件
   错链。

## 处置

- 不改代码、不入 triage 基线。后续扫描若复报上述锚点，凭本记录直接判误报。
- run status=inconclusive（静态 advisory 性质），verdict effect=none，
  与上轮一致。Mimosa hook 的「尽快重跑完整审计」事项至此闭环。
