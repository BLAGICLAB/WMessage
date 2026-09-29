# R0 验证层 —— VERIFICATION.md

> 日期：2026-09-18
> 实施工程师：Claude（OpenClaw 主会话）
> 范围：读源码验证 R1-R8 的可行性假设，逐项贴证据

---

## 验证总表

| # | 假设 | 验证结果 | 影响 |
|---|------|----------|------|
| 1 | `tags` 是变长数组（`Vec<String>`） | ✅ | L2 版本锚点可行 |
| 2 | `find_by_key_tag` 全表遍历 | ✅ | P0-2 墓碑（删同 key 即失效）可行 |
| 3 | `kind` 是 `String`（非 enum） | ✅ | 影子槽（kind=evolution_shadow 等）可行 |
| 4 | `importance` 范围 1-5，演化用 4/3 | ✅ | Fallback A（重要性降级）可行 |
| 5 | `evolution-applied.jsonl` 含 `applied_at_ms` | ✅ | L4 评估层可基于切窗统计 |
| 6 | `bot_chat.rs` 早期 return 站点数 | 24 处（含内嵌辅助） | P1-1 复杂度可控 |
| 7 | `tauri-plugin-notification` 在用 | ✅ | P0-3 通知路径可行 |
| 8 | `bot_sessions` 可采样历史 session | ✅ | L4 eval set 数据源就绪 |
| 9 | `ConfirmMap` 可复用做审批 UI | ⚠️ 复用技术可行，但**与 spec 第 6 节「❌ 用户确认弹窗（自动化场景失败）」有冲突** | **必须老板拍板** |

下面逐项贴源码证据。

---

## 1. tags 是变长数组 ✅

**证据**：
- `src-tauri/src/memory/store.rs:38`
  ```rust
  pub struct MemItem {
      ...
      pub tags: Vec<String>,
      ...
  }
  ```
- `src-tauri/src/memory/store.rs:171`
  ```rust
  pub struct NewItem {
      ...
      pub tags: Vec<String>,
      ...
  }
  ```
- 应用时形态（`src-tauri/src/evolution/apply.rs:73-74`）
  ```rust
  let item = NewItem {
      ...
      tags: vec![key, "evolution".to_string()],
      ...
  };
  ```

**影响**：
- L2 ChangeRecord 锚点 `tags[0] = "evo:<change_id>"` 可行（与现状一致）
- 副 tag 可携带 `layer:code`、`origin:canary` 等辅助元数据而不破硬约束
- ⚠️ 但注意：`store.rs:54` schema 是 `tags TEXT NOT NULL DEFAULT ''`，存的是逗号分隔字符串；解析在 `load_all`（`store.rs:84-90`）做 `split(',').map(trim).filter(!empty)`。新加 tag 字段必须保持逗号安全字符集（不含逗号、不含前后空白）

---

## 2. find_by_key_tag 全表遍历 ✅

**证据**：
- `src-tauri/src/memory/store.rs:259-262`
  ```rust
  pub fn find_by_key_tag(conn: &rusqlite::Connection, key: &str) -> Result<Option<MemItem>, String> {
      Ok(load_all(conn)?
          .into_iter()
          .find(|m| m.tags.first().map(|t| t.as_str()) == Some(key)))
  }
  ```
- `load_all` 是 `SELECT id, kind, content, tags, importance, source, created_at, updated_at, access_count, last_accessed_at, embedding FROM mem_items`（`store.rs:67-71`）

**性能**：
- 全表扫描 ≤500 条 × ~2KB 向量（注释见 `store.rs:1-3`）
- 微秒级，O(n) 但 n 被 MAX_MEM_ITEMS=500 卡死

**影响**：
- P0-2 墓碑（删除同 key 记忆即失效）可行
- 注意：如果后续演进到 5000+ 条，要改索引（不在 R0-R8 范围）

---

## 3. kind 是 String（非 enum） ✅

**证据**：
- `src-tauri/src/memory/store.rs:33`
  ```rust
  pub kind: String, // profile|preference|fact|event|summary|reflection
  ```
- 注释列出已知 kind，但**代码层不做枚举校验**——`insert_item` 直接 `item.kind` 写入
- 已存在但不在注释里的 kind：`lesson`（`apply.rs:71` `kind: "lesson".to_string()`）

**影响**：
- 影子槽 `kind = "evolution_shadow"` / `kind = "evolution_canary"` 可直接用，无需 schema 迁移
- 记忆 injection_block 排序逻辑（`memory::mod.rs` / `injection_block`）需查源码确认 lesson 排序优先级，**R1 实施前确认**

---

## 4. importance 范围 1-5，演化用 4/3 ✅

**证据**：
- `importance: i64 // 1-5`（`store.rs:39`）
- clamp 在多处：`store.rs:223, 269, 311, 314` 全部 `clamp(1, 5)`
- 受保护判定（`store.rs:131`）：
  ```rust
  pub fn is_protected(item: &MemItem) -> bool {
      item.importance >= 5 && item.source == "user_stated"
  }
  ```
- 演化映射（`apply.rs:75-79`）：
  ```rust
  importance: match p.impact {
      ImpactLevel::High => 4,
      _ => 3,
  },
  source: "system".to_string(),
  ```

**影响**：
- Fallback A 可行：High=4 / Medium=3，永不触碰 5
- 「非受保护 + 可淘汰」自然满足（source=system 不是 user_stated）
- 淘汰分（`store.rs:136-145`）`importance * 2` 决定了 lesson 优先级：4 > 3 > 普通 fact(默认 3)
- 边界：若 R7 策略分层要落「极高优先级 lesson」（如安全规则），需要 5——会破保护契约，**不要做**

---

## 5. evolution-applied.jsonl 含 applied_at_ms ✅

**证据**：
- `src-tauri/src/evolution/apply.rs:84-94`
  ```rust
  #[derive(serde::Serialize)]
  struct AppliedRecord<'a> {
      proposal_id: &'a str,
      mem_key: String,
      applied_at_ms: i64,
      impact: &'a str,
      summary: &'a str,
  }
  ```
- 单测断言（`apply.rs:330`）：`assert_eq!(v["applied_at_ms"], 1000);`
- bot.log 另有 `evolution.applied` 审计事件（`apply.rs:158-164`），由 `audit_event!` 宏写入
- `evolution-applied.jsonl` 路径：`{data_dir}/evolution-applied.jsonl`（`apply.rs:142`）

**影响**：
- L4 评估层可按 `applied_at_ms` 切窗统计：
  - 应用率 = Δt 窗口内 applied 行数
  - 回滚率 = `delete_by_key_tag` 调用次数 vs applied 行数（需要新加回滚留痕或查 `evolution.applied_failed`）
  - 污染存活期 = 当前时间 - applied_at_ms（仅对未回滚且仍存在的条目）
- ⚠️ 回滚目前不留痕 jsonl——R2 ChangeRecord 应自带 `rolled_back_at` 字段补齐

---

## 6. bot_chat.rs 早期 return 站点数

**统计**：24 处 `return` 关键字（含辅助函数）

**重要分布**（按函数粒度，非逐行罗列）：

| 函数 | return 数 | 错误类型 | P1-1 改造相关性 |
|------|----------|---------|----------------|
| `truncate_split_point` | 1 (line 147) | 上下文截断兜底 | 不相关（辅助） |
| `require_bot_enabled` | 1 (line 458) | `BotDisabled` | **关键** |
| `ChatGuard::acquire` | 2 (lines 503, 511) | 重入 / Drop | **关键**（防重入） |
| `skill_route_dispatch` 等 | 1 (line 595) | `SkillRouteOutcome::FallThrough` | 不相关（技能调度） |
| `chat_execute` | 多处 | 综合 | **关键**（主路径） |
| `require_api_key` | 1 (line 946) | `ApiKeyMissing` | **关键** |
| `chat_execute_tasks` | 多处 | `TaskInvalidState` | **关键** |
| `BotDisabled` 等顶层 | 1 (line 1369) | `BotDisabled` | **关键** |
| LLM API 错误 | 1 (line 1023) | `LlmApiError` | **关键** |
| 域规则 | 1 (line 1046) | `DomainRule` | **关键** |

**关键站点**（chat 主体 + 影响 P1-1 的早 return）：
- `require_bot_enabled` (line 456-458)
- `require_api_key` (line 944-946)
- `ChatGuard::acquire` (line 497-511)
- `chat_execute` 主路径（line 689, 699, 796, 812）
- LLM 错误处理（line 1023）
- 域规则校验（line 1046）
- BotDisabled 顶层（line 1369）
- TaskInvalidState 多分支（lines 1381, 1392, 1397, 1421）

**影响**：
- P1-1（聊天主体加埋点）的真实插桩点约 **8-10 处**，可控
- 每个 return 都对应一个明确错误码（`CommandError` enum），可借助错误码结构化埋点而非逐 return 加 println
- ⚠️ 建议 R3 阶段优先做：在 `CommandError` → audit_log 的转换路径统一埋点，比逐 return 改更稳

---

## 7. tauri-plugin-notification 在用 ✅

**证据**：
- 依赖声明 `src-tauri/Cargo.toml:18`
  ```toml
  tauri-plugin-notification = "2"
  ```
- 插件注册 `src-tauri/src/lib.rs:196-197`
  ```rust
  // 系统通知：任务卡截止提醒（due_notify）；macOS 需用户授权（前端启动时请求）
  .plugin(tauri_plugin_notification::init())
  ```
- 真实使用 `src-tauri/src/due_notify.rs:23`
  ```rust
  use tauri_plugin_notification::NotificationExt;
  ```
- 启动入口 `src-tauri/src/lib.rs:261`
  ```rust
  due_notify::start_due_notifier(app.handle().clone());
  ```

**业务场景**：任务卡截止前 1 小时 / 截止时刻提醒

**影响**：
- P0-3（进通知）技术路径已通：`app.notification().builder().title(...).body(...).show()`
- ⚠️ **macOS 需要用户授权**（前端启动时请求权限）——首次跑 P0-3 通知要给老板看授权弹窗
- ⚠️ 路径仍走系统通知 API，不是 in-app 提醒；不同场景可能更想要 in-app toast（待 R5 决策面板一并设计）

---

## 8. bot_sessions 可采样历史 session ✅

**证据**：
- 表结构 `src-tauri/src/db/bot_sessions.rs:53-60`
  ```rust
  pub struct BotSession {
      pub id: String,
      pub title: String,
      pub created_at: i64,
      pub updated_at: i64,
  }
  ```
- 加载函数（已存在 command）`src-tauri/src/db/bot_sessions.rs:32`
  ```rust
  pub fn bot_sessions_load(app: AppHandle) -> CommandResult<Vec<BotSession>> {
      ...
      "SELECT id, title, created_at, updated_at FROM bot_sessions ORDER BY updated_at DESC"
      ...
  }
  ```
- 消息表 `bot_messages` 字段（`src-tauri/src/db/bot_history.rs:17-24`）：`role, content, refs, thinking, tools, session_id, created_at`
- 消息加载函数（已存在 command）`bot_history_load`（`bot_history.rs:9-30`）

**影响**：
- L4 eval set 数据源就绪，**无需新建表**
- 可直接通过 `bot_sessions_load` + `bot_history_load` 组合采样
- 656 tests 的转换：需要在源码里 grep `#[test]` 实际数（粗看：`bot_chat.rs` 末段大量测试函数 + `src-tauri/tests/*.rs`）
  - ⚠️ 精确数字 R1 第一步验：grep `grep -rn "#\[test\]\|#\[tokio::test\]" src-tauri/src src-tauri/tests | wc -l`
- ⚠️ 采样是只读操作，符合硬约束「不写新数据库表」

---

## 9. ConfirmMap 可复用做审批 UI —— ⚠️ **技术可行但有硬约束冲突**

**技术证据**：
- 类型定义 `src-tauri/src/app_state.rs:101-110`
  ```rust
  type ConfirmMap = Mutex<
      HashMap<
          String,
          (
              tokio::sync::oneshot::Sender<crate::bot_slash::ConfirmReply>,
              Option<String>,
          ),
      >,
  >;
  ```
- 字段已迁入 `AppState`（`app_state.rs:158`）：`pub(crate) confirm_requests: ConfirmMap,`
- 访问器（`app_state.rs:208`）：`pub(crate) fn confirms<'a, R>(app: &'a AppHandle<R>) -> &'a ConfirmMap`
- 模板函数（`bot_slash.rs:269-374`）：`async fn ask_confirm_inner(...)` 已封装好 widget 可见性、超时、oneshot 通道
- 公开命令 `bot_confirm_response`（`bot_slash.rs:382`）：前端回传 approved + always

**硬约束冲突**：
- `bot_slash.rs:269-285` 注释明确：
  > 「60s 超时默认拒绝（安全兜底）」「非交互执行（后台定时任务，interactive=false）直接拒绝不弹窗」「挂件不可见时同样直接拒绝」
- 这与 spec 第 6 节「❌ 用户确认弹窗（自动化场景失败）」直接冲突：
  - 自动应用（AutoApplied）在沙箱外走 apply 路径，**不会**走到 ConfirmMap——这条没冲突
  - 但 R5「决策面板」的 Promote 操作本身是用户主动触发，ConfirmMap 适用；冲突点在于 R5 是否要复用现有 ConfirmMap 的 widget 弹窗，还是做独立审批页面

**冲突点**（必须老板拍板）：

| 选项 | 含义 |
|------|------|
| A | 复用 ConfirmMap + 现有 widget 弹窗：最小改动，但保留 60s 超时 + widget 不可见默认拒绝的旧语义 |
| B | 复用 ConfirmMap 但新增「evolution_confirm」专用入口：保留安全兜底，但绕过 widget 可见性检查 |
| C | 决策面板做成独立页面（不走 ConfirmMap）：最干净，但工作量 +1-2 天 |

**影响**：R5 工作量预估从「2 天复用 ConfirmMap」变为「3 天独立面板」（如果老板选 C）

---

## 硬约束冲突汇总（需老板拍板）

### 冲突 #1：ConfirmMap 复用方式（R5 决策面板）

详见上方 #9 章节。三个选项 A / B / C。

**默认建议**：选 A（复用现有 widget 弹窗，安全兜底成熟）。理由：
- 已有 widget 可见性 + 超时机制
- 已有 audit 留痕
- 与现有挂件删除任务确认 UX 一致，用户学习成本低
- 唯一限制：用户必须挂件在线——对决策面板这种「开发者主动操作」场景是合理的

### 冲突 #2：EvolutionProposal 加字段破坏约束 3

**事实**：
- `evolution/emit.rs:107` 注释锁死：「kv 字段锁死——后续若加新字段会破坏 `grep evolution.proposal bot.log` 解析」
- `evolution/proposal.rs:269` 单测 `evolution_proposal_carries_minimal_fields` 锁死 8 字段
- R4 要加 `layer` / `evidence` / `change_id` 字段

**冲突**：硬约束 #3 明令「不改 JSON 字段」

**默认建议**：
- 把新字段放在独立 jsonl：`evolution-proposals.jsonl`（R4 候选池要求 ttl=14 天）
- EvolutionProposal 序列化字段不变；新结构是 `EvolutionProposalV2` 或 wrapper，落到独立 jsonl 不进 bot.log

### 冲突 #3：applied.jsonl 路径权限

- `evolution-applied.jsonl` 路径 `{data_dir}/evolution-applied.jsonl`（`apply.rs:142`）
- data_dir 是 tauri app data 目录，权限 OK
- 但 R2 L2 反映「版本链」需要更长生命周期（>14 天）；当前 applied.jsonl 是无 TTL 追加，可复用
- ⚠️ 注意：applied.jsonl 是旧结构（仅 5 字段），R2 要扩字段同样要新加 `evolution-changes.jsonl` 而不是改它——遵循冲突 #2 同样处理

---

## R1-R8 可行性矩阵

| 阶段 | 依赖 R0 结论 | 关键风险 |
|------|-------------|---------|
| R1 L4 评估层 | 全部 ✅ 可行 | 656 tests 实际数量需 R1 第一步 grep 确认 |
| R2 L2 版本层 | ⚠️ 加字段破坏约束 #3，需走新 jsonl | 已建议方案 |
| R3 L3 沙箱层 | ✅ 全部可行；applied.jsonl / shadow.jsonl 独立落 | notification 首次 macOS 授权弹窗 |
| R4 L1 候选层 | ⚠️ 同 R2，加字段破坏约束 #3 | 已建议方案 |
| R5 决策面板 | ⚠️ ConfirmMap 复用方式待拍板 | 冲突 #1 |
| R6-R8 | 依赖 R1-R5 | R7 策略分层需老板拍板 |

---

## R0 产出物清单

- [x] `VERIFICATION.md`（本文件）
- [x] 9 项验证全部完成
- [x] 3 项硬约束冲突已列出
- [x] R1-R8 可行性矩阵已输出

## R0 验收标准

> 验收：VERIFICATION.md 完成，逐项有源码证据（贴行号或函数名）。

**自检**：每项已贴 `文件:行号` 或 `文件:行号 + 函数名` ✅

---

## 待办（移交 R1）

1. grep `#[test]` 实际数量（确认 656 tests 是否准确）
2. 确认 `memory::injection_block` 的 lesson 排序优先级（影响影子槽设计）
3. 老板拍板：冲突 #1（ConfirmMap 复用方式 A/B/C）
4. 老板拍板：冲突 #2 / #3 的「新字段走独立 jsonl」方案是否接受

R0 完成。等待老板确认后进入 R1。