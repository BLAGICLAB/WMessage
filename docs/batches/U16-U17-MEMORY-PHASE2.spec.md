# Batch Spec: U16-MEMAUTO + U17-MEMTUNE（记忆升级第二期合批）

## 目的

记忆系统改造第二期（方向对比拍板：C 自动抽取 + B 参数化）。

- **U16 自动事实抽取**：`memory/extract.rs` 新模块——bot_chat 收尾
  fire-and-forget 触发（仅交互式聊天、/stop 中止不抽、每会话 30 分钟限频
  check-and-set、`autoWriteEnabled` 总闸优先），读会话最近 12 条消息 →
  `summarize_messages` + EXTRACT_PROMPT（宁缺勿滥）→ `parse_extract` 容错
  解析（剥围栏/截方括号/逐条 kind 白名单 + importance 钳制 + 超长截断）→
  auto 档 `insert_item_with` 直接入库（[推断] 徽标天然可辨）/ confirm 档进
  `mem_pending` 待确认队列（上限 50 满丢最旧；approve 三段式锁纪律：锁内
  取行 → 锁外预嵌入 → 锁内入库，入库 Err 中止且不删队列行可重试；reject
  直接删）。`MemoryControl` 扩 `autoExtract` 档位（缺字段/非法值 = off）。
- **U17 记忆参数化**：bot-config.json 新增 `memoryTuning` 块（注入预算 /
  topN / recentN / lessonN / 容量 / 衰减天数 / 去重双阈值；`clamped()` 读取
  侧统一钳制）；`io.rs` `read_memory_tuning` 轻量读取（解析失败 stderr 告警，
  与 read_memory_control 对称）；`rank.rs RankParams` / `store.rs
  StoreParams` 的 `_with` 变体（**旧签名全部保留并委托默认值——默认行为
  零变化**，apply_ops 同契约）；injection/format/recall/remember/
  save_summary/apply_reflection/consolidate/import/list 全调用点贯穿；
  前端无 UI（拍板 b），原样回传保存防手改配置被整体替换写冲掉。
- **前端**：记忆权限卡增三档选择器（radiogroup/radio + aria-checked，
  总闸关闭时禁用）；MemoryPanel 增待确认队列区块（单条/全批收下忽略，
  全批忽略带 confirm，队列限高滚动）。

## ocr 复审处置记录（两轮：U16 轮 20 条 + 合并轮 42 条 3H/15M/24L，
json 于 docs/OCR-CODE-REVIEW-2026-10-03-u16.json / -u16u17.json）

- **H 修 3**：抽取钩子在 /stop 中止会话也触发 → `if !aborted` 门禁；
  `hybrid_score_with` 衰减参数无守卫（NaN 分数打穿 sort_by 偏序）→
  非有限/非正回默认 + 钳制读取双防线；「全部忽略」批量丢弃无确认 →
  window.confirm。
- **M 修 12**：Auto 档单条入库失败聚合一条 Warn 审计（原 eprintln 静默）；
  空抽取结果留 Info 审计；approve 入库 Err 中止上抛且**不删队列行**
  （原来 Err 混入 skipped 还删行 = 用户数据丢失）；import/consolidate 同
  Err-上抛契约；confirm 档队列写入事务化 + pending_delete 批量 IN（写锁内
  逐条往返消除）；预嵌入/配置读取挪出写锁临界区；apply_ops 恢复「旧签名
  委托默认值」契约（apply_ops_with 变体）；hybrid_search_with 收敛
  &RankParams 风格且召回路径真贯穿 decay（原 _with 无调用方 = decayDays
  静默失效）；tuning 读取解析失败 stderr 告警；pending_insert 契约守卫；
  approve ids 去重；三档随总闸禁用（静默 no-op 变显式禁用）。
- **L 修 12**：radiogroup/radio 语义、AUTO_EXTRACT_MODES 常量唯一事实源 +
  isAutoExtractMode 守卫去双 as、wire 类型收紧、MEMORY_DECAY_DAYS 常量、
  RankParams 字段文档、io 内核文档对齐、panel lock_db 收敛到 store_lock、
  stats_core 去掉无效 capacity 填充、session_id 去 clone、pending aria 用
  全文、队列容器限高滚动、reject 不刷主列表、reloadPending console.warn、
  actOnPending useCallback、Off 防御臂显式返回。
- **登记不修 6**：锁内 open_db——memory 全模块既有口径（open_db 一次性
  目录成本），统一改造另行批次；auto_extract 存 String 而非枚举——防御性
  选择（枚举反序列化遇手改脏值会炸整个 BotConfig 加载）；热路径每轮配置
  读 ×3（bypass/注入门禁/抽取门禁）——同为 1KB 级文件读，与既有先例同
  成本级；限频表进程级（重启清零）——持久化收益不抵复杂度；approve 中断
  可重入自愈（未处理条目留列表，重试即续）；apply_ops 非默认参数专项
  用例已补 distill×capacity。
- **不适用**：model-meta-service/main.py 若干——untracked 废弃目录。

## 集成测试踩坑实录（承 U15）

共享 `target/debug/deps/` 地雷无新增实证；U16 管线不写集成测试（拍板）：
解析/限频/档位/pending CRUD 全部内核单测覆盖（内存库 + 纯函数），bot_chat
挂点是 4 行薄接线，E2E 需真 LLM 无测点。

## 红线核对

- **默认行为零变化**：autoExtract 缺字段 = off（不抽取）；memoryTuning
  缺字段 = 全默认；旧签名函数全部保留并委托默认值——存量测试零改动通过
  （默认值回归测试锁定 U17 前常量）。
- 抽取只写不删；confirm 档未经用户收下不入库；reject/忽略即删无残留。
- 系统写入流水线（摘要/反思/定时整理）语义不变，仅 insert 走参数化入口。
- keyring / 可用性门禁 / 双窗口主题看板零改动。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U16-MEMAUTO-U17-MEMTUNE",
  "family": "memory-phase2",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U16-U17-MEMORY-PHASE2.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/memory/consolidate/tests.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/rank.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/memory/tests.rs",
    "src-tauri/src/prompts/extract.rs",
    "src-tauri/src/prompts/mod.rs",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/MemoryPanel.test.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 1150,
  "max_lines_removed": 110,
  "max_new_files_lines": 820,
  "findings": [
    { "file": "src-tauri/src/memory/extract.rs", "note": "抽取仅交互会话+/stop 不抽+30 分钟限频+总闸优先；confirm 队列 approve 三段式锁纪律且 Err 不删行可重试；空结果/入库失败均有审计" },
    { "file": "src-tauri/src/memory/rank.rs", "note": "RankParams/_with 变体默认委托（默认行为零变化）；hybrid_score_with 衰减参数防御守卫防 NaN 打穿排序" },
    { "file": "src-tauri/src/memory/store.rs", "note": "StoreParams/_with 变体默认委托；容量/去重阈值参数化，存量测试零改动" },
    { "file": "src-tauri/src/bot/config/io.rs", "note": "read_memory_tuning 读取侧统一钳制；解析失败 stderr 告警与 read_memory_control 对称" },
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "三档 radiogroup/radio + aria-checked；总闸关闭时禁用；AUTO_EXTRACT_MODES 常量唯一事实源" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（nextest 1361 / pytest 审计 / vitest 392+）
cd src-tauri && cargo test --lib memory::   # 72 绿（extract 12 + tuning/params 4）
npx --no-install vitest --run MemoryPanel SettingsPage   # 79 绿
```
