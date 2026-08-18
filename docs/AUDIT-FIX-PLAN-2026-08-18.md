# WMessage Agent 修复计划

- **文档版本**：v1.0
- **生成时间**：2026-08-18 08:30 GMT+8
- **审计基线**：[`AUDIT-REPORT-2026-08-18.md`](./AUDIT-REPORT-2026-08-18.md)（前轮完整审计报告）
- **目标**：把审计发现的 7 个问题分优先级落地，解除 release blocker

---

## 一、优先级矩阵

| 优先级 | 任务 | 工期 | 阻断 release |
|---|---|---|---|
| **P0** | F-1 bypass_llm_on_pre_step_hit 开关 | 0.5 工日 | ✅ 必须修 |
| **P1** | F-4 interactive Skill 规格澄清（A 路径） | 0.5h | 选 |
| **P1** | F-3 事件命名空间统一 | 0.5 工日 | 选 |
| **P1** | F-5 MEMORY.md 补节 | 0.5h | 否 |
| **P2** | F-2 Plugin/Extension 抽象层 | 2 工日 | 否 |
| **P2** | F-6 端到端 event_log 测试 | 2 工日 | 否 |
| **P2** | F-7 扩展点预留 | 0.5h（与 F-2 合并） | 否 |

**最小可发布版本**：F-1 + F-4(A) + F-5 ≈ **1 工日**  
**理想发布版本**：F-1 + F-3 + F-4(A) + F-5 ≈ **1.5 工日**  
**架构完整版**：再 + F-2 + F-6 + F-7 ≈ **5.5 工日**

---

## 二、P0 · F-1 [CRITICAL] bypass_llm_on_pre_step_hit 开关

### 背景
审计发现 `bypass_llm_on_pre_step_hit` 开关在仓库中**完全不存在**（`grep -rn "bypass_llm"` 全仓 0 命中）。Req1 改造单方面推进，无回退路径。

### 改动点

1. **`src-tauri/src/db.rs`**：`BotConfig` 加 `bypass_llm_on_pre_step_hit: bool`（`#[serde(default = "default_true")]`，老配置自动兼容）
2. **`src-tauri/src/bot.rs::bot_chat`**：在 `pre_routed_skill` 命中前先取 cfg：
   - `cfg.bypass_llm_on_pre_step_hit == true` → 当前行为（auto 走 `run_skill_scheduler`，interactive 注入 system prompt）
   - `cfg.bypass_llm_on_pre_step_hit == false` → 旧链路（即便命中也把 `pre_routed_skill` 强制 None，让 LLM 自由选 Skill）
3. **旧链路代码保留**：把 `pre_routed_skill` 块加 `// LEGACY: bypass=false 走原 LLM 路径` 注释，不删
4. **`src/SettingsPage.tsx`**（前端）：机器人设置页加一个 Toggle 控件
5. **`tests-audit/audit_pre_step_pre_execute.py`** 补 2 条：
   - `test_bypass_llm_switch_field_exists`
   - `test_bypass_llm_toggle_off_zero_routing`（搜 `false` 路径分支存在）
6. **`src-tauri/src/bot.rs`** 单测：补 1 条 BotConfig serde default true

### 验证
- `cargo test --lib` 132→133 全过
- `pytest tests-audit/...` 22→24 全过
- 手动：toggle OFF → 输入「帮我做 PPT」→ 抓 bot.log 应看到 `pre_step.route_skill` 事件被跳过、直接进 LLM 自由区

### 风险
- 老用户 `bot-config.json` 缺字段 → `#[serde(default = "default_true")]` 兜底
- 前端 Toggle 默认 ON，与后端默认一致

### 上线门槛
**P0 = 修完才能 release**

---

## 三、P1 · F-4 [HIGH] interactive Skill 仍调 LLM — 规格澄清

### 背景
pytest `test_no_call_llm_after_pre_step_hit_HIGH_RISK_BUG` 当前 xfail。根因：interactive 模式 Skill body 注入 system prompt 后仍调 `run_model_loop`。从安全审计角度看不是逃逸（Skill Running 状态下 pre-execute 按规格放行），但与最初「截断流程禁止 call_llm」的规格描述矛盾。

### 两条路径二选一

#### A 路径（建议）— 规格澄清，零代码改
- 改 `MEMORY.md` + `docs/SKILL_DSL.md`：明确「interactive = LLM 驱动 Skill 步骤，auto = DSL 调度器 bypass LLM」
- pytest `test_no_call_llm_after_pre_step_hit_HIGH_RISK_BUG` 改 `xfail` → `skip`（不再视为 bug）
- 监控端按 `mode` 字段分类统计
- **工期**：~0.5h（纯文档）

#### B 路径 — 强制所有 Skill bypass LLM
- `bot.rs:884` 进 if 块就把 `pre_routed_active_skill` 设为 None
- Skill body 需包含完整步骤脚本（类 DSL）
- 现有 13 个 mock Skill + 6 个业务 Skill 全部要重写
- **工期**：~2-3 工日（重写所有 Skill 文档）

### 决策建议
**A 路径**。interactive 模式 LLM 驱动是「分阶段落地」的合理中间态，不算逃逸。F-1 修完后 toggle ON/OFF 即可让用户按场景切换。

---

## 四、P1 · F-3 [MEDIUM] 事件命名空间统一

### 背景
规格要求事件集：user.message / pre_step.route_skill / pre_execute.deny / skill.start / tool.call / tool.return / llm.request  
实际代码：3 个命中 + 4 个缺失 + 2 个命名偏差。

### 决策建议
**改代码**（监控端按规格接入，逆向改规格成本高）。

### 改动对照表

| 规格要求 | 当前代码 | 改动点 |
|---|---|---|
| `pre_execute.deny` | `pre_execute.blocked` | `bot.rs:1733` 改名（1 处） |
| `tool.call` / `tool.return` | `tool_done` 合并 | `execute_tool` 入口 `audit_event!(tool.call, ...)`，出口保留 `tool.return`（去掉合并字段） |
| `user.message` | free-form `user: ...` | `bot_chat` 入口 `audit_log` 改 `audit_event!(user.message, content=, ms=)` |
| `skill.start` | free-form `skill_start` | `bot_skills::start_skill` 末尾发 `audit_event!(skill.start, name=, risk=, mode=)` |
| `llm.request` | 无 | `run_model_loop` HTTP 发前 `audit_event!(llm.request, model=, msgs_count=)`，resp 收到 `llm.response, status=` |

### 验证
- pytest 补 5 个新 test
- 实跑一条聊天，grep `bot.log` 应包含 7 个事件名

### 工期
- 后端：~3h
- 测试：~1h
- **合计**：~0.5 工日

### 风险
- `tool.call` + `tool.return` 拆分 +5 处 audit_event 调用，单次工具执行 +2 行写入，性能影响 < 1ms
- `llm.request` 暴露 API 调用频率给监控，按需决定是否启用预览字段

---

## 五、P1 · F-5 [MEDIUM] MEMORY.md 补节

### 改动
F-1 / F-3 / F-4 修完一次性补 `~/.openclaw/workspace/MEMORY.md`：
- 新节「Phase 6 修复计划」（2026-08-18）
- Req1 改造 + bypass 开关说明 + 风险边界
- event 命名空间完整版（7 个事件）
- 待决策事项 checklist（F-2/F-7 时间表）

### 工期
~0.5h（顺手补）

---

## 六、P2 · F-2 [HIGH] Plugin/Extension 抽象层

### 背景
审计发现 `lib.rs` 业务模块硬编码 `mod` 声明，无 `trait Plugin` / `trait Middleware` 抽象层。「业务 Skill 禁止注册底层中间件钩子」只是口头约束。

### 新增 `src-tauri/src/middleware.rs`

```rust
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    fn pre_step(&self, input: &str) -> Option<RouteAction>;
    fn pre_execute(&self, name: &str, skill_active: bool) -> Option<String>; // 阻断消息
}

pub struct MiddlewareRegistry {
    pre_step: Vec<Box<dyn Middleware>>,
    pre_execute: Vec<Box<dyn Middleware>>,
}

impl MiddlewareRegistry {
    pub fn register_pre_step(&mut self, m: Box<dyn Middleware>) { ... }
    pub fn run_pre_step(&self, input: &str) -> Option<RouteAction> {
        // 短路求值：任一中间件返回 Skill 即停
    }
    pub fn run_pre_execute(&self, name: &str, active_skill: bool) -> Option<String> {
        // 短路求值：任一中间件返回阻断消息即停
    }
}
```

### 改动
1. **`src-tauri/src/lib.rs`**：setup 里 `app.manage(MiddlewareRegistry::default())` + 把 `IntentRouterMiddleware` / `AtomicGuardMiddleware` 注册进去
2. **`src-tauri/src/bot.rs::bot_chat`**：`registry.run_pre_step(...)` 替代 `route_user_input`
3. **`src-tauri/src/bot.rs::execute_tool`**：`registry.run_pre_execute(...)` 替代 `is_atomic_tool + is_skill_active`
4. **`src/SettingsPage.tsx`** 扩展面板：列出已注册中间件（name + type）
5. **业务模块物理隔离**：bot_skills.rs 只能调 `MiddlewareRegistry` 的查询接口，没有 `register_*` 入口

### 验证
- 5 个新单测：注册顺序 / pre_step 短路 / pre_execute 短路 / 业务模块无 register 接口（编译期）
- pytest 补 `test_middleware_registry_compile_time_isolation`

### 工期
- 后端：~1.5 工日
- 前端：~2h
- 文档：~1h
- **合计**：~2 工日

### 风险
- 抽象层迁移要 event_log 兼容（新名 `pre_execute.deny`，F-3 已统一）
- 业务模块绕过的根因在 `mod` 硬编码 + lib.rs 强制 `mod` 声明，**编译期防线只能挡「运行时新增中间件」，挡不了「往 mod 列表塞业务模块」**。后者必须靠 code review + 配套的 `mod business_skill;` 物理隔离目录

---

## 七、P2 · F-6 [LOW] 端到端 event_log 测试

### 新建 `tests-e2e/` 目录

1. **`tests-e2e/test_pre_step_routing_e2e.py`** — 6 个复合业务输入，断言 bot.log 事件序列
2. **`tests-e2e/test_pre_execute_atomic_e2e.py`** — 非 Skill 状态调 `create_word_revisions`，断言阻断
3. **`tests-e2e/test_skill_dsl_full.py`** — 13 个 mock Skill 端到端跑通
4. CI 接入（可选）

### 工期
~2 工日

### 风险
- 需决定 mock LLM 还是真实 LLM 账号（建议 mock 走本地 fixture）

---

## 八、P2 · F-7 [LOW] 扩展点预留

`pub trait Plugin` / `pub trait Middleware` — F-2 落地后自然产生，单独列只用于文档说明。

**工期**：~0.5h（与 F-2 合并）

---

## 九、推荐执行顺序

### 今天（2026-08-18）剩余白天
1. **F-1** 起手 — 0.5 工日午前搞定，解除 release blocker
2. **F-4 A 路径** 顺手 — 0.5h 文档同步
3. **F-5** 顺手 — 0.5h 文档同步

### 明天（2026-08-19）
4. **F-3** — 0.5 工日，事件命名统一

### 后续择期（2-3 周）
5. **F-2 + F-7** — 2 工日，架构债
6. **F-6** — 2 工日，测试债

---

## 十、待老板决策项

1. **F-4 选 A 还是 B 路径？**（强烈建议 A）
2. **F-3 改代码还是改规格？**（建议改代码）
3. **F-2 是否进今周排期？**（建议推到下周，本周先把 release blocker 解了）
4. **修复计划要不要同步到 MEMORY.md + 今天的 `memory/2026-08-18.md`？**（建议同步，作为今日决策留痕）

---

## 附录：与审计报告对照

| 审计编号 | 严重等级 | 修复条目 | 关联文件 |
|---|---|---|---|
| F-1 | CRITICAL | P0 | src-tauri/src/db.rs, bot.rs, SettingsPage.tsx |
| F-2 | HIGH | P2 | src-tauri/src/middleware.rs（新） |
| F-3 | MEDIUM | P1 | src-tauri/src/bot.rs, bot_skills.rs |
| F-4 | HIGH | P1 | MEMORY.md, docs/SKILL_DSL.md（无代码改） |
| F-5 | MEDIUM | P1 | MEMORY.md |
| F-6 | LOW | P2 | tests-e2e/（新） |
| F-7 | LOW | P2（合并 F-2） | src-tauri/src/middleware.rs |

---

**维护说明**：
- 文档跟随代码演进，Phase 6 落地后归档到 `docs/archive/`
- 任何工期/优先级调整须同步更新本文件 + MEMORY.md
