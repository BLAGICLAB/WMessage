# Batch Spec: FE-08a

## 目的

修 frontend 域「挂件/看板」簇（C5-FE-08a）：DoneCircle type 缺失 + KanbanBoard drag rect 空引用 + TaskCardContent schedule 草稿被覆盖。3 条全实修。

## 人类可读摘要

- family: frontend-widget-kanban
- 覆盖 findings: 3（全实修）
- 预估 diff: 3 改 + 1 新测试文件 + 1 测试追加 / +30/-3（modified）+ 新文件 ~20
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核）

**实修 ① DoneCircle.tsx:13-17（high）**：`<button>` 无 type，form 内默认 submit。
**修法**：`type="button"`（与 FE-05 FoldToggle 同形态）。

**实修 ② KanbanBoard.tsx:179-184（high）**：`active.rect.current.translated ?? active.rect.current.initial` 直解引用——拖拽中元素被卸载时 `active.rect.current` 可为 undefined，属性访问在 onReorder 前抛错。
**修法**：`active.rect.current?.translated ?? active.rect.current?.initial`；`over.rect` 的 top/height 访问已有 truthy 守卫（ternary 判 `over.rect`），不动。

**实修 ③ TaskCardContent.tsx:293-297（high）**：⏰ 每次点击无条件 `setSchedOnce(scheduleToDatetime(task.schedule))`——用户打开面板输入新时间后误触/再点 ⏰ 收起，草稿被旧值覆盖。
**修法**：仅展开（closed→open）且草稿为空时初始化；收起不动草稿，草稿存续到提交/清除（取消定时会把 schedOnce 置空，之后重开自然重新初始化）。
**修法原文**：
```tsx
if (!schedOpen && !schedOnce) setSchedOnce(scheduleToDatetime(task.schedule));
setSchedOpen((v) => !v);
```
**行为变更**：误触 ⏰ 收起再展开，未提交的草稿仍在。

## 测试

- `src/components/DoneCircle.test.tsx`（新建）：type=button + 点击调 onToggle 且不外冒
- `src/components/TaskCardContent.test.tsx`（追加）：打开定时面板输入草稿 → 点 ⏰ 收起 → 再点 ⏰ 展开 → 草稿保留（旧代码此处回退为 schedule 值，翻的是缺陷行为）
- ② KanbanBoard 防御性空引用守卫无行为级测试（unmount-mid-drag 无法经 dnd-kit 测试 harness 复现）——spec 声明

## 红线

- family 一致性：本批只含 frontend-widget-kanban
- 不碰 KanbanBoard 拖拽排序逻辑本体、TaskCardContent 其它面板、DoneCircle 样式
- 不改后端任何文件

## spec 起草后自查三条

1. `expected_files`：DoneCircle.tsx + KanbanBoard.tsx + TaskCardContent.tsx + DoneCircle.test.tsx（新）+ TaskCardContent.test.tsx = 5 ✓
   【校正 1】OCR r1 high 同根因扩 2 文件：TodoCard.tsx 同款 ⏰ 无条件重置（镜像修法）+ 取消定时清草稿；TodoCard.test.tsx 镜像回归。= 7 文件。
2. budget A 类：DoneCircle +1/-0，KanbanBoard +2/-2，TaskCardContent +2/-1，DoneCircle.test +18，TaskCardContent.test +28 ≈ +51/-3，上限 max +70/-15；新文件 DoneCircle.test.tsx ~18 行走 max_new_files_lines 40 ✓
   【校正 1】实测 +84/-4（TodoCard.tsx +6/-1 + TodoCard.test +26 + TaskCardContent 取消清草稿 +1）。改为 max +100/-20。
3. findings 逐条 fix 字段列 ripple 文件+行号 ✓（3 实修 ripple 全文件内；caller TodoCard/WidgetApp/KanbanBoard 零改动——props 不变）

## OCR r1 处置（2 comments 同根因）

- **high TaskCardContent.tsx:297 采纳（扩 TodoCard.tsx 镜像修法）**：TodoCard.tsx:608-616 同款 ⏰ 无条件重置——两文件头注释互为镜像要求同步，修一处漏一处属同根因。TodoCard.tsx:614 同形态修 + TodoCard.test.tsx 镜像回归测试。
- **high TaskCardContent.tsx:366 采纳**：取消定时不清 schedOnce，`!schedOnce` 守卫下重开仍见陈旧草稿，与 spec ③ 修法注释矛盾 → 取消按钮补 `setSchedOnce("")`（TaskCardContent + TodoCard 两侧）。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/FE-08a/,不喊人

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FE-08a",
  "family": "frontend-widget-kanban",
  "expected_files": [
    "src/components/DoneCircle.tsx",
    "src/components/KanbanBoard.tsx",
    "src/components/TaskCardContent.tsx",
    "src/components/DoneCircle.test.tsx",
    "src/components/TaskCardContent.test.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/TodoCard.test.tsx"
  ],
  "max_lines_added": 100,
  "max_lines_removed": 20,
  "max_new_files_lines": 40,
  "findings": [
    {"id": "C5-FE-08a-1", "file": "src/components/DoneCircle.tsx", "line": 13, "fix": "button 无 type → type=button；ripple 无"},
    {"id": "C5-FE-08a-2", "file": "src/components/KanbanBoard.tsx", "line": 179, "fix": "active.rect.current 直解引用 → 可选链；ripple 无"},
    {"id": "C5-FE-08a-3", "file": "src/components/TaskCardContent.tsx", "line": 293, "fix": "⏰ 点击无条件重置草稿 → 仅展开且草稿空时初始化；ripple 无"}
  ],
  "assertions_min": {
    "src/components/DoneCircle.test.tsx": 0,
    "src/components/TaskCardContent.test.tsx": 0
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
fix(fe): FE-08a — frontend-widget-kanban

【family】挂件/看板小修（type 缺失 / drag rect 空引用 / 定时草稿覆盖）
【实修 3 处】
- DoneCircle.tsx:13 type=button
- KanbanBoard.tsx:179 rect 可选链守卫
- TaskCardContent.tsx:293 + TodoCard.tsx:614 定时草稿仅展开初始化 + 取消定时清草稿（镜像）
【自测】vitest + tsc + cargo fmt/check + test-all 全绿
【OCR】r1：exit <code> / <N> comments
【基线】D2: files=5(+A/-R, 新测试 +N) asserts=0→0 tests=vitest
```

验证命令

```bash
python3 scripts/batch-verify.py docs/batches/FE-08a.spec.md --tests
```
