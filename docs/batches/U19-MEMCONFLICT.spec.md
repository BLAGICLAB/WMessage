# Batch Spec: U19-MEMCONFLICT

## 目的

记忆系统第三期第二斧：写入时冲突裁决——auto 抽取的条目不再无脑堆积，
与既有记忆语义相近时先裁决再落库，「改口更新原条目」而不是同一条事实存 N 条。
检索评分/注入/存储层零改动（rank.rs/store.rs/mod.rs 未触碰），
**默认档不变**：autoExtract 默认仍 off，裁决只发生在已开 auto 档的抽取路径。

- **两段式管线**（extract.rs v2）：LLM 抽取（U16 原样）→ `parse_extract` →
  〔Auto 档〕①锁外预嵌入 → ②锁内既有记忆快照（load_all 纯读）→ ③锁外纯函数
  `plan_adjudication`：逐条找 top-1 相似既有记忆，cos ≥ `dedupHint`（0.75，
  读 memoryTuning）才算候选，且只在抽取同域 kind（profile/preference/fact）
  里找（summary/reflection/lesson 不该被「改口」）→ ④有候选才发第二次 LLM
  （ADJUDICATE_PROMPT 新增，prompts 清单锁 16→17）逐条裁决
  new / update(existing_id) / skip → ⑤锁内 `apply_adjudications` 应用：
  New 走原 `insert_item_with`（语义去重/容量淘汰兜底不变）；Update 走
  `update_by_id`——存在性复查后只换 content+向量+updated_at，kind/importance/
  source/tags 保持原口径（改口不改档：key 覆盖语义、pinned 判定、[推断] 徽标
  都不漂移；目标已消失→回退 New）；Skip 丢弃。无候选不发第二次 LLM（省调用）。
- **降级纪律**：坏输出（非 JSON/无数组）整体回退全 New；缺项/未知 action/
  幻觉 existing_id（≠该条候选 id）逐条回退 New——裁决失败只降级为 U16 行为
  （照插入，insert_item_with 语义去重兜底），绝不丢数据。三段式锁纪律保持：
  嵌入/LLM/解析全在锁外，DB 只在快照与应用两段短临界区。
- **可测接线**：管线主体抽成 `run_extract_with`（pub），LLM 调用方注入——
  生产闭包 = `summarize_messages` 配置薄壳（`run_extract` 原路径不变），
  集成测试闭包 = `summarize_http` 直连 mock（同 run_model_loop_core 先例）。
  Confirm 档不经裁决（语义与 U16 一致）。裁决动过 skip/update 时留一条
  `memory.extract_adjudicated` Info 审计（new/update/skip 计数）。

## 验收数字（锚点 = U18 基线）

- **改口集成测试**（tests/memory_conflict.rs，10 组真管线 + mock LLM +
  真实嵌入 + 真实 SQLite）：10/10 组「更新原条目」≥ 9/10 门槛 ✓；每组恰好
  2 次 LLM 调用（抽取+裁决）✓；改写保留原 tags（key 覆盖语义不破）✓；
  旧内容零残留（改口不堆积）✓。3 连跑稳定。
- **recall@5 锚点**：U18 评估器复跑 = 0.9000，与基线持平 ≥ 0.90 ✓
  （检索评分未动，符合预期）。
- 单测 7 条新增：裁决解析矩阵（new/update/skip/幻觉 id/缺项/越界/坏输出回退
  全 new）、候选规划（阈值/ top-1/降级无向量/kind 域过滤）、消息拼装（无候选
  短路不发 LLM）、应用分支（改口保留原列/消失目标回退插入/skip 丢弃/
  容量拒写计 skipped）。`cargo test --lib memory::extract` 17 绿。

## ocr 复审处置记录（两轮：R1 19 条 3H/4M/12L + R2 修复复核 9 条 3M/6L，
## json 于 docs/OCR-CODE-REVIEW-2026-10-03-u19.json / -u19r2.json）

- **H 修 3**：抽取预嵌入直接跑在 async worker 上（原 spawn_blocking 被改丢，
  违反模块「嵌入不占 async worker」纪律）→ 挪回 spawn_blocking；裁决改写
  存储故障的「插入兜底」有数据腐蚀面——空 tags 新条目若语义 merge 到刚改写
  失败的原行，会把原 key 覆盖掉 → 改为计 failed 原条目未动（留待下轮抽取
  重试，消失目标→回退插入的路径保留）；R2 复核两项修复落地，生产代码
  0 findings。
- **M 修 3**：Confirm 档 `now_ms()` 逐条调用 → 循环外统一（同批时间基准，
  U16 原行为）；apply_insert/update Err 丢 `eprintln`（存储故障不可观测
  回归）→ 恢复逐条 eprintln + 聚合审计不变；快照段补 `ensure_table`
  （load_all 遇全新库会报 no such table；apply 段的重复 ensure 移除）。
- **L 修 1**：集成测试 `reqwest::Client::new()` 每次 LLM 回调新建 → 循环外
  建一次按调用克隆。
- **驳回/登记不修 11**（附证伪证据）：
  - 锁内 open_db（快照/应用两段）——memory 全模块既有口径（injection_block
    同模式），U16 已登记不修、统一改造另行批次；
  - `Update` 变体只带 String id 无编译期保证——parse 层已有「id 必须 == 该条
    候选 id」守卫（单测覆盖幻觉 id 场景），应用层消失目标回退插入有注释与
    单测，属有意的防御纵深；
  - `[]` 空数组与坏输出同样回退全 New——「模型没给任何裁决」与「输出坏掉」
    的安全出路相同（逐条缺项本就回退 New，空数组 = 全部缺项），注释已写明；
  - apply 段 `current` 快照与 insert_item_with 内部 load_all 重复——全表读
    微秒级 × 抽取限频 30 分钟一次，成本级；后者是既有去重设计不动；
  - 测试 helper `cand(id)` 全字段字面量——MemItem 加字段时编译期报错正是
    期望行为（强制同步），非脆性；
  - 测试 format! 拼 JSON 串——语料是仓库内固定常量（无引号/反斜杠），
    orig_id 是 uuid simple（纯十六进制），插值不可能含 JSON 特殊字符；
  - 断言循环逐条 load_all——共享测试库 ≤ dozen 行，微秒级；
  - 裁决请求断言用 bodies 全量 any 而非按序号定位——EXTRACT_PROMPT 无
    「已有记忆 id=」文案，该串唯一来自 build_adjudication_msgs，接线断言
    定位到内容而非脆弱的请求序号；
  - 「2×corpus.len()」调用次数断言——每组恰好抽取+裁决两次正是被测契约
    本身（无候选时裁决不发生也体现在这个数上），收紧是设计不是脆性；
  - cleanup `.ok()` 吞清理错误——有意 best-effort（注释写明），失败模式 =
    残留行被下一轮开场清理兜底收走，已 5+ 连跑稳定实证；
  - client/base_url 每迭代双克隆——测试代码可读性取舍，成本纳秒级。
- **不适用 6**：model-meta-service/main.py（hash 颜色 ×2、fallback 键覆盖、
  upsert 200 语义、httpx 连接释放）——untracked 未入库、U12 已拍板废弃的
  独立 Python 服务目录（U15/U16/U18 同口径），不随本批处置。

## 红线核对

- 默认行为零变化：autoExtract 缺字段 = off（不抽取即无裁决）；memoryTuning
  缺字段 = 全默认；检索/注入/存储评分路径零改动，U18 评估器复跑数字持平。
- Confirm 档语义不变（不经裁决）；off 防御臂保留。
- 只写不删：Update 是改写既有行内容，Skip 是不入库，无任何删除路径。
- 嵌入不可用（降级模式）= 无向量 = 无候选 = 全 New，行为与 U16 完全一致。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U19-MEMCONFLICT",
  "family": "memory-phase3",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U19-MEMCONFLICT.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/prompts/extract.rs",
    "src-tauri/src/prompts/mod.rs",
    "src-tauri/tests/memory_conflict.rs"
  ],
  "max_lines_added": 700,
  "max_lines_removed": 60,
  "max_new_files_lines": 450,
  "findings": [
    { "file": "src-tauri/src/memory/extract.rs", "note": "两段式管线三段式锁纪律（嵌入/LLM/解析锁外，快照/应用锁内）；run_extract_with LLM 调用方注入（生产 summarize_messages 薄壳/测试 summarize_http 直连）；plan/parse/apply 纯函数拆分；坏输出回退全 New 不丢数据；幻觉 existing_id 拒绝；改口保留原 kind/importance/source/tags" },
    { "file": "src-tauri/tests/memory_conflict.rs", "note": "10 组改口语料走真管线（mock LLM + 真实嵌入 + 共享测试库）；唯一标记 key + 按内容双向清理 + 开场前移（承 U15 共享目录实录）；嵌入引擎不可用时显式跳过不阻塞无模型环境" },
    { "file": "src-tauri/src/prompts/extract.rs", "note": "ADJUDICATE_PROMPT 逐条裁决口径（宁可 update 不堆积、拿不准给 new、index 对位）" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail", "recall_below_baseline"]
}
```

## 验证命令

```
cargo test --lib memory::extract                                            # 17 绿
cargo nextest run --test memory_conflict                                    # 改口集成 10/10
cargo test --test memory_eval eval_recall_report -- --ignored --nocapture   # 锚点 0.9000
bash scripts/test-fast.sh && bash scripts/test-all.sh                       # 全绿
```
