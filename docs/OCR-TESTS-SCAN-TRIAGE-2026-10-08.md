# OCR 测试代码扫描分诊（2026-10-08）

- 扫描：`ocr scan` 全文件模式，覆盖 77 个测试文件（前端 `*.test.*` 59 +
  `src-tauri/tests/*.rs` 10 + `tests-audit/*.py` 8），产出 87 条 findings
  （critical 1 / high 30 / medium 33 / low 23）。原始报告：
  `docs/OCR-CODE-REVIEW-2026-10-08-tests.json`。
- **前端 59 个 test 文件 0 finding**。
- 判定基线：测试代码只有造成 flaky / 误绿（失败被吞）/ 资源泄漏的才算 REAL。
- 逐条核查 43 条（critical 1 + high 30 + medium bug/security/performance 12），
  其余 44 条为建议性存量（medium 建议类 21 + low 23，按文件聚类见 §3）。

## 判定统计

| 判定 | 数量 | 说明 |
|------|------|------|
| REAL | 8 | 见 §2，全部给最小修法 |
| PARTIAL | 9 | 机制属实但当前无命中/无 flake 向量，顺手修 |
| FALSE_POSITIVE | 4 | edition/ownership/进程模型误读 |
| ALREADY_HANDLED | 2 | 防线已在（apply.rs 512 维确定性单测 / mock_handle 文档注明） |
| WONTFIX | 20 | 威胁模型外/响亮失败非误绿/记录在案的取舍 |

## REAL / PARTIAL 工单（17 条，下批修复）

**误绿向量（最高优先）**
1. `src-tauri/tests/exec_trace.rs:278-366`（REAL）：sink 管道测试用固定
   `call_et_1`/`/a/x.py` 标识、无 panic 守卫、清理 `.ok()`——上次运行 panic
   残留行可让本次「根本没写库」也通过。修法：标识加 per-run uuid + 复用
   TraceCleanupGuard。
2. `src-tauri/tests/task_chat_exec.rs:326-333`（REAL）：共享库 + 固定
   `⏰ 定时：` 标题前缀取「最新行」，失败回复落库回归可被残留行误绿。
3. `src-tauri/tests/task_chat_exec.rs:344-358`（REAL）：同上，按标题 DELETE
   不 scope 到本会话。修法（2+3 同一处）：uuid 后缀标题或按 task_id 反查 sid。

**守卫洞（tests-audit）**
4. `tests-audit/audit_module_map.py:119-121`（REAL）：`f.name not in doc`
   裸 basename 兜底 + 15 组同名文件 → 文档漂移可静默漏登记。修法：只认相对路径。
5. `tests-audit/audit_module_map.py:88`（REAL）：硬编码 5 个 mod.rs，实存 18 个。
   修法：`SRC.rglob("mod.rs")` 动态发现。
6. `tests-audit/audit_evolution_layering.py:83`（REAL）：规则 3 漏 group use
   `evolution::{strategy,…}` → 数据层违规漏报（核心洞）。补
   `evolution::\{[^}]*\bstrategy\b` 模式。
7. `tests-audit/audit_evolution_layering.py:76`（REAL）：规则 2 漏
   `impl<T> EvolutionPolicy` 泛型形态。放宽为 `impl\b[^;{]*?\bEvolutionPolicy\b`。
8. `.gitignore`（REAL，memory_eval #79 连带）：只忽略
   `fixtures/memory_extract_sample.jsonl`，中断残留 `fixtures/*.jsonl.tmp`
   （含真实用户记忆）不在忽略内，`git add .` 会带入。加一行。

**PARTIAL（顺手修）**
9. `tests-audit/audit_no_eval.py:39`：ALIAS_RE 会误伤 `x: Function` 类型注解
   （当前零命中）。收紧为 `\b\w+\s*=\s*Function\b\s*(?!\()`。
10. `tests-audit/audit_no_eval.py:38`：三分支缺 `\b`，会子串命中
    `window.evaluation`（当前零命中）。补 `\b`。
11. `tests-audit/audit_evolution_layering.py:185`：cfg(test) 剥离吞换行 →
    违例行号偏移（诊断性）。替换时透传 `\n`。
12. `tests-audit/audit_evolution_layering.py:172`：正则不认 `pub mod tests`/
    夹层属性（当前全库裸写法）。放宽为 `#\[cfg\(test\)\][^{}]*?mod\s+\w+\s*\{`。
13. `src-tauri/tests/llm_integration.rs:180-182, 835-837`：两处 50ms sleep 死代码
    （计数先于响应写回，无 flake 向量），删。
14. `src-tauri/tests/bot_test_connection.rs:111-118`：bind:0→drop→connect 的
    TOCTOU 极窄竞口。改探 `http://127.0.0.1:1/v1`。
15. `src-tauri/tests/exec_trace.rs:65-83`：cleanup 九连 DELETE `.ok()` 吞错
    （uuid 隔离不串断言，仅共享库堆积）。首个非 OK 错误 `eprintln!` 浮出。
16. `src-tauri/tests/skill_e2e.rs:342-361`：手抄 skill_outcomes DDL 漂移面。
    低优先：抽 `include_str!` 单源化（参考 db/mod.rs:165 subagents 表模式）。

## WONTFIX（有据，择要）

- error_codes/mock_llm 的截断类：触发条件 rustfmt 格式下不可达，后果是响亮
  FAIL 非误绿（#10/#11/#42/#43）。
- mock_llm 串行 accept（#41）：全部用例顺序请求，无掩盖面；加并发用例时再改。
- memory_conflict `.ok()` 清理（#47）：语料固定 + 开场清理前移自愈，失败响亮。
- skill_e2e std Mutex 跨 await（#71）：文件头记载的既定取舍（current_thread
  runtime 串行化）。
- memory_eval 写 0644（#77）与 macOS-only（#78）：用户自己 HOME 下同权限库，
  无新增暴露面；`#[ignore]` 手动工具按设计只支持 macOS。
- evolution_gov 防劫持闸（#62）：ALREADY_HANDLED——apply.rs:604-637 已有 512 维
  确定性单测无条件覆盖。
- task_chat_exec 挂死类（#65）：断言 panic 先于 first.await 传播，runtime 关停
  时 oneshot dropped 自然散，不成立。
- 其余 11 条见原始 JSON（tests-audit 前置 assert 在 -O 下被剥、r## 界符、
  子串从宽等，均「潜在不成立或失败方向响亮」）。

## 建议性存量（44 条，不机械过）

medium 建议类 21 + low 23。分布：memory_eval 7、audit_pre_step 3、
error_codes 3、mock_llm 3、memory_conflict 3、bot_test_connection 3、
exec_trace 3、evolution_gov 3、task_chat_exec 3、audit_tauri_bridge 2、
audit_bot_tools_alignment 2、regen_tools_baseline 2、llm_integration 2、
skill_e2e 2、memory_v2_degraded 1、audit_evolution_layering 1。
值得顺手带走的两条（下批可带）：mock_llm/llm_integration 的 sleep+前置断言
同模式冗余批删（#36/37/46）；memory_eval recall@5 隐性耦合 top_n==5 加一行
debug_assert 锁死（#84）。
