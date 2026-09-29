# WMessage × Kimi Code 架构审计计划（2026-08-18）

> **生成者**：小九（OpenClaw）
> **触发**：老板 17:49 指令——Kimi 套餐少，每次只审计一小部分，先发计划给他看
> **仓库**：`/Users/renshi/Projects/wmessage/`

---

## 1. 背景与教训

- **目标**：让 Kimi Code 审计 wmessage agent 架构（老板 17:49 指令）
- **约束**：Kimi 套餐少，每次只能跑一小部分
- **上次失败**：`docs/AUDIT-REPORT-KIMI-FAILED-2026-08-18.trace.md`——6 个并行 explore agent 把月度额度打爆，403 报错
- **策略**：1 次 Kimi 调用 = 1 个 chunk，**严格串行**，不允许并行；每个 chunk 输出 ≤ 50 行

## 2. 已审过的（避免重复）

| 报告 | 范围 |
|---|---|
| `AUDIT-AGENT-CORE-2026-08-18.md` | Agent 内核编排 / 状态机 / 熔断 / CommandError |
| `AUDIT-AGENT-FLOW-2026-08-18.md` | 端到端 6 流程 trace + CI 双层钩子 |
| `AUDIT-FIX-PLAN-2026-08-18.md` | F-1～F-7 修复落地 |
| `AUDIT-REPORT-MANUAL-2026-08-18.md` | 广角扫描（28 KB） |

**已被覆盖的核心模块**（Kimi 无需再过）：
- bot_chat / bot_model_loop / bot_scheduler / bot_slash / bot_skills
- middleware / intent_router / tool_guard / audit
- lib.rs 顶层 / ChatPanel 主要 UI / CommandError 全栈迁移

**留白的角度**（8 块）：
1. API 层安全 + 边界
2. DB 层 + 迁移
3. Python 沙箱
4. Profile + 中间件 + 审计 深读
5. 前端状态管理 + IPC 错误
6. 错误处理全栈
7. 资源生命周期 + 后台任务
8. 跨平台打包 + 构建

## 3. 8 块切片（独立 prompt）

| # | 标题 | 核心文件 | 字节 | 聚焦 |
|---|---|---|---|---|
| 1 | API 层安全 + 边界 | api_handlers.rs + api_server.rs + api_auth.rs | 37K + 5K + 3K | 鉴权 / 限流 / SSE / 入参校验 / 错误路径 |
| 2 | DB 层 + 迁移 | db.rs + migration.rs | 39K + 43K | 事务 / 迁移幂等 / 并发 / 错误恢复 |
| 3 | Python 沙箱 | bot_py.rs | 37K | 执行限制 / 资源清理 / 错误隔离 |
| 4 | Profile + 中间件 + 审计 | profile.rs + middleware.rs + audit.rs | 22K + 10K + 6K | 架构边界 / 清理路径 / 跨模块一致性 |
| 5 | 前端状态管理 + IPC 错误 | src/storage.ts + ChatPanel.tsx(没审过的部分) + WidgetApp.tsx + TodoCard.tsx + SettingsPage.tsx | ≥100K | 状态竞态 / IPC 错误 / 断线重连 |
| 6 | 错误处理全栈 | error.rs + 全 Rust command + frontend handleCommandError | 13K + 散落 | 错误分级 / 用户可见性 / 监控 |
| 7 | 资源生命周期 + 后台任务 | lib.rs + 全 #[tauri::command] | 17K + 散落 | 连接清理 / 文件句柄 / 后台任务 |
| 8 | 跨平台打包 + 构建 | Cargo.toml + tauri.conf.json + build.rs + capabilities/ | 散落 | target 条件编译 / 资源嵌入 / 跨平台差异 |

## 4. 单 Chunk Prompt 模板

```markdown
# Chunk N: <标题>

## 任务
<一句话聚焦，挂在「老板现在遇到不少 bug，找出架构层面的根因」这条线上>

## 范围
- 必读文件（绝对路径）：
  - src-tauri/src/xxx.rs (N KB)
  - ...
- 重点关注：<3-5 个 bullet>

## 跳过（已审过）
- <列出本 chunk 范围内已被 4 份报告覆盖的项，告诉 Kimi 别再列>

## 输出格式
1. 风险清单（按严重度 P0>P1>P2 排）
   - [P0] [file:line] [一句话描述] [建议]
2. 整体评估（一句话）
3. 与已有审计的差异（重复 = 0 条，新发现 = N 条）

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不重复已有报告的 P1/P2 项
- 不要给"小修小补"建议，只给**架构层面**或**潜在 bug 根因**的发现
```

## 5. 执行方式（待你选）

### 方式 A：TUI 手动（推荐，预算最省）
```
cd /Users/renshi/Projects/wmessage
kimi      # 打开 TUI（已开）
# 粘贴 docs/kimi-audit/kimi-chunk-1.md 内容
# 完成后复制响应到 docs/kimi-audit/kimi-chunk-1-result.md
# 重复 8 次，禁止并行
```

### 方式 B：One-shot（省时但 token 不可控）
```bash
cd /Users/renshi/Projects/wmessage
kimi -p "$(cat docs/kimi-audit/kimi-chunk-1.md)" \
  > docs/kimi-audit/kimi-chunk-1-result.md 2>&1
```

### 方式 C：我来串行调度（中等）
- 老板每次说「跑 chunk N」，我用 `kimi -p` 调一次
- 每次响应自动写到对应 result 文件
- 进度可暂停

## 6. 收尾

- 8 块跑完 = 8 份 `kimi-chunk-N-result.md`
- 我合并去重 → `docs/AUDIT-REPORT-KIMI-2026-08-18.md`
- 与 4 份已有审计交叉
- 重点产出：P0 阻塞 / 隐藏安全 / 隐式时序问题 / 架构层 bug 根因

## 7. 风险

- **Kimi 抽样质量**：单次审计可能漏掉跨模块问题。补救：合并 8 份后由我二次扫
- **配额再爆**：若中途 403，停止串行，等下个账单周期继续
- **回答过长**：单 chunk 如果 Kimi 给 100+ 行，我手动压缩到 50 行核心结论

---

## 8. 决策项（待老板定）

- [ ] **chunk 范围**：8 块切法 OK 吗？要不增减 / 合并？
- [ ] **执行方式**：A（手动粘贴）/ B（one-shot）/ C（我串行调度）？
- [ ] **优先级**：要不要先跑某个 chunk？比如 Chunk 1 API（你最近在 API 上踩坑多？）
- [ ] **预算**：今晚先跑 1-2 块，还是整个 8 块今晚跑完？
