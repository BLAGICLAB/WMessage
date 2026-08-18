# WMessage Skill DSL 编写文档（实操指南）

> 版本 v1.0 · 2026-08-18 · 作者视角（怎么写一个 Skill）
> 配套：[SKILL-RUNTIME.md](./SKILL-RUNTIME.md)（运行模型理论）、[SPEC.md](./SPEC.md)（产品规格）、`src-tauri/src/bot_skills.rs`（实现）

---

## 1. 概述

Skill 是 wmessage 的扩展机制 —— 把多步原子 Function 编排成一个可被意图匹配的复合单元。DSL 格式是 Markdown + YAML frontmatter，简洁到 1 分钟能学会。

**两类 Skill 形态**：

| 模式 | 说明 | 适合场景 |
|---|---|---|
| `auto` | Skill 调度器自动跑完所有 step，**跳过 LLM** | 确定性流程（归档、批量执行、报告生成） |
| `interactive` | Skill body 注入 system prompt，**LLM 驱动**逐步骤决策 | 高阶流程（条件分支、动态规划） |

本文档侧重 `auto` 模式编写（`interactive` 模式等价于给 LLM 写一份参考文档）。所有示例默认 `mode: auto`。

---

## 2. 目录结构

```
skills/
└── <skill-name>/
    ├── SKILL.md           # 必需：DSL 入口（frontmatter + body）
    ├── scripts/           # 可选：被 run_python 引用的脚本
    ├── templates/         # 可选：Word / Excel / PPT 模板
    └── assets/            # 可选：图标 / 图片等资源
```

**Skill 名约束**：`[A-Za-z0-9_-]+`（防路径穿越，已在装载层强制）。

**安装路径**：
- Dev 模式（cargo run / tauri dev）：`src-tauri/target/debug/skills/<name>/SKILL.md` —— 编译产物可见
- Release 模式：`<app_data_dir>/skills/<name>/SKILL.md`
  - macOS：`~/Library/Application Support/com.renshi.wmessage/skills/`
  - Windows portable：`<exe 同目录>/skills/`
  - Windows installer：`%APPDATA%\com.renshi.wmessage\skills\`

**数据目录优先**：dev 模式下，用户已安装的 Skill（数据目录）优先于 `target/debug/skills/` 里同名 Skill —— 防止 dev 版本覆盖用户修改。

---

## 3. Frontmatter 规范

`SKILL.md` 文件首部 `---` 之间的 YAML 块：

```yaml
---
name: minimax-task-summary          # 必填，唯一标识
description: 任务汇总报告生成       # 必填，注入系统提示词的技能清单用
risk_level: low                    # low / medium / high，决定 mode 推导
mode: auto                         # auto / interactive，显式优先于风险推导
max_steps: 5                       # 默认 5，最大 8（硬上限防长流程）
timeout_secs: 60                   # 默认 60，180 已覆盖大多数场景
rollback: auto                     # none / auto，auto 时必须含 ## Rollback 段
enabled: true                      # 启用开关
resumable: false                   # 暂停后是否可续跑
intents:                           # 触发意图关键词（L1 路由硬锁）
  - 任务汇总
  - 总结任务
  - 日报
---
```

**字段优先级**：
- `mode` 显式 > 风险推导：`high` 强制 `interactive`；`low` 强制 `auto`；`medium` 默认 `interactive` 但可显式 `auto`
- `risk_level` 默认 `medium`
- `max_steps` 默认 8，硬上限 100
- `timeout_secs` 默认 180 秒

**`intents` 关键词黑名单**（命中即拒绝启动）：
- `全盘遍历`、`批量删除`、`无确认删除`、`遍历文件系统`、`清空所有`

---

## 4. 步骤语法

### 4.1 基本格式

每个 step 是一个二级标题加一行 JSON 参数：

```markdown
## Step 1: 列出当前所有任务
list_tasks({})

## Step 2: 查询第一张任务卡详情
query_single_task({"id": "${step1.id}"})
```

解析规则：
- `## Step N: 标题` → 步骤定义（标题只是注释，不参与运行）
- 紧接的下一行 `tool_name({...})` → 工具调用（必须是合法 JSON）
- step 编号必须从 1 开始连续递增

### 4.2 工具白名单

DSL 可调用的工具（白名单由 bot.rs Function 工具集定义）：

| 类别 | 工具 |
|---|---|
| 任务卡 | `list_tasks` / `query_single_task` / `search_tasks` / `create_task` / `edit_task` / `complete_task` / `delete_task` / `add_subtask` / `bind_file` |
| 文档抽取 | `extract_document` |
| 文档生成 | `create_word` / `create_excel` / `create_ppt` / `create_pdf` |
| 通用 | `run_python` / `web_search` / `fetch_url` |
| 嵌套 | `use_skill`（启动另一个 Skill） |

**黑名单**（禁止裸调原子 Function）：
- `create_word_revisions`（Word 修订 Skill 专用）
- `link_file_to_task`（Skill 末尾绑产物专用）

非白名单工具直接 return `未知工具` 错误，触发 rollback。

### 4.3 Rollback 段

`rollback: auto` 时必须含 `## Rollback` 段，否则 `rollback: none`：

```markdown
## Rollback
query_single_task({"id": "${step1.id}"})
delete_task({"id": "${step1.id}"})
```

rollback 段也走变量替换（失败前的步骤都已入 ctx），失败时按相反顺序执行。

---

## 4.4 Skill mode 与 LLM 安全边界（2026-08-18 拍板）

本项目区分 Skill 两种运行模式，对 LLM 调用边界完全不同：

### `interactive`（默认 / medium+high 风险）
- **Skill 子流程允许内部调用 LLM**：做自然语言理解、动态规划、多轮交互
- **pre-step 路由层跳过外层主 LLM**（这是 pre-step 的本职）
- **但 Skill 内部 LLM 继续可用**，由 `run_model_loop` 驱动
- **所有 tool 调用强制经过 pre-execute 安全校验**（黑名单 `create_word_revisions` / `link_file_to_task` 仅 Skill Running 状态放行）
- 适用：需要 LLM 决策的复杂流程（条件分支、文档润色、动态问答）

### `auto`（low 风险 / 显式声明）
- **纯 DSL 调度模式**：完整 bypass 全部 LLM
- 参数提取、流程推进全部硬编码（DSL 步骤按文件顺序执行）
- LLM 仅在**失败兜底**路径（`FailedButRecoverable`）才介入一次
- 适用：确定性流程（归档、批量执行、报告生成）

### 安全边界澄清
- **不是逃逸漏洞**：interactive 模式 LLM 调原子工具是**设计意图**（Skill body 注入 system prompt 让 LLM 走步骤）
- pre-execute 持续生效：黑名单工具在非 Skill 状态硬阻断
- **禁止 LLM 裸调**：`create_word_revisions` / `link_file_to_task` 必须通过 `start_skill` 注册才能调用

### 监控
- `pre_step.route_skill` 事件带 `mode` 字段（auto / interactive）
- `skill.start` 事件带 `mode` 字段
- 监控端按 mode 分类统计即可看到「Skill 内部 LLM 调用次数 / 步数」

### 与 F-1 开关的关系
- `bypass_llm_on_pre_step_hit` 控制「pre-step 路由外层是否跳过主 LLM」（F-1 待办）
- Skill 内部 `mode` 决定「Skill 内部是否允许 LLM 调用」
- 两者互相独立：toggle off 时 pre-step 命中也走外层 LLM，但 Skill 内部 LLM 仍按 mode 决定

---

## 5. 变量替换

### 5.1 单段映射（基础 4 种）

| 语法 | 含义 | 示例 |
|---|---|---|
| `${stepN.result}` | 第 N 步工具返回的**原文** | `${step1.result}` |
| `${stepN.id}` | 第 N 步从结果提取的 UUID | `${step2.id}` |
| `${prev.result}` | 上一步工具返回的**原文** | `${prev.result}` |
| `${prev.id}` | 上一步提取的 UUID | `${prev.id}` |

**未匹配的 `${...}` 保留原样** —— 避免误吃合法 JSON 里的 `$` 字符（如 `{"price": "$1.50"}`）。

**UUID 提取规则**：标准 UUID v4 正则 `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`，多 UUID 取首个。

### 5.2 嵌套路径（Phase 4 第 2 项）

如果工具返回 JSON，可用点路径取嵌套字段：

| 语法 | 含义 |
|---|---|
| `${step1.task.id}` | `parsed.task.id` 取值 |
| `${step1.list.0.title}` | 数组 `list` 第 0 项的 `title` |
| `${prev.task.title}` | 上一步 JSON 的 `task.title` |
| `${step1.1.id}` | 数字段作为数组索引 |

**规则**：
- 字段缺失 / parsed 为 None（纯文本 result）→ 保留 `${...}` 原样
- 数字段 → 数组索引（`usize` 解析）
- 非数字段 → 对象字段
- 终值是 String → 原样；Null → `"null"`；数字 / 布尔 / 对象 / 数组 → `serde_json` 序列化成字符串

**嵌套路径要求**：每步执行前自动尝试 `serde_json::from_str(&result)`，成功则 `parsed = Some(json)`，失败则 `parsed = None`。所以**如果工具返回 JSON，nested path 自动 work**。

### 5.3 变量替换时机

每步执行前调 `substitute_vars(&step.args_json, &ctx)`：
- `${stepN.result}` / `${stepN.id}` 优先级最高（精确索引）
- `${prev.result}` / `${prev.id}` 简写指向 ctx 最后一项
- `${stepN.path.to.field}` 嵌套路径（沿 `parsed` JSON 走）

---

## 6. 状态机

每个 step 执行前调 `advance_dsl(run, now_ms)` 决策下一步动作：

| SkillRun.state | 返回动作 | DSL 行为 |
|---|---|---|
| Loaded | `Run` | 正常执行 |
| Running（未超时） | `Run` | 正常执行 |
| Running（超时） | `FailWithRollback(reason)` | 跑 rollback + 报错 |
| Paused | `AwaitUser` | 返回 `__await_user__`，主循环挂起等用户确认 |
| Completed | `Finish` | 跳出循环，跑汇总 |
| Failed | `FailWithRollback(reason)` | 跑 rollback + 报错 |
| Terminated | `Terminate(reason)` | 不跑 rollback + 报错（用户主动取消） |

**5 个 DslAdvanceAction 映射到 run_skill_scheduler 返回类型**：

```
Run              → 继续执行当前 step
Finish           → 跳出循环，返回 DslOutcome::Done(summary)
AwaitUser        → 返回 DslOutcome::AwaitUser
FailWithRollback → 跑 rollback + 返回 DslOutcome::FailedButRecoverable { ... }
Terminate        → 返回 Err(DslFailure::Terminated { ... })
```

**SkillRun.state 何时改变**：
- `Loaded → Running`：`start_skill()` 调用
- `Running → Failed`：`skill_on_step_post` 检测工具返回失败 / `step_check` 步数熔断 / 超时
- `Running → Terminated`：`skill_terminate_all()` 用户 /stop / `start_skill` 切其他技能
- `Running → Paused`：`step_check` Paused 拒绝 / `confirm_state(false)`

---

## 7. LLM 兜底路径

Skill 失败时（`FailedButRecoverable`），不会直接报错给用户 —— 而是把「失败原因 + 已完成产物摘要 + 回滚状态」注入 system prompt，让 LLM 决策下一步：

```
【Skill 失败可恢复上下文】
原因：技能「xxx」Step 2 (query) 失败：错误：网络超时
已完成产物：
- Step 1 (list): [{"id": "7c9e6679-...-90ae7", "title": "任务A"}]
- Step 2 (query): 错误：网络超时
回滚已尝试：否
请基于以上产物决策：重试 / 调整 / 告知用户。
```

LLM 可继续利用已完成产物（例如已知 UUID）重试或调整参数。`Terminated`（用户主动取消）不触发 LLM 兜底，直接报错。

---

## 8. 完整示例

### 8.1 最简单的 Skill：列出所有任务

```markdown
---
name: minimax-task-list
description: 列出当前所有任务卡
risk_level: low
mode: auto
max_steps: 1
timeout_secs: 30
rollback: none
intents:
  - 列出任务
  - 看任务
---

## Step 1: 列出所有任务
list_tasks({})
```

### 8.2 含变量替换的 Skill：归档指定任务

```markdown
---
name: minimax-archive-task
description: 归档指定任务（按 ID）
risk_level: medium
mode: auto
max_steps: 4
timeout_secs: 60
rollback: auto
intents:
  - 归档任务
  - 任务归档
---

## Step 1: 查询目标任务
query_single_task({"id": "${prev.id}"})

## Step 2: 移动任务到归档目录
move_task({"id": "${step1.id}", "to": "archive"})

## Rollback
query_single_task({"id": "${step1.id}"})
```

### 8.3 嵌套路径示例：批量导出 Excel

```markdown
---
name: minimax-export-excel
description: 把任务列表导出为 Excel
risk_level: low
mode: auto
max_steps: 3
timeout_secs: 120
rollback: auto
intents:
  - 导出 Excel
  - 导出表格
---

## Step 1: 列出所有任务
list_tasks({})

## Step 2: 生成 Excel 文件
create_excel({"tasks": "${step1.result}", "path": "/tmp/tasks.xlsx"})

## Rollback
delete_file({"path": "${step2.path}"})
```

`${step1.result}` 是 `list_tasks` 返回的 JSON 数组原文，`create_excel` 直接消费。

### 8.4 嵌套字段 + 数组索引示例：处理第一张任务

```markdown
## Step 1: 列出任务
list_tasks({})

## Step 2: 取第一张任务的 ID
query_single_task({"id": "${step1.0.id}"})
```

`${step1.0.id}` 取 `parsed[0]["id"]`（list_tasks 返回的是数组，第一个元素的 id 字段）。

---

## 9. 调试与测试

### 9.1 lib test 端到端 smoke

`bot_skills::tests::smoke_all_real_skills_run_dsl_loop_with_mock_executor` 自动扫 `target/debug/skills/` 下所有 Skill，跑通用 mock executor 验证 parse + 变量替换 + 状态机 + 嵌套路径链路。新增 Skill 时这个测试自动覆盖。

### 9.2 单测覆盖模式

- `parse_skill_steps` 验证 DSL 文本 → `(Vec<SkillStep>, Vec<SkillStep>)`
- `substitute_vars` 验证变量替换（含嵌套路径）
- `run_dsl_loop_sync` 验证 advance_dsl 5 分支

写新 Skill 时优先加这 3 层的测试 case。

### 9.3 运行时日志

DSL 调度器写 `bot.log`（数据目录），关键事件：

| 事件 | 含义 |
|---|---|
| `skill_dsl_start` | Skill 启动 |
| `skill_dsl_step` | 每个 step 执行 |
| `skill_dsl_var_resolved` | 变量替换前后长度变化 |
| `skill_dsl_rollback_start` / `skill_dsl_rollback_done` | 回滚执行 |
| `skill_dsl_finish_signal` / `skill_dsl_await_user` / `skill_dsl_terminated` | 状态机终态 |
| `skill_dsl_done` | 跑完汇总 |

`grep "skill_dsl" bot.log | tail -50` 调常见问题。

---

## 10. 常见问题

**Q: 写了 Skill 但设置页看不到？**
- 检查 `SKILL.md` frontmatter 格式（YAML 必须合法）
- 检查 `name` 字段与目录名一致
- dev 模式看 `target/debug/skills/<name>/`，release 看数据目录

**Q: 变量替换没生效？**
- 检查 `${...}` 语法（注意 `$` 和 `{` 之间不要有空格）
- 检查 step 编号是否正确（`step1` 是第一个 step）
- 嵌套路径检查 result 是不是合法 JSON（含 `${step1.task.id}` 需要 result 是 JSON）

**Q: 跑了但报错「未知工具」？**
- 检查工具名是否在白名单（见 4.2 节）
- 黑名单（`create_word_revisions` / `link_file_to_task`）只能在 Skill 内部用

**Q: 回滚没跑？**
- 检查 `rollback: auto` 是否在 frontmatter 设置
- 检查 body 末尾是否含 `## Rollback` 段

**Q: 嵌套路径保留 `${...}` 原样？**
- 检查 result 是不是合法 JSON（用 `parse_skill_steps` 测试调试）
- 字段缺失会保留原样（避免误吞 JSON `$`）

**Q: 用户机器上跑不通？**
- 检查 release 模式 `target/debug/skills/` 不可见 —— Skill 必须装到数据目录
- 设置页 → 「打开目录」按钮把目录路径显示出来
- 用设置页「导入技能」按钮从 dev mock 目录导入