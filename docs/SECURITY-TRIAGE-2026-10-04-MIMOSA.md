# Mimosa 深度扫描人工复核（2026-10-04）

- **复核对象**：scan `scan-2026-10-03T22-39-53.951Z-3cb329a8fe74`
  （seal `sha256:1f28d55354c4a76b85686b407bdae4a151d88ec681ec10b3b5bf347b7a0b225f`，
  产物 `~/.mimosa/security-scans/project-35c8c4f5947b2bbdf573b390/scan-2026-10-03T22-39-53.951Z-3cb329a8fe74/`）
- **背景**：U20-EVOGOV 提交推送时扫描器资源不足（scanner_enobufs）未出结论，本扫描为补跑；
  本文档是对 14 条 findings 的逐条人工数据流复核（只读求证，不改生产行为）。
- **方法**：告警点全量通读 + 被指认污点汇的全仓调用穷尽 + 工具面授权链核对，证据均给 file:line。

## 结论总览

14 条 findings：**12 误报 + 1 不适用 + 1 真实但已随废弃目录归档处置**。
未发现本仓可达的可利用路径。依 Mimosa 结论纪律：本报告不构成「项目无风险」判定，
后续引入新污点面应重新扫描。

| # | 告警 | 定级 | 复核结论 |
|---|------|------|----------|
| 1–12 | `py/document.rs` 25/77/127/517/535/567（HIGH+MEDIUM 成对）跨文件污点 → `eval/config.rs:64` path-traversal | 高/中 | **误报**（污点汇归因错误 + 调用边不存在，三重证据见下） |
| 13 | `target/doc/static.files/search-*.js` MongoDB 动态排序注入 | 中 | **不适用**（构建产物，非手写源码；技术栈无 MongoDB） |
| 14 | `model-meta-service/main.py:56` SSRF | 高 | **真实但目录已废弃未入库**——已归档移出工作区（见「处置」） |

## 一、12 条 document.rs 告警的证伪（三重证据）

### 证据 1：告警行全部位于 Python 脚本字符串常量，不是 Rust 代码路径

6 处告警行恰好是 6 个 `pub const *_SCRIPT: &str = r#"…"#` 内嵌 Python 的同一行
`p = json.load(open('params.json', encoding='utf-8'))…`：

| 告警行 | 所在常量 |
|--------|----------|
| document.rs:25 | `EXTRACT_SCRIPT` |
| document.rs:77 | `MAKE_DOCX_SCRIPT` |
| document.rs:127 | `MAKE_DOCX_REVISIONS_SCRIPT` |
| document.rs:517 | `MAKE_XLSX_SCRIPT` |
| document.rs:535 | `MAKE_PDF_SCRIPT` |
| document.rs:567 | `MAKE_PPTX_SCRIPT` |

这些字符串由 `run_python` / `run_dotnet_revisions`（document.rs:1076 / 1124）交给外部
Python 解释器执行；`'params.json'` 是**固定字面量文件名**（Rust 侧把命令参数 serde_json
序列化后写入临时工作目录，脚本按相对名读回）。Rust 层对该文件名没有任何路径操作，
不存在「不可信路径 → 文件操作」的 Rust 污点链。扫描器把内嵌 Python 的
`json.load(open(...))` 与仓内同名 Rust 函数 `eval::config::load` 同型配对，属名称级误配。

### 证据 2：被指认的污点汇 `eval::config::load` 全仓唯一调用方是 CLI 开发工具

- `eval/mod.rs:23`：`pub use config::{load as load_config, …}`；
- 全仓穷尽 grep（src + tests + bin）：唯一调用点 = `src-tauri/src/bin/eval_run.rs:78`，
  `config_path` 来自命令行参数（开发者手动运行），**不在任何 Tauri 命令 / HTTP 面 /
  LLM 工具面可达**；
- `py/document.rs` 全文无 `eval` 引用（无 use、无调用）——告警声称的「跨文件污点」
  调用边不存在。coverage.json 自证了这一点：gaps =「调用图部分不完整：部分调用为
  动态派发或超出分析规模，跨文件可达性可能不完整」。

### 证据 3：真实的文件读取面另有其处，且授权链完整（与告警无关，为完整性核实）

文档工具确实存在「模型/用户提供的路径 → 读文件」的真实数据流，逐环核对如下：

- `extract_document` 是 LLM 可调工具（`bot/tools.rs:1029` `tool_extract_document`），
  `path` 参数来自模型输出——这是附件/文档功能的设计路径；
- 授权链：`extract_path_check`（`bot/tools.rs:998`）→ `canonicalize`（不存在即拒）→
  AI_Gen_Files 目录 / 任务卡绑定文件静默放行 → 其余走
  `bot_fs::resolve_with_perm`（`bot/tools.rs:1023`）四档分流：白名单静默放行 /
  strict 拒 / ask 弹窗授权 / yolo 放——与全部文件工具同一口径；
- `create_word_revisions` 的 `originalPath` 同样过 `extract_path_check`
  （`bot/tools.rs:1153`，注释明示「防回读任意文件」）；
- 非交互场景无 path 直接拒绝（`bot/tools.rs:1050`，防后台执行弹框永久阻塞）；
  全链审计留痕（`py_audit`，document.rs:813/870/918/934/958/1014）；
- 产物路径硬化：`gen_out_path_in`（document.rs:1035）对用户提供的 filename 取
  `Path::file_name()` 剥离目录分量后落 AI_Gen_Files，重名递增不覆盖——输出侧
  无目录穿越面。

残余面登记（非漏洞）：yolo 授权模式下模型可直读白名单外文件——这是 yolo 档的
显式语义（用户全权委托），与 bot_fs 全仓口径一致，非绕权路径。

## 二、其余 2 条

### target/doc 构建产物（#13，MEDIUM）

`src-tauri/target/doc/static.files/search-*.js` 是 rustdoc 生成的静态资源（构建产物，
非手写源码）；本仓技术栈无 MongoDB。`cargo clean` 即消失，无需处置。

### model-meta-service SSRF（#14，HIGH）

`main.py:56` `httpx.get(MODELS_DEV_API, …)` 的代码事实成立（URL 常量拼接、无协议/
主机/IP 边界校验）。但该目录是 **U12 已拍板废弃**的独立 Python 服务、untracked
未入库（U15/U16/U18/U19 历批同口径不随仓库处置）。

**处置（已执行）**：归档至 `~/Projects/wmessage-attic/model-meta-service-2026-10-04.tar.gz`
（内容完整性已验证：main.py / README.md / requirements.txt）后移出工作区。可随时
恢复；若将来复活该服务，须先补 URL 校验（协议白名单 + 解析后 IP 边界，同
bot/config/io.rs `base_url_is_safe` 口径）再接入。

## 覆盖度与边界

- coverage.json：`completeness=partial`、`runStatus=inconclusive`；514 个源文件全部
  解析、零截断零失败；依赖扫描 1069 包、advisory 0 命中。
- 本报告即对「跨文件可达性不完整」缺口的人工补证：12 条告警的调用边经人工追踪证伪，
  真实读取面的授权链经人工核实完整。
- 扫描与复核均为静态分析；运行时可利用性论证不在证据边界内（evidenceBoundary =
  static_only_no_runtime_execution）。
