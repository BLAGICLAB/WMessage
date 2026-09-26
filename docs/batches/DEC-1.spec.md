# Batch Spec: DEC-1

## 目的

9 项已拍板决策中的两项代码批：C4-v2（widget 高度矛盾，拍板 A 底部放开）+ S20（py setrlimit 静默吞，拍板 A 探测降级）。其余 7 项零代码（登记走 docs 批）。

## 人类可读摘要

- family: decision-fixes-r1
- 覆盖: C4-v2 实修 + S20 实修（两项均为用户拍板 2026-09-26）
- 预估 diff: 4 文件 / +318/-24（fmt 后实测校正 ×2：×1 预算、×2 OCR r1 采纳探测值改生产上限）
- OCR 计划: r1（S20 涉沙箱/权限面；C4-v2 为 UI 常量随批）

## 逐条处置（用户拍板 2026-09-26）

**① C4-v2 — WidgetApp/constants.ts:13**：`PANEL_H_MIN` 800→400（`PANEL_H_MAX` 900 与默认 `PANEL_H=560` 不动），可调区间变为 400–900。
机制：MIN 的唯二消费点是 `storage.ts:53` loadSize 校验与 `WidgetApp.tsx:378` resizePanel 钳制，旧值 800>默认 560——存量/默认尺寸落在非法区间，首次 resize 即被强钳到 800，可调区间仅 100px。MIN=400 后默认 560 回到合法区间，矛盾消除。400 为暂行值（老板拍板：别纠结，后续改成本极低）。

**⑤ S20 — py/runtime.rs pre_exec 段**：`libc::setrlimit` 失败旧实现静默吞 →「限额看似生效实则没生效」；硬失败方案此前实测打断 py_exec（P3S-20 FIX→回退登记）。**spec 中途实测修正**：裸 macOS 内核本来就拒 `setrlimit(RLIMIT_AS)`（CPython 把 EINVAL 映射成 ValueError），只有 CPU 一直可设——即生产在 macOS 上 AS 限额从未生效过。整体可用/不可用设计会把本来有效的 CPU 限额一起丢掉，故改**逐资源探测降级**：

- 首次 py 执行前，用同一解释器子进程探测一次 `setrlimit(RLIMIT_AS, 256MB)` 与 `setrlimit(RLIMIT_CPU, 70)`（捕获 OSError + ValueError——CPython 的 EINVAL 走 ValueError），进程内 `OnceLock` 缓存 `SetrlimitSupport`（可用性是 OS/沙箱属性，与具体解释器实例无关，py 路径失效不重探）。
- 逐资源三态：`Available`（该资源照设）/ `Unavailable(原因)`（不设，每运行 audit 留痕「限额未生效(原因)」）/ `ProbeError(原因)`（探测自身失败：spawn/超时/输出缺项/不可解析——**硬约束：探测失败 ≠ 可用**，不设 + 每运行 audit「未探测到 setrlimit 可用性，限额未设(原因)」，绝不允许静默当可用）。
- 部分可用（裸 macOS 典型态：AS 拒 + CPU 可）只设可用项——不丢本来有效的 CPU 防护。
- `process_group(0)` 与限额解耦：任何分支都必须设（kill_tree 进程组杀依赖）。
- 探测可用后 pre_exec 内再失败（环境突变极端组合）仍 best-effort 静默，不回退硬失败（S20 历史教训）。
- 限额缺席时的兜底不靠 rlimit：timeout 强杀 + 父进程看门狗 + 进程组 kill 仍在（unix_scripts_get_parent_watchdog 回归锁锚定的机制不受本批影响）。

## 测试

- frontend：storage.test.ts 新增 loadSize 高度区间 describe（默认 560 在区间内回归钉 + 400/900 边界 + 399/901 越界 + 宽度区间不受影响）。
- Rust：bot_py.rs tests 新增 3 测——探测输出解析三态（纯函数）/ rlimit_warn_line 硬约束文案钉死（纯函数）/ 真实 python 探测给确定结论（detect_python 守卫，无环境跳过）。
- 既有 run_python_at 全链路测试（grandchild/timeout）随批验证探测接线不破坏原行为。

## 红线

- kill_tree / ChildRegGuard / RunLimits 逻辑一字不动（runtime.rs 文件头约束）
- 不动 run_python / run_python_ungated 函数体结构（exiting_check_is_after_gate_lock / unix_scripts_get_parent_watchdog 两把源码回归锁锚定其原文）
- 不回退硬失败；Windows Job Object 路径不动
- PANEL_H / PANEL_H_MAX / PANEL_W_* 不动

## spec 起草后自查三条

1. expected_files 4（constants.ts / storage.test.ts / runtime.rs / bot_py.rs，全路径已列）
2. budget +340/-30（fmt 后实测 +314/-24：runtime.rs 探测块 +200/-21，bot_py 测试 +74，storage.test +36/-2，constants +4/-1；校正 ×1）
3. fix 字段：全部函数内闭环，无跨文件签名变更

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "DEC-1",
  "family": "decision-fixes-r1",
  "expected_files": [
    "src/components/WidgetApp/constants.ts",
    "src/components/WidgetApp/storage.test.ts",
    "src-tauri/src/py/runtime.rs",
    "src-tauri/src/bot_py.rs"
  ],
  "max_lines_added": 340,
  "max_lines_removed": 30,
  "findings": [
    {"id": "C4-v2", "file": "src/components/WidgetApp/constants.ts", "line": 13, "fix": "PANEL_H_MIN 800→400（底部放开，区间 400–900，默认 560 回到合法区间）；MAX/默认不动"},
    {"id": "S20", "file": "src-tauri/src/py/runtime.rs", "line": 461, "fix": "setrlimit 逐资源探测降级：子进程一次探测（RLIMIT_AS/CPU，捕获 OSError+ValueError）+ OnceLock 缓存 SetrlimitSupport 三态（Available 照设 / Unavailable 不设+audit 限额未生效 / ProbeError 不设+audit 未探测到 setrlimit 可用性）；部分可用只设可用项；探测失败≠可用；process_group(0) 与限额解耦保留"}
  ],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/DEC-1.spec.md python3 scripts/batch-verify.py docs/batches/DEC-1.spec.md
```
