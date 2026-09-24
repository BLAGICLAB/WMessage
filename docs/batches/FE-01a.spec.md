# Batch Spec: FE-01a

## 目的

修 frontend 域「core 写入加载路径」簇的可自主部分：diffTaskRows 就地突变 + 导出/导入静默返 0 + loadWorkspaceFromDb 静默返 [] + tasks-updated 守卫漏 bot/api 协议值。（FE-01 原 8 条拆分：4 实修 + 2 FP + 2 转 B 类）

## 人类可读摘要

- family: core-io-contract
- 覆盖 findings: 8（4 实修 + 2 FP 零代码 + 2 转 B 类攒批）
- 预估 diff: 5 files（4 改 + storage.test.ts 加测试）/ +95/-35 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4
- **本批为第 55 批 → reviewer 抽查适用（上次第 50 批 SC-04）**

## 逐条处置（原文均已核）

**实修 ① storage.ts:109（high）**：diffTaskRows 就地突变 caller 的 next 对象（写 updatedAt/expectedUpdatedAt），破坏 React state 引用相等假设。
**修法**：upserts 构造改为产出新对象——内容变化行 `{ ...t, updatedAt: now, expectedUpdatedAt: p?.updatedAt }`；纯排序行 `{ ...t, expectedUpdatedAt: p.updatedAt }`（updatedAt 保留原值）。语义逐项对齐现有 storage.test.ts 的 diffTaskRows 测试（纯排序保戳 / RMW 基线 / 新任务无基线），新增「输入对象不被突变」测试。

**实修 ② storage.ts:62（high）**：exportTasksToFile / importTasksFromFile / exportWorkspaceToFile / importWorkspaceFromFile 失败返 0（与「真 0 条」不可区分）。
**修法**：失败改 `throw`（去掉自带 handleCommandError——4 个 caller（App.tsx:416/434/458/476）本就 try/catch + handleCommandError + onRetry，保留 storage 侧 alert 会双弹）。与文件内 upsert/delete 的「不静默」约定对齐，alert 职责在 caller。

**实修 ③ storage.ts:122（high）**：loadWorkspaceFromDb 失败静默返 []（读失败 ≈ 空库，可触发错误 fallback）。
**修法**：对齐 loadTasksFromDb 的判别式结果——新增 `LoadWorkspaceResult = {ok:true, items} | {ok:false, error}`。ripple：
- WorkspacePage.tsx:70 `loadWorkspaceFromDb().then(setItems)` → `res.ok` 才 setItems（失败保持旧数据，不再被 [] 冲掉）
- WidgetApp.tsx:191 `const list = await ...` → `res.ok` 判定，失败 throw 进既有外层 catch（行为同现状：忽略，但不把失败当空列表）

**实修 ④ App.tsx:291（high）**：tasks-updated 守卫只豁免 "initial-load"/"migration"，但后端协议（src-tauri/src/mutation.rs MutationOrigin + bot/tools.rs:157 实发 `source:"bot"` + api_handlers/commands.rs:116 实发 `source:"api"`）声明 Bot/Api 已由后端落盘——现状 bot/api 事件落入回写分支，:302 注释自己警告的「旧快照覆盖后端新写入（归档/软删被回滚）」竞态是**活的**。
**修法**：豁免集合扩为 `{migration, bot, api, initial-load}`（initial-load 无实际 emitter 但保留兼容）；未知 source 维持 WARN + 按未落盘处理（宁可多写不丢数据，既有防御姿态不动）。这是**实现既有文档化协议**，不是新方向。

**FP ⑤ storage.ts:113（high → FP，reviewer agent-12 pass）**：finding 声称 expectedUpdatedAt 未声明在 Task 上需 cast 逃逸——实际 src/types.ts:90 已声明 `expectedUpdatedAt?: number`，diffTaskRows 全文无 cast。前提错误，零代码处置。

**FP ⑥ App.tsx:254（high → FP，reviewer agent-12 pass）**：finding 声称 mutateFire 注释「错误已经由 storage 层 alert 提示用户」误导、用户看不到 alert——实际 upsertTasks/deleteTaskRows（storage.ts:36-55）throw 前先调 handleCommandError，后者默认（非 silent）走 alert/confirm（errorHandler.ts:202-233）。注释属实，零代码处置。

**转 B 类 ⑦ App.tsx:240（半成功陷阱方向）**：upsert 成功 + delete 抛错时 UI 不更新、不广播——修法方向（回滚 upserts / 仍合并广播 / 仅依赖 storage 层 alert）是语义决策，B 类攒批第 18 项。

**转 B 类 ⑧ App.tsx:191（数据破坏相关修法方向）**：finding 字面前提错误（removeItem 在 upsert 之后，reject 时不会执行）；真实残留是「迁移失败 → 种子落库 → 旧 localStorage 数据永久 orphan」，修法方向（跳过种子待下次重试 / 维持现状）涉数据破坏语义，B 类攒批第 19 项。

## 测试（src/storage.test.ts 追加）

invoke mock 基建已存在：
- diffTaskRows：输入 next 对象不被突变（updatedAt/expectedUpdatedAt 不写进原对象、返回新引用）+ 既有语义全绿
- loadWorkspaceFromDb：resolve → {ok:true, items}；reject → {ok:false, error}
- exportTasksToFile / importTasksFromFile：invoke reject → rejects（不返 0）

## 红线

- family 一致性：本批只含 core-io-contract
- FP 均已 reviewer pass（agent-12）；B 类 2 项入攒批
- 不碰 storage.ts :2/:5/:25/:183 等 medium/low（非本簇 8 条内）

## spec 起草后自查三条

1. `expected_files`：src/storage.ts + src/App.tsx + src/components/WorkspacePage.tsx + src/components/WidgetApp/WidgetApp.tsx + src/storage.test.ts（=5，达批上限）✓
   【校正 2】OCR r1 同根因处置扩 2 文件：+ src/lib/mutationOrigin.ts（新建，镜像 mutation.rs 枚举）+ src/App.test.tsx（守卫回归测试）。=7 文件（5 改 + 1 新 + 1 测试改）。
2. budget A 类：storage.ts +45/-20（4 处改写 + LoadWorkspaceResult），App.tsx +12/-4（守卫集合），WorkspacePage +3/-1，WidgetApp +4/-1，测试 +35 ≈ 合计 +95/-30，上限 max +130/-45
   【校正 1】实测 numstat +107/-65：storage.ts 删除侧 -49 超估（4 处 try/catch 包装整段移除 + diffTaskRows 就地突变分支替换），测试旧断言替换 -11。改为 max +140/-80。其余不变。
   【校正 2】OCR r1 处置追加：mutationOrigin.ts 新建 ~+30（走 max_new_files_lines），App.tsx 守卫改引用模块级集合 ±3，App.test.tsx 回归测试 +65。改为 max +220/-80 + max_new_files_lines 60。
3. findings 逐条 fix 字段列 ripple 文件+行号 ✓

## OCR r1 处置（5 comments，全部同根因——均指向本批新引入的守卫代码）

- **high App.tsx:307-316（test）实收**：新守卫缺回归测试 → App.test.tsx 补 4 条（source=bot/api/migration 落盘跳过 + 未知 source WARN+仍落盘）。
- **medium App.tsx:307-308（maintainability）实收**：守卫集合应有命名的协议载体 → 新建 `src/lib/mutationOrigin.ts` 镜像 mutation.rs 枚举（Rust 头注释本就引用此文件，此前不存在），导出 BACKEND_PERSISTED_SOURCES / KNOWN_SOURCES。
- **low App.tsx:307-308 实收**：常量提升到模块级（随 mutationOrigin.ts 一并，闭包内不再每次事件重建数组）。
- **low App.tsx:307-311 实收**：4 子句 WARN 条件 → KNOWN_SOURCES 集合成员判定（含 main/widget/initial-load，行为不变：已知未持久化 source 不 WARN 仍回写，未知 WARN+回写）。
- **low WorkspacePage.tsx:71-73 FP**：称 error 载荷被静默丢弃——实际 loadWorkspaceFromDb catch 内 handleCommandError(silent) 仍 console.error（errorHandler.ts:209/229，silent 只抑制 alert）。零代码。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/FE-01a/,不喊人

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FE-01a",
  "family": "core-io-contract",
  "expected_files": [
    "src/storage.ts",
    "src/App.tsx",
    "src/components/WorkspacePage.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/storage.test.ts",
    "src/lib/mutationOrigin.ts",
    "src/App.test.tsx"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 80,
  "max_new_files_lines": 60,
  "findings": [
    {"id": "C5-FE-01a", "file": "src/storage.ts", "line": 109, "fix": "diffTaskRows upserts 改产出新对象，不再就地突变 caller 的 next；ripple 无（返回类型不变）"},
    {"id": "C5-FE-01b", "file": "src/storage.ts", "line": 62, "fix": "4 个导出/导入函数失败返 0 → throw（去 storage 侧 alert 防双弹，caller App.tsx:416/434/458/476 已有 catch+handleCommandError+onRetry）"},
    {"id": "C5-FE-01c", "file": "src/storage.ts", "line": 122, "fix": "loadWorkspaceFromDb 静默返 [] → LoadWorkspaceResult 判别式；ripple: WorkspacePage.tsx:70 + WidgetApp.tsx:191"},
    {"id": "C5-FE-01d", "file": "src/App.tsx", "line": 291, "fix": "tasks-updated 回写豁免集合补 bot/api（实现 mutation.rs 文档化协议），未知 source 维持 WARN+回写"}
  ],
  "assertions_min": {
    "src/storage.test.ts": 0
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
fix(core): FE-01a — core-io-contract

【family】core 写入加载路径：就地突变 / 静默返 0 / 静默返 [] / 协议守卫漏值
【实修 4 处】
- storage.ts:109：diffTaskRows 产新对象不就地突变
- storage.ts:62：导出/导入失败返 0 → throw
- storage.ts:122：loadWorkspaceFromDb → 判别式结果（ripple WorkspacePage/WidgetApp）
- App.tsx:291：tasks-updated 豁免补 bot/api（实现 mutation.rs 协议）
【FP 2 条】storage.ts:113（types.ts:90 已声明）+ App.tsx:254（storage 层确有 alert）——reviewer pass
【B 类 2 项】App.tsx:240 半成功方向（#18）+ App.tsx:191 迁移失败种子方向（#19）
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1：exit <code> / <N> comments
【基线】D2: files=5(+A/-R) asserts=0→0 tests=vitest
```

验证命令

```bash
python3 scripts/batch-verify.py docs/batches/FE-01a.spec.md --tests
```
