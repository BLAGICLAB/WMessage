# 人工冒烟验收清单 — T1-QUERYTASKS 任务工具升级（2026-10-05）

> 依据：`docs/batches/T1-QUERYTASKS.spec.md`。
> **用法**：逐条操作 → 对照「预期」→ 通过打 `[x]`，失败留 `[ ]` 并在文末 §8 记录现象与截图。
> ⭐ = 快速冒烟子集（约 20 分钟）。标记：`[x]` 通过 ｜ `[ ]` 未通过/未测 ｜ ➖ 跳过 + 原因。

## 0. 前置条件

- [ ] 测试版本：`main` HEAD ≥ T1-QUERYTASKS
- [ ] **隔离数据目录**（不污染真实数据）：
      `WMESSAGE_TEST_DATA_DIR=/tmp/wm-tools-smoke npm run tauri dev`
- [ ] bot 已配置 API Key + 模型（工具链路依赖 LLM 决策）
- [ ] 看板备好至少：2 张未完成任务、1 张已完成任务、1 张带定时（⏰）任务、
      1 个含 3 节点依赖的工作流、1 条子任务

## 1. 回归底线（先做）

- [ ] ⭐ **老问法路由**：「列一下我的任务」→ 模型调 query_tasks（不传 query），
      挂件工具行显示 query_tasks，返回未完成任务清单
- [ ] ⭐ **老问法路由**：「搜一下有没有 XX 相关的任务」→ 模型调 query_tasks（带 query），
      能搜到已完成/已归档卡
- [ ] ⭐ 防幻觉守卫不回归：诱导模型口头声称「已新建任务」但未调工具 →
      被拦截补一轮（bot.log 有 claims 检测记录）

## 2. query_tasks 视图（T1-1/T1-3）

- [ ] ⭐ 「有哪些已完成的任务」→ query_tasks(view=done)，列出已完成未归档卡
- [ ] 「回收站里有什么」→ view=trash 列软删卡；「全部任务」→ view=all（不含回收站）
- [ ] ⭐ 关键词检索：「找包含 报告 的任务」→ query=报告，标题/备注/标签/子任务命中
- [ ] 标签过滤：view=active + tag=某标签 → 只剩带该标签的卡
- [ ] limit 截断提示：卡数 >50 时输出末尾带「（共 N 条，仅显示前 50 条…）」

## 3. 工作流标记问答（零工具方案）

- [ ] ⭐ 「我有哪些工作流？」→ 模型调 query_tasks(view=all)，按输出行的
      （工作流：名称）标记分组汇总出工作流清单
- [ ] 「XX 工作流进展如何」→ 各节点卡的列状态汇总正确
- [ ] 「这张卡依赖什么」→ query_single_task 输出「依赖：…」行（标题而非裸 id）

## 4. query_single_task 只读行（T1-4）

- [ ] ⭐ 查一张带定时的卡 → 输出「定时：每天 09:30」（或每周 X / 每月 N 日 / 一次性）人性化行
- [ ] 查卡输出「创建：YYYY-MM-DD HH:MM」（新卡；老库 NULL 卡无此行、不报错）
- [ ] 工作流卡输出「所属工作流：名称」；普通卡无此行

## 5. edit_task 新字段（T1-2/T1-4）

- [ ] ⭐ 「帮我把 XX 任务的执行模型换成 deepseek」→ edit_task(model=…)，
      卡片模型徽标变化；「恢复跟随全局」→ 空串清除生效
- [ ] ⭐ 「把 XX 任务归给 张三」（导入过成员数据时）→ edit_task(owner=张三) 成功；
      图谱按成员过滤可见；「归给我自己」→ owner 空串清除
- [ ] 名字歧义：两个同名/近名成员 → 工具返回候选名单，模型向用户确认而非瞎选
- [ ] 未知成员 → 返回「没有找到成员」，模型如实告知

## 6. 子任务 subtaskId（T1-2）

- [ ] ⭐ 先 query_single_task 拿子任务 id，再说「勾选子任务 <id>」→
      toggle_subtask(subtaskId=…) 精确勾选；重复文本关键词的两个子任务不再误勾
- [ ] 「删除子任务 <id>」→ remove_subtask(subtaskId=…) 精确删除
- [ ] 老 口径不回归：「勾选买牛奶」→ 仍按 text 关键词命中

## 7. 审计与守卫

- [ ] `$DATA/bot.log`：query_tasks 的 tool.call/tool.return 审计齐备（含 args_preview）
- [ ] Skill 执行含 query_tasks 步骤时不再记入回滚清单（runtime.rs READONLY）
- [ ] 已安装 Skill 里若手工写过 `list_tasks({})` 步骤 → 执行报「未知工具」并可读
      （用户侧 Skill 需自行改 query_tasks；本仓无捆绑 Skill 引用旧名）

## 8. 问题记录

| # | 现象 | 截图 | 批次归属 |
|---|---|---|---|
|   |   |   |   |
