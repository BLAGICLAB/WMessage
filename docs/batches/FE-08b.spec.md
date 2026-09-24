# Batch Spec: FE-08b

## 目的

修 frontend 域「入口/主题/样式」簇（C5-FE-08b）可自主部分：main.tsx root 直转型 + main.css 悬停 transition 不对称。3 条拆分：2 实修 + 1 FP。

## 人类可读摘要

- family: frontend-entry-style
- 覆盖 findings: 3（2 实修 + 1 FP 零代码）
- 预估 diff: 2 改 + 1 新测试文件 / +12/-2（modified）+ 新文件 ~12
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核）

**实修 ① main.tsx:10-12（high）**：`document.getElementById("root") as HTMLElement` 直转型——root 缺失时 render 期裸抛，违反空值检查规则。
**修法**：取值 + 显式判空，缺失时 throw 带描述的 Error（fail-fast 带诊断信息）。

**实修 ② main.css:131-140（high）**：`.nm-card-hover:hover` 的 transition 含 `transform 0.2s ease`，基类 `.nm-card/.nm-card-hover` 只过渡 background-color/box-shadow——悬停进入平滑、离开瞬间弹回（不对称动画）。
**修法**：基类 transition 补 `transform 0.2s ease`。

**FP ③ theme.ts:33-41（high → FP）**：finding 称 applySetting 本地写不通知 in-process subscribers，组件会失同步。**核后**：subscribeTheme 语义即跨窗口同步（storage 事件只在其他窗口触发，JSDoc 明示「监听其他窗口」）；全仓订阅方仅 App.tsx:126 与 WidgetApp.tsx:142，两者各自就是本地写入的发起方（App.tsx:124 / WidgetApp.tsx:140 applySetting 由自身 state 驱动 effect 调用，写入路径已经过自身 setTheme）——不存在「非写入方的同窗口订阅者」，无从失同步。主窗口/挂件是独立 webview JS 上下文，跨窗同步正是 storage 事件职责。前提错误，零代码处置（reviewer pass 后才可标）。

## 测试

- `src/ui/main.css.test.ts`（新建，静态断言，仿 SettingsPage.test 的 css 读取模式）：基类 `.nm-card-hover` 块 transition 含 `transform 0.2s ease`；hover 块 transition 含 `transform 0.2s ease`（对称）
- ① main.tsx root 守卫为入口级 fail-fast，无组件测试挂点——spec 声明
- ③ FP 零代码

## 红线

- family 一致性：本批只含 frontend-entry-style
- ③ FP 必须 reviewer pass；不采纳则转实修（dispatch CustomEvent）
- 不碰 theme.ts 任何行（:84-95 legacy listener 与 :13-21 coercion 属其他行非本簇）、main.css 其它规则
- 不改后端任何文件

## spec 起草后自查三条

1. `expected_files`：main.tsx + main.css + main.css.test.ts（新）= 3 ✓
2. budget A 类：main.tsx +5/-2，main.css +1/-1，main.css.test.ts +12 ≈ +18/-3，上限 max +35/-10；新文件 ~12 行走 max_new_files_lines 30 ✓
3. findings 逐条 fix 字段列 ripple 文件+行号 ✓（2 实修 ripple 全文件内；App/WidgetApp 零改动）

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/FE-08b/,不喊人

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FE-08b",
  "family": "frontend-entry-style",
  "expected_files": [
    "src/main.tsx",
    "src/ui/main.css",
    "src/ui/main.css.test.ts"
  ],
  "max_lines_added": 35,
  "max_lines_removed": 10,
  "max_new_files_lines": 30,
  "findings": [
    {"id": "C5-FE-08b-1", "file": "src/main.tsx", "line": 10, "fix": "root 直转型 → 判空 + 描述性 Error fail-fast；ripple 无"},
    {"id": "C5-FE-08b-2", "file": "src/ui/main.css", "line": 131, "fix": "基类 transition 补 transform 0.2s ease（悬停进出对称）；ripple 无"}
  ],
  "assertions_min": {
    "src/ui/main.css.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

提交信息骨架

```
fix(fe): FE-08b — frontend-entry-style

【family】入口判空 / 悬停过渡对称
【实修 2 处】
- main.tsx:10 root 判空 + 描述性 Error
- main.css:131 基类 transition 补 transform（进出对称）
【FP 1 条】theme.ts:33 无同窗口非写入方订阅者（reviewer pass）
【自测】vitest + tsc + cargo fmt/check + test-all 全绿
【OCR】r1：exit <code> / <N> comments
【基线】D2: files=3(+A/-R, 新测试 +N) asserts=0→0 tests=vitest
```

验证命令

```bash
python3 scripts/batch-verify.py docs/batches/FE-08b.spec.md --tests
```
