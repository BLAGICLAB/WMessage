# Batch Spec: U15-MEMORYCTRL

## 目的

记忆可控开关 + 导出导入（记忆升级第一期收官：B 开关部分 + F）。

- **开关**：bot-config.json 新增 `memoryControl` 块（`injectionEnabled` 注入
  总闸 / `autoWriteEnabled` 模型主动记忆门禁；serde default 全开，老配置零
  影响）。`io.rs` 增 `read_memory_control` 轻量读取（可测内核
  `read_memory_control_at`，同 `read_bypass_llm_switch` 先例；运行时泛型，
  `config_path` 随之泛型化）。门禁接线：`injection_block` 开头短路（关 =
  记忆保留但不发给模型）；`remember_fact` / `record_lesson` 入口返回 ok
  提示（非故障，模型不重试刷屏）。范围口径：autoWrite 只门禁模型主动写入，
  摘要/反思/定时整理等系统流水线不受影响。
- **导出导入**：`mem_export`（全量 JSON，向量随行带出；plugin-dialog save
  取路径 Rust 侧写文件；原子写临时文件 → rename）/ `mem_import`（解析、
  输入校验与预嵌入在锁外，导入走既有语义去重只增不删；kind 走
  EDITABLE_KINDS 白名单、source 三值契约 + 'user' 历史脏值归一，契约外计
  skipped；存储故障中止上抛不混入 skipped；版本守卫拒过新文件）。
- **前端**：记忆 section 增「记忆权限」双开关卡（role=switch + aria-checked，
  点档即时落盘）；MemoryPanel 头部增导入/导出按钮（结果提示 4s 自动清除）。

## ocr 复审处置记录（47 条 6H/23M/18L，json 于 docs/OCR-CODE-REVIEW-2026-10-03-u15.json）

- **H 修 3**：导入存储故障 `Err(_) => skipped` 把真实故障伪装成输入跳过 →
  改为中止上抛（已完成 N 条随报错带出）；集成场景三的全表计数断言会被并行
  进程写入误伤 → 改键位断言（find_by_key_tag「测试键」唯一可精确判定）；
  收尾清理只在成功路径执行 → 清理前移（每次运行开场先扫历史污染，panic 后
  下一次运行自愈）+ 收尾保留 + 清理失败 eprintln。
- **M 修 10**：预嵌入 ONNX 推理与导出序列化挪出 DB 写锁临界区（两段式：
  parse/prepare 锁外 → import_items 锁内）；导出改轻量返回 MemExportReport
  （count，1MB items 不走 IPC）+ 原子写（同配置文件写先例）；导入 kind/
  source 契约校验（白名单外 skipped，'user' 归一）；双开关补 role=switch +
  aria-checked（同厂商总开关语义）+ 开关行提取去重；ioMsg 4s 自动清除
  （ref 计时器 + 卸载清理，同 consolidateMsg 承诺）；对话框取消不再清上一条
  提示；read_memory_control 入 facade 再导出（config/mod.rs + bot.rs）；
  io 内核补半字段/空对象/显式 null 用例；损坏配置 stderr 告警；导入导出
  路径 .json 守卫。
- **L 修 6**：io.rs 模块文档补漏、泛型/具体签名分歧注释、门禁纯函数与 I/O
  包装注释区分、集成测试冗余 ensure_table 移除 + 种子时间戳注释 + 场景二
  结构化断言（- [fact] 行格式）、按钮顺序（导出/导入/刷新）、open 返回死
  分支收紧、U14 注释归位。
- **驳回/登记不修 8**：门禁每轮全量解析配置——与 read_bypass_llm_switch
  （每 bot_chat 入口全量解析）同成本先例，缓存引入失效复杂度；async 线程
  1KB 文件读——噪音级（同轮随即做数十 ms ONNX 推理），且同路径先例在先；
  config_path 泛型化属收窄非破坏（仓内无函数指针用法、无下游 crate）；
  乐观更新无回滚——setConsolidation/toggleTavily 既有模式族，失败有
  botError 横幅；存储故障注入单测——&Connection 无法注入故障，契约由
  「Err 上抛」类型表达；共享库并发污染残余风险——单测试合并 + 开场清理已
  把窗口压到最小，彻底隔离按 paths.rs 留档的 B3 方案另批处理；
  场景二仅子串断言——已补行格式结构断言；evo: 删除审计——与
  bot_clear_vendor_key 同口径（用户显式操作）。
- **不适用 10**：model-meta-service/main.py——untracked 未入库、U12 已
  拍板废弃的独立 Python 服务目录。

## 集成测试踩坑实录（供后续批次参考）

三个门禁场景最初按独立用例写，nextest 下 flaky：`db::data_dir` 在 cargo
test 下解析到 `target/debug/deps/`，其 bot-config.json 与 wmessage.db 被
**所有测试进程共享**（paths.rs「测试期的跨进程共享」既知地雷再次实证）。
处置：三场景合并为单测试（nextest 下即单进程）、断言增量/键位口径、开场
清理前移。彻底隔离按 paths.rs 留档的 B3（env override）另批处理。

## 红线核对

- 记忆数据零迁移零删除：开关只门禁注入与模型主动写入；导入只增不删。
- 系统写入流水线（摘要/反思/定时整理）不受 autoWrite 门禁影响。
- 老配置（无 memoryControl 字段）行为与 U15 前完全一致（集成 + 内核单测
  双覆盖）。
- 聊天主循环/Planner/摘要路径零改动（仅 injection_block 开头加短路判定）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U15-MEMORYCTRL",
  "family": "memory-control",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U15-MEMORYCTRL.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/tests/llm_integration.rs",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/MemoryPanel.test.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 1250,
  "max_lines_removed": 60,
  "max_new_files_lines": 150,
  "findings": [
    { "file": "src-tauri/src/memory/panel.rs", "note": "导入存储故障中止上抛不混入 skipped；解析/校验/预嵌入在锁外；导出轻量返回 + 原子写；kind/source 契约校验；路径 .json 守卫" },
    { "file": "src-tauri/src/bot/config/io.rs", "note": "read_memory_control 轻量读取（可测内核）；config_path 泛型化（仓内无破坏面）；损坏配置 stderr 告警按全开放行" },
    { "file": "src-tauri/src/memory/mod.rs", "note": "注入总闸 + 模型主动记忆门禁；autoWrite 只门禁模型主动写入，系统流水线不受影响；缺字段/读失败 = 全开" },
    { "file": "src-tauri/tests/llm_integration.rs", "note": "三场景合一单测试（共享 data dir 地雷，见 spec 实录）；键位断言 + 清理前移 + 收尾恢复" },
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "记忆权限双开关 role=switch + aria-checked，点档即时落盘；乐观更新沿用既有模式族（失败有 botError 横幅）" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（nextest 1345 / pytest 审计 / vitest 392）
cd src-tauri && cargo nextest run -E 'test(memory_gate) or test(read_memory_control) or test(memory::)'
npx --no-install vitest --run MemoryPanel SettingsPage   # 75 绿
```
