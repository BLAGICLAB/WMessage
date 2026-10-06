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
