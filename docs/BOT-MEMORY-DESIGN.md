# Bot 轻量记忆模块设计实施方案（v3）

> **⚠️ 已被 memory v2 取代（2026-09-09）**：运行时路径已切到语义记忆体
> （`src-tauri/src/memory/`，新表 `mem_items`），本文档仅作旧系统设计留档。
> 现行设计见 `BOT-MEMORY-V2-DESIGN.md`。
>
> 日期：2026-09-04
> 目标：为内置 Rust bot 实现一套**轻量化、无向量模型**的记忆体系。
> v3 在 v2 基础上引入四项研究驱动的增强（见第 7 节），全部保持"加一列/几十行代码"量级。
> v1 → v2 的取舍见文末附录。

---

## 1. 设计原则

- **一张表**：所有长期记忆（事实/偏好/摘要）统一存进升级后的 `bot_facts` 表。
- **零索引设施**：不用 FTS5、不用向量库。总量上限 300 条，全表扫描 + Rust 打分是微秒级开销。
- **一个摘要时机**：只在历史超预算触发截断时生成摘要（"截断即摘要"），不做独立的会话结束摘要。
- **零前端改动**：无设置页开关，参数全部用常量；工具只给现有工具加可选参数。
- **写入贵于读取**：记忆质量在写入侧保证（冲突提示、来源标注），读取侧保持极简。

## 2. 现状基线（已有的，不动）

- 会话历史：`bot_messages` 表 + 全量覆盖写（`db.rs:905`），发 LLM 前按 `HISTORY_BUDGET_CHARS = 100_000` 截断（`bot_chat.rs:127`），超预算直接丢最旧消息。
- 长期事实：`bot_facts` KV 表（`db.rs:299`），`remember_fact`/`recall_facts` 工具（`bot.rs:917-1023`），上限 200 条，`recall_facts` 全量读回无检索。
- 手动压缩：`/compact`（`bot_chat.rs:691`），逻辑可提炼复用。

## 3. 数据层：升级 `bot_facts`（唯一一张记忆表）

走 `db.rs` 现有 `PRAGMA table_info` + `ALTER TABLE` 迁移模式，加六列：

```sql
ALTER TABLE bot_facts ADD COLUMN kind         TEXT NOT NULL DEFAULT 'fact';
-- fact(事实/偏好) | summary(历史摘要) | reflection(阶段总结)
ALTER TABLE bot_facts ADD COLUMN category     TEXT NOT NULL DEFAULT 'general';
-- profile(画像) | preference(偏好) | project(项目上下文) | general
ALTER TABLE bot_facts ADD COLUMN importance   INTEGER NOT NULL DEFAULT 3;  -- 1-5
ALTER TABLE bot_facts ADD COLUMN source       TEXT NOT NULL DEFAULT 'user_stated';
-- user_stated(用户明确说的) | model_inferred(模型推断的)
ALTER TABLE bot_facts ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;  -- 命中注入次数
ALTER TABLE bot_facts ADD COLUMN accessed_at  INTEGER NOT NULL DEFAULT 0;  -- 最近命中时间
```

- key 为主键不变；`summary` 类 key 用 `summary:{session_id}:{ts}` 生成，`reflection` 用 `reflection:{ts}`。
- 上限从 200 提到 **300 条**。**惰性淘汰**：插入超限时，按淘汰分（`importance` 升序 → `accessed_at` 升序）删最低一条；`importance >= 4` 且 `kind = 'fact'` 的画像不淘汰（超限时报错让模型自己删）。
- 读写函数遵守 `DB_WRITE_LOCK` + `spawn_blocking` 模式（参照 `db.rs:905-924`）。

## 4. 检索：Rust 侧关键词打分（无 FTS5）

每次对话用**当前用户消息**对 300 条记忆全表扫描打分，单条 SQL `SELECT *` 取出后在 Rust 里算：

```
score = keyword_overlap × (importance / 3) × exp(-age_days / 90) × ln(1 + access_count)
```

- **关键词提取**（约 20 行，无依赖）：消息按非字母数字切分取英文/数字词；连续中日韩字符段取**字符 bigram**（"记忆模块" → {记忆, 忆模, 模块}）。
- `keyword_overlap` = 命中词数 / 提取词数，0 命中即 0 分。
- `age_days` 从 `accessed_at`（无则 `updated_at`）起算——**被反复想起的记忆衰减更慢**（MemoryBank/FadeMem 的遗忘曲线思想）。
- 取 top-5 注入并原子地 `access_count + 1, accessed_at = now`；全部 0 分（闲聊居多）时回退为**最近 3 条 summary/reflection + 全部高重要度 fact**。

## 5. 写入路径

### 5.1 模型主动记：`remember_fact` 升级（含冲突提示，见 7.1）

- 参数加可选 `category`、`importance`、`source`；空 value 删除语义不变（顺带就是"遗忘"能力）。
- **写入即检索**：写入前用同一套打分对现有 fact 找 top-3 相似项，写进工具结果返回：`已写入。相似已有记忆：[key=城市, value=上海]——如需更新请用同 key 覆盖`。模型自行决定覆盖/保留，零额外 LLM 调用。
- 同步更新三处：`TOOLS` schema（`bot_model_loop.rs:86`）、`execute_tool` 分发（`bot.rs:866` 附近）、白名单（`tool_guard.rs:85`）。已在幻觉守卫 `MUTATING_TOOLS` 里（`bot_model_loop.rs:45`），无需动。
- 更新 `SYSTEM_PROMPT` 规则 21（`bot_chat.rs:74`）：明确何时记、category/importance/source 标准、收到冲突提示时优先覆盖而非堆积、措辞用规范名词。

### 5.2 系统自动记：截断即摘要

改造 `truncate_chat_history`（`bot_chat.rs:127`）：

1. 超预算时，把**将被丢弃的消息**发给模型生成 ≤200 字摘要（提炼 `bot_compact` 的摘要 prompt 为公共函数 `summarize_messages()`）；
2. 摘要作为一条 system 消息（`[早前对话摘要] ...`）保留在截断后历史的开头；
3. 同一份摘要写入 `bot_facts`（`kind='summary'`，`importance=2`），供跨会话检索；
4. **Reflection 触发**（见 7.3）：写完后若 `summary` 类条数 ≥ 10，把最旧的 10 条喂给 `summarize_messages()` 合成一条 `kind='reflection'`、`importance=3` 的阶段总结，删除原 10 条。

失败兜底：摘要/Reflection 调用失败退回现在的行为（直接丢弃/跳过合并），不阻塞对话。`/compact` 手动命令复用 `summarize_messages()`，行为不变。

## 6. 注入：`build_memory_block()`

在 `bot_chat.rs:436-646` 拼 system prompt 处追加一段，预算 `MEMORY_BUDGET_CHARS = 4_000`（常量）：

```text
## 记忆
### 用户画像与偏好
- [preference] 回复喜欢简洁中文
- [project][推断] 用户主要在 macOS 上开发
### 相关记忆
- [2026-08-30] 用户在做 WMessage 挂件的记忆功能设计
### 近期摘要
- [09-02] 讨论了记忆模块选型，确定不用向量模型
```

拼装顺序：先放 `importance >= 4` 的 fact（无条件），再放检索 top-5，剩余额度放近期 summary/reflection；超预算从后往前砍。`source = model_inferred` 的一律带 `[推断]` 前缀（见 7.2）。

## 7. 四项增强的研究依据与创新点

### 7.1 写入时冲突提示（自整合）——学 Mem0，不用向量

Mem0 的核心收益来自写入侧的"抽取→冲突检测→更新"流水线（[Mem0 论文](https://arxiv.org/html/2504.19413v1)），而非向量检索本身。我们把"冲突检测"降级为关键词相似度回查、把"解决决策"交还模型（它反正要读工具结果），用一次零成本的同步检索替代 Mem0 的 LLM 合并调用。这同时呼应了记忆准入控制研究（[Adaptive Memory Admission Control, arXiv 2603.04549](https://arxiv.org/html/2603.04549)）：**写入时验证比事后清理便宜**。

### 7.2 来源标注——防止记忆幻觉自我强化

2026 年的系统对比指出 Mem0 的结构性短板是"所有记忆同等可信"（[arXiv 2603.25097](https://arxiv.org/pdf/2603.25097)）。模型推断的记忆一旦无标记注入，会在后续对话中被当作事实引用、再被记入——幻觉自我强化循环。`source` 列 + `[推断]` 前缀让模型在引用时自行降权，成本是一列加一个字符串前缀。

### 7.3 Reflection 极简版——摘要的摘要

Generative Agents（[Park et al. 2023，综述引用](https://arxiv.org/html/2607.08032)）验证了"定期把多条显著记忆合成高层见解"对长期行为一致性的价值。我们的极简版只作用于 `summary` 类：攒够 10 条合一次，防止摘要随会话数线性膨胀，同时自然形成"原始摘要 → 阶段总结"的层级。复用 `summarize_messages()`，无新流水线。

### 7.4 访问强化——艾宾浩斯复习效应

MemoryBank（AAAI 2024）和 FadeMem（[2026 调研](https://github.com/temm1e-labs/temm1e/blob/main/tems_lab/LAMBDA_MEMORY_RESEARCH.md)）都把遗忘曲线作为记忆淘汰的一等机制。`access_count` + `ln(1+count)` 打分项 + 命中刷新 `accessed_at`，实现"常被想起的记忆更难忘"，共一列两个打分项。

> 显式不做：Zep 的时序知识图谱、MemOS 的三层治理、Hindsight 的目标感知打分——都需要重存储/重计算基础设施，违背轻量化约束（参考 [2026 对比报告](https://zilmac.com/en/blog/articles/mem0-vs-zep-vs-tencentdb-agent-memory-2026-comparison.html)）。

## 8. 实施步骤（两步）

**Step 1：截断即摘要 + 数据层**（独立可用，收益最大）
1. `db.rs`：`bot_facts` 迁移（6 列）+ 惰性淘汰写入函数。
2. `bot_chat.rs`：提炼 `summarize_messages()`，改造 `truncate_chat_history`；含 Reflection 触发。
3. 测试：仿 `tests/llm_integration.rs` mock LLM，验证长历史触发摘要、摘要留在历史开头、Reflection 合并、失败兜底。

**Step 2：检索、注入与写入增强**
4. `db.rs` / `bot.rs`：关键词提取与打分函数、检索 top-N、`remember_fact` 升级（含冲突提示回传）。
5. `bot_chat.rs`：`build_memory_block()` 接入 prompt 组装；更新规则 21 文案。
6. 测试：仿 `bot.rs:2745` 的 `phase4_facts_tests` 用内存库测打分排序、淘汰、冲突提示触发；mock LLM 测记忆块注入与预算截断。

**回归基准（轻量 eval）**：仿照 LongMemEval 思路写 5-8 个脚本化对话场景（告知偏好 → 隔 N 条闲聊 → 提问验证召回；改口 → 验证覆盖而非堆积），用现有 mock LLM 基建跑，作为记忆功能的回归测试。

预计改动：`db.rs` ~250 行、`bot_chat.rs` ~180 行、`bot.rs` ~100 行，外加测试。**不新增任何 crate 依赖。**

## 9. 已知局限（有意取舍）

- 关键词匹配命中不了语义近义词（"那个文件"这类指代完全不行）。缓解靠写入时的规范措辞引导；量级上去后仍嫌不够，再考虑 FTS5 trigram——**schema 兼容，届时只加一张虚拟表，不用推翻**。
- 冲突提示靠关键词相似度，会漏掉措辞不同但语义矛盾的记忆；模型读到提示后的决策质量取决于模型能力。
- 摘要/Reflection 质量依赖模型能力；保留原始 session_id 可追溯，`/compact` 仍可手动精细控制。
- 无 token 精确计数，继续字符估算。

## 附录：v1 → v2 砍掉了什么

| v1 设计 | v2 处理 | 理由 |
|---|---|---|
| 独立 `bot_memories` 情景记忆表 | 合并进 `bot_facts`（`kind` 列区分） | 少一次建表、少一套读写函数；两类记忆检索逻辑本就相同 |
| FTS5 trigram 虚拟表 + 同步触发器 + bm25 | Rust 全表扫描 + bigram 关键词打分 | 300 条数据用不到索引；消除 MATCH 转义注入风险和触发器维护成本 |
| "会话结束自动摘要" + "截断即摘要"两个时机 | 只留截断即摘要 | 同一条流水线不跑两遍；超预算才摘要，不白烧 token |
| `bot_scheduler` 每日定时清理 | 写入时惰性淘汰 | 300 条上限下淘汰是 O(n) 一次性判断，不需要调度器 |
| `forget_memory` 新工具 | 复用"空 value 删除" | 现有语义已够用 |
| `BotConfig` 三个新开关 + 设置页 UI | 全部改常量 | 零前端改动；真有需求再加配置 |
| 三期实施 | 两步 | 砍掉的部分本来就不该排在计划里 |
