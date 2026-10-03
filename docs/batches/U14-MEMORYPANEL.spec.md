# Batch Spec: U14-MEMORYPANEL

## 目的

设置页记忆库管理面板（记忆模块升级第一期，老板在方向对比后拍板 A+B 开关
+F 路线，本批为方向 A + 数据小修）。此前「记忆」section 只有整理开关/频率/
上次时间/立即整理，机器人记了什么看不见、改不了、删不掉；本机真实数据
3 条记忆仅 1 条有向量、嵌入是否降级无从得知。

- 后端新模块 `memory/panel.rs`：`mem_list`（无查询词按 updated_at 倒序全量；
  带查询词走 rank::hybrid_search 混合检索，**纯读不刷 access_count**——访问
  强化只属于真实聊天注入）、`mem_update`（content/importance/kind 可改，
  tags/source 不可改；内容变更重算嵌入、未变保留旧向量）、`mem_delete`
  （允许删任何条目，evo: 自进化条目由前端确认文案点名影响）、`mem_stats`
  （SQL 聚合统计 + 嵌入引擎状态）。`MemItemView` 不含向量本体只给
  hasEmbedding 标志（同 BotConfigView 不含 key 先例）。内核函数收
  &Connection（内存库可单测），锁/open_db 包装在命令层。
- `embed.rs` 增 `engine_status()` 查询口（懒加载缓存复用，降级原因透出）。
- `store.rs` `load_all` 读取侧归一历史 source 脏值 'user'→'user_stated'
  （否则 importance=5 口述条目进不了 is_protected 容量淘汰保护）。
- 前端 `MemoryPanel.tsx` 挂进记忆 section：统计行（N/500 · 向量覆盖 m/n）、
  嵌入降级横幅、搜索（300ms 防抖 + 竞态令牌）、类型筛选 chips、紧凑行
  （类型/来源徽标 + 重要度星标 + 被想起次数 title）、行内编辑（内容/
  重要度/类型）、删除确认、空态引导。

## ocr 复审处置记录（21 条 6M/12L/3 无级别，json 于 docs/OCR-CODE-REVIEW-2026-10-03-u14.json）

- **M 修 3**：搜索竞态（慢的旧查询响应覆盖新结果）→ 竞态令牌 reqIdRef 丢弃
  过期结果；`Promise.all` 耦合 mem_stats 失败拖垮列表 → `allSettled` 解耦
  （统计只管横幅与计数）；stats_core 全表反序列化 ~1MB 向量在写锁内 →
  改 SQL 聚合（COUNT/GROUP BY）。
- **M 拍板保留 1**：mem_update 引擎降级时向量落 NULL——降级模式写入本来就
  无向量（store.rs 既有契约），NULL 防语义检索按旧内容误命中，行为正确；
  可见性由三层覆盖（降级横幅 / 行徽标半透明 / 向量覆盖统计）。
- **L 修 5**：编辑与取消按钮 busy 门禁（防在途保存与新编辑互相打架）、
  空内容前端预检、tags 可选链防后端字段漂移、reload 成功清行级错误、
  测试时间戳走 store::ts_to_text（存储格式一处跟随）。
- **驳回 2（附证伪）**：「update_core 写回历史脏 source 'user'」不成立——
  existing 来自 load_all 已归一为 user_stated，写回即契约值
  （legacy_user_source_normalized_and_protected 单测锁定）；「unmount 后
  setState 警告」不成立——React 18 起卸载组件 setState 为 no-op 无警告，
  竞态令牌已挡错序显示。
- **登记不修 6**：update_core 两次 load_all（500 条微秒级，注释已量化）；
  错误 String 扁平化（与 record_lesson_core 同口径）；delete 无审计（用户
  显式删除，与 bot_clear_vendor_key 同口径）；list 读路径 kind 不做白名单
  （有意宽容，新类型不被面板挡）；SOURCE_USER_STATED 常量提取（两处字面量
  均有注释锚定）；编辑态字段取消后残留（startEdit 全量重填，无实际影响）。
- **不适用 3**：model-meta-service/main.py——untracked 未入库、U12 已拍板
  废弃的独立 Python 服务目录。

## 红线核对

- 聊天主循环 / Planner / 摘要 / 整理流水线零改动（injection_block、
  consolidate、save_summary/apply_reflection 路径原样）。
- mem_list 搜索不刷新 access_count（访问强化只属于聊天注入，单测锁定）。
- 盘上历史数据零迁移（source 归一只在读取侧）。
- keyring / 配置文件 / 可用性门禁零改动。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U14-MEMORYPANEL",
  "family": "memory-panel",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U14-MEMORYPANEL.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/lib.rs",
    "src-tauri/src/memory/embed.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/store.rs",
    "src/components/SettingsPage/MemoryPanel.test.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 10,
  "max_new_files_lines": 1450,
  "findings": [
    { "file": "src-tauri/src/memory/panel.rs", "note": "mem_list 混合检索纯读不刷访问计数；mem_update 引擎降级时向量落 NULL 为降级模式既有正确语义（横幅/徽标/统计三层可见）" },
    { "file": "src-tauri/src/memory/store.rs", "note": "load_all 读取侧归一历史 source 脏值 'user'→'user_stated'，盘上数据零迁移" },
    { "file": "src/components/SettingsPage/MemoryPanel.tsx", "note": "搜索 300ms 防抖 + 竞态令牌；allSettled 解耦统计失败；evo: 条目删除确认文案点名自进化影响" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（nextest 1340 / pytest 审计 / vitest 387）
cd src-tauri && cargo test --lib memory::   # 54 绿（含 panel 8 条）
npx --no-install vitest --run MemoryPanel   # 7 绿
```
