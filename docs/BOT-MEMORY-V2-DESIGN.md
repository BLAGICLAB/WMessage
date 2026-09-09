# Bot 记忆系统 v2 设计：轻量语义记忆体

> 日期：2026-09-09
> 状态：已实施（取代 `BOT-MEMORY-DESIGN.md` 的纯关键词方案）。系统未上线即切换：
> 旧表 `bot_facts` 废弃不导入（不做任何数据迁移；表与旧函数原样保留，
> 仅供 tests/memory_regression.rs 旧行为基准回归使用）。
> 代码：`src-tauri/src/memory/`（`mod.rs` 门面 / `embed.rs` 嵌入引擎 / `store.rs` 存储 / `rank.rs` 混合打分）

---

## 1. 目标与旧系统差异

| 维度 | 旧系统（bot_facts） | v2（mem_items） |
|---|---|---|
| 检索 | 纯关键词 bigram 重合 | 语义向量（0.55）+ 关键词（0.20）+ 重要度（0.15）+ 新近度（0.10）混合打分 |
| 容量 | fact 200 条 / 全表 300 条双层上限 | 统一 500 条，综合分淘汰 |
| 去重 | 仅同 key 覆盖 + 关键词冲突提示 | 语义去重：cos≥0.92 合并，0.75~0.92 冲突提示 |
| 载体 | 升级旧表 bot_facts | 新表 mem_items（旧表 bot_facts 废弃不导入，系统未上线不做迁移） |
| 任务卡执行 | 不注入记忆 | execute_task_core 也注入记忆块 |
| 降级 | 无（无模型可缺） | 嵌入模型缺失 → 全局纯关键词模式，任何路径不 panic |

不变的部分：工具名/schema（`remember_fact`/`recall_facts`）、`## 记忆` 三段式注入格式
（用户画像与偏好 / 相关记忆 / 近期摘要）、记忆块预算 4000 字、`[推断]` 前缀规则、
DB 访问纪律（DB_WRITE_LOCK + spawn_blocking 单写者）。

## 2. 嵌入引擎（embed.rs）

- 模型：bge-small-zh-v1.5（ONNX 量化版，hidden=512，4 层，vocab 21128，max_position 512）。
- 路径解析优先级：环境变量 `WMESSAGE_BGE_MODEL_DIR` → exe 同目录 `bge-small-zh-v1.5/`
  → 开发时仓库根（`CARGO_MANIFEST_DIR/../bge-small-zh-v1.5`）。
  显式指定（环境变量）但目录不完整时**不回退**——路径写错应当显式降级并在日志指出。
- 推理：tokenizer.json 加载 + 截断 512 token → ONNX 跑 `last_hidden_state`
  → attention-mask 加权 mean pooling → L2 归一化 → 512 维 f32。
- 加载：全局 OnceLock 懒加载；失败原因进程内缓存一次，之后 `embed_text` 恒返回 None
  （= 降级关键词模式）。首次成功加载后后台线程跑一次 dummy 推理预热。
- 线程纪律：ONNX 推理只在阻塞线程跑（门面层 spawn_blocking 闭包内、持 DB 锁之前），
  不占 async worker。

## 3. 存储（store.rs，新表 mem_items）

```sql
CREATE TABLE IF NOT EXISTS mem_items(
  id TEXT PRIMARY KEY,            -- uuid
  kind TEXT NOT NULL,             -- profile|preference|fact|event|summary|reflection
  content TEXT NOT NULL,          -- ≤800 字
  tags TEXT NOT NULL DEFAULT '',  -- 逗号分隔（remember_fact 的 key 落 tags[0]）
  importance INTEGER NOT NULL DEFAULT 3,
  source TEXT NOT NULL DEFAULT 'model_inferred',
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,   -- RFC3339
  access_count INTEGER NOT NULL DEFAULT 0,
  last_accessed_at TEXT,
  embedding BLOB                  -- 512×f32 little-endian，可无（降级模式）
);
```

- **统一容量 500 条**：插入前检查，超限淘汰综合分最低者——
  `淘汰分 = importance×2 + exp(-age_days/30) + ln(1+access_count)/5`（importance 权重最高；
  age 基准 = max(updated_at, last_accessed_at)）。
  `importance=5 且 source=user_stated` 的条目不可淘汰；无可淘汰条目时拒写，
  原因进工具结果让模型自己清理。
- **语义去重**（仅写入侧带向量时）：
  - cos ≥ 0.92 → 同一条：合并更新已有条目（刷 content/updated_at/access+1/向量），不新增；
  - 0.75 ≤ cos < 0.92 → 不拦截，相似条目（top-3，带 key/content）拼进工具结果作冲突提示，
    替代旧 `fact_conflict_hint` 的关键词方案；
  - 降级模式（无向量）跳过去重；`remember_fact` 的同 key 覆盖语义由 tags[0] 精确匹配保证。
- **向量检索**：全表读 embedding 暴力余弦（500 条 × 2KB，微秒级），不引 sqlite-vec。

## 4. 混合打分与注入（rank.rs）

```
score = 0.55·cosine(query, item) + 0.20·keyword_overlap + 0.15·(importance/5) + 0.10·exp(-age_days/30)
```

- keyword_overlap 复刻旧 `extract_keywords`（CJK bigram），匹配文本 = tags + content。
- age 基准 = max(updated_at, last_accessed_at)（被反复想起的记忆衰减更慢）。
- **降级模式归一**：无查询向量时语义项记 0，剩余权重 ÷0.45 归一；此时关键词零重合
  直接 0 分（与旧系统口径一致，保住「零命中 → 近期摘要兜底」语义，否则重要度/新近度
  常正项会让全表都进 top-5）。
- 注入三段：pinned（importance≥4 且 kind=profile/preference，无条件）→ 混合打分 top-5
  （已 pinned 不重复）→ 最近 3 条 summary/reflection（排除已命中，兼零命中兜底）。
  命中条目同连接原子刷新 access_count/last_accessed_at。
- 查询文本 = 用户消息（或任务标题+备注）前 200 字，每轮嵌入一次。

## 5. 集成点

1. `bot_chat.rs::build_memory_block` → `memory::injection_block`（静默降级 + WARN 审计不变）。
2. `remember_fact`/`recall_facts` 工具分发切到 `memory::tool_*`（schema、MUTATING_TOOLS、
   prompt 规则均未动；key→tags[0]，value→content；旧 `tool_*` 薄壳保留不调用）。
3. 摘要/反思流水线（`persist_summary_and_reflect`）写 mem_items 并嵌入向量；
   Reflection 仍是 summary 攒 10 条合成一条 + 同事务删原摘要。
4. `execute_task_core`（任务卡执行/定时调度）注入记忆块（查询 = 标题+备注前 200 字）。
5. 旧数据：**不做迁移**。系统未上线即切换，旧表 `bot_facts` 废弃不导入；
   表结构与 db.rs 旧函数原样保留（tests/memory_regression.rs 回归基准仍走旧路径）。
6. `tauri.conf.json` bundle resources 增加 `../bge-small-zh-v1.5`（打包随产物分发；
   便携包脚本需把模型目录放在 exe 同目录）。

## 6. 降级策略（验收硬约束）

- 模型目录缺失/损坏、tokenizer 或 ONNX 加载失败、单条推理失败 —— 一律返回 None，
  全系统按纯关键词模式运行：去重跳过、语义项记 0 权重归一、注入/工具照常。
- 注入路径任何失败 → 无记忆块 + WARN 审计（同旧系统）；工具路径失败 → 错误文本回模型。
- 回归：`tests/memory_v2_degraded.rs`（独立进程，env 指向不存在目录，全链路不 panic）；
  lib 单测 `degraded_full_flow_keyword_only_no_panic`。

## 7. 模型打包说明

- 依赖：`ort 2.0.0-rc.13`（`download-binaries`）+ `tokenizers 0.23`（`fancy-regex`，
  无 http/onig）。onnxruntime 在本机（macOS arm64）由 build script 下载并**静态链接**，
  产物无需随附 dylib；二进制体积增大约 30MB（debug 测试二进制 ~98MB 含符号）。
- 若构建机网络无法下载 onnxruntime：改用 `load-dynamic` 特性 + `ORT_LIB_LOCATION`/
  系统 onnxruntime，或预先执行一次成功的 `cargo build` 让 ort-sys 缓存二进制。
- 运行时模型目录三优先级见第 2 节；Windows 便携包把 `bge-small-zh-v1.5/` 放 exe 同目录即可。

## 8. 测试

- 新增 lib 单测 17 例（memory::embed 6 + memory::tests 11）：mean pooling / L2 归一化、
  模型路径解析、语义去重三分支（合并/提示/新增）、降级跳过去重、容量淘汰顺序与
  importance=5 保护、全保护拒写、混合打分权重（语义主导 / 降级归一）、注入三段结构、
  同 key 覆盖/删除。
- `tests/memory_v2_degraded.rs`：降级模式全链路集成测试。
- `#[ignore]` 真实模型冒烟 `memory::embed::tests::real_model_embed_smoke`
  （512 维、归一化、近义句余弦显著高于无关句），手动 `cargo test --lib memory::embed -- --ignored`。
- 既有回归不动：`tests/memory_regression.rs` 17 用例（旧系统行为基准）全绿。

## 9. lesson（教训记忆，2026-09-09 追加）

让 agent 从失败和纠正中学习：

- **类型**：`kind='lesson'`，importance 默认 4；tags[0]=`lesson` + 场景标签（工具名/任务类型）。
  表是 TEXT 无约束，无需 schema 变更。
- **两个写入来源**：
  1. `record_lesson` 工具（模型主动）：参数 `lesson`（≤800 字）+ `scenario`（可选 ≤50 字），
     source=model_inferred。已注册进 TOOLS schema / bot.rs 分发 / MUTATING_TOOLS
     （与 remember_fact 同待遇，幻觉守卫「已记录」口径）；tool_guard 是原子黑名单制，
     新工具天然不在黑名单。SYSTEM_PROMPT 规则 21 补了记教训的时机
     （被纠正 / 工具连续失败 / 发现更优做法）。
  2. `execute_task_core` 失败自动沉淀（source=system）：直接拼装「任务标题 + 失败原因」
     （不调 LLM，保持轻量），写失败只记审计不影响原错误返回。
- **去重**：走 insert_item 的正常语义去重——同类失败的教训合并更新而不是堆积。
- **注入**：记忆块第四段「### 经验教训」追加在最末（`## 记忆` 总标题与前三段不变），
  混合检索 kind=lesson 的 top-3；lesson 不进「相关记忆」段（避免一处内容两处出现）；
  命中同样刷新访问计数。超预算从后往前砍——lesson 段最先被砍，画像段不砍的规则不变。

## 10. 定时记忆整理（consolidation，2026-09-09 追加）

- **引擎** `memory/consolidate.rs`：候选 = 上次整理以来更新的条目（无上次则近 7 天）
  + access_count≥3 的活跃条目，按淘汰分降序取前 100 条 → 非流式 LLM 调用
  （复用 `bot_chat::summarize_messages`，配置缺失自然报 ApiKeyMissing）→ 解析 JSON
  指令列表 → 一个事务内应用：
  - `merge{ids, content}`：目标 = importance 最高（平手取最新）的现存条目，content/向量
    重算/时间更新到目标，其余来源删除；
  - `contradiction{keep, drop, content}`：keep 更新（content/向量重算），drop 删除；
  - `distill{ids, content}`：新建 kind=reflection、importance=4、source=system 条目
    （引用 id 全不存在时跳过，防 LLM 幻觉 id 凭空造规律）。
- **解析健壮性**：剥代码围栏/截取首尾花括号；整体解析失败 → 本轮静默放弃记审计；
  单条缺字段/未知 action 跳过不影响其它。
- **调度**：`start_consolidation_scheduler`（bot_scheduler 同模式，tauri async_runtime +
  tokio interval 每 10 分钟检查一次）。频率 off/12h/daily/weekly（默认 daily）；
  从未整理过先记基线（首个间隔后才真正跑，防启动即白跑一次 LLM）；
  本轮失败（LLM 不可用等）仍推进 last_run_at 防下个 tick 立刻重试。
- **配置**：`bot-config.json` 的 `memoryConsolidation` 字段
  （`{enabled, interval, lastRunAt}`，None = 默认启用+daily），读写跟随现有
  `bot_get_config`/`bot_set_config` 整份配置模式；另有 `memory_consolidate_now`
  命令（手动立即整理，返回 `{merged, distilled, contradictions}` 给前端 toast）。
- **前端**：设置页机器人区「记忆整理」块——开关 + 频率按钮组（关闭/每12小时/每天/每周）
  + 上次整理时间 + 「立即整理」按钮（转圈 → 结果文案短暂展示），样式跟随现有控件。
