# 自进化 Phase 2：提案应用闭环

> 接续 phase-1.md。Phase 1 提案只写 audit；Phase 2 把达门槛的提案落成 lesson 记忆，
> 经既有记忆注入链路改变后续行为——闭环完成。

## 闭环路径

```
consolidate 反思 → derive 提案 → emit 写 audit（Phase 1，不变）
                               → apply（Phase 2 新增）
                                 ↓ MemoryHint 且 impact ∈ {High, Medium}
                    mem_items 写入 kind=lesson 记忆（tags[0]=evo:<proposal_id>）
                                 ↓
              下轮对话 injection_block 的 lesson 专属槽位自动带出 → 行为改变
```

## 设计决策

- **门槛**：仅 `MemoryHint` + High/Medium 自动应用（老板拍板，激进档）。
  Low 与 PromptHint / ToolSchemaHint / SkillHint 永不自动应用，仍只写 audit。
- **幂等**：`tags[0] = evo:<proposal_id>`，写入前 `find_by_key_tag` 查重——
  持久幂等，同时弥补 Phase 1 emit 层进程内 24h dedup 重启丢失的缺口。
  emit 的 dedup 不动，两条去重链路互不影响。
- **生效零改动**：复用 `memory::injection_block` 的 lesson 槽位
  （rank.rs `MEMORY_LESSON_N`），不碰 prompt_builder / TOOLS / 事件名。
- **回滚**：`apply::rollback_applied(conn, proposal_id)` = 删同 key 记忆；
  用户也可在记忆管理界面直接删。留痕 `{data_dir}/evolution-applied.jsonl`
  （proposal_id / mem_key / applied_at_ms / impact / summary）。
- **importance 从 impact 派生**：High=4 / Medium=3，source="system"——
  可淘汰、非受保护，记忆库满时按既有淘汰分正常出局。
- **应用是 fire-and-forget 独立阻塞任务**：嵌入在持 DB 写锁前批量算好
  （与 memory 门面层同纪律），失败只进 audit（`evolution.apply_failed`），
  不影响 consolidate 主链路。`post_consolidation` 签名不变（spec 约束）。

## spec 硬约束复核

| 约束 | 状态 |
|---|---|
| 不调第二次 LLM | ✓ 复用 consolidate 反思产出 |
| 不写新表 / 不改 schema | ✓ 复用 mem_items |
| 不改 prompt / TOOLS / 命令名 / 既有事件名 | ✓（新增 audit 事件 `evolution.applied` / `evolution.apply_failed`，不改动既有） |
| memory 不感知 evolution | ✓ 依赖方向不变 |

## 测试

`evolution::apply` 7 个用例（内存库）：门槛四类、lesson 落库字段、
importance 映射、key 幂等、回滚、jsonl 留痕。

## 顺带修复（测试基建）

- `emit` 测试共享全局 dedup 表的计数断言在进程内并行下互相干扰 →
  加 `EMIT_TEST_LOCK` 串行（对齐 SKILL_SCHED_TEST_LOCK 模式）。
- `audit::p2_6_1` 与 `bot::config::bot_log_read_fail` 两用例都
  删除/读全量共享的 target/debug/deps/bot.log → NotFound 竞态 →
  加 `audit::BOT_LOG_TEST_LOCK` 互斥（paths.rs 留档的「进程内并行」
  假设的实证落锤）。

## 验证

- `cargo test` 全量 812 通过 / 0 失败，连跑 6 轮稳定
- `cargo fmt --check` ✓；`cargo clippy --all-targets` 0 error（无新增 warning）

## 明确不做（Phase 3 候选）

- PromptHint / ToolSchemaHint / SkillHint 的任何自动应用
- trace 前后对比的效果回流验证（`tool_calls_count` 目前无数据，phase-1.md 已知）
- 提案评审 UI / 前端通知
